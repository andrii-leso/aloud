//! User settings, persisted as JSON.
//!
//! `load` deliberately cannot fail. This file gates app startup, and a
//! hand-edited or truncated JSON must never produce an app that will not
//! launch — the same failure class the runtime shortcut registration
//! exists to avoid (see `src/shortcut.rs` and the M4 research, R3).
//! Every recoverable problem is logged and replaced with a default.

use crate::log_line;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const DEFAULT_SHORTCUT: &str = "CmdOrCtrl+Shift+R";
pub const DEFAULT_VOICE: &str = "F5";
pub const DEFAULT_SPEED: f32 = 1.0;

/// Speed bounds. Below ~0.27x one chunk exceeds 30s of audio and
/// false-positives the player's stall watchdog; the CLI clamps to the
/// same window.
pub const MIN_SPEED: f32 = 0.7;
pub const MAX_SPEED: f32 = 2.0;

/// The ten Supertonic voice styles are all under 3 MB together, but only
/// these two are exposed: F5 (female) and M5 (male) are the defaults the
/// design settled on.
pub const VOICES: [&str; 2] = ["F5", "M5"];

const FILE_NAME: &str = "settings.json";

/// Two serde behaviours here are load-bearing for migration, in both
/// directions, and neither may be tightened. The container-level
/// `#[serde(default)]` lets a file written *before* a field existed still
/// load, filling the gap rather than rejecting the document. And the
/// absence of `deny_unknown_fields` lets a file written *after* a field
/// was removed still load: any `settings.json` a Phase 1 build saved
/// carries a `pause_shortcut` key for the since-retired pause chord, and
/// it must be ignored silently, leaving the region shortcut, voice, speed
/// and launch state intact. Either failure mode ends the same way —
/// `load` falls back to defaults and the user silently loses every saved
/// value.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub region_shortcut: String,
    pub voice: String,
    pub speed: f32,
    /// Whether the user has asked Aloud to start at login.
    ///
    /// Unlike every other field here, this one is **not** the source of
    /// truth for the thing it names. The real state lives in macOS's
    /// Background Task Management store, is readable via
    /// `SMAppService.mainApp.status`, and can be changed behind Aloud's
    /// back in System Settings → General → Login Items. So this records
    /// what the user asked Aloud for; `crate::login_item` reports what
    /// is actually true, and the settings window renders the latter.
    ///
    /// Kept anyway because the two diverging is worth noticing and
    /// saying out loud (see `setup()` in `src/bin/aloud.rs`) — but it
    /// deliberately never drives a re-registration, since the commonest
    /// cause of divergence is the user deliberately switching Aloud off
    /// in System Settings.
    ///
    /// The container-level `#[serde(default)]` above is what lets a
    /// `settings.json` written before this field existed still load.
    pub launch_at_login: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            region_shortcut: DEFAULT_SHORTCUT.to_string(),
            voice: DEFAULT_VOICE.to_string(),
            speed: DEFAULT_SPEED,
            // Off. Building the feature must not turn it on for anyone,
            // least of all silently on the machine it was built on.
            launch_at_login: false,
        }
    }
}

impl Settings {
    pub fn path(dir: &Path) -> PathBuf {
        dir.join(FILE_NAME)
    }

    /// Reads settings from `dir`, falling back to defaults for anything
    /// missing, unreadable, or invalid. Never fails.
    pub fn load(dir: &Path) -> Self {
        let path = Self::path(dir);
        let raw = match std::fs::read_to_string(&path) {
            Ok(raw) => raw,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                log_line!("settings: none at {}, using defaults", path.display());
                return Self::default();
            }
            Err(e) => {
                log_line!("settings: unreadable ({e}), using defaults");
                return Self::default();
            }
        };

        let mut s: Self = match serde_json::from_str(&raw) {
            Ok(s) => s,
            Err(e) => {
                log_line!("settings: invalid JSON ({e}), using defaults");
                return Self::default();
            }
        };

        s.normalize();
        s
    }

    /// Writes settings to `dir`, creating it if needed. Clamps first, so
    /// what is written is always what will be read back.
    pub fn save(&self, dir: &Path) -> anyhow::Result<()> {
        let mut s = self.clone();
        s.normalize();
        std::fs::create_dir_all(dir)?;
        let json = serde_json::to_string_pretty(&s)?;
        std::fs::write(Self::path(dir), json)?;
        log_line!("settings: saved to {}", Self::path(dir).display());
        Ok(())
    }

    /// Forces every field into a usable range. Applied on both load and
    /// save so the two can never disagree.
    fn normalize(&mut self) {
        self.speed = Self::clamp_speed(self.speed);
        if !VOICES.contains(&self.voice.as_str()) {
            log_line!("settings: unknown voice {:?}, using default", self.voice);
            self.voice = DEFAULT_VOICE.to_string();
        }
        if self.region_shortcut.trim().is_empty() {
            self.region_shortcut = DEFAULT_SHORTCUT.to_string();
        } else if crate::shortcut::is_media_accelerator(&self.region_shortcut) {
            // A hand-edited settings.json is the only way a media key can
            // reach `register()` without passing through
            // `Chord::to_accelerator`, and registering one creates a
            // session-level CGEventTap — the thing that makes macOS demand
            // Accessibility / Input Monitoring access. Aloud never asks for
            // that, so the value does not survive a load or a save.
            log_line!(
                "settings: region shortcut {:?} names a media key, using default",
                self.region_shortcut
            );
            self.region_shortcut = DEFAULT_SHORTCUT.to_string();
        }
    }

    pub fn clamp_speed(v: f32) -> f32 {
        if v.is_nan() {
            return DEFAULT_SPEED;
        }
        v.clamp(MIN_SPEED, MAX_SPEED)
    }
}
