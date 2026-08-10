use aloud::play::{player::Player, sink::AudioSink};
use aloud::tts::{Pcm, TtsEngine};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Fake engine: slow enough that stop can land mid-run, counts calls.
struct FakeEngine {
    calls: AtomicUsize,
}

impl TtsEngine for FakeEngine {
    fn synthesize(&self, _text: &str, _lang: &str, _speed: f32) -> anyhow::Result<Pcm> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        std::thread::sleep(std::time::Duration::from_millis(20));
        Ok(Pcm {
            samples: vec![0.0; 4410],
            sample_rate: 44100,
            duration_s: 0.1,
        })
    }
}

/// Fake sink: drains instantly, needs no audio device.
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
fn stop_prevents_synthesising_the_remaining_sentences() {
    let engine = Arc::new(FakeEngine {
        calls: AtomicUsize::new(0),
    });
    let sink = Arc::new(FakeSink {
        appended: AtomicUsize::new(0),
    });
    let player = Arc::new(Player::new(engine.clone(), sink));

    let p = Arc::clone(&player);
    let handle = std::thread::spawn(move || {
        let long = "One. Two. Three. Four. Five. Six. Seven. Eight. Nine. Ten.";
        let _ = p.speak(long, "en", 1.0);
    });

    std::thread::sleep(std::time::Duration::from_millis(50));
    player.stop();
    handle.join().expect("speak should return after stop");

    let calls = engine.calls.load(Ordering::SeqCst);
    assert!(
        calls < 10,
        "stop should have cut synthesis short, but all {calls} sentences were synthesised"
    );
    assert!(
        !player.is_speaking(),
        "player should not report speaking after stop"
    );
}

#[test]
fn speaking_everything_appends_one_buffer_per_sentence() {
    let engine = Arc::new(FakeEngine {
        calls: AtomicUsize::new(0),
    });
    let sink = Arc::new(FakeSink {
        appended: AtomicUsize::new(0),
    });
    let player = Player::new(engine, Arc::clone(&sink) as Arc<dyn AudioSink>);

    player
        .speak("One. Two. Three.", "en", 1.0)
        .expect("should speak");

    assert_eq!(sink.appended.load(Ordering::SeqCst), 3);
}

/// Sink that models serial playback: each appended buffer occupies the
/// queue for `play_duration` before `queued()` stops counting it, rather
/// than draining instantly like `FakeSink` above. Used to make the
/// one-sentence-ahead throttle in `Player::run` actually engage.
struct SlowDrainSink {
    play_duration: Duration,
    // Wall-clock time each still-outstanding buffer finishes "playing".
    finish_times: Mutex<Vec<Instant>>,
}

impl SlowDrainSink {
    fn new(play_duration: Duration) -> Self {
        Self {
            play_duration,
            finish_times: Mutex::new(Vec::new()),
        }
    }
}

impl AudioSink for SlowDrainSink {
    fn append(&self, _pcm: Pcm) -> anyhow::Result<()> {
        let mut times = self.finish_times.lock().unwrap();
        let now = Instant::now();
        // Buffers play back-to-back: this one starts when the last one
        // (if any, and if still playing) finishes.
        let start = times.last().copied().unwrap_or(now).max(now);
        times.push(start + self.play_duration);
        Ok(())
    }

    fn queued(&self) -> usize {
        let mut times = self.finish_times.lock().unwrap();
        let now = Instant::now();
        times.retain(|&finish| finish > now);
        times.len()
    }

    fn stop(&self) {
        self.finish_times.lock().unwrap().clear();
    }

    // Never paused in these tests; pause/resume are covered by
    // `tests/player_pause.rs` against a sink that models them.
    fn pause(&self) {}
    fn resume(&self) {}
    fn is_paused(&self) -> bool {
        false
    }
}

/// Engine that records how many buffers were still queued (per
/// `SlowDrainSink::queued`) at the moment each `synthesize` call started.
struct ProbingEngine {
    sink: Arc<SlowDrainSink>,
    synth_duration: Duration,
    observed_queue_depths: Mutex<Vec<usize>>,
}

impl TtsEngine for ProbingEngine {
    fn synthesize(&self, _text: &str, _lang: &str, _speed: f32) -> anyhow::Result<Pcm> {
        self.observed_queue_depths
            .lock()
            .unwrap()
            .push(self.sink.queued());
        std::thread::sleep(self.synth_duration);
        Ok(Pcm {
            samples: vec![0.0; 100],
            sample_rate: 44100,
            duration_s: 0.01,
        })
    }
}

#[test]
fn never_synthesises_more_than_one_sentence_ahead_of_playback() {
    // Synthesis is fast (5ms) and playback is slow (40ms), so without the
    // throttle the player would race ahead and pile up buffers.
    let sink = Arc::new(SlowDrainSink::new(Duration::from_millis(40)));
    let engine = Arc::new(ProbingEngine {
        sink: Arc::clone(&sink),
        synth_duration: Duration::from_millis(5),
        observed_queue_depths: Mutex::new(Vec::new()),
    });
    let player = Player::new(
        Arc::clone(&engine) as Arc<dyn TtsEngine>,
        sink as Arc<dyn AudioSink>,
    );

    player
        .speak("One. Two. Three. Four. Five.", "en", 1.0)
        .expect("should speak");

    let depths = engine.observed_queue_depths.lock().unwrap();
    assert_eq!(depths.len(), 5, "expected one synthesize call per sentence");
    for (i, &depth) in depths.iter().enumerate() {
        assert!(
            depth <= 1,
            "sentence {i}: synthesis started with {depth} buffers already queued, \
             the player ran ahead of the one-sentence-ahead bound"
        );
    }
}
