pub mod ocr;
pub mod play;
pub mod text;
pub mod tts;
pub mod vendor;

use anyhow::{anyhow, Result};
use std::path::PathBuf;

/// Root of the Supertonic model assets.
///
/// Resolution order, first hit wins:
/// 1. `$ALOUD_MODEL_DIR`, if set — always takes precedence.
/// 2. `~/.cache/supertonic3`, if it already exists. Not what
///    `dirs::cache_dir()` returns on macOS (that's `~/Library/Caches`), but
///    it's where the model has always lived on this machine, so an
///    existing install must keep resolving there.
/// 3. `dirs::cache_dir()/supertonic3` — the platform-correct location
///    (`~/Library/Caches` on macOS, `%LOCALAPPDATA%` on Windows, XDG on
///    Linux) for a fresh install, e.g. on the Windows port.
///
/// Errors, rather than panics, when no cache directory can be found at all.
pub fn model_dir() -> Result<PathBuf> {
    if let Ok(p) = std::env::var("ALOUD_MODEL_DIR") {
        return Ok(PathBuf::from(p));
    }

    if let Some(home) = dirs::home_dir() {
        let legacy = home.join(".cache/supertonic3");
        if legacy.exists() {
            return Ok(legacy);
        }
    }

    dirs::cache_dir()
        .map(|dir| dir.join("supertonic3"))
        .ok_or_else(|| {
            anyhow!("no cache directory found for the Supertonic model; set $ALOUD_MODEL_DIR")
        })
}

pub fn onnx_dir() -> Result<PathBuf> {
    Ok(model_dir()?.join("onnx"))
}

pub fn voice_style_path(voice: &str) -> Result<PathBuf> {
    Ok(model_dir()?
        .join("voice_styles")
        .join(format!("{voice}.json")))
}
