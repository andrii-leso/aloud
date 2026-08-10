//! Tests for the two speaking pipelines in `src/app/actions.rs`, and for
//! the busy guard in `src/app/mod.rs` that serializes access to the shared
//! `Player`.

use aloud::app::actions::{read_region, speak_selection, Outcome};
use aloud::app::{App, SelectionOutcome};
use aloud::capture::RegionSelector;
use aloud::ocr::OcrEngine;
use aloud::play::player::Player;
use aloud::play::sink::AudioSink;
use aloud::tts::{Pcm, TtsEngine};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------
// Fakes: capture / OCR seams
// ---------------------------------------------------------------------

/// A selector that reports a deliberate user cancel.
struct CancelSelector;
impl RegionSelector for CancelSelector {
    fn select(&self) -> anyhow::Result<Option<PathBuf>> {
        Ok(None)
    }
}

/// A selector that reports success, handing back the path to a real temp
/// file created on disk — real, so deletion can actually be observed.
struct FileSelector {
    path: PathBuf,
}
impl RegionSelector for FileSelector {
    fn select(&self) -> anyhow::Result<Option<PathBuf>> {
        Ok(Some(self.path.clone()))
    }
}

/// Creates a uniquely-named temp file standing in for a captured
/// screenshot, and returns its path.
fn make_temp_image(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "aloud-actions-test-{name}-{}.png",
        std::process::id()
    ));
    std::fs::write(&path, b"fake png bytes").expect("write temp fixture");
    path
}

/// An OCR engine that reports success with the given text, regardless of
/// what path it is given.
struct SuccessOcr {
    text: &'static str,
}
impl OcrEngine for SuccessOcr {
    fn recognise(&self, _image_path: &Path) -> anyhow::Result<String> {
        Ok(self.text.to_string())
    }
}

/// An OCR engine that reports success with nothing but whitespace — the
/// "found no text" case.
struct EmptyOcr;
impl OcrEngine for EmptyOcr {
    fn recognise(&self, _image_path: &Path) -> anyhow::Result<String> {
        Ok("   \n\t  ".to_string())
    }
}

/// An OCR engine that always fails.
struct FailingOcr;
impl OcrEngine for FailingOcr {
    fn recognise(&self, _image_path: &Path) -> anyhow::Result<String> {
        Err(anyhow::anyhow!("vision framework exploded"))
    }
}

/// An OCR engine that must never be called — used to prove a cancelled
/// capture short-circuits before OCR runs at all.
struct UnreachableOcr;
impl OcrEngine for UnreachableOcr {
    fn recognise(&self, _image_path: &Path) -> anyhow::Result<String> {
        panic!("OCR must not run after a cancelled capture");
    }
}

// ---------------------------------------------------------------------
// Fakes: TTS engine / audio sink seams (mirrors tests/player_stop.rs)
// ---------------------------------------------------------------------

/// Records every `synthesize` call: how many, and the `lang` passed each
/// time. Instant — no sleep — except where a test explicitly wants a slow
/// one (see `SlowEngine` below).
struct RecordingEngine {
    calls: AtomicUsize,
    langs: Mutex<Vec<String>>,
}
impl RecordingEngine {
    fn new() -> Self {
        Self {
            calls: AtomicUsize::new(0),
            langs: Mutex::new(Vec::new()),
        }
    }
}
impl TtsEngine for RecordingEngine {
    fn synthesize(&self, _text: &str, lang: &str, _speed: f32) -> anyhow::Result<Pcm> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.langs.lock().unwrap().push(lang.to_string());
        Ok(Pcm {
            samples: vec![0.0; 10],
            sample_rate: 44100,
            duration_s: 0.01,
        })
    }
}

/// Like `RecordingEngine`, but each call blocks for `delay` — long enough
/// for a concurrent second call to have a real chance to land while the
/// first is still in flight, which is exactly what the busy-guard test
/// needs to exercise.
struct SlowEngine {
    calls: AtomicUsize,
    delay: Duration,
    /// Live and peak concurrent `synthesize` calls. `Player::speak` is
    /// documented single-caller — two concurrent ones interleave
    /// `sink.append()` and race the shared `speaking`/`stop_flag` state —
    /// so this is the property the busy flag's compare-exchange actually
    /// buys, and the only direct way to observe it from outside.
    inflight: AtomicUsize,
    max_inflight: AtomicUsize,
}
impl SlowEngine {
    fn new(delay: Duration) -> Self {
        Self {
            calls: AtomicUsize::new(0),
            delay,
            inflight: AtomicUsize::new(0),
            max_inflight: AtomicUsize::new(0),
        }
    }
}
impl TtsEngine for SlowEngine {
    fn synthesize(&self, _text: &str, _lang: &str, _speed: f32) -> anyhow::Result<Pcm> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let now = self.inflight.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_inflight.fetch_max(now, Ordering::SeqCst);
        std::thread::sleep(self.delay);
        self.inflight.fetch_sub(1, Ordering::SeqCst);
        Ok(Pcm {
            samples: vec![0.0; 10],
            sample_rate: 44100,
            duration_s: 0.01,
        })
    }
}

/// Always fails — for exercising the busy guard's release on an ordinary
/// `Err` return (as opposed to a panic; see `PanicOnceEngine` below).
struct FailingEngine {
    calls: AtomicUsize,
}
impl TtsEngine for FailingEngine {
    fn synthesize(&self, _text: &str, _lang: &str, _speed: f32) -> anyhow::Result<Pcm> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(anyhow::anyhow!("simulated synthesis failure"))
    }
}

/// Panics on its first call, then behaves like `RecordingEngine`. Models a
/// `TtsEngine` (or, more realistically, the chunker feeding it — see
/// `src/text/chunk.rs`'s manual `Vec<char>` slicing over arbitrary OCR
/// text) panicking on some adversarial input rather than returning `Err`.
struct PanicOnceEngine {
    calls: AtomicUsize,
}
impl TtsEngine for PanicOnceEngine {
    fn synthesize(&self, _text: &str, _lang: &str, _speed: f32) -> anyhow::Result<Pcm> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst);
        if n == 0 {
            panic!("simulated TTS engine panic");
        }
        Ok(Pcm {
            samples: vec![0.0; 10],
            sample_rate: 44100,
            duration_s: 0.01,
        })
    }
}

/// Drains instantly; no audio device needed.
struct FakeSink;
impl AudioSink for FakeSink {
    fn append(&self, _pcm: Pcm) -> anyhow::Result<()> {
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

const ENGLISH_TEXT: &str = "The applicant must submit the completed form within four weeks.";

/// A second, clearly different passage — for the paths that turn on the
/// delivered text differing from what is playing.
const OTHER_ENGLISH_TEXT: &str =
    "The committee will publish its decision on the following Monday morning.";

/// Three sentences, so a read of it is still in flight — and has already
/// put a buffer into the sink — when a second press lands.
const ENGLISH_THREE_SENTENCES: &str = "The applicant must submit the completed form within four \
     weeks. A written decision follows shortly after that. No further documents are required.";

/// Builds an `App` around a fresh `RecordingEngine`/`FakeSink` pair at the
/// given speed. Used by the speed-liveness tests below, which only care
/// about `App::speed`/`set_speed`, never about what was actually spoken.
fn build_test_app(speed: f32) -> App {
    let engine = Arc::new(RecordingEngine::new());
    let player = Player::new(engine, Arc::new(FakeSink));
    App::new(player, speed)
}

// ---------------------------------------------------------------------
// read_region
// ---------------------------------------------------------------------

#[test]
fn cancelled_region_speaks_nothing_and_is_not_an_error() {
    let engine = Arc::new(RecordingEngine::new());
    let player = Player::new(engine.clone(), Arc::new(FakeSink));

    let result = read_region(&CancelSelector, &UnreachableOcr, &player, 1.0);

    assert!(result.is_ok(), "a cancelled capture must not be an error");
    assert_eq!(
        result.unwrap(),
        Outcome::Cancelled,
        "a cancel must be reported as Outcome::Cancelled, not silently as Ok(())"
    );
    assert_eq!(
        engine.calls.load(Ordering::SeqCst),
        0,
        "nothing should have been spoken"
    );
}

#[test]
fn empty_ocr_output_speaks_nothing_and_deletes_the_temp_image() {
    let image = make_temp_image("empty-ocr");
    let selector = FileSelector {
        path: image.clone(),
    };
    let engine = Arc::new(RecordingEngine::new());
    let player = Player::new(engine.clone(), Arc::new(FakeSink));

    let result = read_region(&selector, &EmptyOcr, &player, 1.0);

    assert!(result.is_ok());
    assert_eq!(
        result.unwrap(),
        Outcome::Empty,
        "OCR success with no usable text must be reported as Outcome::Empty"
    );
    assert_eq!(engine.calls.load(Ordering::SeqCst), 0);
    assert!(
        !image.exists(),
        "the temp image must be deleted even though there was nothing to speak"
    );
}

#[test]
fn successful_region_speaks_once_with_the_detected_language() {
    let image = make_temp_image("success");
    let selector = FileSelector {
        path: image.clone(),
    };
    let ocr = SuccessOcr { text: ENGLISH_TEXT };
    let engine = Arc::new(RecordingEngine::new());
    let player = Player::new(engine.clone(), Arc::new(FakeSink));

    let result = read_region(&selector, &ocr, &player, 1.0);

    assert!(result.is_ok());
    assert_eq!(
        result.unwrap(),
        Outcome::Spoke,
        "a successful capture-and-speak must be reported as Outcome::Spoke"
    );
    assert_eq!(
        engine.calls.load(Ordering::SeqCst),
        1,
        "a single-sentence passage should synthesize exactly once"
    );
    assert_eq!(engine.langs.lock().unwrap().as_slice(), ["en"]);
    assert!(!image.exists(), "the temp image must be deleted on success");
}

#[test]
fn temp_image_is_deleted_even_when_ocr_fails() {
    let image = make_temp_image("ocr-fails");
    let selector = FileSelector {
        path: image.clone(),
    };
    let engine = Arc::new(RecordingEngine::new());
    let player = Player::new(engine.clone(), Arc::new(FakeSink));

    let result = read_region(&selector, &FailingOcr, &player, 1.0);

    assert!(result.is_err(), "an OCR failure must propagate as an error");
    assert_eq!(engine.calls.load(Ordering::SeqCst), 0);
    assert!(
        !image.exists(),
        "the temp image must be deleted on the OCR-failure path too"
    );
}

// ---------------------------------------------------------------------
// speak_selection
// ---------------------------------------------------------------------

#[test]
fn empty_selection_speaks_nothing() {
    let engine = Arc::new(RecordingEngine::new());
    let player = Player::new(engine.clone(), Arc::new(FakeSink));

    let result = speak_selection("   \n\t  ", &player, 1.0);

    assert!(result.is_ok());
    assert_eq!(engine.calls.load(Ordering::SeqCst), 0);
}

#[test]
fn successful_selection_speaks_once_with_the_detected_language() {
    let engine = Arc::new(RecordingEngine::new());
    let player = Player::new(engine.clone(), Arc::new(FakeSink));

    let result = speak_selection(ENGLISH_TEXT, &player, 1.0);

    assert!(result.is_ok());
    assert_eq!(engine.calls.load(Ordering::SeqCst), 1);
    assert_eq!(engine.langs.lock().unwrap().as_slice(), ["en"]);
}

// ---------------------------------------------------------------------
// App: the busy guard
// ---------------------------------------------------------------------

#[test]
fn busy_guard_prevents_a_second_concurrent_speak() {
    // The real probe of `acquire_within`'s compare-exchange, and it has to
    // deliver *different* text to be one: the same text short-circuits at
    // the `TogglePause` early return in `App::speak_selection` and never
    // reaches the busy flag at all, so a version of this test built on a
    // repeat press stays green with the CAS deleted. (That the repeat
    // press toggles is a separate property, asserted below.)
    //
    // Different text takes the Interrupt path, which stops the read in
    // flight and then *waits* for the flag — so the two `Player::speak`
    // calls must still never overlap. Replace the CAS with a plain store
    // and the second call starts synthesising while the first is still
    // inside its 200ms `synthesize`, which `max_inflight` sees as 2.
    let engine = Arc::new(SlowEngine::new(Duration::from_millis(200)));
    let player = Player::new(
        Arc::clone(&engine) as Arc<dyn TtsEngine>,
        Arc::new(FakeSink),
    );
    let app = Arc::new(App::new(player, 1.0));

    let app_bg = Arc::clone(&app);
    let handle = std::thread::spawn(move || app_bg.speak_selection(ENGLISH_TEXT));

    // Give the background call time to set the busy flag and enter
    // `engine.synthesize` (which then sleeps for 200ms).
    std::thread::sleep(Duration::from_millis(50));

    let second_result = app.speak_selection(OTHER_ENGLISH_TEXT);
    let first_result = handle.join().expect("first call should not panic");

    assert_eq!(
        second_result.unwrap(),
        SelectionOutcome::Spoke { replaced: true },
        "a different selection must displace the read in flight and be \
         spoken itself"
    );
    assert!(
        first_result.is_ok(),
        "a displaced read returns Ok, it is not an error"
    );
    assert_eq!(
        engine.max_inflight.load(Ordering::SeqCst),
        1,
        "two `Player::speak` calls ran concurrently — the busy flag's \
         compare-exchange did not hold, and `sink.append()` calls from the \
         two reads would interleave"
    );
}

#[test]
fn the_same_selection_while_a_read_is_in_flight_toggles_instead_of_speaking() {
    // The other half of what the busy guard used to be tested for: a
    // repeat press must not start a second `speak()`. It no longer
    // reaches the busy flag to be refused — it is answered earlier, by
    // the toggle — so this asserts the toggle, not the flag.
    let engine = Arc::new(SlowEngine::new(Duration::from_millis(200)));
    let player = Player::new(
        Arc::clone(&engine) as Arc<dyn TtsEngine>,
        Arc::new(FakeSink),
    );
    let app = Arc::new(App::new(player, 1.0));

    let app_bg = Arc::clone(&app);
    let handle = std::thread::spawn(move || app_bg.speak_selection(ENGLISH_THREE_SENTENCES));

    // Not merely "the engine has been entered": `Player::pause` refuses
    // until audio has actually reached the sink, because pausing an empty
    // sink parks the read forever (see `tests/selection_toggle.rs`). A
    // second `synthesize` call means the first buffer has been appended.
    let deadline = Instant::now() + Duration::from_secs(5);
    while engine.calls.load(Ordering::SeqCst) < 2 {
        assert!(Instant::now() < deadline, "the read never produced audio");
        std::thread::sleep(Duration::from_millis(5));
    }

    let second_result = app.speak_selection(ENGLISH_THREE_SENTENCES);
    assert_eq!(
        second_result.unwrap(),
        SelectionOutcome::Toggled { paused: true },
        "the same selection while a read is in flight must toggle it, \
         never start a second concurrent speak"
    );

    // `FakeSink` models no pause, so the read runs on regardless — this
    // test is about what the *decision* did, not about playback.
    let first_result = handle.join().expect("first call should not panic");
    assert_eq!(
        first_result.unwrap(),
        SelectionOutcome::Spoke { replaced: false },
        "the first call should have run to completion"
    );
    assert_eq!(
        engine.max_inflight.load(Ordering::SeqCst),
        1,
        "the toggle must not have started a second concurrent speak"
    );
}

#[test]
fn busy_guard_releases_after_completion_so_the_next_call_runs() {
    let engine = Arc::new(RecordingEngine::new());
    let player = Player::new(engine.clone(), Arc::new(FakeSink));
    let app = App::new(player, 1.0);

    let first = app.speak_selection(ENGLISH_TEXT).unwrap();
    let second = app.speak_selection(ENGLISH_TEXT).unwrap();

    assert_eq!(
        first,
        SelectionOutcome::Spoke { replaced: false },
        "first call should run"
    );
    assert_eq!(
        second,
        SelectionOutcome::Spoke { replaced: false },
        "once the first call has returned, the guard must be released — and \
         the text it was reading must not linger and turn this into a toggle"
    );
    assert_eq!(engine.calls.load(Ordering::SeqCst), 2);
}

#[test]
fn busy_guard_releases_after_an_ordinary_failure_so_the_next_call_runs() {
    // The first call fails with an ordinary `Err` (not a panic). If the
    // guard's release were ever conditioned on success, this would be the
    // test that catches it: the second call must still be attempted, not
    // silently skipped as busy.
    let engine = Arc::new(FailingEngine {
        calls: AtomicUsize::new(0),
    });
    let player = Player::new(
        Arc::clone(&engine) as Arc<dyn TtsEngine>,
        Arc::new(FakeSink),
    );
    let app = App::new(player, 1.0);

    let first = app.speak_selection(ENGLISH_TEXT);
    assert!(first.is_err(), "the engine's Err must propagate");

    let second = app.speak_selection(ENGLISH_TEXT);
    assert!(
        second.is_err(),
        "the second call must have actually run (and failed the same way) \
         rather than being skipped as busy, which would read Ok(Skipped)"
    );
    assert_eq!(
        engine.calls.load(Ordering::SeqCst),
        2,
        "both calls must have reached the engine"
    );
}

#[test]
fn busy_guard_releases_after_a_panic_so_the_next_call_proceeds() {
    // Regression test for the defect this fix addresses: a plain
    // `self.busy.store(false, ...)` placed *after* `f()` returns is
    // skipped when `f()` unwinds instead of returning. Both real callers
    // (the hotkey handler and the Service callback) run `App::read_region`
    // / `App::speak_selection` inside `std::thread::spawn`, so that panic
    // kills only the worker thread — the process, and the stuck `busy`
    // flag, survive. Every later hotkey press or Service delivery would
    // then read `Ok(false)` forever: no error, no log, no audio, until the
    // app is restarted. The RAII `BusyRelease` in `src/app/mod.rs` fixes
    // this by releasing in `Drop`, which also runs during an unwind. This
    // test fails (second call reads busy) if that guard is reverted to a
    // plain post-call store.
    let engine = Arc::new(PanicOnceEngine {
        calls: AtomicUsize::new(0),
    });
    let player = Player::new(
        Arc::clone(&engine) as Arc<dyn TtsEngine>,
        Arc::new(FakeSink),
    );
    let app = App::new(player, 1.0);

    let first = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        app.speak_selection(ENGLISH_TEXT)
    }));
    assert!(
        first.is_err(),
        "the engine panic should have unwound through speak_selection"
    );

    let second = app.speak_selection(ENGLISH_TEXT);
    assert_eq!(
        second.unwrap(),
        SelectionOutcome::Spoke { replaced: false },
        "the busy flag must be released even though the first call panicked; \
         a stuck flag here means the app has silently bricked itself"
    );
    assert_eq!(
        engine.calls.load(Ordering::SeqCst),
        2,
        "the second call must have actually reached the engine, not been \
         skipped as busy"
    );
}

// ---------------------------------------------------------------------
// App: live speed (Task 9)
// ---------------------------------------------------------------------

#[test]
fn speed_changes_take_effect_without_rebuilding_the_app() {
    let app = build_test_app(1.0);
    assert_eq!(app.speed(), 1.0);
    app.set_speed(1.75);
    assert_eq!(app.speed(), 1.75);
}

#[test]
fn speed_is_clamped_at_the_app_boundary_too() {
    let app = build_test_app(1.0);
    app.set_speed(99.0);
    assert_eq!(app.speed(), 2.0);
}

// ---------------------------------------------------------------------
// App: live voice swap (Task 9)
// ---------------------------------------------------------------------

#[test]
fn swap_player_blocks_until_the_in_flight_utterance_releases_the_read_lock() {
    // Mirrors busy_guard_prevents_a_second_concurrent_speak's shape: a
    // slow engine ties up the Player's read lock for 200ms. swap_player,
    // called from another thread while that read is in flight, must not
    // return until it releases — proving the RwLock's write lock actually
    // blocks rather than swapping the Player out from under an in-flight
    // speak() call and cutting it off. If swap_player were, say, a plain
    // field assignment behind no lock, this would return almost
    // instantly and the timing assertion below would fail.
    let engine = Arc::new(SlowEngine::new(Duration::from_millis(200)));
    let player = Player::new(
        Arc::clone(&engine) as Arc<dyn TtsEngine>,
        Arc::new(FakeSink),
    );
    let app = Arc::new(App::new(player, 1.0));

    let app_bg = Arc::clone(&app);
    let handle = std::thread::spawn(move || app_bg.speak_selection(ENGLISH_TEXT));

    // Give the background call time to acquire the read lock and enter
    // engine.synthesize (which then sleeps for 200ms) — same margin as
    // the busy-guard test above.
    std::thread::sleep(Duration::from_millis(50));

    let new_engine = Arc::new(RecordingEngine::new());
    let new_player = Player::new(new_engine.clone(), Arc::new(FakeSink));

    let swap_started = Instant::now();
    app.swap_player(new_player);
    let swap_waited = swap_started.elapsed();

    let ran = handle.join().expect("speak_selection should not panic");
    assert_eq!(
        ran.unwrap(),
        SelectionOutcome::Spoke { replaced: false },
        "the in-flight utterance should have run to completion, not been \
         cut off by the voice swap"
    );
    assert!(
        swap_waited >= Duration::from_millis(120),
        "swap_player returned after {swap_waited:?}, too fast to have \
         waited for the in-flight read lock (the engine delay was 200ms, \
         starting ~50ms before the swap) — the write lock should have \
         blocked until the read released"
    );

    // The swap actually took effect: a follow-up call reaches the new
    // engine, not the old (still-slow) one.
    assert_eq!(
        app.speak_selection(ENGLISH_TEXT).unwrap(),
        SelectionOutcome::Spoke { replaced: false }
    );
    assert_eq!(
        new_engine.calls.load(Ordering::SeqCst),
        1,
        "the follow-up call should have reached the swapped-in engine"
    );
}
