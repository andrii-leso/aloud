# ⌘⇧A as a selection-aware play/pause toggle

FORGE, 2026-08-10. Branch `feat/pause-resume`, on top of the Phase 1 pause/resume work at
`00d044f`. macOS 26.6, M1 Air.

Semantics chosen by the owner:

| press | state | result |
|---|---|---|
| ⌘⇧A | nothing playing | read the selection |
| ⌘⇧A | **same** text still selected, speaking | pause |
| ⌘⇧A | same text, paused | resume, from the exact sample |
| ⌘⇧A | **different** text selected | stop the current read, start the new one |
| ⌘⇧R | a read already in flight | **unchanged**: silent no-op |

---

## 1. Why this is possible at all

⌘⇧A is not a hotkey Aloud owns. It is the `NSKeyEquivalent` on the `NSServices` entry in
`Info.plist`, so every invocation is macOS *handing Aloud the selected text*. That is the
whole trick: the delivered text is the intent signal, so "the same passage again" and "this
other passage instead" are already distinct in the data, with no mode and no second chord.

The old behaviour could not distinguish them because it never looked at the text. `App`
guarded both flows with one `AtomicBool` and answered a second call with silence, and
`src/app/mod.rs`'s doc comment recorded stop-then-start as *considered and rejected* —
because it "would interrupt whatever the user is already listening to on every stray
double-press". That reasoning was correct for a repeat and wrong for a different selection;
it just had no way to tell which it was holding.

## 2. The seam

`src/app/intent.rs` is a pure function of two values:

```rust
pub fn decide_selection(delivered: &str, current: Option<&Current>) -> SelectionAction
```

`Current` is `Region` or `Selection(String)`; `SelectionAction` is `Speak`, `TogglePause` or
`Interrupt`. No `Player`, no sink, no `AppHandle`, no thread — so every branch, including the
ones a human would have to press a hotkey at exactly the right millisecond to reach, is an
ordinary unit test rather than something verified by ear.

Two decisions inside it are worth keeping:

**`Current::Region` carries no text.** A region read is speaking OCR output, not a selection,
so a selection press can never be toggling it — even if the OCR happened to produce the same
characters. Modelling that as a variant makes it a fact about the code rather than a
coincidence about the input.

**`TogglePause` is one action, not a `Pause`/`Resume` pair.** Which way it resolves is read
from the sink when it is performed (`App::toggle_pause` → `Player::is_paused`). Deciding it
here would need a `paused` snapshot that can already be stale by the time it is acted on —
the tray's Pause item is a second control that can toggle in between. The sink stays the
single source of truth, which is why `paused` is not a parameter of `decide_selection` at
all.

## 3. The comparison rule: trimmed, not raw

`intent::comparison_key` is `text.trim()`. Ends only; the interior is untouched.

The press this feature turns on is the *second* one, with the same passage still highlighted
— and "the same passage" is not byte-stable as macOS delivers it. Re-selecting by
double-click-drag picks up a trailing space in some apps and not others; dragging to the end
of a line may or may not take the newline. Those differences are invisible on screen, so a
raw comparison would refuse the toggle for a reason the user cannot see, and it would fail
into the worst available behaviour — the passage restarts from the top. Raw is more literal.
It is not more predictable *to the person pressing the key*, which is the only
predictability that matters here.

Interior whitespace is deliberately not normalised: collapsing runs of spaces would let two
selections the user can *see* are different (two indentation levels of the same line of code)
compare equal, and equal means "pause" rather than "read". Trimming the ends cannot merge
anything a user would call different — a difference at the ends is whitespace by definition.
Collapsing the middle can.

The comparison is on what the Service delivered, **not** on the normalized text that reaches
the engine (`text::normalize::normalize_ocr`). That normalizer is a synthesis detail that
will keep changing; binding the toggle to it would mean an unrelated edit there could
silently change which presses toggle and which restart.

Accepted trade-off, on the owner's instruction: identical text selected in two different
places reads as "toggle", not "restart". There is no signal that distinguishes them and no
heuristic was added to fake one.

## 4. Executing the decision — three things that had to be got right

**Interrupt has to wait, and the wait has to be bounded.** `Player::stop` is asynchronous by
design: it acts on the sink so it takes effect while the speaking thread is blocked inside
`engine.synthesize()`. So the displaced read still holds the busy flag for a moment after
`stop()` returns. Taking the flag on a plain try-acquire would lose that race and drop the
new selection *having already silenced the old one* — strictly worse than the no-op it
replaced. `App::acquire_within` therefore polls for it, up to `TAKEOVER_WAIT` (35s, just past
the player's own 30s stall watchdog). That bound is a hang-guard, not a latency budget: the
expected wait is milliseconds, and the worst realistic one is a single `synthesize` call.

**Two rapid presses must not read the same selection twice.** With a blocking takeover, a
second press landing while the first is still waiting would queue behind it and speak the
same text again, back to back. `taking_over` is a single-slot flag: a second takeover
attempt while one is in flight is dropped (`SelectionOutcome::Skipped`). It is released the
moment the new read owns the busy flag *and* has published its text — not when the read
finishes — so from that instant a further press sees the new read and toggles it. Releasing
it any later would break the feature it protects; any earlier and the window reopens.

**A superseded *paused* read must not poison the new one.** This is Phase 1's discovered
rodio trap: a paused sink swallows whatever is appended next, silently, with a queue depth
that never moves. It needs no special handling here, and that is load-bearing rather than
lucky — `Player::stop` clears the paused state via the `AudioSink::stop` contract, and
`Player::speak` clears it again on entry. `tests/selection_toggle.rs` pins it anyway, with a
deadline wrapper: a regression would otherwise *hang* the test binary rather than fail one
test, because a paused sink never drains and the stall watchdog deliberately does not fire
while paused.

## 4a. Five more things the first round got wrong

Four independent reviews of the first implementation found five defects that all share one
shape: **the toggle turned states that used to be harmless into states that are acted on**,
and each of those states already existed without a guard, because nothing had ever reached
them automatically before.

**A stopped read still names itself.** `current` was published by `acquire_within` and
cleared only by `BusyRelease`, i.e. when the reading thread actually returns — which is up
to one whole `engine.synthesize()` call after `stop()` silenced the audio (7-15s on a loaded
machine). Tray → Stop, then ⌘⇧A on the same passage to start it again, was therefore decided
as `TogglePause` and swallowed. `App::stop` now clears `current` itself; the press becomes an
ordinary `Speak` that waits for the dying read to let go of the busy flag.

**`is_speaking()` is true before there is any sound.** `Player::speak` sets it *before* the
first `synthesize` call, so through the whole seconds-long silent pre-roll `Player::pause`'s
guard reported "speaking" against an empty sink — precisely the paused-and-empty state that
guard exists to make unrepresentable. A user who hears nothing and presses again used to get
a no-op; they got a permanent park instead (first buffer into a paused sink, `wait_for_drain`
on a frozen depth, `StallWatch` correctly refusing to fire while paused, busy flag held for
good). `Player` now also tracks `audible` — set at the first `append`, cleared by `speak`
entry and by every stop path — and `pause()` requires both. Deliberately not
`sink.queued() > 0`: synthesis runs *behind* playback on a loaded machine, so an honest
mid-read moment can show a depth of zero.

**Stopping before knowing there is anything to say.** The Interrupt path called `stop()` and
only then normalized. `normalize_ocr` drops every short all-digit block once a selection has
more than one `"\n\n"` block — so `"42\n\n"` (a list marker with the blank line the drag
picked up) or `"2024\n\n2025"` is a delivery `selection_worth_speaking` accepts and the
normalizer empties. The live passage died and nothing replaced it, reported as
`Spoke { replaced: true }`. `App::speak_selection` now runs `actions::speakable_selection`
first and returns `SelectionOutcome::Empty` without touching anything.

**A stop can be aimed at a read that has not started.** `speak_selection` releases the
takeover slot as soon as it owns the busy flag and has published `current` — deliberately, so
a further press can toggle — but `Player::speak` has not been entered yet, and its first act
is to clear the stop flag. A press landing in that window (the player read lock,
`detect_lang`'s lazy lingua load, `split_sentences`) issued a `stop()` that was then wiped,
and blocked on the busy flag while the passage it meant to replace played out in full; past
`TAKEOVER_WAIT`, the selection the user asked for was dropped entirely. `acquire_within` now
re-asserts the stop on **every** poll rather than once before the loop.

**rodio's third contract: `append` blocks after a `stop`.** `rodio::Player::stop` only sets a
flag; the queue is emptied on the audio output thread, and `sound_count` falls only as the
mixer pulls the samples. `append` therefore *begins* with `sleep_until_end()` — a bare
`Receiver::recv()` with no timeout — when a stop is outstanding and buffers remain
(rodio-0.22.2 `src/player.rs:109-115,313-316`). On a healthy device that is 5-15ms. On a
device that has stopped consuming (Bluetooth drop, DAC unplugged) it never returns, and since
the thread never reaches `wait_for_drain`, the 30s stall watchdog — the app's only guard for
exactly that condition — never runs either: the busy flag is held by a parked thread and
Aloud is bricked until relaunch, with nothing in the log. This pairing pre-dates the toggle
(the tray's Stop could produce it) but was a rare manual sequence; the Interrupt path fires
it automatically on every different-selection press. `RodioInner` now marks the stop and
waits, bounded, for `len()` to reach zero before appending, turning a permanent silent hang
back into an ordinary `Err` that unwinds the read and surfaces in the tray.

**And a reporting fix that falls out of the same review:** `Player::speak` returns
`SpeakEnd::{Completed, Interrupted}` rather than `Ok(())`, so a displaced read no longer logs
"completed, spoke" and resets the tray status while its replacement is already speaking.
That propagates to `Outcome::Interrupted` and `SelectionOutcome::Cut { replaced }`.

## 5. What is deliberately unchanged

The region path. `App::read_region` acquires with `Duration::ZERO`, i.e. a plain
try-acquire, so a repeat ⌘⇧R while a read is in flight is still dropped on the floor. There
is no delivered text there to tell a deliberate re-trigger from a stray double-press, so the
rejected-stop-then-start reasoning still applies in full.
`a_repeat_region_press_is_still_a_silent_no_op` is the test that fails if that is weakened —
notably, it was the one test in the new file that **passed** against the unimplemented
state machine, which is exactly the right signal: the region path does not go through it.

## 6. Coverage

`tests/selection_toggle.rs`, 16 tests. Six drive the pure function; ten drive `App` against a
real `Player` with a fake engine and a sink that models rodio's pause semantics.

| Test | Pins |
|---|---|
| `the_same_selection_delivered_again_toggles_pause` | same text → toggle, not restart, not drop |
| `a_different_selection_interrupts_the_read_in_flight` | different text → interrupt |
| `a_selection_delivered_during_a_region_read_always_interrupts` | a region read is never a toggle target |
| `nothing_in_flight_reads_the_delivered_text` | `None` → read. (There used to be a second, byte-identical test named for the finished-read case; it could not observe anything the first did not, and the property it was credited with is pinned at the `App` level below.) |
| `comparison_ignores_whitespace_at_the_ends` / `..._does_not_normalise_interior_whitespace` | the §3 rule, in both directions |
| `the_same_selection_pressed_again_pauses_then_resumes` | pause leaves the read in flight; resume continues; exactly one synthesis per sentence |
| `a_different_selection_stops_the_current_read_and_starts_the_new_one` | the displaced read is cut short, the new one is read in full, and `max_inflight == 1` |
| `a_selection_arriving_during_a_region_read_takes_over` | the ⌘⇧A-during-⌘⇧R case works, does not deadlock, and **actually stops** the region read rather than waiting it out (`calls_mentioning("Alpha") < 12`, plus `Outcome::Interrupted`) |
| `a_takeover_still_stops_a_read_that_has_not_reached_the_player_yet` | §4a's re-asserted stop: a blocking `RegionSelector` parks a read that owns the busy flag but has not entered `Player::speak`, which is the state a single stop is wiped in |
| `a_repeat_region_press_is_still_a_silent_no_op` | the region guard is not weakened |
| `a_finished_read_does_not_leave_text_behind_that_toggles_instead_of_reading` | a second identical press after completion reads, not pauses |
| `stop_then_the_same_selection_starts_a_fresh_read_rather_than_toggling` | `App::stop` clears `current`, so a Stop followed by ⌘⇧A is not a pause of silence |
| `a_press_during_the_silent_pre_roll_is_not_taken_as_a_pause` | the `audible` half of `Player::pause`'s guard; without it the read parks forever and the test binary hangs (hence the deadline wrapper) |
| `a_selection_that_normalizes_to_nothing_leaves_the_read_in_flight_alone` | nothing is stopped before the new text is known to be speakable; asserts its own premise against `normalize_ocr` and `selection_worth_speaking` |
| `superseding_a_paused_read_does_not_leave_the_new_one_playing_into_a_paused_sink` | the rodio trap, end to end |

Several of these assert `ConcurrencyEngine::max_inflight == 1`, which is the real replacement
for what the old blanket no-op was buying: `Player::speak` stays single-caller across an
interrupt.

`tests/actions.rs::busy_guard_prevents_a_second_concurrent_speak` delivers *different* text on
purpose. A repeat press short-circuits at the `TogglePause` early return and never reaches the
busy flag, so a version of that test built on a repeat stays green with the compare-exchange
deleted — it asserts `max_inflight == 1` across an Interrupt instead. The repeat-press property
is asserted separately by `the_same_selection_while_a_read_is_in_flight_toggles_instead_of_speaking`.
`src/play/sink.rs`'s unit tests cover the bounded stopped-queue drain from §4a.

Each of the eight guards above was checked by mutation: the fix was reverted one at a time and
the named test confirmed to fail.

The stall-watchdog guarantee (a pause longer than 30s must not abort the read) is untouched
and still pinned by Phase 1's `a_paused_queue_is_not_a_stalled_device` and
`resuming_gives_the_watchdog_a_fresh_budget`, which drive `StallWatch` against a synthetic
clock.

**Not verified:** nobody has pressed ⌘⇧A in the real bundled app for this change — per the
brief, no bundle was installed over `/Applications/Aloud.app`. Everything below the OS
boundary is covered; the Service delivery path itself is unmodified.
