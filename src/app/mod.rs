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

pub mod actions;

use crate::capture::RegionSelector;
use crate::ocr::OcrEngine;
use crate::play::player::Player;
use anyhow::Result;
use std::sync::atomic::{AtomicBool, Ordering};

/// Holds the shared `Player` and the busy guard around it.
pub struct App {
    player: Player,
    busy: AtomicBool,
    speed: f32,
}

impl App {
    pub fn new(player: Player, speed: f32) -> Self {
        Self {
            player,
            busy: AtomicBool::new(false),
            speed,
        }
    }

    /// Stops whatever is currently speaking. Safe to call at any time,
    /// including when nothing is speaking.
    pub fn stop(&self) {
        self.player.stop();
    }

    /// Runs the region flow (see `actions::read_region`), guarded so a
    /// second call while one is already in flight is a no-op.
    ///
    /// Returns `Ok(false)` when skipped because the player was already
    /// busy, `Ok(true)` when it ran to completion.
    pub fn read_region(&self, selector: &dyn RegionSelector, ocr: &dyn OcrEngine) -> Result<bool> {
        self.guarded(|| actions::read_region(selector, ocr, &self.player, self.speed))
    }

    /// Runs the selection flow (see `actions::speak_selection`), guarded
    /// the same way and against the same busy flag — a region capture and
    /// a Service-delivered selection share the one `Player`, so pressing
    /// the hotkey while a selection is still being read must also be a
    /// no-op, not a second concurrent `speak()`.
    pub fn speak_selection(&self, text: &str) -> Result<bool> {
        self.guarded(|| actions::speak_selection(text, &self.player, self.speed))
    }

    fn guarded<F: FnOnce() -> Result<()>>(&self, f: F) -> Result<bool> {
        if self
            .busy
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Ok(false);
        }
        let result = f();
        self.busy.store(false, Ordering::SeqCst);
        result.map(|()| true)
    }
}
