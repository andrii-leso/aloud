//! Windows selection source — **stub, body not implemented, and out of scope
//! for the first Windows prototype.**
//!
//! Written on macOS as part of the M6 preparation pass so the seam exists and
//! the PC is not left inventing structure for it. References no Win32 type on
//! purpose: WinRT/Win32 bindings do not compile on macOS (hard constraint 7).

use super::SelectionSource;
use anyhow::{bail, Result};

/// "Read the text the user currently has selected", Windows edition.
///
/// # There is no Windows equivalent of the macOS mechanism
///
/// On macOS this is a system **Service** (Services → Read Aloud). The app never
/// asks for Accessibility permission and never touches the clipboard; the OS
/// hands the selected text to the app. That is a deliberate owner decision, and
/// Windows has nothing like it — no Services menu, no system-mediated "give this
/// app the selection" channel.
///
/// So it has to be a *pull*: hotkey -> go and get the text. That is the shape
/// this trait was already written to hold; only the mechanism behind it was
/// open. It is now decided.
///
/// # The mechanism is UI Automation. The clipboard route is REJECTED.
///
/// **This type's name is now a misnomer** — it predates the decision. Rename it
/// to `UiaSelection` when the body is written; nothing outside this file
/// references it. Design in full:
/// `BKM/PC-Queue/TASK-M6-aloud-windows-prototype.md` Appendix S.
///
/// **Do NOT implement `SendInput(Ctrl+C)` + `CF_UNICODETEXT`.** Not as the
/// mechanism, not as a fallback, not behind an off-by-default setting. It loses
/// on every axis:
///
/// * Every synthesised copy is appended to the user's **Win+V clipboard
///   history**, and restoring the current slot does not remove the entry. With
///   cloud sync on it leaves the machine. Reading three paragraphs aloud would
///   silently write three paragraphs into a persistent history the user never
///   asked to write to.
/// * A faithful save/restore is **not achievable in general** — the clipboard is
///   multi-format and an owner may delay-render a format (`SetClipboardData`
///   with a NULL handle + `WM_RENDERFORMAT`), which a third party cannot
///   capture and replay. `OpenClipboard` also races every clipboard manager.
/// * It does **not** buy back the elevated-window case, which is the only thing
///   that would have justified keeping it: `SendInput` is UIPI-blocked against
///   a higher-integrity window, and neither the return value nor
///   `GetLastError` indicates that it was blocked. The app then reads a *stale*
///   clipboard and speaks whatever was copied twenty minutes ago — worse than
///   failing.
/// * The post-`SendInput` delay has no completion signal, so it is a tuned
///   magic number that fails on a loaded machine — the failure class hard
///   constraint 5 already records as untrustworthy on this hardware.
///
/// # What to build instead
///
/// `IUIAutomation::GetFocusedElement` -> `GetCurrentPattern(UIA_TextPatternId)`
/// -> `IUIAutomationTextPattern::GetSelection` -> concatenate each range's
/// `GetText`. Bindings are already in the `windows` crate (feature
/// `Win32_UI_Accessibility`) — no new dependency.
///
/// **This does not breach "never require an accessibility-style grant".** That
/// constraint is about *permissions*, not about which API family the code
/// calls, and Windows has no grant to ask for: a non-packaged app without
/// `uiAccess` in its manifest has full UIA client access against every
/// ordinary same-integrity application, with no prompt, no signature and no
/// elevation. The only thing it cannot reach is elevated UI. UIA is in fact the
/// *non-invasive* option here — one read-only query, on one element, only when
/// the user presses a hotkey. It is the clipboard route that synthesises input.
///
/// That non-invasiveness is a rule, not an accident: **never subscribe to
/// global UIA events** (`TextSelectionChangedEvent`, focus-change) and **never
/// poll**. A standing subscription watching every selection in every app is the
/// keylogger-shaped thing the constraint guards against, and it buys nothing —
/// there is a hotkey. One query per press.
///
/// Three further rules, each load-bearing rather than polish:
///
/// * **Run `grab()` off the UI thread** — Microsoft documents that UIA calls on
///   the UI thread "can lead to very slow performance or even cause the
///   application to stop responding". Matches the macOS handler's contract.
/// * **Bound the wait** via `IUIAutomation2::put_ConnectionTimeout` /
///   `put_TransactionTimeout`. A hung provider must not brick Aloud.
/// * **`put_AutoSetFocus(false)`** — UIA otherwise sets focus to the target
///   element on pattern calls, which is precisely the focus-steal bug hard
///   constraint 16 exists to prevent. Do not import it onto the new platform.
///
/// # Mapping the outcomes onto this trait — the trait shape is unchanged
///
/// The failures here are **silent NULLs, not error codes** — the same trap
/// class as `Windows.Media.Ocr`'s `TryCreateFromLanguage`. Three outcomes, two
/// return shapes:
///
/// * Degenerate (empty) range at the caret — i.e. the hotkey was pressed with
///   nothing selected — arrives as **success with an empty string**, not an
///   error. Map to `Ok(None)`.
/// * NULL pattern, NULL ranges, or a timeout -> `Err`. NULL means
///   "unsupported"; never treat it as success.
/// * Multiple non-contiguous spans -> one range each; join them.
///
/// `Err` and `Ok(None)` must be handled **identically** upstream: **stop
/// nothing.** Hard constraint 14(c) already forbids stopping a read in flight
/// before the new text is known speakable, and an `Err` is squarely that case —
/// route it through `actions::speakable_selection` exactly as
/// `SelectionOutcome::Empty` is routed today. `intent::decide_selection` stays
/// pure and untouched; where the text came from is not its business.
///
/// # Two costs to handle deliberately, not discover
///
/// * **A second registered chord.** macOS's ⌘⇧A is a Service, not a hotkey;
///   Windows has no push channel, so selection reading needs a second
///   `RegisterHotKey` chord alongside `region_shortcut`. Hard constraint 15(b)
///   records that dropping `pause_shortcut` left exactly one registered chord
///   and thereby closed the probe-ordering trap in
///   `docs/2026-08-10-pause-resume-phase1.md` §4 — "the trap comes back the
///   moment a second one is added". This reopens it. The new chord also goes
///   through `is_media_accelerator` like every other persisted shortcut.
/// * **Aloud must not take foreground when the chord fires**, or
///   `GetFocusedElement` returns Aloud's own element instead of the user's.
///   `RegisterHotKey` posts `WM_HOTKEY` without activating the registering
///   window so this should hold for free — but that is exactly the assumption
///   that failed on macOS (constraint 16). Probe it; do not assume it.
///
/// # Fallback, and why the clipboard is not needed
///
/// When UIA yields nothing — no `TextPattern` anywhere up the ancestor chain, a
/// degenerate range, or an elevated foreground window — **say so and point at
/// the region hotkey**, which needs no permission, works against elevated
/// windows and PDFs alike, and is already built and tested. The mechanism
/// cannot distinguish "nothing selected" from "elevated window" from "app
/// exposes no text pattern", and the user can act on all three the same way, so
/// one honest message beats a silent no-op.
///
/// # Prototype scope
///
/// The first Windows prototype is **region capture -> OCR -> speak** plus a tray
/// icon. Selection reading is not part of it, and nothing above is scheduled.
/// This type exists so the seam is occupied and the decision is recorded, not
/// because it is next.
pub struct ClipboardSelection;

impl ClipboardSelection {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ClipboardSelection {
    fn default() -> Self {
        Self::new()
    }
}

impl SelectionSource for ClipboardSelection {
    /// `Ok(None)` means nothing was selected. Note the asymmetry with macOS:
    /// that implementation is push-driven and always returns `Ok(None)` here,
    /// whereas this one would do the actual work in `grab()`.
    fn grab(&self) -> Result<Option<String>> {
        bail!("selection reading is not implemented on Windows yet")
    }
}
