# Aloud

A macOS menubar app that reads text aloud with a local neural voice. No
cloud, no account, no network — everything runs on-device.

Two ways to trigger it:

- **Region hotkey** — `Cmd+Shift+R`, drag a rectangle over any on-screen
  text (works over anything: a PDF, a browser tab, a scanned form), and
  Aloud captures it, runs it through on-device OCR, and speaks it. Press
  `Escape` to cancel a selection; nothing happens, nothing is spoken.
- **Selection Service** — select text in any app, then right-click →
  **Services → Read Aloud** (or use the Services menu under the app's own
  menu), or press **Cmd+Shift+A**. This route never touches Accessibility
  or the clipboard — see "Permissions" below for why.

While Aloud is speaking, **Cmd+Shift+P** pauses and resumes it, as does
the tray's **Pause** / **Resume** item. This is a true pause: playback
halts mid-sentence and continues from the exact sample, with nothing
re-read and nothing already synthesised thrown away. Pressing it while
nothing is speaking does nothing.

It is an ordinary chord, not the keyboard's play/pause **media** key.
That is deliberate and permanent: media keys are the only path in
`global-hotkey` that creates a `CGEventTap`, and an active tap from an
untrusted process is refused outright — measured on this machine, macOS
26.6 (`docs/media-key-control-research.md` §4). Aloud never asks for
Accessibility, so it never takes that route. A side benefit is that
pause keeps working while Spotify or Music has the media keys.

Both the region shortcut and the voice/speed the app reads with are
configurable from the tray's **Settings…** window — see "Settings"
below. The pause chord is not yet rebindable in that window; it is read
from `pause_shortcut` in `settings.json`.

Cmd+Shift+A is Aloud's default shortcut for the Service, shipped in
`Info.plist` (`NSKeyEquivalent`). Change or clear it any time in **System
Settings → Keyboard → Keyboard Shortcuts → Services**, under the Text
category. **Known conflict:** some apps bind Cmd+Shift+A themselves —
Chrome's tab search and Xcode both do. A Services shortcut takes
precedence while Aloud is running, so in those apps Cmd+Shift+A triggers
Read Aloud instead of the app's own binding; rebind one of the two in
System Settings if that collides with your workflow.

Aloud lives in the menubar only: no Dock icon, and no window until you
open one yourself. Use the tray icon to trigger a region read, pause or
resume it, stop whatever is currently speaking, open **Settings…**, or
quit. The Pause item names what the click will do, not the state it is
in: it reads **Resume** exactly while playback is paused.

## Permissions

Aloud asks for exactly one macOS permission: **Screen Recording**,
required for the region hotkey (`screencapture` needs it to see the
screen at all). Grant it in **System Settings → Privacy & Security →
Screen Recording**, then **quit and reopen Aloud** — macOS only applies a
freshly-granted Screen Recording permission after the app restarts.

**Accessibility is deliberately never requested.** The obvious way to
read an arbitrary selection is to synthesize ⌘C and read the clipboard,
but that requires Accessibility, which grants an app the ability to
drive the whole machine and read other applications' UI — far more than
a reading app needs. Instead, the selection path registers a macOS
Service (`NSServices`): the system hands Aloud the selected text
directly when you choose Services → Read Aloud, with no Accessibility
grant and no clipboard use at all. `NSServices` also supports
`NSKeyEquivalent`, which is how Aloud ships a default shortcut
(Cmd+Shift+A) for the Service without needing Accessibility at all — see
above.

If a permission problem (or another failure on the hotkey path — OCR
finding no text, or the OCR helper itself failing) stops a read from
happening, Aloud surfaces it in the tray rather than failing silently:
the status item at the top of the tray menu shows a short `⚠ ...`
message, and the tray icon's tooltip carries the full, actionable text.
(An earlier build used a macOS notification banner via `osascript`; that
was removed because such notifications are attributed to `osascript`'s
own identity, not Aloud's, so they land under the wrong app in
Notification settings and can be silently suppressed there with no
connection back to Aloud. See `src/bin/aloud.rs`.) A deliberate `Escape`
cancel stays silent on purpose — that's not a failure.

## Settings

Open the settings window from the tray's **Settings…** item.

**Region shortcut.** The region hotkey (`Cmd+Shift+R` by default) is
rebindable there: click the shortcut button, press a new chord, and
Aloud saves it right away — then asks you to press that same chord once
more. That second press isn't a formality: macOS has no API to report
whether a chord is already owned by another app. `register()` succeeds
either way, and if something else already has it, your press is
silently shadowed with no error and no way to ask the OS about it.
Pressing it again and watching for Aloud to actually respond is an
empirical confirmation, not an availability check — a check isn't
possible here. If nothing happens within 10 seconds, the shortcut is
probably reserved elsewhere; try a different one.

**Selection shortcut.** `Cmd+Shift+A` is not rebindable in the settings
window, because it isn't Aloud's to rebind — it's a macOS Service
shortcut (see "Permissions" above), and it already works with no setup:
`Info.plist` ships it as the Service's `NSKeyEquivalent`. The tray's
**Change Selection Shortcut…** item opens **System Settings → Keyboard →
Keyboard Shortcuts → Services** directly, where it (or the conflicts
described above) can be changed.

**Voice and speed.** Pick between the two shipped voices, **F5** (female,
default) and **M5** (male), and adjust speed from **0.7×** to **2.0×**.
Both apply to the running engine immediately, no relaunch needed. The
0.7× floor exists because below it a single chunk of speech would exceed
30 seconds of audio and false-positive the player's stall watchdog. That
threshold is around **0.65-0.67×**, not the ~0.27× stated here
previously: 0.27 was computed from a 120-char chunk cap that now applies
only to the first chunk, while later chunks are capped at 300 characters
(~20 s at 1.0×). So the floor has 1-4 s of margin, not 30.

Speed is applied *after* synthesis, by `src/tts/timestretch.rs`, not by
Supertonic's own `speed` argument — that argument shrinks the canvas the
decoder renders into, and above ~1.1× it silently drops words
(2026-08-10; `docs/2026-08-10-text-drop-diagnosis.md`). The engine is
always driven at 1.0. The retiming preserves pitch, so a faster male
voice stays a male voice.

**Launch at login.** Off by default. Switching it on registers the app
bundle with macOS through `SMAppService` (13.0+), so macOS launches Aloud
at login the same way Finder would — through LaunchServices, which is what
registers the Service that `Cmd+Shift+A` depends on. (A LaunchAgent
pointing at the inner `Contents/MacOS/aloud` executable would launch the
same binary and silently lose the Service; that is why
`tauri-plugin-autostart` is not used here.)

This one setting is **not** stored in `settings.json` in any meaningful
sense. The real state lives in macOS's Background Task Management store,
survives deleting the app, and can be changed in **System Settings →
General → Login Items** without Aloud being told. So the checkbox shows
what the OS currently reports, re-read whenever the window opens *or comes
back to the front*: switch Aloud off in Login Items, return to the
settings window, and the checkbox will already be off — including if you
left it open the whole time. If macOS says approval is needed, the window says so and offers a
button straight to that pane — Aloud cannot grant its own consent, and
re-registering behind your back to "fix" it would override a deliberate
choice, so it never does. See `docs/2026-08-10-launch-at-login.md`.

Settings persist at
`~/Library/Application Support/com.andriileso.aloud/settings.json` —
inspect or delete that file to reset to defaults.

The settings window is implemented and covered by unit/characterisation
tests at the logic layer, but its interactive behaviour — the shortcut
recorder, the confirmation flow, live voice/speed switching, the
launch-at-login toggle — has not been exercised end-to-end by anyone
clicking through the actual window. Treat it as built, not yet hands-on
verified. Launch at login additionally needs one real logout/login to
confirm the Service still works from a login-item launch; everything
short of that has been verified.

## Building

```bash
export PATH="$HOME/.cargo/bin:$PATH"   # if cargo isn't already on PATH
packaging/make-app.sh
```

This one command builds the release binary, builds the Swift OCR helper
(`helpers/macos-ocr/build.sh` — a separate step because `swiftc` is not
something `cargo build` runs for you), assembles `target/Aloud.app`, and
signs it with the self-signed **"Aloud Dev"** certificate. The Service
(Services → Read Aloud) only registers from an installed `.app` bundle —
the raw `cargo build` binary alone won't show up there, so build the
app, not just the binary, if you want to test the selection path.

The app is signed with a **self-signed development certificate**
("Aloud Dev" — `Authority=Aloud Dev` on the installed bundle), not an
Apple Developer ID certificate, and no longer with an ad-hoc signature
either. This is a **development** requirement, not a distribution one —
it has nothing to do with notarization or Gatekeeper trust. The reason
it exists: macOS binds the Screen Recording (TCC) permission grant to
the app's designated requirement. Ad-hoc signing (`--sign -`) has no
certificate, so the designated requirement falls back to the binary's
cdhash — every rebuild produces a new hash, which silently invalidates
the grant while System Settings still shows the app enabled, with no
error anywhere to explain why reads have stopped working. Signing with
a certificate anchors the designated requirement to that certificate
instead, so the grant survives rebuilds. Because losing that grant
silently was a real, time-costing bug during development, a missing
"Aloud Dev" certificate is now a **hard build failure** in
`packaging/make-app.sh` — it no longer falls back to ad-hoc.

Gatekeeper still blocks the app on first launch as being from an
unidentified developer — a self-signed certificate doesn't change that,
and isn't meant to. Per Apple's current documentation for macOS 15/26,
the way past it is **System Settings → Privacy & Security**, scroll
down to the blocked-app message naming Aloud, and click **Open Anyway**
— available for roughly an hour after the blocked launch attempt.
(Older guidance suggested right-click/Control-click the app in Finder
and choose Open; that path does not appear in Apple's current support
documentation for this OS and should not be relied on.)

## Debugging

Aloud is a menubar app with no console: launched via LaunchServices
(double-click or `open`) it has no attached terminal, so `eprintln!`
output goes nowhere retrievable — the settings window is a webview and
shows none of it either. Everything worth diagnosing — app
start, engine load, hotkey presses, each stage of a region capture
(preflight, `screencapture` invocation and exit status, output-file
presence/size, OCR text length, detected language, speak start/finish),
the Service callback firing, and every error — is also logged to
`~/Library/Logs/Aloud/aloud.log` (append-only, truncated at startup past
1 MB). `tail -f` it while reproducing an issue.

## Known limits

- **macOS only.** Windows support is planned for a later milestone;
  there is no build for it yet.
- **First audio takes a few seconds, and it's load-dependent.** On a
  quiet machine, expect roughly 3 seconds from triggering a read to
  hearing the first word (model load happens once at launch, not per
  read). Under load — other CPU-heavy work running at the same time —
  this gets noticeably slower; it is not a fixed budget, it is a
  property of how busy the machine is at that moment.
- **The Supertonic model must already exist at `~/.cache/supertonic3`**
  (or `$ALOUD_MODEL_DIR`, if set) — there is no first-run download yet.
- **The hotkey and the Service are silently ignored while a read is
  already speaking.** A trigger that lands while one is in flight is
  dropped with no notification, the same as a deliberate cancel.
