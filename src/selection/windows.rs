//! Windows selection source — **stub, body not implemented, and out of scope
//! for the first Windows prototype.**
//!
//! Written on macOS as part of the M6 preparation pass so the seam exists and
//! the PC is not left inventing structure for it. References no Win32 type on
//! purpose: WinRT/Win32 bindings do not compile on macOS (hard constraint 7).

use super::SelectionSource;
use anyhow::{bail, Result};

/// "Read the text the user currently has selected", Windows edition.
///
/// # There is no Windows equivalent of the macOS mechanism
///
/// On macOS this is a system **Service** (Services → Read Aloud). The app never
/// asks for Accessibility permission and never touches the clipboard; the OS
/// hands the selected text to the app. That is a deliberate owner decision, and
/// Windows has nothing like it — no Services menu, no system-mediated "give this
/// app the selection" channel.
///
/// The Windows story is therefore the *pull* shape this trait was written to
/// hold: synthesise Ctrl+C with `SendInput`, read `CF_UNICODETEXT` off the
/// clipboard, and restore the previous clipboard contents afterwards. There is
/// no permission gate, so the privacy objection that ruled this out on macOS
/// does not apply — but it is not free either:
///
/// * It **destroys and restores the user's clipboard** on every invocation.
///   Restoration is best-effort: the clipboard is a shared, racy, multi-format
///   resource and a faithful save/restore of every format is not achievable.
/// * It depends on the foreground app honouring Ctrl+C, which not all do.
/// * The paste-ready delay after `SendInput` is a guess; there is no completion
///   signal to wait on.
///
/// # Prototype scope
///
/// The first Windows prototype is **region capture -> OCR -> speak** plus a tray
/// icon. Selection reading is not part of it. This type exists so the seam is
/// occupied and the decision is recorded, not because it is next.
pub struct ClipboardSelection;

impl ClipboardSelection {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ClipboardSelection {
    fn default() -> Self {
        Self::new()
    }
}

impl SelectionSource for ClipboardSelection {
    /// `Ok(None)` means nothing was selected. Note the asymmetry with macOS:
    /// that implementation is push-driven and always returns `Ok(None)` here,
    /// whereas this one would do the actual work in `grab()`.
    fn grab(&self) -> Result<Option<String>> {
        bail!("selection reading is not implemented on Windows yet")
    }
}
