//! Proves the real (non-fake) half of Task 9's voice swap: that
//! `SupertonicEngine::spawn` succeeds for a second voice and that
//! `App::swap_player` produces a `Player` that actually speaks through the
//! new engine — exercised directly against the library, since the harness
//! available for this task cannot drive the Tauri settings UI (no
//! Accessibility grant, no synthetic keystrokes into an accessory app; see
//! the Task 9 report).
//!
//! The concurrency contract itself — the write lock blocking against an
//! in-flight read, so a voice change never cuts off the utterance it lands
//! during — is proven deterministically with a fake engine in
//! `tests/actions.rs`
//! (`swap_player_blocks_until_the_in_flight_utterance_releases_the_read_lock`).
//! This file only needs to show the real engine path works end to end, so
//! it stays sequential (no timing races) and keeps both utterances short —
//! real synthesis on this machine is load-dependent and can run to several
//! seconds per sentence (see `Aloud/CLAUDE.md` constraint 5).
//!
//! Real model loads (~1.4s per voice), like the existing `engine_thread.rs`
//! and `latency_budget.rs`. Uses a `FakeSink` so it never touches the audio
//! device or makes noise during `cargo test`.

use aloud::app::{App, SelectionOutcome};
use aloud::play::player::Player;
use aloud::play::sink::AudioSink;
use aloud::tts::supertonic_engine::SupertonicEngine;
use aloud::tts::Pcm;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// Drains instantly; no audio device needed and no noise during `cargo
/// test` — this proves the swap mechanics, not that audio plays.
struct FakeSink {
    appended: AtomicUsize,
}

impl AudioSink for FakeSink {
    fn append(&self, _pcm: Pcm) -> anyhow::Result<()> {
        self.appended.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    fn queued(&self) -> usize {
        0
    }
    fn stop(&self) {}
    // Never paused in these tests; pause/resume are covered by
    // `tests/player_pause.rs` against a sink that models them.
    fn pause(&self) {}
    fn resume(&self) {}
    fn is_paused(&self) -> bool {
        false
    }
}

#[test]
fn voice_swap_spawns_the_new_engine_and_speaks_through_it() {
    let sink: Arc<dyn AudioSink> = Arc::new(FakeSink {
        appended: AtomicUsize::new(0),
    });

    let f5 = Arc::new(SupertonicEngine::spawn("F5").expect("F5 should spawn"));
    let player = Player::new(f5, Arc::clone(&sink));
    let app = App::new(player, 1.0);

    assert_eq!(
        app.speak_selection("Hello.").unwrap(),
        SelectionOutcome::Spoke { replaced: false },
        "the app should speak on the original F5 engine"
    );

    let m5 = SupertonicEngine::spawn("M5").expect("M5 should spawn");
    let new_player = Player::new(Arc::new(m5), Arc::clone(&sink));
    app.swap_player(new_player);

    assert_eq!(
        app.speak_selection("Hello again.").unwrap(),
        SelectionOutcome::Spoke { replaced: false },
        "the app should still speak after swapping to the M5 engine — a \
         swap that silently broke the Player would surface here as an \
         Err or a Skipped"
    );
}
