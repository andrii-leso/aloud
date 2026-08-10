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
/// # Ukrainian is absent — VERIFIED — and `lingua` cannot rescue it
///
/// `Windows.Media.Ocr` has **no Ukrainian recognizer at any price.** This was
/// LIKELY when this stub was written (a Microsoft Q&A answer, not an API
/// reference); it is now **VERIFIED** against Microsoft's LP-to-FOD mapping
/// spreadsheet — the sheet carries 35 OCR locale rows, and `uk-ua` appears in
/// the whole workbook exactly once, as a **Basic** language FOD with no OCR
/// sibling. Other Cyrillic OCR does exist (`bg-bg`, `sr-cyrl-rs`), so the
/// engine is not Cyrillic-incapable; Ukrainian is specifically absent.
///
/// `lingua-rs` runs *after* OCR, so it can only classify glyphs the recognizer
/// already produced: Ukrainian text pushed through the Russian model mangles
/// і, ї, є, ґ before detection ever sees them. That is a real feature-parity
/// gap against macOS Vision, not an oversight.
///
/// **And it is worse than "three languages on Windows".** German and Russian
/// OCR are *themselves* Features on Demand, present only if the user added
/// those language features — so on a stock en-US machine this engine is a
/// **one-language** design. The Ukrainian gap and the hard-constraint-1
/// conflict ("zero runtime system dependencies" vs `Add-WindowsCapability`)
/// are therefore one problem, not two: anything that fixes Ukrainian by
/// bundling also removes the language-pack install step entirely.
///
/// # Seam note — what the designed replacement would change, and what it would not
///
/// A bundled ONNX engine (PP-OCRv5, ~20.6 MB, Apache-2.0) has been researched
/// and costed as the fix for both halves above. It is **not built and not
/// scheduled** — the decision is the owner's. Full analysis:
/// `BKM/PC-Queue/TASK-M6-aloud-windows-prototype.md` §12. What matters *here*
/// is which of this file's assumptions would survive it:
///
/// * **The `OcrEngine` trait does not change.** `recognise(&Path) -> Result<String>`
///   stays as-is, and it must **not** grow a language parameter: the caller
///   cannot know the language before any text exists, so that argument is
///   circular. Two impls picked at runtime by language is rejected for the same
///   reason. The shape that works is a **single composite impl** owning both
///   recognizer models internally (shared language-agnostic detector run once,
///   then both recognizers over the cropped lines, keep the better-scoring
///   result) with the picker as a pure function — same idiom as
///   `intent::decide_selection` and `text::detect::detect_lang`, unit-testable
///   with fixture crops and no OS.
/// * **`WindowsOcr::new() -> Result<Self>` is the assumption that does NOT
///   survive.** It is zero-arg and cheap here because `Windows.Media.Ocr` is
///   inbox — nothing to locate, nothing to load, nothing to fetch. A bundled
///   engine needs a model directory, a first-run download and cache-integrity
///   check (mirroring the 385 MB TTS model's path in `lib.rs`), and a resident
///   session whose load cost is paid once. The precedent for that in this
///   codebase is `SupertonicEngine::spawn` — the `ort` session lives on its own
///   named thread behind a `Mutex<Sender<Job>>`, which is how an ONNX-backed
///   type satisfies `OcrEngine: Send + Sync` at all. **Expect
///   `spawn(model_dir) -> Result<Self>`, not `new()`, and expect it to live in
///   a platform-neutral `src/ocr/onnx.rs` rather than here.**
/// * **It replaces this type, it does not sit beside it.** Two engines would
///   mean two confidence semantics and two failure modes, plus a selection
///   problem *between engines* on top of the one between models. Replacing is
///   also the only thing that makes hard constraint 1 literally true on Windows.
/// * `oar-ocr` (the ready-made Rust PP-OCR crate) **cannot be added** — it pins
///   `ort =2.0.0-rc.13` against this crate's `ort =2.0.0-rc.7`, which is a hard
///   Cargo resolution failure, not a duplicate-crate warning. Use it as a
///   reference implementation only. Do not bump the `ort` pin for OCR.
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
