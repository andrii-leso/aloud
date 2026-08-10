//! Pause/resume: the stall-watchdog interaction, and the guarantee that
//! pausing never costs already-synthesised audio.

use aloud::play::player::{Player, SpeakEnd, StallWatch};
use aloud::play::sink::AudioSink;
use aloud::tts::{Pcm, TtsEngine};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The watchdog's timeout. Duplicated here rather than exported: the test
/// asserts against the documented 30s contract, so a change to the
/// constant should break this test rather than silently move with it.
const STALL_TIMEOUT: Duration = Duration::from_secs(30);

// ---------------------------------------------------------------------
// The watchdog, against a synthetic clock.
//
// These drive `StallWatch` directly instead of sitting through 30s of
// wall time in the real drain loop. `wait_for_drain` contains no other
// stall logic — it polls the sink and forwards to `observe` — so this is
// the whole decision under test.
// ---------------------------------------------------------------------

#[test]
fn a_paused_queue_is_not_a_stalled_device() {
    let t0 = Instant::now();
    // Depth 2, frozen — exactly what a paused sink reports, by definition.
    let mut watch = StallWatch::new(2, t0);

    // Five minutes paused: ten times the stall timeout.
    for secs in 1..=300 {
        let stalled = watch.observe(2, true, t0 + Duration::from_secs(secs));
        assert!(
            !stalled,
            "the watchdog aborted a paused read after {secs}s - a paused sink's \
             queue depth is frozen by definition, so pause must not count \
             toward the {STALL_TIMEOUT:?} stall timeout"
        );
    }
}

#[test]
fn a_frozen_queue_that_is_not_paused_still_trips_the_watchdog() {
    // The control: the guard must keep catching a genuinely dead device.
    let t0 = Instant::now();
    let mut watch = StallWatch::new(2, t0);

    assert!(
        !watch.observe(2, false, t0 + STALL_TIMEOUT - Duration::from_millis(1)),
        "should not fire before the timeout"
    );
    assert!(
        watch.observe(2, false, t0 + STALL_TIMEOUT),
        "a frozen, unpaused queue is a stalled device and must still abort"
    );
}

#[test]
fn resuming_gives_the_watchdog_a_fresh_budget() {
    // A long pause must not leave the read on a hair trigger: the time
    // spent paused is not merely capped, it does not count at all.
    let t0 = Instant::now();
    let mut watch = StallWatch::new(2, t0);

    let paused_for = Duration::from_secs(600);
    for secs in 1..=600 {
        assert!(!watch.observe(2, true, t0 + Duration::from_secs(secs)));
    }

    assert!(
        !watch.observe(
            2,
            false,
            t0 + paused_for + STALL_TIMEOUT - Duration::from_millis(1)
        ),
        "after a 10-minute pause the watchdog must start its 30s over, \
         not fire the instant playback resumes"
    );
    assert!(
        watch.observe(2, false, t0 + paused_for + STALL_TIMEOUT),
        "and it must still fire 30s after the resume if nothing drains"
    );
}

// ---------------------------------------------------------------------
// End-to-end through a real `Player`, against a sink that models rodio's
// pause semantics: output halts, the queue is preserved, and playback
// continues from where it stopped.
// ---------------------------------------------------------------------

/// Serial-playback sink that can be paused. While paused it consumes no
/// samples and its depth is frozen — the same shape as rodio's
/// `Pausable`, which yields silence without advancing the inner source.
struct PausableSink {
    play_duration: Duration,
    state: Mutex<PausableState>,
    appended: AtomicUsize,
    paused: AtomicBool,
    /// Set if `append` is ever called while paused with an empty queue,
    /// i.e. audio was handed to a sink that would silently swallow it.
    appended_into_dead_sink: AtomicBool,
}

struct PausableState {
    /// Remaining playback time of each outstanding buffer, oldest first.
    remaining: Vec<Duration>,
    last_tick: Instant,
}

impl PausableSink {
    fn new(play_duration: Duration) -> Self {
        Self {
            play_duration,
            state: Mutex::new(PausableState {
                remaining: Vec::new(),
                last_tick: Instant::now(),
            }),
            appended: AtomicUsize::new(0),
            paused: AtomicBool::new(false),
            appended_into_dead_sink: AtomicBool::new(false),
        }
    }

    /// Advances playback by the wall time since the last call — unless
    /// paused, in which case the clock is consumed but no audio is.
    fn tick(&self, st: &mut PausableState) {
        let now = Instant::now();
        let mut elapsed = now.duration_since(st.last_tick);
        st.last_tick = now;
        if self.paused.load(Ordering::SeqCst) {
            return;
        }
        while elapsed > Duration::ZERO {
            let Some(head) = st.remaining.first_mut() else {
                break;
            };
            if *head > elapsed {
                *head -= elapsed;
                break;
            }
            elapsed -= *head;
            st.remaining.remove(0);
        }
    }
}

impl AudioSink for PausableSink {
    fn append(&self, _pcm: Pcm) -> anyhow::Result<()> {
        let mut st = self.state.lock().unwrap();
        self.tick(&mut st);
        if self.paused.load(Ordering::SeqCst) && st.remaining.is_empty() {
            self.appended_into_dead_sink.store(true, Ordering::SeqCst);
        }
        st.remaining.push(self.play_duration);
        self.appended.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn queued(&self) -> usize {
        let mut st = self.state.lock().unwrap();
        self.tick(&mut st);
        st.remaining.len()
    }

    fn stop(&self) {
        let mut st = self.state.lock().unwrap();
        st.remaining.clear();
        st.last_tick = Instant::now();
        // Per the trait contract: stop leaves the sink ready to play.
        self.paused.store(false, Ordering::SeqCst);
    }

    fn pause(&self) {
        let mut st = self.state.lock().unwrap();
        self.tick(&mut st);
        self.paused.store(true, Ordering::SeqCst);
    }

    fn resume(&self) {
        let mut st = self.state.lock().unwrap();
        st.last_tick = Instant::now();
        self.paused.store(false, Ordering::SeqCst);
    }

    fn is_paused(&self) -> bool {
        self.paused.load(Ordering::SeqCst)
    }
}

struct CountingEngine {
    calls: AtomicUsize,
}

impl TtsEngine for CountingEngine {
    fn synthesize(&self, _text: &str, _lang: &str, _speed: f32) -> anyhow::Result<Pcm> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(Pcm {
            samples: vec![0.0; 100],
            sample_rate: 44100,
            duration_s: 0.01,
        })
    }
}

fn spawn_speaking(
    player: Arc<Player>,
    text: &'static str,
) -> std::thread::JoinHandle<anyhow::Result<SpeakEnd>> {
    std::thread::spawn(move || player.speak(text, "en", 1.0))
}

#[test]
fn pause_holds_playback_and_resume_finishes_without_resynthesising() {
    let sink = Arc::new(PausableSink::new(Duration::from_millis(60)));
    let engine = Arc::new(CountingEngine {
        calls: AtomicUsize::new(0),
    });
    let player = Arc::new(Player::new(
        Arc::clone(&engine) as Arc<dyn TtsEngine>,
        Arc::clone(&sink) as Arc<dyn AudioSink>,
    ));

    let handle = spawn_speaking(Arc::clone(&player), "One. Two. Three. Four. Five.");

    // Let it get going, then pause mid-read.
    std::thread::sleep(Duration::from_millis(80));
    assert!(player.pause(), "pause should take while speaking");
    assert!(player.is_paused());

    // Nothing may drain while paused: buffered audio has to survive.
    //
    // Sampled after a settling delay, not immediately: pause halts
    // *playback*, and the synthesis thread is one sentence behind it, so
    // a single further buffer can still land after the pause takes
    // effect (`wait_for_drain(2)` had already returned). That append is
    // the documented one-ahead bound, not a drain. Once it has landed,
    // the depth must then hold perfectly still across five buffers'
    // worth of playback time.
    std::thread::sleep(Duration::from_millis(150));
    let depth = sink.queued();
    assert!(depth > 0, "the pause should have caught buffered audio");
    assert!(
        depth <= 2,
        "the one-sentence-ahead bound must still hold while paused, got {depth}"
    );
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(
        sink.queued(),
        depth,
        "a paused sink must not drain - buffered audio has to survive the pause"
    );
    let synthesised_while_paused = engine.calls.load(Ordering::SeqCst);
    assert!(
        synthesised_while_paused < 5,
        "the player ran the whole text ahead while paused ({synthesised_while_paused} \
         sentences) - the look-ahead bound is not holding"
    );

    player.resume();
    assert!(!player.is_paused());

    handle
        .join()
        .expect("speak thread should not panic")
        .expect("a paused read must not abort as a stalled device");

    assert_eq!(
        engine.calls.load(Ordering::SeqCst),
        5,
        "exactly one synthesis per sentence - resume must continue, never re-read"
    );
    assert!(
        synthesised_while_paused <= 5,
        "the one-sentence-ahead bound must still hold while paused"
    );
    assert_eq!(
        sink.appended.load(Ordering::SeqCst),
        5,
        "one buffer appended per sentence, none replayed"
    );
    assert!(!player.is_speaking());
    assert!(
        !player.is_paused(),
        "a finished read must not be left paused"
    );
}

#[test]
fn pause_is_refused_when_nothing_is_speaking() {
    let sink = Arc::new(PausableSink::new(Duration::from_millis(10)));
    let player = Player::new(
        Arc::new(CountingEngine {
            calls: AtomicUsize::new(0),
        }),
        Arc::clone(&sink) as Arc<dyn AudioSink>,
    );

    assert!(!player.pause(), "pausing an idle player must be a no-op");
    assert!(
        !sink.is_paused(),
        "an idle sink must not be left paused - the next read would be silent"
    );
}

#[test]
fn stopping_while_paused_leaves_a_playable_sink() {
    let sink = Arc::new(PausableSink::new(Duration::from_millis(60)));
    let engine = Arc::new(CountingEngine {
        calls: AtomicUsize::new(0),
    });
    let player = Arc::new(Player::new(
        Arc::clone(&engine) as Arc<dyn TtsEngine>,
        Arc::clone(&sink) as Arc<dyn AudioSink>,
    ));

    let handle = spawn_speaking(Arc::clone(&player), "One. Two. Three. Four. Five.");
    std::thread::sleep(Duration::from_millis(80));
    player.pause();
    player.stop();
    handle.join().expect("speak should return after stop").ok();

    assert!(
        !sink.is_paused(),
        "Stop while paused must clear the pause, or the next read plays into a dead sink"
    );

    // The next read must actually be heard.
    let before = sink.appended.load(Ordering::SeqCst);
    player
        .speak("Six. Seven.", "en", 1.0)
        .expect("should speak");
    assert_eq!(
        sink.appended.load(Ordering::SeqCst) - before,
        2,
        "a read after a paused stop must play normally"
    );
    assert!(
        !sink.appended_into_dead_sink.load(Ordering::SeqCst),
        "audio was appended into a paused, empty sink - it would never be heard"
    );
}

// ---------------------------------------------------------------------
// Against the REAL rodio sink and a real audio device.
//
// Everything above runs on a test double that models rodio's documented
// behaviour. This one checks the model is right, on this machine — the
// same "measure it, don't infer it" standard the CGEventTap question was
// settled to (docs/media-key-control-research.md §4).
//
// `#[ignore]`d: it opens the default output device, which no headless
// runner has. The samples are silence, so running it makes no noise.
// Run with:
//   cargo test --release --test player_pause -- --ignored
// ---------------------------------------------------------------------

#[test]
#[ignore = "opens the default audio device; run explicitly with --ignored"]
fn rodio_really_pauses_without_discarding_buffers() {
    use aloud::play::sink::RodioSink;

    let sink = RodioSink::new().expect("default audio device should open");

    // Three buffers of 0.5s silence each: 1.5s of audio queued up front,
    // exactly the ahead-of-playback shape the real player produces.
    let half_second = |rate: u32| Pcm {
        samples: vec![0.0; (rate / 2) as usize],
        sample_rate: rate,
        duration_s: 0.5,
    };
    for _ in 0..3 {
        sink.append(half_second(44100)).expect("append");
    }
    assert_eq!(sink.queued(), 3, "three buffers should be outstanding");

    // Pause almost immediately, then hold for longer than the whole
    // queue would have taken to play.
    std::thread::sleep(Duration::from_millis(100));
    sink.pause();
    assert!(sink.is_paused(), "rodio should report the sink as paused");

    let depth_at_pause = sink.queued();
    std::thread::sleep(Duration::from_secs(3));
    assert_eq!(
        sink.queued(),
        depth_at_pause,
        "rodio drained {depth_at_pause} -> {} while paused: buffered audio is \
         being consumed, so this is not a real pause",
        sink.queued()
    );
    assert!(
        depth_at_pause > 0,
        "the queue emptied before the pause landed - test is not measuring anything"
    );

    // Resume and let it finish: the audio must still be there to play.
    sink.resume();
    assert!(!sink.is_paused());
    let deadline = Instant::now() + Duration::from_secs(10);
    while sink.queued() > 0 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(
        sink.queued(),
        0,
        "the queue did not drain after resume - playback did not actually restart"
    );

    // And the stop contract, on the real sink.
    sink.append(half_second(44100)).expect("append");
    sink.pause();
    sink.stop();
    assert!(
        !sink.is_paused(),
        "RodioSink::stop must clear rodio's pause flag - rodio's own stop() does not"
    );
}
