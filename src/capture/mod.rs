// The macOS implementation calls two CoreGraphics FFI functions directly
// (Screen Recording permission preflight/request) with no cross-platform
// fallback, so the whole module is gated here rather than gating pieces
// inside it — this is the seam per Aloud's hard constraint 7 ("everything
// platform-specific lives behind a seam"; a platform #[cfg] leaking past
// this point means the seam is in the wrong place). M6 adds a sibling
// `#[cfg(target_os = "windows")] pub mod windows;`.
#[cfg(target_os = "macos")]
pub mod macos;

use anyhow::Result;
use std::path::PathBuf;

/// Interactive selection of a screen region by the user.
///
/// `Ok(None)` means the user cancelled (pressed Escape) — that is normal,
/// not an error. The caller should silently do nothing in that case.
///
/// Anything that prevents a capture for a reason *other* than a deliberate
/// user cancel — most notably missing OS capture permission — is `Err`,
/// never `Ok(None)`. A permission problem and a cancel can look identical
/// at the filesystem level (neither leaves a usable file behind), so
/// implementations must not let the two collapse into the same return
/// value; the caller needs to be able to tell "nothing to do" apart from
/// "something needs fixing before this can ever work."
///
/// # Ownership of the returned path
///
/// The `PathBuf` in `Ok(Some(path))` points at a freshly-written PNG under
/// the OS temp directory, unique to this call. That file is a photograph
/// of the user's own screen — it could be a bank statement — so whoever
/// receives the path owns deleting it. Delete it as soon as it has served
/// its purpose (e.g. immediately after OCR), on every code path including
/// error paths. This trait does not delete it for you.
pub trait RegionSelector: Send + Sync {
    fn select(&self) -> Result<Option<PathBuf>>;
}
