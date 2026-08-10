# Aloud — session handoff, 2026-08-09 (post-M4)

Written at the end of the session that built M4 (settings window, rebindable region
shortcut, live voice/speed, tray-menu fixes, packaging hardening) and produced the
macOS and Windows platform-research passes. Read this plus
[`M3-carry-forward.md`](M3-carry-forward.md), [`M4-platform-research-macos.md`](M4-platform-research-macos.md)
and [`M6-platform-research-windows.md`](M6-platform-research-windows.md) before planning
anything further.

## Where the project actually is

**M1–M4 are built.** The settings window opens from the tray; the region shortcut is
rebindable live with a chord recorder, a system-shortcut denylist, and a liveness probe
that confirms a newly-bound chord actually fires (macOS registers hotkeys
non-exclusively, so `register()` can succeed for a chord another app silently owns —
the probe is the only honest confirmation available); voice and speed changes apply to
the running engine without a restart; the tray menu's disabled "Read Selection" item is
relabeled and the icon dropped the crosshair element that read as a thicker corner at
22pt; and packaging now fails the build outright if the "Aloud Dev" signing certificate
is missing, rather than silently falling back to ad-hoc (see README.md — that fallback
is exactly what caused the M3 Screen-Recording-grant bug).

**What this session could not verify.** The interactive settings-window UI —
clicking the record button, pressing chords, dragging the speed slider, watching the
liveness-probe messages — was implemented and covered by characterisation tests where
the logic allows it, but was **not exercised end-to-end by any agent**. Aloud is a
menubar accessory app with no Dock icon, and `computer-use` cannot be granted access to
an app in that state. The alternative — enabling Accessibility so the window could be
driven that way — was correctly refused: "never Accessibility" is a standing project
constraint (`CLAUDE.md` hard constraint 3), not a preference to trade off against
convenience. See "Owner's outstanding manual pass" below.

## The two things that make the app work at all — do not undo them

1. **Signing.** `packaging/make-app.sh` signs with the self-signed cert **"Aloud Dev"**
   (in the login keychain). macOS binds the Screen Recording (TCC) grant to the app's
   designated requirement; ad-hoc signing re-keys that to the binary's cdhash, so every
   rebuild silently invalidates the grant while System Settings still shows the app
   enabled. A stable certificate fixes that. As of M4 Task 10, a missing "Aloud Dev"
   certificate is a **hard build failure** — there is no more silent ad-hoc fallback.
2. **`NSRequiredContext`** in `Info.plist`. Without it macOS silently omits the Service
   from the Services menu — no error anywhere. This cost most of a debugging session in
   M3.

## The debugging tool that makes this project tractable

`~/Library/Logs/Aloud/aloud.log`. `eprintln!` goes nowhere from a LaunchServices-launched
bundle with no attached terminal, so this file is the only way to see what the app is
actually doing. Read it first when something doesn't work.

To rebuild and reinstall:
```bash
cd "Tracks/Side Projects/Aloud"
./packaging/make-app.sh
rm -rf /Applications/Aloud.app && cp -R target/Aloud.app /Applications/
open /Applications/Aloud.app
```

If a change touched anything under `dist/` (the settings window), `cargo` will not
notice a **new** file there and `generate_context!()` cannot track a file that didn't
exist at the previous compile — the rebuild above will silently embed a stale binary.
Run `cargo clean -p aloud --release` first in that case (see `CLAUDE.md` hard
constraint 11).

## Launch-at-login — spiked and built (2026-08-10)

All three unknowns were spiked against the real installed bundle on this machine and
came back green, so the feature shipped: `SMAppService` via `objc2-service-management`
(`src/login_item/`), a default-off toggle in the settings window, and IPC commands that
report the OS's live status rather than what was saved.

1. **`SMAppService.mainApp.register()` accepts the self-signed "Aloud Dev" bundle.**
   `NotFound → register: Ok → Enabled → unregister: Ok → NotRegistered`, on macOS 26.6,
   with `TeamIdentifier=not set`. No `BTMErrorDomain -98`, no `kSMErrorInvalidSignature`.
2. **The `NSServices` selection path survives.** `SMAppService` registers the *bundle*
   and macOS launches it through LaunchServices — not the inner binary, which is what
   `tauri-plugin-autostart` would have done. Confirmed programmatically with
   `NSPerformService("Read Aloud")` under a LaunchServices launch.
3. **No visible focus grab.** tao does call `activateIgnoringOtherApps` unconditionally,
   but Aloud is `Accessory` with no window at launch, so there is nothing to bring
   forward. Measured: the frontmost app was unchanged across 20 samples spanning a launch.
   Scope note: that measured a *plain* launch. Delivering a **Service** message is a
   different trigger and did steal focus — macOS activates the provider itself, launch or
   no launch. Fixed 2026-08-10; see `docs/2026-08-10-selection-focus-steal.md`.

Full evidence, the design, and the remaining manual step: `docs/2026-08-10-launch-at-login.md`.

## M6 (Windows): seven constraint conflicts awaiting an owner decision

The Windows research (`docs/M6-platform-research-windows.md`, "Constraint conflicts")
found seven places Windows collides with one of Aloud's hard constraints — mostly
"zero runtime system dependencies." None of these are decided; each section gives
options and a recommendation, not a decision. Summary, worst-first:

- **C7 — the MSVC runtime.** `ort`'s prebuilt `onnxruntime.lib` is `/MD` (dynamic CRT);
  fully static linking is verified to fail with `ort` outright. Most likely thing to
  stop the PC build dead. Recommended: try `staticVCRuntime: true` first, fall back to
  `bundleVCRuntime: true` (Tauri copies the CRT DLLs beside the exe — Microsoft
  explicitly sanctions this as "local deployment," no user step either way).
- **C1 — WebView2.** Not guaranteed present on every Windows 10 machine. Recommended:
  the offline installer (`offlineInstaller`, +127 MB) for anything distributed; the
  zero-cost bootstrapper is fine for Andrii's own PC.
- **C2 / C3 — Windows OCR language packs, and Ukrainian specifically.**
  `Windows.Media.Ocr`'s language models are Features-on-Demand, not inbox — installing
  one is exactly the "first install X" step constraint 1 forbids. Ukrainian likely has
  no Windows recognizer at any price (unconfirmed — question 16 settles it). Recommended
  floor: enumerate what the machine actually has and degrade the UI to match, regardless
  of what else gets decided.
- **C4 — the WGC yellow border vs. the crosshair-drag UX.** `Windows.Graphics.Capture`
  draws a system border that can only be suppressed from a packaged app. Recommended:
  use GDI `BitBlt` instead — Aloud captures one still frame, which is exactly BitBlt's
  shape, and it sidesteps the packaging pressure entirely.
- **C5 — elevating for hotkeys vs. launch-at-login.** *Only* a live conflict if the
  UIPI/elevated-window claim turns out to be real for `RegisterHotKey` (see
  `M3-carry-forward.md` landmine 3 — it currently is not established). Recommended:
  stay non-elevated, and settle the premise (question 24) before deciding anything here.
- **C6 — MSIX sideloading.** An untrusted/self-signed MSIX needs a manual
  certificate-trust step to install — the same class of forbidden first-install chore.
  Recommended: NSIS `currentUser`, not MSIX, as the primary channel.

Before M6 tasks get written, the "Must be verified on the PC" checklist (32 numbered
questions, `docs/M6-platform-research-windows.md`) needs a pass on the actual hardware
— it settles several of the above (Ukrainian OCR, the UIPI claim, the MSVC static-link
question) with a real answer instead of a recommendation.

## Owner's outstanding manual pass

One hands-on session with the built settings window, since no agent could drive it
(see "What this session could not verify," above). The single most valuable case:

- **Open the settings window, put the chord recorder in "Press a shortcut…" mode, and
  press ⌘W.** The recorder's keydown handler calls `preventDefault()`/`stopPropagation()`
  specifically to win the race against WebKit's page-level handling of ⌘W/⌘Q (see the
  comment in `dist/settings.js`, "WebKit hands the page key equivalents before the app
  menu..."). Confirm ⌘W is captured as a chord candidate and the window does **not**
  close. If it closes instead, the race is being lost and the whole liveness-probe
  design needs a second look.

Also worth a look in the same sitting: rebind the region shortcut to something new and
confirm the "Saved… press it now to confirm" → "Confirmed" flow (and the 10-second
timeout path, if you wait it out); switch voice and speed and confirm both apply to a
read already selected from the tray, not just a subsequent one.

**And the one thing launch-at-login could not settle without you:** switch "Launch Aloud
at login" on, log out, log back in, and confirm both that the menu-bar icon is there and
that **⌘⇧A still reads a selection**. Everything short of a real logout says it will
work; nothing short of it proves it. It ships default off and was left off.

## Open, lower priority

- The `I'd` → `l'd` OCR misread: a normaliser fix is in place but the misread was never
  reproduced across four fonts and four resolutions. If it recurs, capture the raw helper
  output (`target/aloud-ocr <image>`) before trusting the fix.
- M5 (custom per-monitor overlay) is lower value than originally thought —
  `screencapture` already handles multi-monitor natively.
- M6 (Windows) needs `icons/icon.ico` before anything else builds, plus the PC-side
  verification pass above. The Windows research is done; nothing has been built yet.
- Release paperwork still owed before any distribution: OpenRAIL-M Attachment A mirrored
  into a EULA, licence shipped, attribution. Rektor drafts it.

## Machine notes

- Disk is the binding constraint on this M1 Air — it hit 1.1 GB free at worst during
  the M1–M3 session. **Always `cargo test --release`** — a stray debug build costs ~3 GB.
  Check `df -h /` before and after any build.
- Spotlight indexing is incomplete on this Mac, so `request_access`-style app lookups
  cannot resolve app names that are not in `/Applications`.
