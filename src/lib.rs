pub mod app;
pub mod capture;
pub mod log;
pub mod login_item;
pub mod ocr;
pub mod play;
pub mod selection;
pub mod settings;
pub mod shortcut;
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
/// 3. `dirs::cache_dir()` + [`MODEL_CACHE_SUBPATH`] — the platform-correct
///    location for a fresh install, e.g. on the Windows port.
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
        .map(|dir| MODEL_CACHE_SUBPATH.iter().fold(dir, |p, seg| p.join(seg)))
        .ok_or_else(|| {
            anyhow!("no cache directory found for the Supertonic model; set $ALOUD_MODEL_DIR")
        })
}

/// Path segments appended to `dirs::cache_dir()` to reach the model root.
///
/// On macOS `dirs::cache_dir()` is `~/Library/Caches` — already a shared cache
/// root that every app namespaces itself inside — so one segment is correct.
///
/// On **Windows it is not a cache root at all**. `dirs` 5.0 collapses cache and
/// local-data with no `Caches` subdirectory, so `dirs::cache_dir()` returns
/// `%LOCALAPPDATA%` *itself*. A bare `supertonic3` would therefore drop the
/// 385 MB model in `%LOCALAPPDATA%\supertonic3`, **outside**
/// `%LOCALAPPDATA%\com.andriileso.aloud\` — and Tauri's NSIS uninstaller only
/// removes folders named after the bundle identifier, so nothing would ever
/// clean it up. 385 MB orphaned on every uninstall, forever.
///
/// Prefixing the identifier lands the model in the folder the uninstaller
/// already deletes. That is the same resulting path as Tauri's
/// `app_cache_dir()` without threading an `AppHandle` into this deliberately
/// Tauri-free module — which `src/bin/aloud_say.rs`, having no Tauri app at
/// all, could not supply.
///
/// The identifier is duplicated from `tauri.conf.json`'s `identifier` field;
/// they must be changed together.
#[cfg(target_os = "windows")]
const MODEL_CACHE_SUBPATH: &[&str] = &["com.andriileso.aloud", "supertonic3"];
#[cfg(not(target_os = "windows"))]
const MODEL_CACHE_SUBPATH: &[&str] = &["supertonic3"];

pub fn onnx_dir() -> Result<PathBuf> {
    Ok(model_dir()?.join("onnx"))
}

pub fn voice_style_path(voice: &str) -> Result<PathBuf> {
    Ok(model_dir()?
        .join("voice_styles")
        .join(format!("{voice}.json")))
}
