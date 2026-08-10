//! Phase 2: the Now Playing session boundary, and the routing decision
//! for a system remote command.
//!
//! What these cover is everything *below* the OS boundary — that `Player`
//! publishes exactly the states the media key depends on, at exactly the
//! right moments. What they cannot cover is macOS's side of it: whether
//! `.playing` actually wins the media key from Music.app, and whether
//! `.stopped` actually hands it back. That needs the bundled app and a
//! human finger on F8 — see `docs/2026-08-10-media-key-phase2.md`.
//!
//! The one property worth stating plainly: **`Stopped` on every exit
//! path.** `Playing` that is never followed by `Stopped` is Aloud holding
//! the owner's media key with nothing to play, which is worse than not
//! shipping the feature at all.

use aloud::now_playing::{should_toggle, NowPlaying, PlaybackState, RemoteCommand};
use aloud::play::player::Player;
use aloud::play::sink::AudioSink;
use aloud::tts::{Pcm, TtsEngine};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Records every published state, in order.
#[derive(Default)]
struct RecordingNowPlaying {
    published: Mutex<Vec<PlaybackState>>,
}

impl RecordingNowPlaying {
    fn states(&self) -> Vec<PlaybackState> {
        self.published.lock().unwrap().clone()
    }

    fn last(&self) -> Option<PlaybackState> {
        self.published.lock().unwrap().last().copied()
    }
}

impl NowPlaying for RecordingNowPlaying {
    fn publish(&self, state: PlaybackState) {
        self.published.lock().unwrap().push(state);
    }
}

/// Drains instantly: `queued()` is always 0, so `wait_for_drain` returns
/// on its first poll and a `speak()` runs to completion without sleeping.
#[derive(Default)]
struct InstantSink {
    paused: AtomicBool,
}

impl AudioSink for InstantSink {
    fn append(&self, _pcm: Pcm) -> anyhow::Result<()> {
        Ok(())
    }
    fn queued(&self) -> usize {
        0
    }
    fn stop(&self) {
        self.paused.store(false, Ordering::SeqCst);
    }
    fn pause(&self) {
        self.paused.store(true, Ordering::SeqCst);
    }
    fn resume(&self) {
        self.paused.store(false, Ordering::SeqCst);
    }
    fn is_paused(&self) -> bool {
        self.paused.load(Ordering::SeqCst)
    }
}

/// Plays nothing until the test says so, which parks `speak()` inside
/// `wait_for_drain` with a read genuinely in flight — the only state in
/// which a pause is meaningful.
#[derive(Default)]
struct HoldingSink {
    depth: AtomicUsize,
    paused: AtomicBool,
    /// Cleared until the test releases it. While clear the queue depth
    /// never falls, so the read cannot run away before the test reaches
    /// its `pause()` call.
    draining: AtomicBool,
}

impl HoldingSink {
    fn release(&self) {
        self.draining.store(true, Ordering::SeqCst);
    }
}

impl AudioSink for HoldingSink {
    fn append(&self, _pcm: Pcm) -> anyhow::Result<()> {
        self.depth.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    fn queued(&self) -> usize {
        let d = self.depth.load(Ordering::SeqCst);
        // A paused sink's depth is frozen by definition; an unreleased
        // one is simply not playing yet. Either way, nothing drains.
        if self.paused.load(Ordering::SeqCst) || !self.draining.load(Ordering::SeqCst) {
            return d;
        }
        if d > 0 {
            self.depth.fetch_sub(1, Ordering::SeqCst);
        }
        d
    }
    fn stop(&self) {
        self.depth.store(0, Ordering::SeqCst);
        self.paused.store(false, Ordering::SeqCst);
    }
    fn pause(&self) {
        self.paused.store(true, Ordering::SeqCst);
    }
    fn resume(&self) {
        self.paused.store(false, Ordering::SeqCst);
    }
    fn is_paused(&self) -> bool {
        self.paused.load(Ordering::SeqCst)
    }
}

struct QuietEngine;

impl TtsEngine for QuietEngine {
    fn synthesize(&self, _text: &str, _lang: &str, _speed: f32) -> anyhow::Result<Pcm> {
        Ok(Pcm {
            samples: vec![0.0; 100],
            sample_rate: 44100,
            duration_s: 0.01,
        })
    }
}

/// Fails on the Nth call, to exercise the error unwind.
struct FailingEngine;

impl TtsEngine for FailingEngine {
    fn synthesize(&self, _text: &str, _lang: &str, _speed: f32) -> anyhow::Result<Pcm> {
        anyhow::bail!("synthesis exploded")
    }
}

/// Panics rather than returning `Err`, which is the case a plain
/// `store(false)` at the bottom of `speak()` would skip.
struct PanickingEngine;

impl TtsEngine for PanickingEngine {
    fn synthesize(&self, _text: &str, _lang: &str, _speed: f32) -> anyhow::Result<Pcm> {
        panic!("synthesis panicked over whatever was on screen")
    }
}

fn player_with(
    engine: Arc<dyn TtsEngine>,
    sink: Arc<dyn AudioSink>,
    np: Arc<RecordingNowPlaying>,
) -> Player {
    Player::new(engine, sink).with_now_playing(np as Arc<dyn NowPlaying>)
}

#[test]
fn a_read_takes_the_session_and_gives_it_back() {
    let np = Arc::new(RecordingNowPlaying::default());
    let player = player_with(
        Arc::new(QuietEngine),
        Arc::new(InstantSink::default()),
        Arc::clone(&np),
    );

    player.speak("One. Two.", "en", 1.0).unwrap();

    assert_eq!(
        np.states(),
        vec![PlaybackState::Playing, PlaybackState::Stopped],
        "a read must publish Playing at its start and Stopped at its end - \
         Stopped is what hands the media key back to whatever was playing \
         before"
    );
}

#[test]
fn an_empty_text_never_takes_the_session() {
    let np = Arc::new(RecordingNowPlaying::default());
    let player = player_with(
        Arc::new(QuietEngine),
        Arc::new(InstantSink::default()),
        Arc::clone(&np),
    );

    // `speak` returns early before anything is spoken.
    player.speak("   ", "en", 1.0).unwrap();

    assert!(
        np.states().is_empty(),
        "nothing was ever spoken, so the media key must not have been \
         taken and released - got {:?}",
        np.states()
    );
}

#[test]
fn a_failed_read_still_gives_the_session_back() {
    let np = Arc::new(RecordingNowPlaying::default());
    let player = player_with(
        Arc::new(FailingEngine),
        Arc::new(InstantSink::default()),
        Arc::clone(&np),
    );

    assert!(player.speak("One.", "en", 1.0).is_err());

    assert_eq!(
        np.last(),
        Some(PlaybackState::Stopped),
        "an error unwind must still release the media key"
    );
}

#[test]
fn a_panicking_read_still_gives_the_session_back() {
    let np = Arc::new(RecordingNowPlaying::default());
    let player = player_with(
        Arc::new(PanickingEngine),
        Arc::new(InstantSink::default()),
        Arc::clone(&np),
    );

    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = player.speak("One.", "en", 1.0);
    }));
    assert!(panicked.is_err(), "the engine was supposed to panic");

    assert_eq!(
        np.last(),
        Some(PlaybackState::Stopped),
        "a panic inside synthesis must still release the media key - \
         otherwise Aloud holds F8 for the life of the process with \
         nothing to play, which is the exact failure this phase is gated \
         against"
    );
    assert!(
        !player.is_speaking(),
        "the panic unwind must also clear `speaking`, or every later \
         pause is accepted with nothing playing"
    );
}

#[test]
fn pausing_and_resuming_a_live_read_keeps_the_session() {
    let np = Arc::new(RecordingNowPlaying::default());
    let sink = Arc::new(HoldingSink::default());
    let player = Arc::new(player_with(
        Arc::new(QuietEngine),
        Arc::clone(&sink) as Arc<dyn AudioSink>,
        Arc::clone(&np),
    ));

    let speaking = {
        let player = Arc::clone(&player);
        std::thread::spawn(move || player.speak("One. Two. Three. Four. Five.", "en", 1.0))
    };

    // Wait until the read is genuinely under way — and, because the sink
    // is holding, parked mid-utterance rather than about to finish.
    let mut waited = Duration::ZERO;
    while sink.queued() < 2 && waited < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(5));
        waited += Duration::from_millis(5);
    }
    assert!(player.is_speaking(), "the read never started");

    assert!(player.pause());
    assert_eq!(
        np.last(),
        Some(PlaybackState::Paused),
        "pause must publish Paused, NOT Stopped - Stopped would release \
         the media key and the next press could never resume Aloud"
    );

    assert!(!player.resume());
    assert_eq!(
        np.last(),
        Some(PlaybackState::Playing),
        "resume must publish Playing again"
    );

    sink.release();
    speaking.join().unwrap().unwrap();
    assert_eq!(
        np.last(),
        Some(PlaybackState::Stopped),
        "and the finished read must still release"
    );
}

#[test]
fn pausing_with_nothing_speaking_publishes_nothing() {
    let np = Arc::new(RecordingNowPlaying::default());
    let player = player_with(
        Arc::new(QuietEngine),
        Arc::new(InstantSink::default()),
        Arc::clone(&np),
    );

    assert!(!player.pause(), "pause is refused when nothing is speaking");
    assert!(
        np.states().is_empty(),
        "a refused pause must not publish anything - got {:?}",
        np.states()
    );
}

// ---------------------------------------------------------------------
// Command routing. Pure, so it needs neither a player nor an OS.
// ---------------------------------------------------------------------

#[test]
fn toggle_always_flips() {
    assert!(should_toggle(RemoteCommand::TogglePlayPause, false));
    assert!(should_toggle(RemoteCommand::TogglePlayPause, true));
}

#[test]
fn directional_commands_are_idempotent() {
    assert!(
        should_toggle(RemoteCommand::Play, true),
        "Play on a paused read resumes it"
    );
    assert!(
        !should_toggle(RemoteCommand::Play, false),
        "Play on a playing read must do nothing - blindly toggling here \
         would let one physical key press that delivered both a \
         directional command and the toggle flip twice and look dead"
    );
    assert!(
        should_toggle(RemoteCommand::Pause, false),
        "Pause on a playing read pauses it"
    );
    assert!(
        !should_toggle(RemoteCommand::Pause, true),
        "Pause on an already-paused read must do nothing"
    );
}

#[test]
fn play_while_idle_does_not_start_a_read() {
    // Idle means not paused, so `Play` resolves to "nothing to resume".
    // The media key must never trigger a region drag or a selection the
    // user did not make.
    assert!(!should_toggle(RemoteCommand::Play, false));
}
