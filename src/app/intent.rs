//! The selection hotkey's decision, as a pure function.
//!
//! ⌃⌘S is a macOS **Service**, not a hotkey Aloud owns (`Info.plist`'s
//! `NSServices` entry — see `src/selection/`). That is what makes this
//! module possible: the system hands Aloud the selected text on *every*
//! invocation, so the delivered text is itself the intent signal.
//!
//! | delivered text vs. what is playing | what follows |
//! |---|---|
//! | nothing is playing | read it |
//! | the same text | toggle pause on the read in flight |
//! | different text, or a region read | stop that and read this |
//!
//! No mode, no second chord, no timer: "the same passage again" and "this
//! other passage instead" are already distinct in the data. The old code
//! could not tell them apart because it never looked at the text, so it
//! answered both with silence.
//!
//! Seamed out of `App` deliberately. Inlined, these branches would be
//! reachable only through a live `Player`, an audio device and a spawned
//! thread — testable by ear. Here everything is a total function of two
//! values, so every branch (including ones a human would have to press a
//! hotkey at exactly the right millisecond to reach) is an ordinary unit
//! test. See `tests/selection_toggle.rs`.

/// What Aloud is reading right now, if anything.
///
/// `App` publishes this for exactly as long as a read holds the busy flag
/// and clears it the moment that read releases — see `src/app/mod.rs`. A
/// stale value would be worse than none: it would turn the next press into
/// a pause of silence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Current {
    /// A region capture (⌘⇧R). Deliberately carries no text.
    ///
    /// What a region read is speaking came out of OCR, not out of a
    /// selection, so it is never the thing a selection press is toggling —
    /// not even in the freak case where the OCR output happens to match
    /// the selection character for character. Modelling that as a variant
    /// rather than as "text that probably won't match" makes it a fact
    /// about the code instead of a coincidence about the input.
    Region,
    /// A selection read, carrying the `comparison_key` of the text that
    /// started it.
    Selection(String),
}

/// What a selection delivery should cause.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionAction {
    /// Nothing is in flight: read the delivered text.
    Speak,
    /// The same selection is in flight: pause it, or resume it if it is
    /// already paused.
    ///
    /// One action rather than a `Pause`/`Resume` pair, and that is a
    /// decision rather than an omission: which of the two it resolves to
    /// is read from the sink at the moment it is performed
    /// (`App::toggle_pause` -> `Player::is_paused`). Deciding it *here*
    /// would need a `paused` snapshot taken before the decision, which can
    /// already be stale by the time it is acted on — a second control
    /// (the tray's Pause item) can toggle in between. That is precisely
    /// the lying-control defect class this app has spent the week
    /// removing, so the sink stays the single source of truth.
    TogglePause,
    /// Something else is in flight: stop it and read the delivered text.
    Interrupt,
}

/// The form two selections are compared in: the delivered text with
/// leading and trailing whitespace removed, and **nothing else changed**.
///
/// **Trimmed, not raw.** The press this whole feature turns on is the
/// second one, with the same passage still highlighted — and "the same
/// passage" is not byte-stable as macOS delivers it. Re-selecting by
/// double-click-drag picks up a trailing space in some apps and not
/// others; dragging to the end of a line may or may not take the newline
/// with it. Those differences are invisible on screen, so a raw comparison
/// would refuse the toggle for a reason the user cannot see, and it would
/// fail into the worst available behaviour: the passage restarts from the
/// top. Raw is more literal; it is not more predictable *to the person
/// pressing the key*, which is the only predictability that counts here.
///
/// **Interior whitespace is deliberately NOT normalised.** Collapsing runs
/// of spaces or newlines would let two selections the user can see are
/// different — two indentation levels of the same line of code, a
/// paragraph with and without its line breaks — compare equal, and equal
/// means "pause" rather than "read this". Trimming the ends cannot merge
/// anything a user would call different, because a difference at the ends
/// is whitespace by definition; collapsing the middle can.
///
/// **Compared on what the Service delivered, not on the normalized text
/// that reaches the engine** (`text::normalize::normalize_ocr`). The
/// normalizer is a synthesis detail that will keep changing; binding the
/// toggle to it would mean an unrelated edit there could silently change
/// which presses toggle and which restart.
pub fn comparison_key(text: &str) -> &str {
    text.trim()
}

/// Given the delivered selection and what is currently being read, what
/// should happen.
///
/// Note what is *not* a parameter: whether playback is paused. It changes
/// no branch here — a paused read is still a read in flight, and which way
/// `TogglePause` resolves is settled against the sink when it is performed
/// rather than snapshotted here (see `SelectionAction::TogglePause`).
/// Threading it through would add a value this function must ignore and a
/// second place that can disagree with the audio device.
pub fn decide_selection(delivered: &str, current: Option<&Current>) -> SelectionAction {
    match current {
        None => SelectionAction::Speak,
        Some(Current::Region) => SelectionAction::Interrupt,
        Some(Current::Selection(active)) => {
            if comparison_key(active) == comparison_key(delivered) {
                SelectionAction::TogglePause
            } else {
                SelectionAction::Interrupt
            }
        }
    }
}
