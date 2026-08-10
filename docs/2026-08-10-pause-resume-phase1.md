# Pause/resume, Phase 1 — what was built and what it measured

FORGE, 2026-08-10. Branch `feat/pause-resume`, built in an isolated worktree off `main` at
`7334671`. macOS 26.6, M1 Air.

**Verdict: it works, and it works properly.** Real mid-sentence pause, nothing re-synthesised,
nothing dropped. Neither kill criterion was hit. The 30-second stall-watchdog bug named in the
research was real, was reproduced as a failing test, and is fixed.

Scope was Phase 1 only: **no media key, no `MPRemoteCommandCenter`, no `objc2-media-player`.**
Nothing in this change set touches that decision, and the research doc's §2 hand-back test is
still unrun.

---

## 1. What `rodio` 0.22.2 actually supports

The research called this **VERIFIED** from the docs. It is stronger than that — the mechanism is
the right one, not just an API with the right name.

`Player::pause()` / `play()` / `is_paused()` (`rodio-0.22.2/src/player.rs:212, 264, 272`) set and
read a single `pause` atomic on the shared `controls`. That flag is consumed at
`player.rs:149` by `Pausable::set_paused`, and `Pausable`'s iterator is the part that matters:

```rust
// rodio-0.22.2/src/source/pausable.rs:85-95
fn next(&mut self) -> Option<I::Item> {
    if self.remaining_paused_samples > 0 { … return Some(0.0); }
    if let Some(paused_channels) = self.paused_channels {
        self.remaining_paused_samples = paused_channels.get() - 1;
        return Some(0.0);          // silence — and note what is NOT called:
    }
    self.input.next()              // the inner source is never advanced while paused
}
```

While paused it emits silence **without pulling from the inner source**. The queued
`SamplesBuffer`s are therefore not consumed, not decoded, not discarded — the part-played buffer
holds its exact position. Resume continues at the sample it stopped on. That is a genuine pause,
not a fade or a mute, and it is why the "stop and re-synthesise" fallback (which would re-read the
sentence from its start, and is not pause) was never needed.

Two things `rodio` does **not** do, both of which had to be handled here:

- **`stop()` does not clear the pause flag.** `stop()` sets `stopped`; `pause` is an independent
  control it never touches (`player.rs:264` vs `:301`). Stop-while-paused therefore leaves a sink
  that is empty *and* paused, and the next `append()` plays into it silently, forever, with a
  queue depth that never moves. `RodioSink::stop` now calls `play()` after `stop()`, and the
  `AudioSink::stop` doc pins this as a trait-level contract so test doubles model it too.
- **Pausing an idle sink is legal and silently poisonous.** Nothing stops you pausing a sink with
  nothing playing; the damage only appears on the *next* read. `Player::pause()` refuses unless
  `is_speaking()`, and `Player::speak()` clears the pause flag on entry to close the narrow race
  where a pause lands just as the previous utterance drains its last buffer.

**Verified against a real device, not just the source.** `tests/player_pause.rs` carries an
`#[ignore]`d test, `rodio_really_pauses_without_discarding_buffers`, that opens the default output
device, queues 1.5s of (silent) audio, pauses 100ms in, holds for 3s — twice as long as the whole
queue would take to play — and asserts the depth has not moved, then resumes and asserts it
drains. It also asserts the `stop()`-clears-pause contract on the real sink. It passes on this
machine. Run it whenever `src/play/` changes:

```
cargo test --release --test player_pause -- --ignored
```

## 2. Pause vs. ahead-of-playback synthesis

This was the part most likely to be subtly wrong, and it needed one correction during
implementation.

`Player::run` keeps at most one sentence buffered ahead (`wait_for_drain(2)`). Pause acts on
**playback**, so at the moment it lands the synthesis thread is somewhere else entirely — usually
blocked inside `engine.synthesize()`, which can take seconds. Consequences, all intended:

- **Pause is immediate regardless of what the synthesis thread is doing.** Same property that
  makes the existing `stop()` responsive: it acts on the sink, not on the loop.
- **Exactly one further buffer can still land after the pause takes effect** — the one already in
  flight, whose `wait_for_drain(2)` had already returned before the pause. It is appended to the
  paused sink (correctly: it is preserved, not played), the next `wait_for_drain(2)` then blocks,
  and no more synthesis happens. The look-ahead bound holds at ≤2 buffers throughout.
- **A pause costs no CPU.** The synthesis thread sits in the drain loop's 20ms poll rather than
  synthesising.

The correction: my first end-to-end test asserted the queue depth was *frozen* from the instant of
pause, and it failed — depth went 1 → 2. That was not a bug in the feature, it was the look-ahead
append above, and the test was asserting the wrong invariant. The honest invariant is that nothing
**drains** while paused. The test now waits for the in-flight append to land, then asserts the
depth holds perfectly still across five buffers' worth of playback time, and separately asserts
the ≤2 bound and that the whole text was not run ahead.

## 3. The stall-watchdog fix

`wait_for_drain` aborts a read with `"audio device appears stalled"` when `queued()` has not
changed for 30s. **A paused sink's depth is frozen by definition**, so before this change any
pause longer than 30 seconds killed the read. Real bug, not hypothetical.

Written as a failing test first. The red run:

```
running 6 tests
test a_frozen_queue_that_is_not_paused_still_trips_the_watchdog ... ok
test pause_is_refused_when_nothing_is_speaking ... ok
test resuming_gives_the_watchdog_a_fresh_budget ... FAILED
test a_paused_queue_is_not_a_stalled_device ... FAILED
test stopping_while_paused_leaves_a_playable_sink ... ok
test pause_holds_playback_and_resume_finishes_without_resynthesising ... ok

---- a_paused_queue_is_not_a_stalled_device stdout ----
thread 'a_paused_queue_is_not_a_stalled_device' panicked at tests/player_pause.rs:34:9:
the watchdog aborted a paused read after 30s - a paused sink's queue depth is frozen by
definition, so pause must not count toward the 30s stall timeout
```

Note which test caught it. The *end-to-end* pause test passed while the bug was live, because it
only pauses for a few hundred milliseconds — a wall-clock integration test would have had to sit
through 31 seconds to see this. So the watchdog decision was extracted into `StallWatch`, a small
struct taking `now: Instant` as a parameter, and driven against a synthetic clock: 300 simulated
seconds of pause in 0.6s of real time. This follows the pattern already used in this codebase for
exactly this reason — `shortcut::plan_apply`, `apply_shortcut_with`, `take_probe`: seam out the
decision, leave the I/O loop thin. `wait_for_drain` retains no stall logic of its own.

The fix is that time spent paused does not accumulate toward the timeout at all — while paused the
deadline is carried forward on every tick, so a resume starts a fresh full 30s.

**What was deliberately not done: `STALL_TIMEOUT` was not raised.** That is the band-aid. It
weakens the guard against a genuinely dead device *and* still breaks on a long enough pause. The
root cause is that the guard could not distinguish "device died" from "user paused", and the fix
is to tell it. There are two regression tests pinning the guard still works: a frozen *unpaused*
queue still aborts at exactly 30s, and it still aborts 30s after a resume.

## 4. Controls

**Tray item.** A `Pause` / `Resume` item, sitting above `Stop`. Its label is produced only by
`pause_label(paused, accel)` and only ever from `App::is_paused()`, which reads straight through
to the sink — there is no second copy of the state to drift. The label names the **action the
click performs**, never the state it is in; a "Pause" item on a paused read is precisely the
lying-control defect class this project has spent the week removing, and there are unit tests
asserting both directions. It is re-rendered after every mutation point: both toggle triggers,
`Stop` (which clears the pause), and the end of a read on all paths including the error unwind.
When no chord is registered it renders as a bare `Pause` rather than advertising an empty one.

**Hotkey.** `Cmd+Shift+P`, an ordinary chord. It goes through the existing Carbon
`RegisterEventHotKey` path — no `CGEventTap`, no TCC prompt, and it keeps working while Spotify
owns the media keys. It reuses `apply_shortcut_with` unchanged, pointed at a second ground-truth
slot (`Runtime.registered_pause_shortcut`), so it inherits the rollback and the media-key screen
for free. Registration failure is soft: it falls back to the default, and if that also fails it
says so in the tray and the tray item still works.

The chord passes the existing denylist — `KeyP` is not in `SYSTEM_CHORDS`, carries a modifier, and
is not a documented macOS system shortcut (⌘P is Print, which is app-level, not a global
reservation). A test asserts the recorded-chord path produces exactly the shipped default string,
so a default the validator would reject cannot ship.

**One ordering subtlety worth naming.** The settings window's liveness probe consumes the *next*
hotkey press as proof the region chord is live, and it does not look at which shortcut arrived
(`probe_consumed`'s parameter is `_shortcut`). With a second global hotkey now registered, a pause
press could have falsely "confirmed" a region chord that is in fact shadowed by another app —
turning the one honest confirmation mechanism into a liar. The handler therefore dispatches on the
fired chord *before* the probe check.

**Settings.** `pause_shortcut` is persisted alongside `region_shortcut`, and the media-key screen
in `Settings::normalize` was generalised to run over both fields rather than just the region one.
The container-level `#[serde(default)]` means the owner's existing `settings.json` — which has no
`pause_shortcut` key — loads untouched, with his shortcut, voice and speed intact; there is a test
for exactly that.

**Settings-window UI: not built, deliberately.** The page has no section this fits without adding
one, and adding one would force the window resize the brief said to avoid. The tray item and the
hotkey are the deliverable; the chord is editable in `settings.json` meanwhile.

## 5. What was tested

`cargo test --release`, full suite: **170 passed, 0 failed, 3 ignored** across 17 test binaries.
One of those ignored is the new real-device test, run explicitly and passing; the other two are
the pre-existing `#[ignore]`d absolute-latency and ASR-backed speed tests.

(`tests/ocr_macos.rs` fails on a fresh worktree until `helpers/macos-ocr/build.sh` is run — the
Swift helper is a build artefact, not a checked-in binary. That is environmental and pre-existing;
it is included in the passing count above because the helper was built.)

New coverage:

| Test | Pins |
|---|---|
| `a_paused_queue_is_not_a_stalled_device` | 300 simulated seconds paused never aborts the read |
| `a_frozen_queue_that_is_not_paused_still_trips_the_watchdog` | the guard still catches a dead device, at exactly 30s |
| `resuming_gives_the_watchdog_a_fresh_budget` | a 10-minute pause does not leave the read on a hair trigger |
| `pause_holds_playback_and_resume_finishes_without_resynthesising` | nothing drains while paused; exactly one synthesis per sentence across a pause; the ≤2 look-ahead bound holds |
| `pause_is_refused_when_nothing_is_speaking` | no paused-and-empty sink to swallow the next read |
| `stopping_while_paused_leaves_a_playable_sink` | Stop clears pause, and the next read is actually audible |
| `rodio_really_pauses_without_discarding_buffers` (ignored) | the above, against a real audio device |
| `pause_label_tests` (×3) | the tray item names the action, not the state |
| 4 × `tests/settings.rs` | old configs load; media keys don't survive on the new field; the default chord is bindable |

`cargo clippy --release --all-targets` produces 6 warnings, all pre-existing and all in files this
change does not touch (`src/text/chunk.rs`, `src/vendor/`). `cargo fmt` applied.

**Not tested, and I want to be explicit about it:** nobody has pressed `Cmd+Shift+P` in the real
bundled app. That needs a `packaging/make-app.sh` build and a human, and per the brief no bundle
was installed over `/Applications/Aloud.app`. Everything below the OS boundary is covered; the
registration itself is the same code path the region chord already uses.

## 6. Arguments against continuing

Honest list. None of them is a reason to revert Phase 1, but two are worth the owner's attention.

1. **`⌘⇧P` will collide with something.** It is a common in-app binding (Command Palette in VS
   Code, Print Preview in some apps). A global Carbon hotkey registers non-exclusively and wins
   while Aloud runs, so in those apps `⌘⇧P` will pause Aloud instead of doing what the app expects
   — the same trade the `⌘⇧A` Service shortcut already makes, and documented in the README. There
   is no settings UI to rebind it yet, which makes this sharper than it would otherwise be. **If
   this bites, the fix is the settings row, not a different default.**
2. **The tray label is refreshed at four call sites.** They all funnel through one helper that
   reads ground truth, so none can render a stale value — but a *fifth* mutation path added later
   and not wired up would show a stale label. The state itself cannot drift (it lives in the
   sink); only the rendering can lag. A menu-will-open hook would remove the class entirely; Tauri
   2 does not expose one reliably for `show_menu_on_left_click(true)`.
3. **`refresh_pause_label` takes the `Player` read lock on the main thread.** If a voice swap
   holds the write lock, that blocks until the in-flight utterance finishes. This is not new — the
   existing tray `Stop` handler has the same exposure and `App::stop`'s doc comment addresses it —
   but Phase 1 adds two more main-thread readers, so it is now a slightly wider surface.
4. **Phase 2 is not de-risked by any of this.** The media key still hinges on the one unrun
   experiment: whether releasing the Now Playing session hands the keys back to Spotify or leaves
   them dead / launches Music.app (research §2). Phase 1 deliberately does not make that question
   easier or harder — which was the point of splitting them.

## 7. Outstanding, not done here

- `docs/M4-platform-research-macos.md:522-526` still describes the `CGEventTap` gate as *"arguably
  not triggered (LIKELY, not verified)"*. The media-key research settled that against us by direct
  measurement and flagged the line for correction. It is unrelated to pause/resume and was left
  alone rather than silently folded into this change set — but it is stale, and it is the kind of
  stale that could talk a future agent into the tap.
- The `MEDIA_KEYS` doc comment in `src/shortcut.rs` could likewise be upgraded from "could produce
  a prompt" to the measured finding. Same call: named, not silently done.
