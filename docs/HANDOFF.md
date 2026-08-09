# Aloud — session handoff, 2026-08-09

Written at the end of the session that built M1–M3 and did the first real user testing.
Read this plus [`M3-carry-forward.md`](M3-carry-forward.md) before doing anything.

## Where the project actually is

**M1–M3 are done and merged to `main`** (46 commits, 80 tests, zero warnings, `cargo fmt`
clean). The app is **installed at `/Applications/Aloud.app` and working on Andrii's Mac.**
Both features have been verified end to end by driving the real app, not by inference:

- **Region:** `Cmd+Shift+R` → crosshair → drag → Vision OCR → normalise → detect language
  → speak. Confirmed in the log.
- **Selection:** select text anywhere → Services → **Read Aloud** (or `Cmd+Shift+A`).
  Confirmed: `Service callback fired, pasteboard text length=64 chars … speak finished`.
- **Tray:** icon renders (square + "A"), Stop works, Quit works.

Andrii has used it. It is a real tool now, not a demo.

## The two things that make it work — do not undo them

1. **Signing.** `packaging/make-app.sh` signs with the self-signed cert **"Aloud Dev"**
   (in Andrii's login keychain). This is not cosmetic: macOS binds Screen Recording
   grants to the designated requirement, and ad-hoc signing re-keys it to the binary
   hash so **every rebuild silently invalidates the permission** while System Settings
   still shows the app enabled. Verified fixed by deliberately rebuilding — the grant
   survived. If the cert is missing the script falls back to ad-hoc with a loud warning.

2. **`NSRequiredContext`** in `Info.plist`. Without it macOS silently omits the service
   from the Services menu — no error anywhere. This cost most of a debugging session.

## The debugging tool that makes this project tractable

`~/Library/Logs/Aloud/aloud.log`. Before it existed the app was undebuggable, because
`eprintln!` goes nowhere from a LaunchServices-launched bundle. **Every diagnosis in the
last session came from this file.** When something does not work, read it first.

To rebuild and reinstall:
```bash
cd "Tracks/Side Projects/Aloud"
./packaging/make-app.sh
rm -rf /Applications/Aloud.app && cp -R target/Aloud.app /Applications/
open /Applications/Aloud.app
```

## Do this FIRST, before planning M4

**A macOS + Windows platform-integration research pass.** This is the standing lesson
from the last session and Andrii called it out directly.

M3 was planned with every Rust crate API verified from source — and zero research into
what *the operating system* requires. Andrii's first real use hit four bugs that 80
passing tests missed; three were documented platform requirements findable in one search
(`NSRequiredContext`, the signing/TCC relationship, `CGPreflightScreenCaptureAccess`
false-negatives).

So before writing an M4 plan, research and write down what macOS requires of: a menubar
app with a settings window, launch-at-login, a global-shortcut rebinding UI, and app
packaging/distribution. Do the Windows equivalents at the same time for M6. Record the
findings in the plan before writing tasks.

See [[feedback_research_platform_integration]] in auto-memory.

## Then: three things owed to Andrii

1. **Make the region shortcut rebindable from the tray.** He asked for this explicitly.
   The region shortcut is ours so it can be rebound live. The **selection** shortcut is
   macOS's (it is a Service), so it cannot be set in-app — add a menu item that opens the
   right System Settings pane instead, and say so plainly rather than pretending.
   He also wants `Cmd+Shift+A` mentioned up front so users know it exists.
2. **Relabel the disabled "Read Selection" tray item.** It reads as broken. Something
   like "Read Selection — use Services or ⌘⇧A".
3. **Tray icon legibility.** His spec (square, crosshair top-left, "A" inside, speaker
   bottom-right) is implemented, but at 22pt only the square and "A" read. Decide whether
   to simplify.

## Open, lower priority

- The `I'd` → `l'd` OCR misread: a normaliser fix is in place but the misread was never
  reproduced across four fonts and four resolutions. If it recurs, capture the raw helper
  output (`target/aloud-ocr <image>`) before trusting the fix.
- M5 (custom per-monitor overlay) is lower value than originally thought — `screencapture`
  already handles multi-monitor natively.
- M6 (Windows) is blocked on a missing `icons/icon.ico`; that is the first brief item.
- Release paperwork still owed before any distribution: OpenRAIL-M Attachment A mirrored
  into a EULA, licence shipped, attribution. Rektor drafts it.

## Machine notes

- Disk was the binding constraint all session (hit 1.1 GB at worst). Andrii has since
  freed space. **Always `cargo test --release`** — a stray debug build costs ~3 GB.
- Spotlight indexing is incomplete on this Mac, so `request_access` cannot resolve app
  names that are not in `/Applications`.
