//! Launch at login on Windows — **deliberately not implemented.**
//!
//! This is a seam stub, the same shape as `capture/windows.rs`,
//! `ocr/windows.rs` and `selection/windows.rs`: it exists so the crate
//! compiles and the IPC surface is present, not so the feature works.
//!
//! **Do not implement it here without asking Andrii first.** The
//! mechanism is an open product decision (`BKM/PC-Queue/TASK-M6-…` §8
//! item 4), and the candidates are not equivalent:
//!
//! - the **HKCU `Run` key** (what `tauri-plugin-autostart` uses on
//!   Windows — its macOS ban in `CLAUDE.md` is macOS-scoped and does not
//!   apply here),
//! - a **Startup-folder shortcut**,
//! - a **Task Scheduler** entry.
//!
//! They differ in whether the user can see and revoke the entry, whether
//! a moved `.exe` breaks it, and what an uninstaller has to clean up.
//! Whichever is chosen, it must be per-user and unelevated: Windows
//! **refuses** to launch a `requireAdministrator` app from the Run key or
//! the Startup folder, so the manifest's `asInvoker` execution level is a
//! precondition of this feature, not an unrelated setting.
//!
//! # Why this reports `Unsupported` rather than "off"
//!
//! `LoginItemStatus::NotRegistered` means *the OS was asked and said no*.
//! Nothing is asked here, so reporting it would put a live, switchable
//! toggle over a mechanism that does not exist — the settings window
//! would offer to turn on something that can only fail. `Unsupported`
//! carries a note and lets the page render the control disabled, which
//! is the same discipline as constraint 12: the toggle shows what is
//! true, never what was requested.

use super::{LoginItemError, LoginItemService, LoginItemStatus};

/// The Windows `LoginItemService`: honest about not existing.
pub struct UnsupportedLoginItem;

impl LoginItemService for UnsupportedLoginItem {
    fn status(&self) -> LoginItemStatus {
        LoginItemStatus::Unsupported
    }

    fn register(&self) -> Result<(), LoginItemError> {
        Err(unsupported())
    }

    fn unregister(&self) -> Result<(), LoginItemError> {
        Err(unsupported())
    }
}

/// The error both directions return.
///
/// `domain`/`code` are shaped for `SMAppService`, so the Windows arm
/// names itself rather than borrowing Apple's domain, and uses `0` —
/// there is no OS error here because no OS call was made.
fn unsupported() -> LoginItemError {
    LoginItemError {
        domain: "AloudWindowsLoginItem".into(),
        code: 0,
        message: "Launch at login is not implemented on Windows yet".into(),
    }
}

/// The Windows counterpart of macOS's "open System Settings → Login
/// Items" deep link.
///
/// There is nothing to open: no OS pane owns a login item Aloud never
/// created. The settings page hides the button for the `Unsupported`
/// status, so this should not be reachable — it logs rather than doing
/// nothing silently, per the M6 rule that Windows failures must not be
/// quiet.
pub fn open_system_settings_login_items() {
    crate::log_line!(
        "login item: no settings pane to open - launch at login is not implemented on Windows"
    );
}
