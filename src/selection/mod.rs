// The macOS implementation is Objective-C interop (a class defined at
// runtime, registered with AppKit as a services provider), so the whole
// module is gated here rather than gating pieces inside it — same shape as
// `capture/mod.rs`, per Aloud's hard constraint 7. `windows` is the M6
// sibling on the other side of that seam.
#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(target_os = "windows")]
pub mod windows;

use anyhow::Result;

/// Source of "the text the user currently has selected".
///
/// macOS receives it via a system Service (Services → Read Aloud), which
/// means no Accessibility permission is ever requested and the clipboard is
/// never touched — a deliberate owner decision, not an implementation
/// detail. See `macos.rs`.
///
/// Windows (M6) will synthesise Ctrl+C and read the clipboard; there is no
/// equivalent permission gate there, so the privacy objection does not
/// apply. This trait exists to hold that pull-shaped implementation.
pub trait SelectionSource: Send + Sync {
    /// Pull-style acquisition. Returns `Ok(None)` when nothing is selected.
    /// The macOS Service implementation is push-driven and returns
    /// `Ok(None)` here.
    fn grab(&self) -> Result<Option<String>>;
}

/// Whether text handed to us by a selection source is worth speaking, and
/// if so the same text back, **unchanged**.
///
/// This is deliberately not a cleanup step. Rejoining hyphenated line
/// breaks, collapsing newlines and normalising quotes belong to the
/// TextNormalizer downstream; doing any of it here would mean the selection
/// path and the OCR path clean text in two different places. The only
/// judgement made here is "is there anything at all to say" — a pasteboard
/// can hand us no string at all, or a run of whitespace, and speaking
/// either is a pointless empty utterance.
pub fn selection_worth_speaking(raw: Option<&str>) -> Option<&str> {
    raw.filter(|s| s.chars().any(|c| !c.is_whitespace()))
}
