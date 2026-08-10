pub mod confusions;
// Gated to match the `capture` and `selection` seams (Aloud's hard constraint
// 7). `macos` shells out to the bundled Swift `aloud-ocr` helper, which does
// not exist on Windows, so compiling it there would produce a type that can
// only fail at runtime. It was previously ungated only because macOS was the
// sole target.
#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(target_os = "windows")]
pub mod windows;
use anyhow::Result;
use std::path::Path;

pub use confusions::fix_confusions;

/// Optical character recognition over a captured image.
/// macOS uses Apple's Vision framework via a bundled Swift helper;
/// Windows will use Windows.Media.Ocr (M6).
///
/// **No language parameter, deliberately — do not add one.** The caller cannot
/// know the language before any text exists, so the argument would be circular,
/// and `lingua-rs` runs *after* OCR. An engine that must choose between
/// language models chooses internally (recognise more than once, score, pick),
/// and the picker stays a pure function. See `windows.rs` for the full seam
/// note behind this.
pub trait OcrEngine: Send + Sync {
    fn recognise(&self, image_path: &Path) -> Result<String>;
}
