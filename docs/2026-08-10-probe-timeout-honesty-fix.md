# Liveness-probe timeout — honesty fix, 2026-08-10

Andrii rebound his region shortcut in Settings and did not press it. Ten seconds later
the window showed, in red:

> Aloud never saw that shortcut. Another app is probably using it — macOS does not
> report this, so trying a different one is the only fix.

The shortcut was fine. The message asserted a diagnosis the app had no evidence for:
all it actually knew was that no press had arrived, and it cannot distinguish "another
app owns this chord" from "you did not press anything" — the second being far more
likely, since the user is looking at the settings window, not their keyboard. Basis:
`docs/M4-platform-research-macos.md` §3 "Detecting 'that chord is taken'" — macOS
registers Carbon hotkeys non-exclusively, `register()` returns `Ok(())` even for a
chord another app owns, and there is no API that reports the contention. The only
signal available at all is a real `ShortcutState::Pressed` arriving later.

## The fix — `dist/settings.js` only

**New copy**, replacing the old timeout message:

> Aloud has not seen that shortcut yet. Press it now to confirm — if you already did,
> another app is probably using it (macOS does not report this, so a different chord
> is the only fix).

States what is known (no press yet) and both explanations, likelier one (not pressed)
first. No contractions, dash for the aside — matches the rest of the page's voice
("Saved. Press it now to confirm it works.", "Confirmed — that shortcut works.").

**No longer styled as an error.** The timeout message now calls `setStatus(chordStatus,
msg, null)` — the same neutral, no-color-class path already used for "Esc to cancel."
and "Saved. Press it now to confirm it works." The `.error` (red) class in
`settings.css` is untouched and still reserved for a genuinely rejected chord (the
`catch` block in the keydown handler). No CSS or `index.html` changes were needed —
the neutral style already existed, it just was not being used here.

**The probe stays armed on timeout instead of being given a verdict.** Previously the
10s `setTimeout` called `invoke("end_probe")`, which disarmed the Rust-side
`PROBE_ACTIVE` flag — so a press arriving at second 11 (or minute 11) landed on a
disarmed probe, fell through to a normal region capture, and the false "never saw
that shortcut" message sat on screen uncorrected forever. That was exactly Andrii's
situation. Now the timeout only rewrites the status text; it does not call
`end_probe`. `PROBE_ACTIVE` (`src/bin/aloud.rs`) is untouched by this change — it
already supported staying armed indefinitely, nothing in Rust needed to change.

**Success path already replaced rather than appended** (`aloud://probe-fired`
listener → `setStatus(...)`, which always overwrites `textContent`) — verified
unaffected, just added `probeArmed = false` bookkeeping (see below).

## Probe lifecycle after this change

Rust-side ground truth is still `PROBE_ACTIVE` (`src/bin/aloud.rs`), armed by the
`begin_probe` IPC command and disarmed by `disarm_probe(reason)`, called from three
places, none of which changed:

1. `end_probe` IPC command (`page request`) — now invoked by JS from exactly one
   place: `startRecording()`, when a new recording begins while a probe from a
   previous save is still outstanding.
2. The `settings` window's `CloseRequested` handler in `open_settings_window`'s
   window-builder branch (`settings window closed`) — window closed after being
   recreated post-destroy.
3. The `settings` window's `CloseRequested` handler attached in `setup()` to the
   config-declared window (`settings window closed`) — the path that actually runs
   in practice, since the window is declared `visible:false` in `tauri.conf.json`
   and normally just re-shown, never rebuilt.
4. Implicitly, on success: `probe_consumed()` in the hotkey handler calls
   `take_probe(&PROBE_ACTIVE)`, an atomic swap-to-false — a confirmed press disarms
   itself, no IPC round trip needed.

JS-side, `dist/settings.js` now tracks a `probeArmed` boolean (new) alongside the
existing `probeTimer` handle, because `probeTimer !== null` stopped being a reliable
proxy for "is a probe outstanding" once the 10s timeout began leaving `PROBE_ACTIVE`
armed instead of calling `end_probe`:

- **Armed:** `beginProbe()`, right after `invoke("begin_probe")` resolves — called
  once, from the keydown handler's success path after a chord saves.
- **Disarmed (JS calls `end_probe`):** `startRecording()`, when `probeArmed` is true —
  covers both "record another chord" and, transitively, Escape (Escape only reaches
  the keydown handler while `recording` is true, and `recording` is only ever set
  true by `startRecording()`, so any Escape path already passed through this same
  guard on the way in).
- **Disarmed (Rust-only, no JS involvement):** window close, via the two
  `CloseRequested` handlers above — necessary because the JS timeout that used to
  call `end_probe` is exactly what got removed; a closed window's JS is torn down
  and can never run its 10s callback anyway, so this path was already Rust-only
  before this change and still is.
- **Disarmed (self, on confirmation):** the `aloud://probe-fired` listener sets
  `probeArmed = false` to keep the JS-side bookkeeping in sync with the Rust flag
  `probe_consumed()` already cleared.
- **Left armed:** the 10s timeout. This is the actual behavior change. The probe
  now stays live for as long as the settings window stays open with no new
  recording started — which is bounded by the same three disarm paths above, so
  "stays armed" means "stays armed while the window is still open showing this
  exact unconfirmed state," not literally forever.

No Rust changes were required — `PROBE_ACTIVE` already had no auto-expiry; the old
JS timeout was the only thing artificially disarming it early.

## What could not be verified

`computer-use` cannot reach Aloud (menubar accessory app, no Dock icon), and enabling
Accessibility or injecting synthetic keystrokes to work around that is explicitly
against the project's "never Accessibility" constraint — per instructions, neither
was attempted. Not exercised end-to-end:

- Clicking "record", pressing a real chord, watching the button/status update live.
- The 10s timeout actually firing in the running app and showing the new neutral
  message (only proved statically: the exact new string is what the code sends to
  `setStatus` with `kind = null`).
- A real hotkey press arriving *after* the 10s mark and flipping the message to
  "Confirmed" (only proved by code inspection: `PROBE_ACTIVE` is not touched by the
  timeout, and `probe_consumed()` unconditionally accepts a press whenever
  `PROBE_ACTIVE` is true, regardless of how long it has been true).
- `startRecording()` correctly firing `end_probe` after a timeout has already
  rewritten the message once (only proved by code inspection of the new `probeArmed`
  guard, which no longer depends on `probeTimer` still being non-null).

What *was* verified: the source diff is confined to `dist/settings.js`; a clean
`cargo clean -p aloud --release` + `packaging/make-app.sh` rebuild picked up the new
file (constraint: a new/changed file under `dist/` needs the explicit clean, since
`cargo` does not watch it for `rerun-if-changed`); a throwaway integration test
(`tauri::generate_context!()` + `Assets::get(&"settings.js".into())`, removed after
use) confirmed the *exact* brotli-embedded bytes in the compiled context contain the
new copy and not the old one — plain `strings` on the binary cannot see this text at
all, since Tauri brotli-compresses embedded frontend assets; and the installed
`/Applications/Aloud.app` is signed with the same "Aloud Dev" identity, running, with
`~/Library/Application Support/com.andriileso.aloud/settings.json` unchanged
(`CmdOrCtrl+Shift+R`, voice `M5`, speed `1.5`) and the hotkey re-registering cleanly
on launch per `~/Library/Logs/Aloud/aloud.log`.
