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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub region_shortcut: String,
    pub voice: String,
    pub speed: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            region_shortcut: DEFAULT_SHORTCUT.to_string(),
            voice: DEFAULT_VOICE.to_string(),
            speed: DEFAULT_SPEED,
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
        }
    }

    pub fn clamp_speed(v: f32) -> f32 {
        if v.is_nan() {
            return DEFAULT_SPEED;
        }
        v.clamp(MIN_SPEED, MAX_SPEED)
    }
}
