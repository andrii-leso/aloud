use aloud::play::player::Player;
use aloud::play::sink::AudioSink;
use aloud::text::chunk::split_sentences;
use aloud::tts::{supertonic_engine::SupertonicEngine, Pcm, TtsEngine};
use anyhow::Result;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// **Test 1: Time-to-first-audio via the production `Player::speak()` path.**
///
/// The spec's load-bearing claim is that per-sentence synthesis in `Player::speak()`
/// keeps first audio near 1.5s instead of 5.8s for an unchunked paragraph. This test
/// measures time from `speak()` to the first buffer reaching the sink, in the
/// production code path, then compares to a direct (unchunked) engine synthesis.
///
/// Why a ratio? On an idle machine the ratio is ~0.33 (first audio is 1/3 the
/// time of the whole paragraph). Under load (load average ~24 on an 8-core M1),
/// it's ~0.55. If chunking were removed, the ratio would be 1.0 (first audio
/// waits for the whole text to synthesize). 0.70 sits safely between the worst
/// observed real value (~0.55) and the failure condition (~1.0).
///
/// The test also asserts that the paragraph produced multiple appends (count > 1),
/// which alone fails if chunking is removed. This is the cheapest, most direct guard.
///
/// **Why three measurements, not two.** `cargo test --release` runs this binary
/// concurrently with `engine_smoke.rs` and `engine_thread.rs` — three ONNX test
/// binaries doing real inference at once, which saturates the machine. If the two
/// measurements below (first-audio, whole-paragraph) land under different load
/// levels, monotonic drift between them corrupts the ratio even though the
/// underlying chunking behaviour is unchanged (observed: ratio 0.82 against the
/// 0.70 gate under concurrent load; passes reliably via `--test latency_budget`
/// alone). Fix: measure whole-paragraph synthesis, then first-audio, then
/// whole-paragraph again, and compare first-audio to the *mean* of the two
/// whole-paragraph measurements. Drift that occurs between the first whole
/// measurement and the last raises (or lowers) both halves of the ratio equally,
/// instead of only the denominator.
///
/// **Run in release mode** — debug ONNX inference is ~10x slower and would fail
/// this test misleadingly.
///
/// Derived numbers printed with --nocapture so we can monitor them over time.
#[test]
fn first_sentence_is_much_faster_than_whole_paragraph() {
    let engine = Arc::new(SupertonicEngine::spawn("F5").expect("engine should spawn"));

    // Warm up the engine with one throwaway synthesis.
    let _ = engine.synthesize("Warm up.", "en", 1.0).expect("warmup");

    let paragraph = "The applicant must submit the completed form together with proof of \
                     residence within four weeks. Incomplete submissions will be returned \
                     without processing. If you require an extension, contact the office in \
                     writing before the deadline expires.";

    // Measurement 1 of 3: unchunked synthesis (direct engine call on whole paragraph).
    let t0 = Instant::now();
    let pcm1 = engine
        .synthesize(paragraph, "en", 1.0)
        .expect("whole paragraph synthesis (1) should succeed");
    let whole_ms_1 = t0.elapsed().as_millis();
    assert!(!pcm1.samples.is_empty());

    // Measurement 2 of 3: time first audio via Player::speak() with a recording sink.
    let sink = Arc::new(RecordingSink::new());
    let player = Player::new(engine.clone(), sink.clone());

    let t0 = Instant::now();
    player
        .speak(paragraph, "en", 1.0)
        .expect("speak should succeed");
    let first_audio_ms = sink
        .first_append_instant()
        .map(|instant| (instant - t0).as_millis())
        .expect("sink should have recorded a first append");

    let append_count = sink.append_count();
    assert!(
        append_count > 1,
        "paragraph should produce multiple appends (chunking in effect). \
         Got {append_count} appends. If chunking is removed, this fails."
    );

    // Measurement 3 of 3: unchunked synthesis again, to bracket measurement 2.
    let t0 = Instant::now();
    let pcm2 = engine
        .synthesize(paragraph, "en", 1.0)
        .expect("whole paragraph synthesis (2) should succeed");
    let whole_ms_2 = t0.elapsed().as_millis();
    assert!(!pcm2.samples.is_empty());

    // Compare against the mean of the two whole-paragraph measurements, so
    // monotonic load drift across the three measurements cancels out instead of
    // only inflating (or deflating) one side of the ratio.
    let whole_ms_mean = (whole_ms_1 + whole_ms_2) as f64 / 2.0;
    let ratio = (first_audio_ms as f64) / whole_ms_mean;
    println!(
        "whole_paragraph_ms_1={whole_ms_1}, first_audio_ms={first_audio_ms}, \
         whole_paragraph_ms_2={whole_ms_2}, whole_paragraph_ms_mean={whole_ms_mean:.1}, \
         ratio={ratio:.2}, append_count={append_count}"
    );

    assert!(
        (first_audio_ms as f64) <= whole_ms_mean * 0.70,
        "first audio (via chunked speak) should be ≤70% as long as mean whole-paragraph \
         synthesis. first={first_audio_ms}ms, whole_mean={whole_ms_mean:.1}ms, ratio={ratio:.2} \
         (raw: whole_1={whole_ms_1}ms, whole_2={whole_ms_2}ms). \
         If ratio approaches 1.0, chunking may have stopped working. \
         If append_count is 1, chunking is definitely broken."
    );
}

/// Fake sink that records the instant of the first append and counts total appends.
struct RecordingSink {
    first_append: Mutex<Option<Instant>>,
    count: AtomicUsize,
}

impl RecordingSink {
    fn new() -> Self {
        Self {
            first_append: Mutex::new(None),
            count: AtomicUsize::new(0),
        }
    }

    fn first_append_instant(&self) -> Option<Instant> {
        *self.first_append.lock().unwrap()
    }

    fn append_count(&self) -> usize {
        self.count.load(Ordering::SeqCst)
    }
}

impl AudioSink for RecordingSink {
    fn append(&self, _pcm: Pcm) -> Result<()> {
        let mut g = self.first_append.lock().unwrap();
        if g.is_none() {
            *g = Some(Instant::now());
        }
        drop(g); // Release lock before fetch_add.
        self.count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn queued(&self) -> usize {
        0 // Drain instantly: we want synthesis-bound timing, not look-ahead throttle's wait.
    }

    fn stop(&self) {}
}

/// **Test 2: Absolute time-to-first-audio budget (ignored by default).**
///
/// The spec's original claim is ≤2.0s warm. However, absolute millisecond budgets
/// measure the machine state as much as the code: they flake on busy laptops, in
/// CI containers, or under thermal load. The ratio test above is the real guard.
///
/// This test is marked `#[ignore]` and run deliberately with `cargo test --release
/// -- --ignored` only on idle systems where we want to verify absolute performance.
/// It will fail on any reasonably loaded machine and that is expected and correct.
///
/// **Run in release mode** — debug ONNX inference is ~10x slower.
const BUDGET_MS: u128 = 2000;

#[test]
#[ignore]
fn absolute_time_to_first_audio_budget() {
    let engine = SupertonicEngine::spawn("F5").expect("engine should spawn");
    let _ = engine.synthesize("Warm up.", "en", 1.0).expect("warmup");

    let paragraph = "The applicant must submit the completed form together with proof of \
                     residence within four weeks. Incomplete submissions will be returned \
                     without processing. If you require an extension, contact the office in \
                     writing before the deadline expires.";
    let sentences = split_sentences(paragraph);
    assert!(!sentences.is_empty());

    let t0 = Instant::now();
    let pcm = engine
        .synthesize(&sentences[0], "en", 1.0)
        .expect("synthesis should succeed");
    let elapsed = t0.elapsed().as_millis();

    assert!(!pcm.samples.is_empty());
    println!("time-to-first-audio: {elapsed}ms");

    assert!(
        elapsed <= BUDGET_MS,
        "time-to-first-audio was {elapsed}ms, over the {BUDGET_MS}ms budget. \
         Expected ~1500ms on an idle M1 Air. \
         This test only passes on idle systems; run with --ignored only when appropriate."
    );
}
