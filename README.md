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
  menu). This route never touches Accessibility or the clipboard — see
  "Permissions" below for why.

Assign the Service a keyboard shortcut of your own in **System Settings →
Keyboard → Keyboard Shortcuts → Services**, under the Text category —
Aloud only ships the region hotkey built in; the Service shortcut is
yours to pick.

Aloud lives in the menubar only: no Dock icon, no window. Use the tray
icon to trigger a region read, stop whatever is currently speaking, or
quit.

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
grant and no clipboard use at all. The cost is that the keyboard shortcut
for it is assigned in System Settings rather than baked into the app.

If a permission problem (or another failure on the hotkey path — OCR
finding no text, or the OCR helper itself failing) stops a read from
happening, Aloud shows a macOS notification banner explaining why,
rather than failing silently. A deliberate `Escape` cancel stays silent
on purpose — that's not a failure.

## Building

```bash
export PATH="$HOME/.cargo/bin:$PATH"   # if cargo isn't already on PATH
packaging/make-app.sh
```

This one command builds the release binary, builds the Swift OCR helper
(`helpers/macos-ocr/build.sh` — a separate step because `swiftc` is not
something `cargo build` runs for you), assembles `target/Aloud.app`, and
ad-hoc codesigns it. The Service (Services → Read Aloud) only registers
from an installed `.app` bundle — the raw `cargo build` binary alone
won't show up there, so build the app, not just the binary, if you want
to test the selection path.

The app is **ad-hoc signed**, not signed with an Apple Developer
certificate — there's no Gatekeeper trust chain behind it. The first
launch, right-click the app in Finder and choose **Open** (rather than a
plain double-click) to get past Gatekeeper's unidentified-developer
warning. Ad-hoc signing still matters even without notarization: it
gives the app a stable identity, so the Screen Recording grant persists
across rebuilds instead of macOS asking again every time the binary's
hash changes.

## Known limits

- **macOS only.** Windows support is planned for a later milestone;
  there is no build for it yet.
- **First audio takes a few seconds, and it's load-dependent.** On a
  quiet machine, expect roughly 3 seconds from triggering a read to
  hearing the first word (model load happens once at launch, not per
  read). Under load — other CPU-heavy work running at the same time —
  this gets noticeably slower; it is not a fixed budget, it is a
  property of how busy the machine is at that moment.
