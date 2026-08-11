//! File logging to the standard per-app log location: on macOS
//! `~/Library/Logs/Aloud/aloud.log`, on Windows
//! `%LOCALAPPDATA%\com.andriileso.aloud\logs\aloud.log`. See [`log_path`].
//!
//! This exists because `eprintln!` is not debuggable in practice: when
//! Aloud is launched as a bundle via LaunchServices (the *only* way it
//! works correctly — the macOS Service in `src/selection/macos.rs` only
//! registers from an installed `.app`, not a loose binary), stderr is not
//! attached to anything the owner can read. The hotkey does nothing, and
//! there is no way to find out why. A file the owner can `cat` any time
//! is the fix.
//!
//! No new dependency: `std::fs::OpenOptions` in append mode, plus
//! `SystemTime::now()` formatted as seconds-since-epoch for a timestamp.
//! Ordering matters far more than pretty formatting here.
//!
//! Logging must never be the reason the app crashes or behaves
//! differently: every function in this module swallows its own errors
//! (a missing home directory, a permissions problem, a full disk) rather
//! than propagating them, and the `log_line!` call sites elsewhere in the
//! crate never touch a `Result` from this module.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// Truncation threshold checked once at `init()`. Append-only logging
/// across many app launches would otherwise grow without bound; this
/// caps it at a size that is still trivially `cat`-able.
const MAX_LOG_BYTES: u64 = 1024 * 1024; // 1 MB

/// The open log file, if `init()` managed to open one. `None` means
/// every `line()` call is a silent no-op — deliberately, since a logging
/// failure (no home dir, read-only filesystem, whatever) must never
/// escalate into an app failure.
static LOG_FILE: Mutex<Option<File>> = Mutex::new(None);

/// macOS: `~/Library/Logs/Aloud/aloud.log`, the standard per-app location.
#[cfg(not(target_os = "windows"))]
fn log_path() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join("Library/Logs/Aloud/aloud.log"))
}

/// Windows: `%LOCALAPPDATA%\com.andriileso.aloud\logs\aloud.log`.
///
/// The bundle identifier is in the path for the same reason
/// `MODEL_CACHE_SUBPATH` in `lib.rs` carries it: Tauri's NSIS uninstaller only
/// removes folders named after the identifier, so anything written beside it
/// rather than inside it is orphaned on every uninstall. The log is ~1 MB
/// against the model's 385 MB, but there is no reason to have two rules.
///
/// `dirs::data_local_dir()` is `%LOCALAPPDATA%` here — the same directory
/// `dirs::cache_dir()` returns on this platform, since `dirs` 5.0 collapses
/// the two on Windows. `data_local_dir` is named for what a log actually is.
///
/// The identifier is duplicated from `tauri.conf.json`'s `identifier` field
/// and from `lib.rs`; all three must change together.
#[cfg(target_os = "windows")]
fn log_path() -> Option<PathBuf> {
    dirs::data_local_dir().map(|dir| {
        dir.join("com.andriileso.aloud")
            .join("logs")
            .join("aloud.log")
    })
}

/// Opens (creating [`log_path`]'s parent directory if absent, truncating
/// the log first if it has grown past `MAX_LOG_BYTES`) the log file for the
/// life of the process.
///
/// Call once, at the very top of `main`, before anything else that might
/// log — every `line()` call before `init()` runs is silently dropped.
pub fn init() {
    let Some(path) = log_path() else {
        return; // no home directory resolvable; nothing we can do
    };
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    if let Ok(meta) = fs::metadata(&path) {
        if meta.len() > MAX_LOG_BYTES {
            let _ = fs::remove_file(&path);
        }
    }
    if let Ok(file) = OpenOptions::new().create(true).append(true).open(&path) {
        if let Ok(mut guard) = LOG_FILE.lock() {
            *guard = Some(file);
        }
    }
}

fn timestamp_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Appends one line to the log file, prefixed with a seconds-since-epoch
/// timestamp. A no-op if `init()` was never called or failed to open a
/// file. Use the `log_line!` macro rather than calling this directly —
/// it does the `format!` for you.
pub fn line(msg: &str) {
    let mut guard = match LOG_FILE.lock() {
        Ok(guard) => guard,
        // A poisoned mutex (a panic while holding the lock, elsewhere)
        // must not take logging down with it — recover the inner value
        // and keep going.
        Err(poisoned) => poisoned.into_inner(),
    };
    if let Some(file) = guard.as_mut() {
        let _ = writeln!(file, "[{}] {msg}", timestamp_secs());
        let _ = file.flush();
    }
}

/// Formats its arguments and appends them as one timestamped line to the
/// platform log file ([`crate::log::line`]). Usable anywhere in the crate via
/// `crate::log_line!(...)`; from the `aloud` binary, `aloud::log_line!(...)`.
#[macro_export]
macro_rules! log_line {
    ($($arg:tt)*) => {
        $crate::log::line(&format!($($arg)*))
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `line()` before any `init()` (or when `init()` found no file to
    /// open) must not panic — it is the steady state for every doc test
    /// and every other unit test in this crate, none of which call
    /// `init()`.
    #[test]
    fn line_without_init_does_not_panic() {
        // Whatever LOG_FILE currently holds (None, in a test binary that
        // never calls init()), this must return cleanly.
        line("line_without_init_does_not_panic probe");
    }

    #[test]
    fn timestamp_secs_is_a_plausible_unix_time() {
        // Any time after 2024-01-01T00:00:00Z (1704067200) — a loose
        // sanity bound, not a precise assertion, since the point is just
        // "this is seconds since the epoch, not something else."
        assert!(timestamp_secs() > 1_704_067_200);
    }

    #[test]
    fn log_path_is_under_library_logs_aloud() {
        let Some(path) = log_path() else {
            return; // no home dir in this environment; nothing to assert
        };
        assert!(path.ends_with("Library/Logs/Aloud/aloud.log"));
    }

    /// `init()` actually opens an appendable file at the expected path,
    /// and a subsequent `line()` call appends a timestamped line to it.
    /// Uses `$ALOUD_MODEL_DIR`-style isolation via `$HOME` override so
    /// this does not touch the real `~/Library/Logs/Aloud/aloud.log`.
    #[test]
    fn init_then_line_appends_a_timestamped_entry() {
        let tmp_home = std::env::temp_dir().join(format!(
            "aloud-log-test-home-{}-{}",
            std::process::id(),
            timestamp_secs()
        ));
        std::fs::create_dir_all(&tmp_home).unwrap();
        let real_home = std::env::var("HOME").ok();
        // SAFETY: this test does not run concurrently with other tests
        // that read $HOME from another thread in a way that would race
        // observably — `dirs::home_dir()` is read fresh inside `init()`
        // and `log_path()`, both called only from this test's own thread
        // for the duration of the override.
        unsafe { std::env::set_var("HOME", &tmp_home) };

        init();
        line("init_then_line_appends_a_timestamped_entry probe");

        let log_path = tmp_home.join("Library/Logs/Aloud/aloud.log");
        let contents = std::fs::read_to_string(&log_path)
            .expect("init() should have created an appendable log file");

        unsafe {
            match real_home {
                Some(h) => std::env::set_var("HOME", h),
                None => std::env::remove_var("HOME"),
            }
        }
        std::fs::remove_dir_all(&tmp_home).ok();
        // Reset the module-level file handle so later tests in this
        // binary (if any come to depend on init() having run) see a
        // clean slate rather than a handle pointed at the now-deleted
        // temp-home log file.
        if let Ok(mut guard) = LOG_FILE.lock() {
            *guard = None;
        }

        assert!(contents.contains("init_then_line_appends_a_timestamped_entry probe"));
        assert!(
            contents.starts_with('['),
            "expected a timestamp prefix, got: {contents}"
        );
    }
}
