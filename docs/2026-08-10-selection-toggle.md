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

## 5. What is deliberately unchanged

The region path. `App::read_region` acquires with `Duration::ZERO`, i.e. a plain
try-acquire, so a repeat ⌘⇧R while a read is in flight is still dropped on the floor. There
is no delivered text there to tell a deliberate re-trigger from a stray double-press, so the
rejected-stop-then-start reasoning still applies in full.
`a_repeat_region_press_is_still_a_silent_no_op` is the test that fails if that is weakened —
notably, it was the one test in the new file that **passed** against the unimplemented
state machine, which is exactly the right signal: the region path does not go through it.

## 6. Coverage

`tests/selection_toggle.rs`, 13 tests. Seven drive the pure function; six drive `App` against
a real `Player` with a fake engine and a sink that models rodio's pause semantics.

| Test | Pins |
|---|---|
| `the_same_selection_delivered_again_toggles_pause` | same text → toggle, not restart, not drop |
| `a_different_selection_interrupts_the_read_in_flight` | different text → interrupt |
| `a_selection_delivered_during_a_region_read_always_interrupts` | a region read is never a toggle target |
| `a_finished_read_leaves_nothing_to_toggle_against` | no stale "currently reading" after a natural end |
| `comparison_ignores_whitespace_at_the_ends` / `..._does_not_normalise_interior_whitespace` | the §3 rule, in both directions |
| `the_same_selection_pressed_again_pauses_then_resumes` | pause leaves the read in flight; resume continues; exactly one synthesis per sentence |
| `a_different_selection_stops_the_current_read_and_starts_the_new_one` | the displaced read is cut short, the new one is read in full, and `max_inflight == 1` |
| `a_selection_arriving_during_a_region_read_takes_over` | the ⌘⇧A-during-⌘⇧R case works and does not deadlock against the busy flag |
| `a_repeat_region_press_is_still_a_silent_no_op` | the region guard is not weakened |
| `a_finished_read_does_not_leave_text_behind_that_toggles_instead_of_reading` | a second identical press after completion reads, not pauses |
| `superseding_a_paused_read_does_not_leave_the_new_one_playing_into_a_paused_sink` | the rodio trap, end to end |

Two of these assert `ConcurrencyEngine::max_inflight == 1`, which is the real replacement for
what the old blanket no-op was buying: `Player::speak` stays single-caller across an
interrupt.

The stall-watchdog guarantee (a pause longer than 30s must not abort the read) is untouched
and still pinned by Phase 1's `a_paused_queue_is_not_a_stalled_device` and
`resuming_gives_the_watchdog_a_fresh_budget`, which drive `StallWatch` against a synthetic
clock.

**Not verified:** nobody has pressed ⌘⇧A in the real bundled app for this change — per the
brief, no bundle was installed over `/Applications/Aloud.app`. Everything below the OS
boundary is covered; the Service delivery path itself is unmodified.
