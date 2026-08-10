//! `SMAppService` (macOS 13+), the current-generation login-item API.
//!
//! `SMAppService.mainAppService` registers **the app bundle**, and macOS
//! launches it at login the same way Finder would — through
//! LaunchServices. That is the property this whole approach was chosen
//! for: it is what registers the `NSServices` provider, so the ⌘⇧A
//! selection path keeps working. A hand-written LaunchAgent pointing at
//! `Contents/MacOS/aloud` would launch the same executable and silently
//! lose the Service.
//!
//! `SMLoginItemSetEnabled` is deprecated as of macOS 13 and cannot
//! register a main app anyway (it enables a helper in
//! `Contents/Library/LoginItems`).
//!
//! **What this writes is not the app's to clean up.** Registration is
//! persisted by the system and outlives the bundle: Apple DTS, on an
//! entry that survived deleting every copy of the app, "That is the
//! current behavior. The state is persisted to preserve user intent."
//! The undo is `sfltool resetbtm` plus a reboot.
//!
//! `mainAppService` resolves through `NSBundle.mainBundle`, so every
//! call here is about whichever bundle contains the running executable.
//! Run the loose `target/release/aloud` and you are asking about a
//! directory that is not an app; only the installed `/Applications/Aloud.app`
//! gives an answer that means anything.

use super::{LoginItemError, LoginItemService, LoginItemStatus};
use objc2_foundation::NSError;
use objc2_service_management::SMAppService;

/// The real `LoginItemService`, backed by `SMAppService.mainAppService`.
pub struct AppServiceLoginItem;

impl LoginItemService for AppServiceLoginItem {
    fn status(&self) -> LoginItemStatus {
        // SAFETY: `mainAppService` is a no-argument class method
        // returning a `SMAppService`, and `status` is a plain property
        // read on it. Neither is documented main-thread-only, and
        // neither takes a pointer we could get wrong.
        let raw = unsafe { SMAppService::mainAppService().status() };
        LoginItemStatus::from_raw(raw.0)
    }

    fn register(&self) -> Result<(), LoginItemError> {
        // SAFETY: as above; the `_` method-family binding turns the
        // `NSError**` out-parameter into a `Result` for us.
        unsafe { SMAppService::mainAppService().registerAndReturnError() }.map_err(|e| from_ns(&e))
    }

    fn unregister(&self) -> Result<(), LoginItemError> {
        // SAFETY: as above.
        unsafe { SMAppService::mainAppService().unregisterAndReturnError() }.map_err(|e| from_ns(&e))
    }
}

/// Opens System Settings → General → Login Items.
///
/// The only route out of `RequiresApproval`: that status means the user
/// (or macOS) has withheld consent, and no amount of re-registering from
/// inside the app can grant it.
pub fn open_system_settings_login_items() {
    // SAFETY: a no-argument class method with no return value.
    unsafe { SMAppService::openSystemSettingsLoginItems() };
}

/// `NSError` → `LoginItemError`, keeping the domain and code verbatim.
///
/// Both matter for diagnosis and neither is inferable from the message:
/// the on-record failure shape for a bundle BTM will not accept is
/// `SMAppServiceErrorDomain` code 1, whose entire localized description
/// is "Operation not permitted", with the real reason only visible in
/// `log stream` as `failed to construct identifier`.
fn from_ns(e: &NSError) -> LoginItemError {
    LoginItemError {
        domain: e.domain().to_string(),
        code: e.code(),
        message: e.localizedDescription().to_string(),
    }
}
