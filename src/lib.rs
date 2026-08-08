pub mod tts;
pub mod vendor;

use std::path::PathBuf;

/// Root of the Supertonic model assets.
/// Override with $ALOUD_MODEL_DIR; defaults to ~/.cache/supertonic3.
pub fn model_dir() -> PathBuf {
    if let Ok(p) = std::env::var("ALOUD_MODEL_DIR") {
        return PathBuf::from(p);
    }
    dirs::home_dir()
        .expect("no home directory")
        .join(".cache/supertonic3")
}

pub fn onnx_dir() -> PathBuf {
    model_dir().join("onnx")
}

pub fn voice_style_path(voice: &str) -> PathBuf {
    model_dir().join("voice_styles").join(format!("{voice}.json"))
}
