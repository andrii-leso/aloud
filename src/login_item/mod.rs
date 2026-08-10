//! Launch at login.
//!
//! macOS registers the **app bundle** as a login item through
//! `SMAppService` (13.0+). The whole Objective-C side is gated into
//! `macos.rs`, same shape as `capture/`, `ocr/` and `selection/` — per
//! Aloud's hard constraint 8. This file holds the parts that are not
//! platform interop: the status model, and the apply logic that decides
//! what an OS error actually means.
//!
//! Two things about this feature are unlike every other setting in the
//! app, and the design here follows from them:
//!
//! 1. **The state does not live in `settings.json` — it lives in macOS's
//!    Background Task Management store, outside the app, and it survives
//!    the app being deleted.** So the persisted `launch_at_login` bool is
//!    a record of what the user asked for, and `status()` is the record
//!    of what is actually true. When they disagree, `status()` wins on
//!    screen. See `LoginItemStatus::is_on`.
//! 2. **A user can turn it off in System Settings → General → Login
//!    Items, and Aloud is never told.** That shows up here as
//!    `RequiresApproval`. Re-`register()`ing to "fix" it would override a
//!    deliberate opt-out, so nothing in this crate ever does that
//!    automatically — only an explicit toggle in the settings window
//!    registers.
//!
//! Why not `tauri-plugin-autostart`: it pins `auto-launch 0.5`, whose
//! default macOS mode writes a LaunchAgent pointing at
//! `Contents/MacOS/aloud` — the inner binary — which bypasses
//! LaunchServices, the mechanism that registers the `NSServices`
//! provider, and would silently kill "Read Aloud". Full evidence in
//! `docs/M4-platform-research-macos.md` §2 and `CLAUDE.md`.

#[cfg(target_os = "macos")]
pub mod macos;

/// What the OS currently says about Aloud's login item.
///
/// Mirrors `SMAppService.Status`. `Unknown` exists because Apple can add
/// a case: an unrecognised value must read as "not on" rather than
/// panicking or being silently coerced into `Enabled`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginItemStatus {
    /// Never registered, or explicitly unregistered.
    NotRegistered,
    /// Registered and eligible to run at the next login.
    Enabled,
    /// Registered, but macOS is waiting on the user — **including the
    /// case where the user switched it off in System Settings**. Apple,
    /// verbatim: "The framework also returns this status if the user
    /// revokes consent for the service to run in System Settings."
    RequiresApproval,
    /// The framework could not find this service at all.
    NotFound,
    /// A status value this build does not know about.
    Unknown(isize),
}

impl LoginItemStatus {
    /// `SMAppServiceStatus` raw value → this enum.
    pub fn from_raw(raw: isize) -> Self {
        match raw {
            0 => Self::NotRegistered,
            1 => Self::Enabled,
            2 => Self::RequiresApproval,
            3 => Self::NotFound,
            other => Self::Unknown(other),
        }
    }

    /// Whether the settings toggle should read "on".
    ///
    /// **Only `Enabled`.** This is the whole point of reading the live
    /// status instead of the persisted bool: a user who turned Aloud off
    /// in System Settings leaves the OS reporting `RequiresApproval`, and
    /// a toggle that still showed "on" there would be asserting something
    /// about the system that is not true.
    pub fn is_on(self) -> bool {
        matches!(self, Self::Enabled)
    }

    /// A short machine-readable tag for the settings page.
    pub fn tag(self) -> &'static str {
        match self {
            Self::NotRegistered => "not_registered",
            Self::Enabled => "enabled",
            Self::RequiresApproval => "requires_approval",
            Self::NotFound => "not_found",
            Self::Unknown(_) => "unknown",
        }
    }

    /// What to tell the user, or `None` when the state is unremarkable
    /// (on, or plainly off) and a message would just be noise.
    ///
    /// Both messages that exist name System Settings, because that is the
    /// only place the user can change the state Aloud is reporting.
    pub fn note(self) -> Option<&'static str> {
        match self {
            Self::Enabled | Self::NotRegistered => None,
            Self::RequiresApproval => Some(
                "macOS has not approved this yet. Switch Aloud on in \
                 System Settings → General → Login Items.",
            ),
            Self::NotFound => Some(
                "macOS could not find Aloud's login item. Check \
                 System Settings → General → Login Items.",
            ),
            Self::Unknown(_) => Some("macOS reported a login-item state Aloud does not recognise."),
        }
    }
}

/// An `SMAppService` failure, kept in the shape the OS reported it so the
/// log can carry the real domain and code rather than a paraphrase.
///
/// Apple publishes no mapping of `SMAppServiceErrorDomain` numeric codes
/// (`docs/M4-platform-research-macos.md` §2), so `message` — the
/// localized description — is usually the only human-readable part, and
/// it is often as thin as "Operation not permitted".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginItemError {
    pub domain: String,
    pub code: isize,
    pub message: String,
}

impl std::fmt::Display for LoginItemError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} (({}) {})", self.message, self.domain, self.code)
    }
}

/// `kSMErrorAlreadyRegistered` — `register()` on something already
/// registered. Not a failure: the requested end state already holds.
pub const SM_ERROR_ALREADY_REGISTERED: isize = 12;

/// `kSMErrorJobNotFound` — `unregister()` on something not registered.
/// Same reasoning as above, mirrored.
pub const SM_ERROR_JOB_NOT_FOUND: isize = 6;

/// The three OS operations this feature needs, seamed out — same pattern
/// as `ShortcutRegistrar` in `src/bin/aloud.rs` — so the idempotence and
/// read-back-the-truth logic has unit tests that need neither a bundle
/// nor a live `SMAppService`.
///
/// `Send + Sync` because the app holds one of these in Tauri managed
/// state, reachable from any IPC command.
pub trait LoginItemService: Send + Sync {
    fn status(&self) -> LoginItemStatus;
    fn register(&self) -> Result<(), LoginItemError>;
    fn unregister(&self) -> Result<(), LoginItemError>;
}

/// Asks the OS for `want`, then returns **what the OS actually reports
/// afterwards** — never `want` itself.
///
/// Returning the read-back status rather than the request is what stops
/// the settings toggle from claiming a state the system does not hold.
/// The most common way that happens in practice: `register()` succeeds
/// but the user has previously revoked consent, so the real status is
/// `RequiresApproval`, not `Enabled`, and macOS will not launch Aloud at
/// login until they say so in System Settings.
///
/// The two "already in the requested end state" errors are folded into
/// success — an idempotent toggle must not report failure for arriving
/// where it was told to go.
pub fn apply(svc: &dyn LoginItemService, want: bool) -> Result<LoginItemStatus, LoginItemError> {
    let result = if want {
        svc.register()
    } else {
        svc.unregister()
    };

    match result {
        Ok(()) => {}
        Err(e) if want && e.code == SM_ERROR_ALREADY_REGISTERED => {}
        Err(e) if !want && e.code == SM_ERROR_JOB_NOT_FOUND => {}
        Err(e) => return Err(e),
    }

    Ok(svc.status())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn raw_status_values_map_to_apples_documented_cases() {
        assert_eq!(LoginItemStatus::from_raw(0), LoginItemStatus::NotRegistered);
        assert_eq!(LoginItemStatus::from_raw(1), LoginItemStatus::Enabled);
        assert_eq!(
            LoginItemStatus::from_raw(2),
            LoginItemStatus::RequiresApproval
        );
        assert_eq!(LoginItemStatus::from_raw(3), LoginItemStatus::NotFound);
        assert_eq!(LoginItemStatus::from_raw(99), LoginItemStatus::Unknown(99));
    }

    #[test]
    fn only_enabled_reads_as_on() {
        // The defect class this whole design exists to avoid: a user who
        // switched Aloud off in System Settings leaves the OS reporting
        // RequiresApproval. A toggle that showed "on" for that would be
        // lying about the system.
        assert!(LoginItemStatus::Enabled.is_on());
        assert!(!LoginItemStatus::RequiresApproval.is_on());
        assert!(!LoginItemStatus::NotRegistered.is_on());
        assert!(!LoginItemStatus::NotFound.is_on());
        assert!(!LoginItemStatus::Unknown(7).is_on());
    }

    #[test]
    fn only_the_states_the_user_can_act_on_carry_a_note() {
        assert!(LoginItemStatus::Enabled.note().is_none());
        assert!(LoginItemStatus::NotRegistered.note().is_none());
        for s in [
            LoginItemStatus::RequiresApproval,
            LoginItemStatus::NotFound,
            LoginItemStatus::Unknown(7),
        ] {
            assert!(s.note().is_some(), "{s:?} must explain itself");
        }
    }

    /// A scripted `LoginItemService`. `status` is a queue so a test can
    /// script the *read-back* value independently of what register /
    /// unregister returned — which is the exact divergence `apply` exists
    /// to surface.
    pub struct FakeService {
        pub register_result: Result<(), LoginItemError>,
        pub unregister_result: Result<(), LoginItemError>,
        pub status_after: LoginItemStatus,
        pub calls: Mutex<Vec<&'static str>>,
    }

    impl FakeService {
        pub fn ok(status_after: LoginItemStatus) -> Self {
            Self {
                register_result: Ok(()),
                unregister_result: Ok(()),
                status_after,
                calls: Mutex::new(Vec::new()),
            }
        }
    }

    impl LoginItemService for FakeService {
        fn status(&self) -> LoginItemStatus {
            self.calls.lock().unwrap().push("status");
            self.status_after
        }
        fn register(&self) -> Result<(), LoginItemError> {
            self.calls.lock().unwrap().push("register");
            self.register_result.clone()
        }
        fn unregister(&self) -> Result<(), LoginItemError> {
            self.calls.lock().unwrap().push("unregister");
            self.unregister_result.clone()
        }
    }

    fn err(code: isize) -> LoginItemError {
        LoginItemError {
            domain: "SMAppServiceErrorDomain".into(),
            code,
            message: "fake".into(),
        }
    }

    #[test]
    fn turning_it_on_registers_then_reports_the_read_back_status() {
        let svc = FakeService::ok(LoginItemStatus::Enabled);
        assert_eq!(apply(&svc, true), Ok(LoginItemStatus::Enabled));
        assert_eq!(svc.calls.lock().unwrap().as_slice(), ["register", "status"]);
    }

    #[test]
    fn turning_it_off_unregisters_then_reports_the_read_back_status() {
        let svc = FakeService::ok(LoginItemStatus::NotRegistered);
        assert_eq!(apply(&svc, false), Ok(LoginItemStatus::NotRegistered));
        assert_eq!(
            svc.calls.lock().unwrap().as_slice(),
            ["unregister", "status"]
        );
    }

    #[test]
    fn a_successful_register_that_the_os_will_not_honour_reports_the_truth() {
        // register() returning Ok is NOT proof the app will launch at
        // login: if the user previously revoked consent in System
        // Settings the real status is RequiresApproval. Returning `want`
        // here instead of the read-back would put a green "on" toggle
        // over a login item macOS is refusing to run.
        let svc = FakeService::ok(LoginItemStatus::RequiresApproval);
        assert_eq!(apply(&svc, true), Ok(LoginItemStatus::RequiresApproval));
        assert!(!apply(&svc, true).unwrap().is_on());
    }

    #[test]
    fn already_registered_is_success_when_turning_it_on() {
        let svc = FakeService {
            register_result: Err(err(SM_ERROR_ALREADY_REGISTERED)),
            ..FakeService::ok(LoginItemStatus::Enabled)
        };
        assert_eq!(apply(&svc, true), Ok(LoginItemStatus::Enabled));
    }

    #[test]
    fn job_not_found_is_success_when_turning_it_off() {
        let svc = FakeService {
            unregister_result: Err(err(SM_ERROR_JOB_NOT_FOUND)),
            ..FakeService::ok(LoginItemStatus::NotRegistered)
        };
        assert_eq!(apply(&svc, false), Ok(LoginItemStatus::NotRegistered));
    }

    #[test]
    fn the_idempotence_codes_are_not_swapped_between_directions() {
        // kSMErrorAlreadyRegistered from an *unregister* (and
        // kSMErrorJobNotFound from a *register*) mean something has gone
        // genuinely wrong, and must not be swallowed by the leniency
        // above.
        let svc = FakeService {
            unregister_result: Err(err(SM_ERROR_ALREADY_REGISTERED)),
            ..FakeService::ok(LoginItemStatus::Enabled)
        };
        assert!(apply(&svc, false).is_err());

        let svc = FakeService {
            register_result: Err(err(SM_ERROR_JOB_NOT_FOUND)),
            ..FakeService::ok(LoginItemStatus::NotRegistered)
        };
        assert!(apply(&svc, true).is_err());
    }

    #[test]
    fn a_real_failure_propagates_and_never_reads_back_a_status() {
        // Nothing changed, so there is nothing to read back — and
        // reporting a status here would let the caller mistake a failed
        // apply for a completed one.
        let svc = FakeService {
            register_result: Err(err(1)), // "Operation not permitted"
            ..FakeService::ok(LoginItemStatus::Enabled)
        };
        assert_eq!(apply(&svc, true), Err(err(1)));
        assert_eq!(svc.calls.lock().unwrap().as_slice(), ["register"]);
    }
}
