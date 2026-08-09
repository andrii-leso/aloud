use aloud::settings::{Settings, DEFAULT_SHORTCUT};
use std::fs;

fn tmpdir(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("aloud-settings-test-{name}"));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn missing_file_yields_defaults() {
    let d = tmpdir("missing");
    let s = Settings::load(&d);
    assert_eq!(s.region_shortcut, DEFAULT_SHORTCUT);
    assert_eq!(s.voice, "F5");
    assert_eq!(s.speed, 1.0);
}

#[test]
fn round_trips_through_disk() {
    let d = tmpdir("roundtrip");
    let s = Settings {
        region_shortcut: "Alt+Shift+E".into(),
        voice: "M5".into(),
        speed: 1.25,
    };
    s.save(&d).unwrap();
    let back = Settings::load(&d);
    assert_eq!(back.region_shortcut, "Alt+Shift+E");
    assert_eq!(back.voice, "M5");
    assert_eq!(back.speed, 1.25);
}

#[test]
fn corrupt_file_yields_defaults_rather_than_failing() {
    let d = tmpdir("corrupt");
    fs::write(d.join("settings.json"), b"{ this is not json").unwrap();
    let s = Settings::load(&d);
    assert_eq!(s.region_shortcut, DEFAULT_SHORTCUT);
}

#[test]
fn unknown_fields_are_ignored_and_missing_fields_defaulted() {
    let d = tmpdir("partial");
    fs::write(
        d.join("settings.json"),
        br#"{"voice":"M5","future_field":42}"#,
    )
    .unwrap();
    let s = Settings::load(&d);
    assert_eq!(s.voice, "M5");
    assert_eq!(s.region_shortcut, DEFAULT_SHORTCUT);
    assert_eq!(s.speed, 1.0);
}

#[test]
fn speed_is_clamped_on_load_and_on_save() {
    let d = tmpdir("clamp");
    fs::write(d.join("settings.json"), br#"{"speed":9.0}"#).unwrap();
    assert_eq!(Settings::load(&d).speed, 2.0);

    fs::write(d.join("settings.json"), br#"{"speed":0.01}"#).unwrap();
    assert_eq!(Settings::load(&d).speed, 0.7);

    // save()'s own normalize(), asserted on the raw file rather than
    // through load(): load() clamps too, so a load-based assertion passes
    // even if save() wrote the out-of-range value straight to disk —
    // which would leave the clamp on the write side untested and free to
    // regress silently.
    Settings {
        speed: 9.0,
        ..Settings::default()
    }
    .save(&d)
    .unwrap();
    let raw = fs::read_to_string(d.join("settings.json")).unwrap();
    let written: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(written["speed"], 2.0, "save() must clamp before writing");
}

#[test]
fn a_media_key_shortcut_in_a_hand_edited_file_does_not_survive_a_load() {
    // The only path by which a media key could reach
    // `global_shortcut().register()` — which routes into
    // `start_watching_media_keys` -> `CGEventTapCreate`, the session-level
    // tap that makes macOS demand Accessibility / Input Monitoring. Aloud
    // never asks for that, so this value cannot be allowed to persist.
    // Case is irrelevant: the plugin's parser uppercases before matching.
    for raw in [
        br#"{"region_shortcut":"CmdOrCtrl+MediaPlayPause"}"#.to_vec(),
        br#"{"region_shortcut":"cmdorctrl+mediatrackprev"}"#.to_vec(),
    ] {
        let d = tmpdir("mediakey");
        fs::write(d.join("settings.json"), &raw).unwrap();
        assert_eq!(
            Settings::load(&d).region_shortcut,
            DEFAULT_SHORTCUT,
            "a media key must be replaced by the default on load"
        );
    }
}

#[test]
fn a_media_key_shortcut_is_not_written_back_out_by_a_save() {
    // normalize() runs on save as well as load, so even an in-memory
    // Settings carrying a media key cannot put one on disk for the next
    // launch to read.
    let d = tmpdir("mediakey-save");
    Settings {
        region_shortcut: "CmdOrCtrl+MediaPlayPause".into(),
        ..Settings::default()
    }
    .save(&d)
    .unwrap();
    let raw = fs::read_to_string(d.join("settings.json")).unwrap();
    let written: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(written["region_shortcut"], DEFAULT_SHORTCUT);
}

#[test]
fn save_creates_the_directory_if_absent() {
    let d = tmpdir("mkdir").join("nested").join("deeper");
    Settings::default().save(&d).unwrap();
    assert!(d.join("settings.json").exists());
}

#[test]
fn an_unknown_voice_falls_back_to_the_default() {
    let d = tmpdir("badvoice");
    fs::write(d.join("settings.json"), br#"{"voice":"Z9"}"#).unwrap();
    assert_eq!(Settings::load(&d).voice, "F5");
}
