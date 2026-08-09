use aloud::shortcut::{Chord, ChordError};

fn chord(code: &str, meta: bool, ctrl: bool, alt: bool, shift: bool) -> Chord {
    Chord {
        code: code.into(),
        meta,
        ctrl,
        alt,
        shift,
    }
}

#[test]
fn letter_with_cmd_shift_becomes_the_plugin_accelerator() {
    let c = chord("KeyR", true, false, false, true);
    assert_eq!(c.to_accelerator().unwrap(), "CmdOrCtrl+Shift+R");
}

#[test]
fn digits_lose_the_digit_prefix() {
    let c = chord("Digit5", true, false, true, false);
    assert_eq!(c.to_accelerator().unwrap(), "CmdOrCtrl+Alt+5");
}

#[test]
fn named_keys_pass_through_unchanged() {
    let c = chord("ArrowUp", false, false, true, true);
    assert_eq!(c.to_accelerator().unwrap(), "Alt+Shift+ArrowUp");
}

#[test]
fn modifier_order_is_stable_regardless_of_press_order() {
    let c = chord("KeyE", true, true, true, true);
    assert_eq!(c.to_accelerator().unwrap(), "CmdOrCtrl+Alt+Control+Shift+E");
}

#[test]
fn a_bare_key_is_rejected() {
    let c = chord("KeyR", false, false, false, false);
    assert!(matches!(c.to_accelerator(), Err(ChordError::NoModifier)));
}

#[test]
fn media_keys_are_rejected_because_they_create_a_cgeventtap() {
    for code in [
        "MediaPlayPause",
        "MediaTrackNext",
        "MediaTrackPrevious",
        "MediaFastForward",
        "MediaRewind",
    ] {
        let c = chord(code, true, false, false, true);
        assert!(
            matches!(c.to_accelerator(), Err(ChordError::MediaKey)),
            "{code} should be rejected"
        );
    }
}

#[test]
fn intl_backslash_is_rejected_because_the_plugin_cannot_parse_it() {
    let c = chord("IntlBackslash", true, false, false, true);
    assert!(matches!(
        c.to_accelerator(),
        Err(ChordError::Unsupported(_))
    ));
}

#[test]
fn documented_system_chords_are_rejected() {
    // Apple support 102650. register() would return Ok for these and the
    // chord would then be silently shadowed, so the denylist is the only
    // place a user can be told.
    assert!(matches!(
        chord("Space", true, false, false, false).to_accelerator(),
        Err(ChordError::SystemReserved(_))
    ));
    assert!(matches!(
        chord("Tab", true, false, false, false).to_accelerator(),
        Err(ChordError::SystemReserved(_))
    ));
    assert!(matches!(
        chord("Digit5", true, false, false, true).to_accelerator(),
        Err(ChordError::SystemReserved(_))
    ));
}

#[test]
fn a_bare_modifier_press_is_rejected_rather_than_recorded() {
    let c = chord("ShiftLeft", false, false, false, true);
    assert!(matches!(
        c.to_accelerator(),
        Err(ChordError::Unsupported(_))
    ));
}

#[test]
fn the_default_shortcut_survives_a_round_trip() {
    let c = chord("KeyR", true, false, false, true);
    assert_eq!(
        c.to_accelerator().unwrap(),
        aloud::settings::DEFAULT_SHORTCUT
    );
}
