use aloud::shortcut::plan_apply;

#[test]
fn a_change_unregisters_the_old_chord_then_registers_the_new_one() {
    let steps = plan_apply(Some("CmdOrCtrl+Shift+R"), "Alt+Shift+E");
    assert_eq!(
        steps,
        vec![
            aloud::shortcut::Step::Unregister("CmdOrCtrl+Shift+R".into()),
            aloud::shortcut::Step::Register("Alt+Shift+E".into()),
        ]
    );
}

#[test]
fn first_registration_has_nothing_to_unregister() {
    let steps = plan_apply(None, "CmdOrCtrl+Shift+R");
    assert_eq!(
        steps,
        vec![aloud::shortcut::Step::Register("CmdOrCtrl+Shift+R".into())]
    );
}

#[test]
fn rebinding_to_the_same_chord_is_a_no_op() {
    // Unregister-then-register of the same chord would leave a window in
    // which the hotkey is dead, for no benefit.
    assert!(plan_apply(Some("CmdOrCtrl+Shift+R"), "CmdOrCtrl+Shift+R").is_empty());
}
