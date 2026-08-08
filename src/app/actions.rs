//! The two speaking pipelines, kept free of Tauri types so they can be
//! unit-tested with fakes (see `tests/actions.rs`).
//!
//! `read_region` acquires text via screen capture + OCR; `speak_selection`
//! is handed text that has already been extracted (the macOS Service from
//! Task 4, or a future Windows pull-based grabber) and has no acquisition
//! step at all. Both funnel into the same normalize -> detect -> speak
//! tail, so a region-captured passage and a selected one sound identical.
//!
//! Neither function guards against concurrent `Player::speak` calls —
//! `Player::speak` is documented single-caller/serialized and does not
//! lock internally, so serializing calls into these functions is the
//! caller's job. `crate::app::App` (`src/app/mod.rs`) is that caller for
//! the Tauri app.

use crate::capture::RegionSelector;
use crate::ocr::OcrEngine;
use crate::play::player::Player;
use crate::text::detect::detect_lang;
use crate::text::normalize::normalize_ocr;
use anyhow::Result;
use std::path::Path;

/// Deletes the wrapped path when dropped — on ordinary return, on an
/// early `?`, and on a panic unwind alike. `ocr.recognise` runs over
/// whatever text happened to be on the user's screen; it is not a
/// function to bet "will never panic" on, and a screenshot left behind in
/// the temp directory after an unwind is a privacy leak, not just a mess.
struct DeleteOnDrop<'a>(&'a Path);

impl Drop for DeleteOnDrop<'_> {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(self.0);
    }
}

/// Captures a screen region, OCRs it, and speaks the result.
///
/// Flow: `selector.select()` -> `Ok(None)` means the user pressed Escape,
/// a deliberate cancel, so this returns `Ok(())` silently, not an error ->
/// `ocr.recognise()` -> `normalize_ocr` -> an empty result means nothing
/// worth speaking, returns silently -> `detect_lang` -> `player.speak()`.
///
/// `RegionSelector::select` documents the caller as the owner of deleting
/// the returned temp file (it is a screenshot of the user's screen). This
/// function deletes it as soon as OCR has had its chance to run against
/// it — on every path, including when OCR itself fails or panics.
pub fn read_region(
    selector: &dyn RegionSelector,
    ocr: &dyn OcrEngine,
    player: &Player,
    speed: f32,
) -> Result<()> {
    let Some(image_path) = selector.select()? else {
        return Ok(());
    };

    let text = {
        // Constructed before `recognise` runs, so its `Drop` fires no
        // matter how that call exits.
        let _cleanup = DeleteOnDrop(&image_path);
        ocr.recognise(&image_path)?
    };

    let normalized = normalize_ocr(&text);
    if normalized.is_empty() {
        return Ok(());
    }

    let lang = detect_lang(&normalized);
    player.speak(&normalized, &lang, speed)
}

/// Speaks text that has already been extracted. No capture, no OCR, no
/// temp file — just normalize -> detect -> speak.
///
/// An empty (or whitespace-only) result after normalization means nothing
/// worth speaking, and returns silently rather than as an error.
pub fn speak_selection(text: &str, player: &Player, speed: f32) -> Result<()> {
    let normalized = normalize_ocr(text);
    if normalized.is_empty() {
        return Ok(());
    }

    let lang = detect_lang(&normalized);
    player.speak(&normalized, &lang, speed)
}
