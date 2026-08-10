//! Tauri-free application core: owns the long-lived `Player` (paid once at
//! startup — the ~1.4s Supertonic model load must happen at launch, not on
//! the first hotkey) and serializes calls into the two flows in
//! `actions.rs`.
//!
//! `Player::speak` is documented single-caller/serialized and deliberately
//! does not lock internally — see its doc comment in `src/play/player.rs`.
//! `App` is where that contract gets honoured: a busy flag makes a second
//! `read_region`/`speak_selection` call, arriving while one is still
//! speaking (a second hotkey press, or a Service delivery mid-utterance),
//! a silent no-op rather than a second concurrent `speak()` call that
//! would interleave `sink.append()` calls and race the shared
//! `speaking`/`stop_flag` state. A stop-then-start alternative was
//! considered and rejected: it would interrupt whatever the user is
//! already listening to on every stray double-press, which is worse
//! ordinary-use behaviour than just ignoring the repeat.
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

use crate::capture::RegionSelector;
use crate::ocr::OcrEngine;
use crate::play::player::Player;
use anyhow::Result;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::RwLock;

/// Holds the shared `Player` and the busy guard around it.
pub struct App {
    // RwLock rather than a plain field: a voice change replaces the whole
    // Player (the voice style is loaded into the engine at construction),
    // and that can arrive while a read is in flight. The busy flag already
    // serializes speak() calls; this only guards the swap itself.
    player: RwLock<Player>,
    busy: AtomicBool,
    // f32 bits. Speed is a per-call argument to Player::speak, so it can
    // change between utterances with no reconstruction.
    speed: AtomicU32,
}

impl App {
    pub fn new(player: Player, speed: f32) -> Self {
        Self {
            player: RwLock::new(player),
            busy: AtomicBool::new(false),
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
    pub fn stop(&self) {
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
    /// busy; `Ok(Some(outcome))` when it ran to completion, carrying
    /// which of the three things happened (cancelled / found nothing /
    /// spoke) — the caller (`src/bin/aloud.rs`) uses that to decide
    /// whether a notification is warranted.
    pub fn read_region(
        &self,
        selector: &dyn RegionSelector,
        ocr: &dyn OcrEngine,
    ) -> Result<Option<actions::Outcome>> {
        self.guarded(|| {
            let player = self.player.read().unwrap();
            actions::read_region(selector, ocr, &player, self.speed())
        })
    }

    /// Runs the selection flow (see `actions::speak_selection`), guarded
    /// the same way and against the same busy flag — a region capture and
    /// a Service-delivered selection share the one `Player`, so pressing
    /// the hotkey while a selection is still being read must also be a
    /// no-op, not a second concurrent `speak()`.
    ///
    /// Returns `Ok(false)` when skipped because the player was already
    /// busy, `Ok(true)` when it ran to completion.
    pub fn speak_selection(&self, text: &str) -> Result<bool> {
        self.guarded(|| {
            let player = self.player.read().unwrap();
            actions::speak_selection(text, &player, self.speed())
        })
        .map(|ran| ran.is_some())
    }

    /// Runs `f` unless a call is already in flight, in which case it is
    /// skipped (`Ok(None)`) rather than run concurrently. `T` is generic
    /// so both callers above can keep their own return shape (`read_region`
    /// needs to carry `actions::Outcome`; `speak_selection` only ever
    /// needed a bool, preserved via the `.map` above) without duplicating
    /// this guard.
    fn guarded<T, F: FnOnce() -> Result<T>>(&self, f: F) -> Result<Option<T>> {
        if self
            .busy
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Ok(None);
        }
        // RAII release rather than a plain `store(false, ...)` after `f()`
        // returns: `f()` calls into `Player::speak`, which calls into
        // `TtsEngine::synthesize` over arbitrary OCR/selection text — an
        // unwinding panic in there must not skip the release. A plain
        // post-call store would: the unwind jumps straight past it, the
        // process survives (the panic is inside a spawned thread on every
        // caller), and every later hotkey press or Service delivery reads
        // `busy == true` forever and silently no-ops. `_release`'s `Drop`
        // runs on the ordinary-return path and on an unwind alike.
        let _release = BusyRelease(&self.busy);
        f().map(Some)
    }
}

/// Stores `false` into the wrapped flag when dropped, on any exit —
/// normal return, `?`, or a panic unwind.
struct BusyRelease<'a>(&'a AtomicBool);

impl Drop for BusyRelease<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}
