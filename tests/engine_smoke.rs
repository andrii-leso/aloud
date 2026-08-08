use aloud::vendor::supertonic::{load_text_to_speech, load_voice_style};
use aloud::{onnx_dir, voice_style_path};

#[test]
fn synthesizes_nonempty_audio_with_f5() {
    let mut tts = load_text_to_speech(onnx_dir().to_str().unwrap(), false)
        .expect("model should load from ALOUD_MODEL_DIR");
    let style = load_voice_style(
        &[voice_style_path("F5").to_string_lossy().into_owned()],
        false,
    )
    .expect("F5 voice style should load");

    let (samples, duration) = tts
        .call("Hello from Aloud.", "en", &style, 8, 1.0, 0.3)
        .expect("synthesis should succeed");

    assert!(!samples.is_empty(), "expected audio samples");
    assert!(duration > 0.0, "expected positive duration");
    assert_eq!(tts.sample_rate, 44100, "confirm the actual sample rate here");
}
