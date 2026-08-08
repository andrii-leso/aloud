use super::RegionSelector;
use anyhow::{anyhow, Result};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

// CGPreflightScreenCaptureAccess / CGRequestScreenCaptureAccess have no
// stable Rust binding, so they're declared directly rather than pulling in
// a crate for two functions. This whole file is macOS-only — gated at the
// `pub mod macos;` declaration in mod.rs, not here — so no per-item
// `#[cfg(target_os = "macos")]` is needed inside it.
#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGPreflightScreenCaptureAccess() -> bool;
    fn CGRequestScreenCaptureAccess() -> bool;
}

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
    /// `Ok(None)` here means only a deliberate user cancel (Escape) — never
    /// "something went wrong." Anything that stops the capture from
    /// happening for another reason, most importantly missing Screen
    /// Recording permission, is `Err`: a missing grant leaves the same
    /// "no file" signature on disk as a cancel, so it is checked for
    /// explicitly, up front, rather than left to fall into the cancel path.
    fn select(&self) -> Result<Option<PathBuf>> {
        ensure_screen_capture_access()?;

        let path = unique_capture_path();
        // -i: interactive crosshair selection; -x: no camera-shutter sound.
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

/// Confirms Screen Recording permission is granted before a capture is
/// attempted.
///
/// Without this check, a missing grant and a user cancel are
/// indistinguishable: both leave no file at the output path, so
/// `check_capture_result` alone would silently report `Ok(None)` for a
/// permission problem — the most likely first-run experience for this app
/// (hotkey pressed, permission never granted, nothing happens, no clue why,
/// forever). Preflighting here turns that into a loud `Err` instead.
///
/// If permission is absent, this also fires `CGRequestScreenCaptureAccess`
/// once so macOS shows the user the system permission prompt — otherwise a
/// first-run user would get the error message with no prompt ever having
/// appeared.
fn ensure_screen_capture_access() -> Result<()> {
    let has_access = unsafe { CGPreflightScreenCaptureAccess() };
    if !has_access {
        unsafe {
            CGRequestScreenCaptureAccess();
        }
    }
    access_result(has_access)
}

/// The decision behind `ensure_screen_capture_access`, factored out as a
/// pure function of the preflight result so it's testable: on a machine
/// that already has the grant (this one), `CGPreflightScreenCaptureAccess`
/// cannot be made to return `false` to exercise the denial branch, but this
/// function can be called directly with `false`.
fn access_result(has_access: bool) -> Result<()> {
    if has_access {
        Ok(())
    } else {
        Err(anyhow!(
            "Aloud needs Screen Recording permission to capture your screen. \
             Grant it in System Settings → Privacy & Security → Screen Recording, \
             then quit and reopen Aloud — macOS requires a restart after granting \
             before the permission takes effect."
        ))
    }
}

/// Decides whether an interactive capture aimed at `path` was completed or
/// cancelled, by inspecting what (if anything) ended up on disk.
///
/// A cancelled (Escape) selection leaves either nothing, or an empty file,
/// at `path` — `screencapture`'s exit code does not reliably distinguish
/// the two across macOS versions, so file presence/size is the only signal
/// used here. Split out from `select` so this logic is testable without
/// driving the actual interactive UI, which cannot be automated. Permission
/// problems are handled earlier, in `ensure_screen_capture_access` — by the
/// time this runs, "no file" means only "the user pressed Escape."
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

    #[test]
    fn access_present_is_ok() {
        assert!(access_result(true).is_ok());
    }

    #[test]
    fn access_absent_is_a_distinct_err_naming_the_fix() {
        let err = access_result(false).unwrap_err().to_string();
        assert!(err.contains("Screen Recording"), "got: {err}");
        assert!(err.contains("System Settings"), "got: {err}");
        assert!(
            err.contains("restart") || err.contains("reopen"),
            "got: {err}"
        );
    }
}
