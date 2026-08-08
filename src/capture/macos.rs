use super::RegionSelector;
use anyhow::{anyhow, Result};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Interactive region capture via macOS's built-in `screencapture -i`,
/// which draws Apple's native crosshair and waits for the user to drag a
/// rectangle, or press Escape to cancel.
pub struct ScreenCapture;

impl ScreenCapture {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ScreenCapture {
    fn default() -> Self {
        Self::new()
    }
}

impl RegionSelector for ScreenCapture {
    fn select(&self) -> Result<Option<PathBuf>> {
        let path = unique_capture_path();
        // -i: interactive crosshair selection: -x: no camera-shutter sound.
        let status = Command::new("screencapture")
            .arg("-i")
            .arg("-x")
            .arg(&path)
            .status()?;

        // screencapture's exit code for a cancelled (Escape) selection is
        // not reliable across macOS versions, so it is deliberately not
        // inspected — the file left on disk (or not) is the only signal
        // used, via check_capture_result below.
        let _ = status;

        check_capture_result(&path)
    }
}

/// Decides whether an interactive capture aimed at `path` was completed or
/// cancelled, by inspecting what (if anything) ended up on disk.
///
/// A cancelled (Escape) selection leaves either nothing, or an empty file,
/// at `path` — `screencapture`'s exit code does not reliably distinguish
/// the two across macOS versions, so file presence/size is the only signal
/// used here. Split out from `select` so this logic is testable without
/// driving the actual interactive UI, which cannot be automated.
fn check_capture_result(path: &Path) -> Result<Option<PathBuf>> {
    match std::fs::metadata(path) {
        Ok(meta) if meta.len() > 0 => Ok(Some(path.to_path_buf())),
        Ok(_) => Ok(None), // zero-byte file: cancelled
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None), // nothing written: cancelled
        Err(e) => Err(anyhow!(
            "failed to inspect capture output at {}: {e}",
            path.display()
        )),
    }
}

/// Builds a fresh, unique path under the OS temp directory for one capture
/// invocation.
///
/// Never a fixed name: a fixed path would race between two concurrent
/// invocations (e.g. the hotkey pressed twice in quick succession) and
/// would let a stale capture from a previous run be mistaken for a fresh
/// one. Process ID plus a nanosecond timestamp plus a per-process counter
/// is enough entropy without pulling in a temp-file crate.
fn unique_capture_path() -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!(
        "aloud-capture-{}-{nanos}-{n}.png",
        std::process::id()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_is_treated_as_cancelled() {
        let path = unique_capture_path(); // never created
        let result = check_capture_result(&path).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn zero_byte_file_is_treated_as_cancelled() {
        let path = unique_capture_path();
        std::fs::write(&path, []).unwrap();
        let result = check_capture_result(&path).unwrap();
        std::fs::remove_file(&path).ok();
        assert!(result.is_none());
    }

    #[test]
    fn non_empty_file_yields_the_path() {
        let path = unique_capture_path();
        std::fs::write(&path, [0x89, b'P', b'N', b'G']).unwrap();
        let result = check_capture_result(&path).unwrap();
        std::fs::remove_file(&path).ok();
        assert_eq!(result, Some(path));
    }

    #[test]
    fn unique_capture_path_never_repeats() {
        let a = unique_capture_path();
        let b = unique_capture_path();
        assert_ne!(a, b);
    }
}
