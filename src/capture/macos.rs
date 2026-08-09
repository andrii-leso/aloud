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
    /// Recording permission, is `Err`.
    ///
    /// Order matters here, and it is deliberately *not* "check permission,
    /// then capture": `CGPreflightScreenCaptureAccess` is known to report a
    /// false negative even when Screen Recording is genuinely granted (the
    /// owner saw exactly this — granted in System Settings, app still
    /// reported it missing) — gating the capture on it blocks a capture
    /// that would have worked. So `screencapture` is always attempted
    /// first; the preflight is consulted only afterwards, and only to
    /// explain an empty result (see `resolve_missing_capture`). A missing
    /// grant and a user cancel leave the same "no file" signature on disk,
    /// which is exactly why that second step exists.
    fn select(&self) -> Result<Option<PathBuf>> {
        let path = unique_capture_path();
        crate::log_line!("capture: invoking screencapture -i -x {}", path.display());
        // -i: interactive crosshair selection; -x: no camera-shutter sound.
        let status = Command::new("screencapture")
            .arg("-i")
            .arg("-x")
            .arg(&path)
            .status()?;
        crate::log_line!("capture: screencapture exited with {status}");

        // screencapture's exit code for a cancelled (Escape) selection is
        // not reliable across macOS versions, so it is deliberately not
        // inspected — the file left on disk (or not) is the only signal
        // used, via check_capture_result below.
        let _ = status;

        if let Some(file) = check_capture_result(&path)? {
            return Ok(Some(file));
        }
        resolve_missing_capture()
    }
}

/// Called only when `screencapture` produced no usable file (nothing
/// written, or a zero-byte file) — i.e. `check_capture_result` already
/// returned `Ok(None)`. Decides, only now, whether that was a deliberate
/// user cancel or a missing-permission failure that merely *looks* like
/// one at the filesystem level, by consulting
/// `CGPreflightScreenCaptureAccess`.
///
/// The preflight is logged either way — granted or not — so a lying
/// preflight is visible in the log rather than silently swallowed.
///
/// A `false` reading also fires `CGRequestScreenCaptureAccess` once so
/// macOS shows the user the system permission prompt — otherwise a
/// first-run user would get the error message with no prompt ever having
/// appeared.
fn resolve_missing_capture() -> Result<Option<PathBuf>> {
    let has_access = unsafe { CGPreflightScreenCaptureAccess() };
    crate::log_line!("capture: post-capture preflight has_access={has_access}");
    if has_access {
        // The permission is in fact granted, so the empty result can only
        // have come from the user pressing Escape.
        return Ok(None);
    }
    unsafe {
        CGRequestScreenCaptureAccess();
    }
    // `has_access` is `false` here, so `access_result` always returns
    // `Err` — the `.map` never actually runs, it only makes the return
    // type line up with this function's `Result<Option<PathBuf>>`.
    access_result(has_access).map(|()| None)
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

/// Decides whether an interactive capture aimed at `path` produced a usable
/// file, by inspecting what (if anything) ended up on disk.
///
/// `screencapture`'s exit code does not reliably distinguish a cancelled
/// (Escape) selection from a permission failure across macOS versions, so
/// file presence/size is the only signal used here. Split out from `select`
/// so this logic is testable without driving the actual interactive UI,
/// which cannot be automated. This function alone cannot tell a cancel
/// apart from a permission problem — both leave nothing (or a zero-byte
/// file) at `path` — so `Ok(None)` here means only "no usable file", and
/// `select` disambiguates the two afterwards, in `resolve_missing_capture`.
fn check_capture_result(path: &Path) -> Result<Option<PathBuf>> {
    match std::fs::metadata(path) {
        Ok(meta) if meta.len() > 0 => {
            crate::log_line!("capture: output file present, {} bytes", meta.len());
            Ok(Some(path.to_path_buf()))
        }
        Ok(meta) => {
            crate::log_line!(
                "capture: output file present but empty ({} bytes)",
                meta.len()
            );
            Ok(None)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            crate::log_line!("capture: no output file written");
            Ok(None)
        }
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

    /// Fix 2's core guarantee: a capture that actually produced a file
    /// must never be overridden by a lying `CGPreflightScreenCaptureAccess`.
    /// `select()` cannot be driven directly here (it launches the real
    /// interactive `screencapture` UI), so this exercises the same
    /// short-circuit at the level `select()` itself relies on — once
    /// `check_capture_result` reports a usable file, `select()` returns it
    /// immediately and never reaches `resolve_missing_capture` at all.
    /// The second half demonstrates *why* that ordering matters: taking
    /// the path `select()` deliberately avoids here — asking
    /// `resolve_missing_capture` to judge a `false` preflight on its own —
    /// produces an `Err`, i.e. exactly the false negative a preflight-first
    /// gate would have surfaced instead of the successful capture.
    #[test]
    fn a_produced_file_wins_even_though_preflight_would_say_false() {
        let path = unique_capture_path();
        std::fs::write(&path, [0x89, b'P', b'N', b'G']).unwrap();
        let file = check_capture_result(&path).unwrap();
        std::fs::remove_file(&path).ok();

        assert_eq!(
            file,
            Some(path),
            "a usable file must short-circuit before any preflight check"
        );
        assert!(
            resolve_missing_capture_result(false).is_err(),
            "sanity: had select() consulted preflight first instead, a \
             false reading alone would have been reported as the missing- \
             permission error, blocking a capture that just succeeded"
        );
    }

    #[test]
    fn resolve_missing_capture_result_true_is_a_cancel() {
        assert_eq!(resolve_missing_capture_result(true).unwrap(), None);
    }

    #[test]
    fn resolve_missing_capture_result_false_is_the_permission_err() {
        let err = resolve_missing_capture_result(false)
            .unwrap_err()
            .to_string();
        assert!(err.contains("Screen Recording"), "got: {err}");
    }

    /// `resolve_missing_capture` itself calls the real
    /// `CGPreflightScreenCaptureAccess`/`CGRequestScreenCaptureAccess` FFI,
    /// which cannot be driven to a chosen value in a unit test. This
    /// mirrors its decision as a function of an already-known preflight
    /// reading, exactly like `access_result` already does for the error
    /// message alone — kept test-only rather than added to the real
    /// function, since the real function's contract is "query the FFI",
    /// not "take a parameter".
    fn resolve_missing_capture_result(has_access: bool) -> Result<Option<PathBuf>> {
        if has_access {
            return Ok(None);
        }
        access_result(has_access).map(|()| None)
    }
}
