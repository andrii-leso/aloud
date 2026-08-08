pub mod macos;

use anyhow::Result;
use std::path::PathBuf;

/// Interactive selection of a screen region by the user.
///
/// `Ok(None)` means the user cancelled (pressed Escape) — that is normal,
/// not an error. The caller should silently do nothing in that case.
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
