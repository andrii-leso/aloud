pub mod confusions;
pub mod macos;
use anyhow::Result;
use std::path::Path;

pub use confusions::fix_confusions;

/// Optical character recognition over a captured image.
/// macOS uses Apple's Vision framework via a bundled Swift helper;
/// Windows will use Windows.Media.Ocr (M6).
pub trait OcrEngine: Send + Sync {
    fn recognise(&self, image_path: &Path) -> Result<String>;
}
