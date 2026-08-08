use aloud::play::{player::Player, sink::AudioSink};
use aloud::tts::{Pcm, TtsEngine};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

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
    fn sample_rate(&self) -> u32 {
        44100
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
    assert!(!player.is_speaking(), "player should not report speaking after stop");
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

    player.speak("One. Two. Three.", "en", 1.0).expect("should speak");

    assert_eq!(sink.appended.load(Ordering::SeqCst), 3);
}
