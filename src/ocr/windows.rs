//! Windows OCR via `Windows.Media.Ocr` — **stub, body not implemented.**
//!
//! Written on macOS as part of the M6 preparation pass, so the PC fills in
//! bodies rather than inventing structure. References no WinRT type on purpose:
//! WinRT bindings do not compile on macOS (hard constraint 7), so the file
//! could not otherwise have been typechecked before handover. The Windows facts
//! live in the doc comments, sourced from `docs/M6-platform-research-windows.md`.

use super::OcrEngine;
use anyhow::{bail, Result};
use std::path::Path;

/// OCR through the inbox `Windows.Media.Ocr` engine.
///
/// Shaped to mirror `ocr::macos::VisionOcr` — `new() -> Result<Self>`, so the
/// `VisionOcr::new()?` call site in `src/bin/aloud.rs` needs only a `#[cfg]`ed
/// import swap. Unlike the macOS path this is **in-process**, not a helper
/// subprocess: `Windows.Media.Ocr` has usable Rust bindings via the `windows`
/// crate, so there is no equivalent of the Swift `aloud-ocr` helper to ship.
///
/// # The engine is inbox; the *language models* are not
///
/// `OcrEngine` ships in `windows.dll` on every Windows 10/11 SKU — no Windows
/// App SDK, no redistributable. But each language is a **Feature on Demand** the
/// user installs (`Language.OCR~~~<tag>~0.0.1.0`). A stock en-US machine has
/// English and nothing else. This is the sharpest collision with hard constraint
/// 1 ("zero runtime system dependencies") in the whole port, and the prototype's
/// answer is to constrain the feature to what the machine already has rather
/// than to instruct the user to install anything.
///
/// # `TryCreateFromLanguage` fails by returning **null**, silently
///
/// Not an exception, not an `Err` — a null. `AvailableRecognizerLanguages`
/// likewise returns an *empty list* rather than throwing. So:
///
/// 1. Enumerate `AvailableRecognizerLanguages` at construction and **log it**.
/// 2. Default via `TryCreateFromUserProfileLanguages()`.
/// 3. Treat a null from either as a real, reported error here — never as
///    "recognised no text".
///
/// Any language UI must be driven from that enumeration, never a hardcoded list.
///
/// # Ukrainian is expected to be absent, and `lingua` cannot rescue it
///
/// `Windows.Media.Ocr` appears to have no Ukrainian recognizer at any price
/// (LIKELY — Microsoft Q&A, not an API reference; confirm with
/// `Get-WindowsCapability -Online | ? Name -Like 'Language.OCR*'`). `lingua-rs`
/// runs *after* OCR, so it can only classify glyphs the recognizer already
/// produced: Ukrainian text pushed through the Russian model mangles і, ї, є, ґ
/// before detection ever sees them. Aloud's four-language design is a
/// three-language design on Windows. That is a real feature-parity gap against
/// macOS Vision, not an oversight.
///
/// # Small regions are the failure case, and they are Aloud's primary gesture
///
/// Windows OCR returns *nothing* on images that are too small. PowerToys
/// upscales by 1.5x first, guarded against the ceiling:
/// `if bmp.Width * 1.5 > OcrEngine.MaxImageDimension` — clamp to
/// `MaxImageDimension`, otherwise upscale ~1.5x before `RecognizeAsync`.
///
/// # COM/WinRT initialisation is the caller's job
///
/// windows-rs does **not** initialise COM. Call
/// `RoInitialize(RO_INIT_MULTITHREADED)` explicitly (WinRT wants `RoInitialize`,
/// not `CoInitializeEx`); relying on implicit initialisation has produced real
/// segfaults when another crate called `CoUninitialize`. Both `OcrEngine` and
/// the capture APIs are `ThreadingModel.Both` + agile, so no apartment is
/// imposed.
pub struct WindowsOcr;

impl WindowsOcr {
    /// Should fail loudly when no recognizer can be constructed at all, so the
    /// condition surfaces at startup rather than as an empty transcription on
    /// the first hotkey press.
    pub fn new() -> Result<Self> {
        bail!("Windows.Media.Ocr engine is not implemented yet")
    }
}

impl OcrEngine for WindowsOcr {
    /// Decode the PNG to `BitmapPixelFormat.Bgra8` / `BitmapAlphaMode.Premultiplied`
    /// (what Microsoft's own OCR sample uses — convention, not a documented
    /// contract) and hand it to `RecognizeAsync`.
    fn recognise(&self, _image_path: &Path) -> Result<String> {
        bail!("Windows.Media.Ocr recognition is not implemented yet")
    }
}
