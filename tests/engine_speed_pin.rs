//! Always-on guard for the engine-speed pin (`CLAUDE.md` hard constraint 6).
//!
//! Supertonic's `speed` argument shrinks the canvas its decoder renders into,
//! so above ~1.1x it silently deletes words. `SupertonicEngine` therefore calls
//! the engine at `ENGINE_SPEED = 1.0` always and applies the caller's speed to
//! the rendered audio with `time_stretch`. Nothing enforced that: the constant
//! is referenced only at its own definition and at the single call site, so
//! re-plumbing `job.speed` back into `TextToSpeech::call` would reintroduce the
//! original bug with a fully green `cargo test --release`.
//!
//! `tests/speed_preserves_words.rs` would catch it, but it is `#[ignore]`d
//! (ONNX + Whisper per case, and that load lands on the latency ratio guard).
//! The `timestretch` unit tests cannot catch it either — they test *retiming*,
//! not *where the speed is applied*.
//!
//! This closes that hole without a transcriber. Supertonic's duration predictor
//! is deterministic, so for a given text the rendered sample count is fixed.
//! That makes the pin observable as an identity:
//!
//! > synthesising at speed S must return exactly what you get by synthesising
//! > at 1.0 and time-stretching that by S.
//!
//! Which is only true when the engine is driven at 1.0. If `job.speed` reaches
//! the engine instead, the returned buffer is the engine's own (shorter)
//! rendering, and it differs from the stretched one by the head and tail that
//! `time_stretch` emits verbatim — tens of milliseconds, far outside the
//! tolerance below.

use aloud::tts::supertonic_engine::SupertonicEngine;
use aloud::tts::timestretch::time_stretch;
use aloud::tts::TtsEngine;

/// Short on purpose: this runs in the default suite, so it buys its coverage
/// with two syntheses of a ~1.4 s utterance rather than a paragraph.
const TEXT: &str = "Read Region";

#[test]
fn the_engine_is_always_driven_at_unity_speed() {
    let engine = SupertonicEngine::spawn("M5").expect("engine");

    let base = engine.synthesize(TEXT, "en", 1.0).expect("1.0");
    assert!(!base.samples.is_empty(), "1.0 rendering is empty");

    for speed in [1.5f32, 2.0] {
        let fast = engine.synthesize(TEXT, "en", speed).expect("fast");
        let expected = time_stretch(&base.samples, base.sample_rate, speed).len();

        // Exact equality is what the pin actually produces (the duration
        // predictor is deterministic). A few samples of slack keeps this from
        // becoming a tripwire for an unrelated rounding change, while staying
        // orders of magnitude tighter than the difference the bug creates.
        let slack = 64i64;
        let delta = fast.samples.len() as i64 - expected as i64;
        assert!(
            delta.abs() <= slack,
            "speed {speed}: engine returned {} samples, but synthesising at 1.0 \
             and stretching gives {expected} (delta {delta}). The engine is not \
             pinned to 1.0 — see CLAUDE.md hard constraint 6.",
            fast.samples.len()
        );

        // The retiming must still be doing its job, or a pin that returned the
        // 1.0 buffer untouched would pass the check above at speed 1.0 only.
        assert!(
            fast.samples.len() < base.samples.len(),
            "speed {speed}: output is not shorter than the 1.0 rendering"
        );
    }
}
