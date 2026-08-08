//! The macOS selection source: a system **Service**.
//!
//! The usual way to read the user's selection is to synthesise ⌘C and read
//! the clipboard. On macOS that needs the **Accessibility** permission,
//! which lets an app drive the whole machine and read every other app's UI —
//! wildly disproportionate for reading a selection. Owner decision,
//! 2026-08-08: we do not do that.
//!
//! Instead the app declares an `NSServices` provider in `Info.plist` and the
//! system hands us the selected text (Services → Read Aloud). No
//! Accessibility permission is requested and the clipboard is never touched.
//! The costs are real and accepted: the keyboard shortcut is assigned by the
//! user in System Settings rather than in-app, and **Services only register
//! from an installed `.app` bundle** — running the raw binary registers
//! nothing.
//!
//! Windows (M6) will synthesise Ctrl+C, because it has no equivalent
//! permission gate. Do not port that approach back here.

use crate::selection::selection_worth_speaking;
use anyhow::{anyhow, Result};
use objc2::rc::Retained;
use objc2::runtime::NSObject;
use objc2::{define_class, msg_send, AnyThread, DefinedClass, MainThreadMarker};
use objc2_app_kit::{NSApplication, NSPasteboard, NSPasteboardTypeString, NSUpdateDynamicServices};
use objc2_foundation::NSString;
use std::sync::Arc;

/// What to do with a selection the system has handed us.
///
/// Called on a **background thread**, never on the main thread — see
/// `read_selection` below for why that matters.
pub type SelectionHandler = Arc<dyn Fn(String) + Send + Sync + 'static>;

/// Instance variables of the provider class.
pub struct ProviderIvars {
    handler: SelectionHandler,
}

define_class!(
    // SAFETY:
    // - NSObject has no subclassing requirements.
    // - ServiceProvider does not implement Drop.
    #[unsafe(super(NSObject))]
    // The runtime name is fixed rather than auto-generated so that it is
    // stable and greppable when debugging with `class-dump` / `lldb`.
    #[name = "AloudServiceProvider"]
    #[ivars = ProviderIvars]
    struct ServiceProvider;

    impl ServiceProvider {
        /// The Objective-C method macOS invokes for our Service:
        ///
        /// ```objc
        /// - (void)readSelection:(NSPasteboard *)pboard
        ///              userData:(NSString *)userData
        ///                 error:(NSString **)error;
        /// ```
        ///
        /// The selector must be exactly `NSMessage` + `:userData:error:`,
        /// where `NSMessage` is `readSelection` in `Info.plist`. A mismatch
        /// fails silently: the Services menu item still appears and clicking
        /// it does nothing at all.
        ///
        /// `userData` is nil unless the plist entry carries an `NSUserData`
        /// key (ours does not), and `error` is an out-parameter for a
        /// human-readable failure string. Both are taken as raw pointers
        /// because we use neither, and a raw pointer cannot be made unsound
        /// by a nil the way `&NSString` could.
        #[unsafe(method(readSelection:userData:error:))]
        fn read_selection(
            &self,
            pboard: &NSPasteboard,
            _user_data: *const NSString,
            _error: *mut *mut NSString,
        ) {
            // SAFETY: reading an AppKit extern constant.
            let string_type = unsafe { NSPasteboardTypeString };
            let Some(text) = pboard.stringForType(string_type) else {
                // Nothing on the pasteboard we can read as text.
                return;
            };
            let text = text.to_string();
            if selection_worth_speaking(Some(&text)).is_none() {
                return;
            }

            // This callback runs on the main thread. Synthesis and playback
            // block for seconds, so doing them here would freeze the whole
            // app — the tray, the event loop, everything — for the duration
            // of the utterance. Hand the text to a worker and return
            // immediately.
            let handler = Arc::clone(&self.ivars().handler);
            std::thread::spawn(move || handler(text));
        }
    }
);

impl ServiceProvider {
    fn new(handler: SelectionHandler) -> Retained<Self> {
        let this = Self::alloc().set_ivars(ProviderIvars { handler });
        unsafe { msg_send![super(this), init] }
    }
}

/// Registers the Service provider with AppKit for the life of the process.
///
/// Must be called on the main thread, after the `NSApplication` exists —
/// i.e. from Tauri's `setup`.
///
/// This only wires up the Rust side. Whether the Service actually appears in
/// other apps' Services menus depends on the `NSServices` declaration in
/// `Info.plist` being present in an **installed `.app` bundle**; macOS does
/// not register Services for a loose executable. So a successful return here
/// is not evidence that the Service works — that can only be verified by
/// hand from an installed bundle.
pub fn register_service_provider(handler: SelectionHandler) -> Result<()> {
    let mtm = MainThreadMarker::new().ok_or_else(|| {
        anyhow!("the macOS Service provider must be registered on the main thread")
    })?;

    let provider = ServiceProvider::new(handler);
    let app = NSApplication::sharedApplication(mtm);

    // SAFETY: `provider` is a live instance of our own class, which
    // implements the selector named by `NSMessage` in Info.plist — the
    // "correct type" the setter's safety comment asks for.
    unsafe { app.setServicesProvider(Some(&provider)) };

    // Deliberate leak. AppKit does not keep the services provider alive for
    // us, and if this object is deallocated the Service stops firing with no
    // error and no log — the single hardest failure mode here to diagnose.
    // The provider is needed for as long as the process can receive a
    // Service invocation, which is the whole process lifetime, so we forget
    // the strong reference rather than trying to find an owner for it.
    std::mem::forget(provider);

    // Tell the system to re-read our Services declaration now, rather than
    // whenever it next happens to scan. Safe wrapper; no `unsafe` needed.
    NSUpdateDynamicServices();

    Ok(())
}
