//! The two speaking pipelines, kept free of Tauri types so they can be
//! unit-tested with fakes (see `tests/actions.rs`).
//!
//! `read_region` acquires text via screen capture + OCR; `speak_selection`
//! is handed text that has already been extracted (the macOS Service from
//! Task 4, or a future Windows pull-based grabber) and has no acquisition
//! step at all. Both funnel into the same normalize -> detect -> speak
//! tail, so a region-captured passage and a selected one sound identical
//! — except that `read_region` alone runs `fix_confusions` first, since
//! only OCR output carries OCR misreads; `speak_selection`'s input is the
//! user's own exact characters and must never be "corrected".
//!
//! Neither function guards against concurrent `Player::speak` calls —
//! `Player::speak` is documented single-caller/serialized and does not
//! lock internally, so serializing calls into these functions is the
//! caller's job. `crate::app::App` (`src/app/mod.rs`) is that caller for
//! the Tauri app.

use crate::capture::RegionSelector;
use crate::ocr::{fix_confusions, OcrEngine};
use crate::play::player::{Player, SpeakEnd};
use crate::text::detect::detect_lang;
use crate::text::normalize::normalize_ocr;
use anyhow::Result;
use std::path::Path;

/// What a completed `read_region` attempt actually did. `Player::speak`
/// and permission/OCR failures are unambiguous (an `Err`), but the
/// success paths need to stay distinguishable to the caller: a deliberate
/// cancel (Escape) must never surface a notification, "captured something
/// but found no text" is worth telling the user about, and a read that was
/// displaced must not report completion and reset the status on behalf of
/// the read that replaced it — see the notification handling in
/// `src/bin/aloud.rs`. Collapsing them into a bare `Ok(())`, as this used
/// to do, made those distinctions impossible downstream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The user pressed Escape. Silent, on purpose.
    Cancelled,
    /// Capture (and OCR, for the region path) succeeded but there was no
    /// usable text.
    Empty,
    /// Text was normalized, a language was detected, and `Player::speak`
    /// ran to the end of the passage.
    Spoke,
    /// Speaking began but a `stop()` cut it short — a ⌃⌘S takeover, or the
    /// tray's Stop. Not an error (the stop was asked for), but not
    /// `Spoke` either: reporting it as `Spoke` had the displaced read log
    /// "completed" and reset the tray status while its replacement was
    /// already speaking.
    Interrupted,
}

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
/// a deliberate cancel, so this returns `Ok(Outcome::Cancelled)`, not an
/// error -> `ocr.recognise()` -> `fix_confusions` -> `normalize_ocr` -> an
/// empty result means nothing worth speaking, returns `Ok(Outcome::Empty)`
/// -> `detect_lang` -> `player.speak()` -> `Ok(Outcome::Spoke)`, or
/// `Ok(Outcome::Interrupted)` if a `stop()` cut it short.
///
/// `fix_confusions` runs here, between OCR and normalization, and nowhere
/// else — it corrects OCR misreads (Vision's lowercase `l` for uppercase
/// `I`), which only make sense to apply to OCR output. `speak_selection`,
/// below, is handed the user's own exact characters and must never run it.
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
) -> Result<Outcome> {
    let Some(image_path) = selector.select()? else {
        crate::log_line!("region flow: cancelled (Escape)");
        return Ok(Outcome::Cancelled);
    };
    crate::log_line!("region flow: captured to {}", image_path.display());

    let text = {
        // Constructed before `recognise` runs, so its `Drop` fires no
        // matter how that call exits.
        let _cleanup = DeleteOnDrop(&image_path);
        ocr.recognise(&image_path)?
    };

    let text = fix_confusions(&text);
    let normalized = normalize_ocr(&text);
    if normalized.is_empty() {
        crate::log_line!("region flow: normalized text is empty, nothing to speak");
        return Ok(Outcome::Empty);
    }

    let lang = detect_lang(&normalized);
    crate::log_line!(
        "region flow: detected language={lang}, normalized length={} chars",
        normalized.chars().count()
    );
    crate::log_line!("region flow: speak started");
    match player.speak(&normalized, &lang, speed)? {
        SpeakEnd::Completed => {
            crate::log_line!("region flow: speak finished");
            Ok(Outcome::Spoke)
        }
        SpeakEnd::Interrupted => {
            crate::log_line!("region flow: speak was stopped before the end");
            Ok(Outcome::Interrupted)
        }
    }
}

/// The normalized form of a delivered selection, or `None` when there is
/// nothing left worth speaking.
///
/// Split out of `speak_selection` so a caller can find that out **before**
/// it commits to anything irreversible. `App::speak_selection` needs
/// exactly that: a delivery that normalizes to nothing must not silence
/// the read in flight on its way to speaking nothing.
///
/// Reaching this with an empty result is not a freak case that
/// `selection_worth_speaking` already screens out. That screen only
/// rejects text with no non-whitespace character at all, whereas
/// `normalize_ocr` drops every short all-digit block once a selection has
/// more than one `"\n\n"`-separated block — so `"42\n\n"` (a numbered-list
/// marker with the trailing blank line the drag picked up), `"2024\n\n2025"`
/// or two page numbers caught across a page break all arrive as perfectly
/// valid deliveries and normalize to nothing. The normalizer's escape
/// hatch for a bare number preserves only the *single*-block case.
pub fn speakable_selection(text: &str) -> Option<String> {
    let normalized = normalize_ocr(text);
    (!normalized.is_empty()).then_some(normalized)
}

/// Speaks text that has already been extracted. No capture, no OCR, no
/// temp file — just normalize -> detect -> speak.
///
/// An empty (or whitespace-only) result after normalization means nothing
/// worth speaking, and returns silently rather than as an error.
pub fn speak_selection(text: &str, player: &Player, speed: f32) -> Result<SpeakEnd> {
    let Some(normalized) = speakable_selection(text) else {
        crate::log_line!("selection flow: normalized text is empty, nothing to speak");
        return Ok(SpeakEnd::Completed);
    };
    speak_normalized(&normalized, player, speed)
}

/// The tail of `speak_selection`, for a caller that has already run
/// `speakable_selection` and does not want the work done twice.
pub fn speak_normalized(normalized: &str, player: &Player, speed: f32) -> Result<SpeakEnd> {
    let lang = detect_lang(normalized);
    crate::log_line!(
        "selection flow: detected language={lang}, normalized length={} chars",
        normalized.chars().count()
    );
    crate::log_line!("selection flow: speak started");
    let end = player.speak(normalized, &lang, speed)?;
    match end {
        SpeakEnd::Completed => crate::log_line!("selection flow: speak finished"),
        SpeakEnd::Interrupted => {
            crate::log_line!("selection flow: speak was stopped before the end")
        }
    }
    Ok(end)
}
