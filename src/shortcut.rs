//! Turning a chord recorded in the settings webview into an accelerator
//! string `tauri-plugin-global-shortcut` can parse.
//!
//! The important thing this module does NOT do is tell you whether a
//! chord is free. It cannot: macOS registers Carbon hotkeys
//! non-exclusively, so `register()` returns `Ok(())` for a chord already
//! owned by the system or another app and the event is then silently
//! shadowed (CarbonEvents.h: "it is not an error to register the same hot
//! key in multiple processes"). `is_registered()` only reports our own
//! bookkeeping. The denylist below therefore covers the *documented*
//! system chords only, and everything else is confirmed empirically by
//! asking the user to press the chord after saving.

use serde::Deserialize;
use std::fmt;

/// A chord as recorded by a `keydown` listener in the settings page.
/// `code` is `KeyboardEvent.code` — layout-invariant, and 1:1 with the
/// plugin's key names and with macOS virtual keycodes.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Chord {
    pub code: String,
    pub meta: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChordError {
    NoModifier,
    MediaKey,
    Unsupported(String),
    SystemReserved(String),
}

impl fmt::Display for ChordError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoModifier => write!(
                f,
                "Add at least one modifier — a plain key would fire while you type."
            ),
            Self::MediaKey => write!(
                f,
                "Media keys can't be used. They would make macOS ask for Accessibility access, \
                 and Aloud never asks for that."
            ),
            Self::Unsupported(k) => write!(f, "That key ({k}) can't be used as a shortcut."),
            Self::SystemReserved(what) => {
                write!(f, "macOS already uses that shortcut for {what}.")
            }
        }
    }
}

/// The five keys `global-hotkey` routes into `start_watching_media_keys`,
/// which is the only path in the whole dependency chain that calls
/// `CGEventTapCreate` — and therefore the only one that can produce an
/// Accessibility / Input Monitoring prompt. Rejecting them means the tap
/// is never created.
///
/// This is measured, not precautionary. `global-hotkey` 0.8.0 passes
/// `CGEventTapOptions::Default` (the *active* option), and upstream
/// states plainly that it *"will trigger OS to request `Accessibility`
/// permission"* — [PR #71](https://github.com/tauri-apps/global-hotkey/pull/71).
/// On this machine (macOS 26.6, 25G72) an active tap from an untrusted
/// process returns `NULL` whatever it masks, so the feature would not
/// even work in exchange for the prompt.
///
/// **This list is not what makes the play/pause media key work.** That is
/// `src/now_playing/` — `MPRemoteCommandCenter`, which is a registration
/// surface rather than a tap and needs no grant of any kind. The two are
/// unrelated paths and this denylist stays exactly as it is: it guards
/// *hotkey rebinding*, which must never reach the tap.
const MEDIA_KEYS: [&str; 5] = [
    "MediaPlayPause",
    "MediaTrackNext",
    "MediaTrackPrevious",
    "MediaFastForward",
    "MediaRewind",
];

/// True if `accel` — an already-built *accelerator string*, the form that
/// is persisted and handed to `global_shortcut().register()` — names one
/// of `MEDIA_KEYS`.
///
/// `Chord::to_accelerator` screens the same list, but that guards the IPC
/// path only: a chord recorded in the settings window. A hand-edited
/// `settings.json` reaches `register()` without ever passing through a
/// `Chord`, and `"CmdOrCtrl+MediaPlayPause"` there is enough to create
/// the session-level event tap — which is the one thing this app must
/// never do. So both entry points screen with this, and
/// `Settings::normalize` refuses to let such a value survive a load or a
/// save in the first place.
///
/// Matching is case-insensitive and accepts `MediaTrackPrev`, because the
/// plugin's own parser is and does: `global-hotkey-0.8.0`
/// `hotkey.rs::parse_key` matches on `key.to_uppercase()`, with
/// `"MEDIATRACKPREV" | "MEDIATRACKPREVIOUS" => MediaTrackPrevious`. An
/// exact-case `MEDIA_KEYS.contains()` would let both spellings straight
/// through to the tap.
pub fn is_media_accelerator(accel: &str) -> bool {
    accel.split('+').any(|token| {
        let token = token.trim().to_uppercase();
        token == "MEDIATRACKPREV" || MEDIA_KEYS.iter().any(|k| k.to_uppercase() == token)
    })
}

/// Documented system shortcuts (Apple support 102650). Registering any of
/// these succeeds and then does nothing, so this list is the only place
/// the user can be told the truth.
/// Tuple: (code, meta, ctrl, alt, shift, what it does).
const SYSTEM_CHORDS: [(&str, bool, bool, bool, bool, &str); 8] = [
    ("Space", true, false, false, false, "Spotlight"),
    (
        "Space",
        true,
        false,
        false,
        true,
        "the previous input source",
    ),
    ("Tab", true, false, false, false, "switching apps"),
    ("Digit3", true, false, false, true, "screenshots"),
    ("Digit4", true, false, false, true, "screenshots"),
    ("Digit5", true, false, false, true, "screenshots"),
    ("ArrowUp", false, true, false, false, "Mission Control"),
    ("ArrowDown", false, true, false, false, "App Exposé"),
];

/// Codes that are modifiers themselves — a `keydown` fires for these
/// while the user is still assembling a chord, and they can never be the
/// main key.
const MODIFIER_CODES: [&str; 8] = [
    "ShiftLeft",
    "ShiftRight",
    "ControlLeft",
    "ControlRight",
    "AltLeft",
    "AltRight",
    "MetaLeft",
    "MetaRight",
];

impl Chord {
    pub fn to_accelerator(&self) -> Result<String, ChordError> {
        if MEDIA_KEYS.contains(&self.code.as_str()) {
            return Err(ChordError::MediaKey);
        }
        if MODIFIER_CODES.contains(&self.code.as_str()) {
            return Err(ChordError::Unsupported(self.code.clone()));
        }
        // Neither `parse_key` nor `key_to_scancode` in global-hotkey 0.8
        // knows this ISO-keyboard key; registering it is a hard failure.
        if self.code == "IntlBackslash" {
            return Err(ChordError::Unsupported(self.code.clone()));
        }
        if !(self.meta || self.ctrl || self.alt || self.shift) {
            return Err(ChordError::NoModifier);
        }
        for (code, meta, ctrl, alt, shift, what) in SYSTEM_CHORDS {
            if self.code == code
                && self.meta == meta
                && self.ctrl == ctrl
                && self.alt == alt
                && self.shift == shift
            {
                return Err(ChordError::SystemReserved(what.to_string()));
            }
        }

        let key = normalize_code(&self.code)?;

        let mut parts: Vec<&str> = Vec::with_capacity(5);
        // Fixed order, so the same chord always produces the same string
        // no matter which modifier the user pressed first.
        if self.meta {
            parts.push("CmdOrCtrl");
        }
        if self.alt {
            parts.push("Alt");
        }
        if self.ctrl {
            parts.push("Control");
        }
        if self.shift {
            parts.push("Shift");
        }
        parts.push(&key);
        Ok(parts.join("+"))
    }
}

/// One OS-level operation. Extracted so the ordering decision is
/// testable without a live registration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    Unregister(String),
    Register(String),
}

/// The steps needed to move from `old` to `new`. Rebinding to the chord
/// already in force is deliberately empty: unregister-then-register of
/// the same accelerator would leave a window where the hotkey is dead,
/// and buys nothing.
pub fn plan_apply(old: Option<&str>, new: &str) -> Vec<Step> {
    match old {
        Some(o) if o == new => Vec::new(),
        Some(o) => vec![
            Step::Unregister(o.to_string()),
            Step::Register(new.to_string()),
        ],
        None => vec![Step::Register(new.to_string())],
    }
}

/// `KeyboardEvent.code` → the plugin's key token. `KeyR` → `R`,
/// `Digit5` → `5`; everything else (`ArrowUp`, `Backquote`, `F7`,
/// `Space`, `Enter`) is already the plugin's own name.
fn normalize_code(code: &str) -> Result<String, ChordError> {
    if let Some(rest) = code.strip_prefix("Key") {
        if rest.len() == 1 {
            return Ok(rest.to_string());
        }
    }
    if let Some(rest) = code.strip_prefix("Digit") {
        if rest.len() == 1 {
            return Ok(rest.to_string());
        }
    }
    if code.is_empty() {
        return Err(ChordError::Unsupported("unknown".into()));
    }
    Ok(code.to_string())
}
