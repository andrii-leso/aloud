//! Tauri-free application core: owns the long-lived `Player` (paid once at
//! startup — the ~1.4s Supertonic model load must happen at launch, not on
//! the first hotkey) and serializes calls into the two flows in
//! `actions.rs`.
//!
//! `Player::speak` is documented single-caller/serialized and deliberately
//! does not lock internally — see its doc comment in `src/play/player.rs`.
//! `App` is where that contract gets honoured: a busy flag makes it
//! impossible for a second `read_region`/`speak_selection` call to run a
//! concurrent `speak()` that would interleave `sink.append()` calls and
//! race the shared `speaking`/`stop_flag` state.
//!
//! **What a second call does while the flag is held depends on the path,
//! and that difference is the whole of the selection toggle.**
//!
//! - **Region (⌘⇧R): a silent no-op, unchanged.** A stop-then-start
//!   alternative was considered and rejected, and that reasoning still
//!   holds here: the region hotkey delivers no text, so a second press is
//!   indistinguishable from a stray double-press, and acting on it would
//!   interrupt whatever the user is already listening to.
//! - **Selection (⌘⇧A): decided from the delivered text.** The selection
//!   path is a macOS Service, so *every* invocation hands us the selected
//!   text — which means a repeat and a genuine "read this instead" are
//!   distinguishable after all, and the old blanket no-op was answering
//!   both with silence. `intent::decide_selection` (`src/app/intent.rs`)
//!   makes that call: same text -> toggle pause on the read in flight;
//!   different text (or a region read in flight) -> stop it and read the
//!   new selection. The rejected-stop-then-start rationale above applies
//!   only to the *repeat* case, which is now a pause rather than a
//!   restart.
//!
//! `current` is what makes that decision possible: it names the read that
//! holds the busy flag *and is still worth toggling*. It is published
//! after the flag is taken and cleared before it is released (see
//! `BusyRelease`), and it is cleared again by `stop()` — because a stopped
//! read goes on holding the busy flag for as long as it takes to unwind,
//! which is up to one whole `engine.synthesize()` call, and during that
//! time there is nothing left to pause. A stale value would turn the next
//! press into a pause of silence.
//!
//! The busy flag covers `speak()` calls and nothing else. `stop()` and
//! `toggle_pause()` are outside it by design: both only set atomics on
//! the sink reached through the `Player`'s internal `Arc`s, and both must
//! work *while* a read holds the flag. Guarding either one with the busy
//! flag would make it acceptable only when there was nothing to act on.
//!
//! A separate, unrelated concern: the `Player` itself can be replaced
//! outright — a live voice change (Task 9) rebuilds the whole engine and
//! swaps in a new `Player`. That is guarded by a `RwLock` around `Player`,
//! not the busy flag: `read_region`/`speak_selection` take a read lock for
//! the duration of one `speak()` call, and `swap_player` takes the write
//! lock. A write lock request blocks until every outstanding read lock
//! releases, so a voice change that lands mid-utterance waits for that
//! utterance to finish rather than cutting it off — it does not call
//! `stop()`. The busy flag and the `RwLock` compose without conflict:
//! the busy flag serializes *speak* calls against each other, the
//! `RwLock` serializes a *swap* against whichever speak call (if any)
//! currently holds the read lock.

pub mod actions;
pub mod intent;

use crate::app::intent::{comparison_key, decide_selection, Current, SelectionAction};
use crate::capture::RegionSelector;
use crate::ocr::OcrEngine;
use crate::play::player::{Player, SpeakEnd};
use anyhow::Result;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Mutex, MutexGuard, RwLock};
use std::time::{Duration, Instant};

/// How long a selection takeover waits for the read it is displacing to
/// let go of the busy flag.
///
/// Not a latency budget. The expected wait is a few milliseconds —
/// `Player::stop` acts on the sink rather than on the loop, and every
/// iteration of `Player::run`/`wait_for_drain` checks the stop flag — and
/// at worst one `engine.synthesize()` call, which is seconds on a loaded
/// machine (CLAUDE.md constraint 5). This bound exists only so that a read
/// which never releases at all cannot hang the Service callback thread
/// forever; it sits just past the player's own 30s stall watchdog, which
/// is what bounds the pathological case.
const TAKEOVER_WAIT: Duration = Duration::from_secs(35);

/// Poll interval while waiting for that release.
const ACQUIRE_POLL: Duration = Duration::from_millis(5);

/// What a selection delivery actually did.
///
/// Three outcomes rather than the old `bool`, because ⌘⇧A now has three
/// honest answers and the caller logs (and, for a toggle, re-renders the
/// tray from) which one happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionOutcome {
    /// The delivered text was read to the end. `replaced` is true when a
    /// different read was stopped to make room for it.
    Spoke { replaced: bool },
    /// The delivered text started being read but was itself cut short —
    /// by a later ⌘⇧A takeover, or by the tray's Stop. `replaced` carries
    /// the same meaning as on `Spoke`.
    ///
    /// Distinct from `Spoke` on purpose: a displaced read used to report
    /// completion, so `src/bin/aloud.rs` logged "completed, spoke" and
    /// reset the tray status for a passage that had been silenced, while
    /// its replacement was already speaking.
    Cut { replaced: bool },
    /// The same selection was already in flight, so this press toggled
    /// pause instead of starting anything. Carries the resulting paused
    /// state as `Player::pause`/`resume` report it — for logging only.
    /// The tray label is never rendered from this: it re-reads
    /// `App::is_paused()`, so there is exactly one source of truth for
    /// what the audio device is doing.
    Toggled { paused: bool },
    /// The delivered text had nothing speakable left in it once
    /// normalized, so nothing was started — and, the point of the
    /// variant, nothing in flight was stopped either. See
    /// `actions::speakable_selection` for how a delivery the Service
    /// accepted can still normalize to nothing.
    Empty,
    /// Nothing happened: another selection was already in the middle of
    /// taking over, or the read being displaced never let go.
    Skipped,
}

/// Holds the shared `Player` and the busy guard around it.
pub struct App {
    // RwLock rather than a plain field: a voice change replaces the whole
    // Player (the voice style is loaded into the engine at construction),
    // and that can arrive while a read is in flight. The busy flag already
    // serializes speak() calls; this only guards the swap itself.
    player: RwLock<Player>,
    busy: AtomicBool,
    /// Names the read that currently holds `busy`, or `None`. Written only
    /// by `acquire_within` and `BusyRelease` — see the module doc comment.
    current: Mutex<Option<Current>>,
    /// Set while one selection delivery is between "stop what is playing"
    /// and "own the busy flag". See `speak_selection`.
    taking_over: AtomicBool,
    // f32 bits. Speed is a per-call argument to Player::speak, so it can
    // change between utterances with no reconstruction.
    speed: AtomicU32,
}

impl App {
    pub fn new(player: Player, speed: f32) -> Self {
        Self {
            player: RwLock::new(player),
            busy: AtomicBool::new(false),
            current: Mutex::new(None),
            taking_over: AtomicBool::new(false),
            speed: AtomicU32::new(crate::settings::Settings::clamp_speed(speed).to_bits()),
        }
    }

    /// Current speed, clamped at construction and at every `set_speed`.
    pub fn speed(&self) -> f32 {
        f32::from_bits(self.speed.load(Ordering::Relaxed))
    }

    /// Updates the speed used by the *next* `speak()` call. Takes effect
    /// immediately for any read/selection started after this returns; an
    /// utterance already in flight keeps the speed it was called with,
    /// since `speed` is captured as a plain argument at the top of
    /// `actions::read_region`/`speak_selection`, not re-read mid-sentence.
    /// Clamped here too, not only in `Settings` — this is a second entry
    /// point (the IPC command also clamps before saving, but `App` must
    /// not trust that as its only guard).
    pub fn set_speed(&self, v: f32) {
        self.speed.store(
            crate::settings::Settings::clamp_speed(v).to_bits(),
            Ordering::Relaxed,
        );
    }

    /// Replaces the engine-bearing `Player`. Blocks until any in-flight
    /// read releases the read lock, so an utterance already speaking is
    /// never cut off mid-sentence by a voice change — it finishes on the
    /// old `Player`, and only the next `speak()` call sees the new one.
    pub fn swap_player(&self, player: Player) {
        *self.player.write().unwrap() = player;
    }

    /// Stops whatever is currently speaking. Safe to call at any time,
    /// including when nothing is speaking.
    ///
    /// A read lock, like the speak calls below — `stop()` only touches
    /// atomics shared via the `Player`'s internal `Arc`s, so it never
    /// needs exclusive access, and taking a plain read lock means Stop
    /// stays responsive (non-blocking against other readers) rather than
    /// queueing behind a pending voice-swap write lock.
    ///
    /// **Clears `current` as well**, and that is not belt-and-braces
    /// duplication of `BusyRelease`. Stop is asynchronous: it silences the
    /// sink at once, but the reading thread does not notice until it
    /// leaves `engine.synthesize()` — 7-15s on a loaded machine, per
    /// CLAUDE.md constraint 5 — and it holds the busy flag, and `current`,
    /// for all of that time. Leaving `current` set means a ⌘⇧A on the same
    /// passage during that window is decided as `TogglePause` and
    /// swallowed: tray → Stop, then ⌘⇧A to start it again, does nothing at
    /// all except flip the tray to "Resume" for a read that is already
    /// dead. Clearing it makes the press an ordinary `Speak`, which then
    /// simply waits for the dying read to let go of the flag.
    ///
    /// The ordering is safe: the read being stopped still holds `busy`, so
    /// no other read can have published a `current` for this line to
    /// erase — a new one is published only after `BusyRelease` has
    /// released the flag.
    pub fn stop(&self) {
        *lock_current(&self.current) = None;
        self.player.read().unwrap().stop();
    }

    /// Toggles pause, returning the resulting paused state.
    ///
    /// Deliberately **not** guarded by the busy flag. That flag serializes
    /// `speak()` calls against each other because `Player::speak` does not
    /// lock internally; this touches none of that machinery — like
    /// `stop()`, it reaches the sink through the `Player`'s internal
    /// `Arc`s and only ever sets an atomic. Taking the busy flag here
    /// would in fact invert the feature: the flag is *held* for the whole
    /// duration of the read, so a pause could only ever be accepted when
    /// there was nothing playing to pause.
    ///
    /// A read lock for the same reason `stop()` takes one — see its doc
    /// comment.
    ///
    /// Pausing while nothing is speaking is a no-op that returns `false`
    /// (see `Player::pause`), so a stray hotkey press cannot park a
    /// paused, empty audio device in front of the next read.
    pub fn toggle_pause(&self) -> bool {
        let player = self.player.read().unwrap();
        if player.is_paused() {
            player.resume()
        } else {
            player.pause()
        }
    }

    /// Whether audio output is currently paused. Ground truth, read
    /// straight through to the sink — the tray label renders this rather
    /// than tracking its own copy.
    pub fn is_paused(&self) -> bool {
        self.player.read().unwrap().is_paused()
    }

    /// Runs the region flow (see `actions::read_region`), guarded so a
    /// second call while one is already in flight is a no-op.
    ///
    /// Returns `Ok(None)` when skipped because the player was already
    /// busy; `Ok(Some(outcome))` when it ran, carrying which of the four
    /// things happened (cancelled / found nothing / spoke / was stopped
    /// before the end) — the caller (`src/bin/aloud.rs`) uses that to
    /// decide whether a notification is warranted.
    ///
    /// Unchanged by the selection toggle, deliberately: `Duration::ZERO` is a
    /// plain try-acquire, so a repeat ⌘⇧R while a read is in flight is
    /// still dropped on the floor rather than interrupting it. The region
    /// hotkey carries no text, so there is nothing here to tell a
    /// deliberate re-trigger apart from a stray double-press.
    pub fn read_region(
        &self,
        selector: &dyn RegionSelector,
        ocr: &dyn OcrEngine,
    ) -> Result<Option<actions::Outcome>> {
        let Some(_guard) = self.acquire_within(Current::Region, Duration::ZERO, false) else {
            return Ok(None);
        };
        let player = self.player.read().unwrap();
        actions::read_region(selector, ocr, &player, self.speed()).map(Some)
    }

    /// Handles one delivery of selected text from the macOS Service.
    ///
    /// The delivered text is the intent signal — see `intent::decide_selection`
    /// for the three-way decision and `intent::comparison_key` for how two
    /// selections are compared. This function is only the execution of that
    /// decision:
    ///
    /// - **TogglePause** returns immediately; the read it refers to keeps
    ///   holding the busy flag, which is exactly right — it has not
    ///   finished, it has only stopped making noise.
    /// - **Interrupt** stops the read in flight and then *waits* for it to
    ///   release the flag. Waiting is not optional: `stop()` is
    ///   asynchronous (the thread it stops may be inside
    ///   `engine.synthesize()`), so taking the flag on a plain try-acquire
    ///   would lose the race and drop the new selection on the floor
    ///   having already silenced the old one. Both the stop and the wait
    ///   live in `acquire_within` — the stop is re-issued on every poll,
    ///   for the reason set out there.
    /// - **Speak** is the same path with nothing to stop first.
    ///
    /// Nothing above is reached at all until the delivered text is known
    /// to survive normalization: stopping a live read to make room for a
    /// selection that then says nothing is strictly worse than the no-op
    /// this replaced.
    ///
    /// The paused state needs no special handling on the Interrupt path,
    /// and that is load-bearing rather than lucky: `Player::stop` clears
    /// it via the `AudioSink::stop` contract, and `Player::speak` clears
    /// it again on entry. A superseded *paused* read would otherwise park
    /// a paused sink in front of the new one, which swallows it silently
    /// (Phase 1's discovered rodio trap).
    pub fn speak_selection(&self, text: &str) -> Result<SelectionOutcome> {
        // Normalized **first**, before any decision and before anything is
        // stopped. The Interrupt path silences the read in flight, and
        // committing to that before the new text is known to be speakable
        // trades a live passage for silence: `normalize_ocr` drops every
        // short all-digit block once a selection has more than one
        // paragraph, so `"42\n\n"` or `"2024\n\n2025"` is a delivery the
        // Service accepts and the normalizer empties. Before ⌘⇧A became a
        // toggle that press was a harmless no-op; the regression would be
        // the silencing, reported as `Spoke { replaced: true }`.
        let Some(normalized) = actions::speakable_selection(text) else {
            crate::log_line!(
                "selection flow: nothing speakable in the delivered text, \
                 leaving whatever is playing alone"
            );
            return Ok(SelectionOutcome::Empty);
        };

        // The lock is released before anything is acted on. Holding it
        // across `toggle_pause` would close a microsecond-wide race (the
        // read ending between the decision and the toggle) at the cost of
        // a real deadlock: `BusyRelease::drop` takes this same lock, and a
        // pending `swap_player` write lock can block the read lock that
        // `toggle_pause` needs. The race it would buy is benign — the
        // toggle lands on a `Player` that is no longer speaking, and
        // `Player::pause` refuses that outright.
        let action = decide_selection(text, lock_current(&self.current).as_ref());

        if let SelectionAction::TogglePause = action {
            return Ok(SelectionOutcome::Toggled {
                paused: self.toggle_pause(),
            });
        }

        // Single-slot takeover. Two presses arriving while the first is
        // still waiting for the previous read to let go carry the same
        // intent — the new read has not started, so there is nothing yet
        // to toggle against — and letting the second queue behind the
        // first would read the same selection twice, back to back.
        //
        // It is held until the new read owns the busy flag and has
        // published its text, which is for as long as the read being
        // displaced takes to notice the stop and unwind — up to one
        // `engine.synthesize()` call. A press inside that window is
        // dropped rather than queued, which is the trade this flag is:
        // one lost press against reading the same passage twice.
        if self
            .taking_over
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            crate::log_line!("selection flow: skipped, a takeover is already under way");
            return Ok(SelectionOutcome::Skipped);
        }
        let takeover = FlagRelease(&self.taking_over);

        // The stop itself is issued by `acquire_within`, on every poll
        // rather than once up front — see its doc comment.
        let replaced = matches!(action, SelectionAction::Interrupt);

        let Some(_guard) = self.acquire_within(
            Current::Selection(comparison_key(text).to_string()),
            TAKEOVER_WAIT,
            replaced,
        ) else {
            crate::log_line!(
                "selection flow: skipped, the read in flight did not release within {TAKEOVER_WAIT:?}"
            );
            return Ok(SelectionOutcome::Skipped);
        };
        // The new read owns the flag and `current` names it, so a further
        // press can now see it and toggle. Released before the (long)
        // speak call, not after it.
        drop(takeover);

        let player = self.player.read().unwrap();
        Ok(
            match actions::speak_normalized(&normalized, &player, self.speed())? {
                SpeakEnd::Completed => SelectionOutcome::Spoke { replaced },
                SpeakEnd::Interrupted => SelectionOutcome::Cut { replaced },
            },
        )
    }

    /// Takes the busy flag and publishes `current`, waiting up to
    /// `timeout` for an in-flight read to release it. `Duration::ZERO` is
    /// a plain try-acquire.
    ///
    /// With `displace`, a `stop()` is issued before **every** attempt, not
    /// once before the loop, and that repetition is the fix for a real
    /// protocol hole rather than defensive noise. `Player::speak` clears
    /// the stop flag as its first act, and the read this call is
    /// displacing may not have entered `speak` yet: `speak_selection`
    /// publishes `current` and releases the takeover slot as soon as it
    /// owns the busy flag — deliberately, so that a further press can
    /// toggle it — which leaves a window (the `Player` read lock,
    /// `detect_lang`'s lazily-loaded lingua models, `split_sentences`) in
    /// which a single stop aimed at that read is wiped on entry. The
    /// displaced passage then plays out in full while this call sits on
    /// the busy flag, and if it outlasts `TAKEOVER_WAIT` the selection the
    /// user actually asked for is dropped: "I selected new text, pressed
    /// ⌘⇧A, and it just kept reading the old passage." Re-asserting puts
    /// the flag back within one poll, and every iteration of
    /// `Player::run`/`wait_for_drain` checks it.
    ///
    /// Re-asserting cannot hit this call's own read: the loop is left the
    /// moment the flag is won, before `Player::speak` is entered, and
    /// `speak` then clears the flag for itself.
    ///
    /// The returned guard releases both on drop — RAII rather than a
    /// plain store after the call returns, because the caller runs
    /// `Player::speak` over arbitrary OCR/selection text, and an unwinding
    /// panic in there must not skip the release. A plain post-call store
    /// would: the unwind jumps straight past it, the process survives (the
    /// panic is inside a spawned thread on every caller), and every later
    /// hotkey press or Service delivery reads `busy == true` forever and
    /// silently no-ops.
    fn acquire_within(
        &self,
        current: Current,
        timeout: Duration,
        displace: bool,
    ) -> Option<BusyRelease<'_>> {
        let deadline = Instant::now() + timeout;
        loop {
            if displace {
                self.stop();
            }
            if self
                .busy
                .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
            {
                break;
            }
            if Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(ACQUIRE_POLL);
        }
        *lock_current(&self.current) = Some(current);
        Some(BusyRelease {
            busy: &self.busy,
            current: &self.current,
        })
    }
}

/// `Mutex::lock` without the poison panic.
///
/// The only things this mutex ever guards are a field assignment and a
/// string comparison, neither of which can panic, so a poisoned flag could
/// only ever be collateral from a panic elsewhere. `BusyRelease::drop`
/// runs *during* an unwind, and a second panic there aborts the process
/// outright — turning the exact class of bug the RAII guard exists to
/// survive into a hard crash.
fn lock_current(m: &Mutex<Option<Current>>) -> MutexGuard<'_, Option<Current>> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Releases the busy flag and clears `current` when dropped, on any exit —
/// normal return, `?`, or a panic unwind.
struct BusyRelease<'a> {
    busy: &'a AtomicBool,
    current: &'a Mutex<Option<Current>>,
}

impl Drop for BusyRelease<'_> {
    fn drop(&mut self) {
        // `current` first, `busy` second, and the order is load-bearing:
        // releasing the flag first would let another thread take it and
        // publish its own `current` before this line ran, and this drop
        // would then erase it. The next press would read "nothing
        // playing" and start a second read on top of a live one.
        *lock_current(self.current) = None;
        self.busy.store(false, Ordering::SeqCst);
    }
}

/// Clears the wrapped flag on drop, on any exit. Same RAII reasoning as
/// `BusyRelease`, for the single-slot takeover flag.
struct FlagRelease<'a>(&'a AtomicBool);

impl Drop for FlagRelease<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}
