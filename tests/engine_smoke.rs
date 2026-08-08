use aloud::vendor::supertonic::{load_text_to_speech, load_voice_style};
use aloud::{onnx_dir, voice_style_path};

#[test]
fn synthesizes_nonempty_audio_with_f5() {
    let onnx = onnx_dir().expect("model dir should resolve");
    let mut tts = load_text_to_speech(onnx.to_str().unwrap(), false)
        .expect("model should load from ALOUD_MODEL_DIR");
    let style_path = voice_style_path("F5").expect("model dir should resolve");
    let style = load_voice_style(&[style_path.to_string_lossy().into_owned()], false)
        .expect("F5 voice style should load");

    let (samples, duration) = tts
        .call("Hello from Aloud.", "en", &style, 8, 1.0, 0.3)
        .expect("synthesis should succeed");

    assert!(!samples.is_empty(), "expected audio samples");
    assert!(duration > 0.0, "expected positive duration");
    assert_eq!(
        tts.sample_rate, 44100,
        "Supertonic's model output is 44.1kHz"
    );
}

/// The duration predictor is fully deterministic (no RNG upstream of it in
/// the graph), so a fixed sentence at a fixed speed pins the entire text
/// front-end + duration-predictor path for free. A non-empty-samples check
/// alone would pass on muffled, truncated, or garbled audio; this catches
/// that class of regression without a spectral/band-energy check.
#[test]
fn duration_is_deterministic_for_fixed_sentence() {
    let onnx = onnx_dir().expect("model dir should resolve");
    let mut tts = load_text_to_speech(onnx.to_str().unwrap(), false)
        .expect("model should load from ALOUD_MODEL_DIR");
    let style_path = voice_style_path("F5").expect("model dir should resolve");
    let style = load_voice_style(&[style_path.to_string_lossy().into_owned()], false)
        .expect("F5 voice style should load");

    let (_samples, duration) = tts
        .call(
            "The applicant must submit the completed form together with proof of residence within four weeks.",
            "en",
            &style,
            8,
            0.95,
            0.3,
        )
        .expect("synthesis should succeed");

    // Measured on this machine: 6.5662136s (matches the reference value below
    // to within 0.0003s).
    //
    // Platform: baselined on aarch64 macOS (M1 Air). The duration predictor
    // is deterministic given identical inputs, but ONNX Runtime's numerics
    // can differ slightly by platform/CPU (x86-64, different SIMD paths,
    // etc.), so this constant may legitimately need re-baselining on
    // Windows/x86-64 rather than being treated as a regression there.
    const EXPECTED_DURATION_SECS: f32 = 6.566;
    assert!(
        (duration - EXPECTED_DURATION_SECS).abs() < 0.01,
        "expected trimmed duration ~{EXPECTED_DURATION_SECS}s, got {duration}s"
    );
}
