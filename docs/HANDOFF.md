# Aloud — current state

**Read this before changing anything.** It is the working-state document: what is built, what
is verified, what is merely believed, and which questions are already settled so they do not
get re-litigated. It is deliberately blunt about the difference between "covered by tests" and
"a human has actually seen it work" — that distinction has cost this project real time twice.

It is also how the project moves between the two machines it is developed on. The macOS half is
built and verified on an M1 Air; the Windows half cannot be compiled there at all (WinRT
bindings do not build on macOS — `CLAUDE.md` hard constraint 7) and is built on a separate
Windows PC. This file travels with the repo, so `git pull` is the handoff.

Read `CLAUDE.md` first — it is short and it carries the hard constraints. Then this file, then
whichever `docs/` record matches what you are touching. Dated records under `docs/` are
implementation history: they keep their original text and carry dated banners when they are
superseded, so a wrong prediction stays visible next to what actually happened.

**Last substantive update: 2026-08-16**, when the Windows port was merged to `main` and first
compiled and tested on a Mac. §5 records what the port landed. Sections not marked otherwise
are the 2026-08-10 macOS state and were not re-verified from Windows.

---

## 1. What works

macOS is complete. **Windows now reads a dragged region and speaks it** (§5).
`cargo test --release` is **194 passed / 0 failed / 3 ignored on macOS** (M1 Air,
2026-08-16) and **202 / 0 / 3 on Windows** (2026-08-11). The two differ legitimately: the
Windows-only capture and overlay tests do not exist on macOS, and the accelerator tests are
`#[cfg]`-split per platform. **Treat the Windows figure as "green with one known
intermittent"** — `tests/engine_speed_pin.rs` flaked roughly one run in four there. It does
not flake on macOS at all: 10 consecutive runs came back with a delta of exactly 0 and
byte-identical sample counts, so the cause is Windows-side and undiagnosed. See
`BKM/PC-Queue/TASK-M6b-aloud-speed-pin-flake.md`.

The repo is on GitHub (private, `andrii-leso/aloud`) and has CI —
`.github/workflows/ci.yml`, now **macOS and Windows**, and its header is worth reading before
you trust a tick. Each job runs everything that needs neither the 385 MB model nor a real
device. **The seven it cannot run include the engine-speed pin and the latency ratio** — the
two guards against silently mangled speech — so a green tick is not a substitute for running
the suite locally. The Windows job additionally does not run `packaging\make-win.ps1`, so it
cannot catch a missing VC++ CRT.

| Capability | Trigger | Notes |
|---|---|---|
| Region read | `Cmd+Shift+R` (rebindable) | drag a rectangle → `screencapture` → Vision OCR → speech |
| Selection read | `Opt+Shift+Cmd+A`, or Services → Read Aloud | a macOS **Service**: the OS hands over the text. No Accessibility, no clipboard |
| Pause / resume | `Opt+Shift+Cmd+A` again, or the tray's Pause/Resume item | true mid-sentence pause; resumes at the exact sample |
| Stop | tray | clears the paused state too |
| Settings window | tray → Settings… | region shortcut, voice (F5/M5), speed (0.7-2.0), launch at login |
| Live voice/speed | settings window | applies to the running engine, no relaunch |
| Launch at login | settings window, **default off** | `SMAppService`, not a LaunchAgent |
| Logging | always | macOS `~/Library/Logs/Aloud/aloud.log`; Windows `%LOCALAPPDATA%\com.andriileso.aloud\logs\aloud.log` |

**`Opt+Shift+Cmd+A` is a toggle, not a "read this."** Same text selected → pause, then resume.
Different text → stop and read the new passage. Nothing in flight → read it. The decision is
a pure function over the delivered text (`src/app/intent.rs`), which is what made every
branch unit-testable instead of testable by ear. `Cmd+Shift+R` is deliberately *not* a
toggle — it carries no text, so a re-trigger and a stray double-press are indistinguishable,
and it stays a silent no-op while a read is in flight, including a paused one.

**There is no separate pause chord.** Phase 1 shipped `Cmd+Shift+P`; it was removed the same
day, once ⌥⇧⌘A became a toggle, as redundant surface that also collided with VS Code's
Command Palette. The tray Pause/Resume item is **not** a convenience and must not be deleted:
macOS will not invoke a Service with nothing selected, so it is the only pause control that
works once you have clicked away and lost the selection. It carries no chord in its label,
deliberately — naming ⌥⇧⌘A there would be a lie in exactly that case.

**There are no media keys anywhere.** See §4.

## 2. Verified vs. unverified — the honest split

Everything below the OS boundary is covered by the suite. What that cannot reach is what
follows.

**Verified empirically, on this machine (M1 Air, macOS 26.6, the real signed bundle):**

- `SMAppService.mainApp.register()` accepts the self-signed "Aloud Dev" bundle —
  `NotFound → register: Ok → Enabled → unregister: Ok → NotRegistered`, `TeamIdentifier=not
  set`, no `BTMErrorDomain -98`.
- The `NSServices` selection path survives an `SMAppService` launch (it registers the
  *bundle*, so macOS goes through LaunchServices).
- tao's unconditional `activateIgnoringOtherApps` produces **no** visible focus grab for an
  accessory app with no window at launch — frontmost unchanged across 20 samples.
- Delivering a **Service** message *does* activate Aloud, unavoidably, and `NSApp.hide(nil)`
  is what hands focus back; `NSApp.deactivate()` was measured inert.
- An active `CGEventTap` from an untrusted process returns `NULL` regardless of mask, and a
  listen-only tap is created **disabled**. The media-key route via `global-hotkey` is dead.
- Supertonic's `speed` argument drops words above ~1.1×; the engine is now always driven at
  1.0 and speed is applied by time-stretch.
- `rodio`'s `pause()` yields silence without advancing the inner source, so nothing queued is
  discarded — checked against a real audio device by the `#[ignore]`d
  `rodio_really_pauses_without_discarding_buffers`.

**Built, tested at the logic layer, but never driven by a human:**

- The whole interactive settings window — the chord recorder, the "press it again to confirm"
  liveness probe and its 10-second timeout, the speed slider, the launch-at-login toggle. No
  agent can drive it: Aloud is an accessory app with no Dock icon, and the alternative
  (enabling Accessibility) is a standing project prohibition, not a convenience to trade off.
- **The current build has not been bundled or installed at all.** The pause-chord removal
  changed the tray item and the shortcut handler; nobody has since run
  `packaging/make-app.sh`, and nothing was installed over `/Applications/Aloud.app` (the
  owner uses it). So: the tray Pause item and ⌥⇧⌘A are unconfirmed *in the running app* since
  that commit, though both are covered below the OS boundary.
- A real logout/login with launch-at-login switched on. Everything short of it says the
  Service will still work; nothing short of it proves it. It ships off and was left off.

**The single most valuable manual check, if you get one sitting:** open the settings window,
put the recorder in "Press a shortcut…" mode, and press **⌘W**. The keydown handler calls
`preventDefault()`/`stopPropagation()` specifically to beat WebKit's page-level handling of
⌘W/⌘Q. If the window closes instead of capturing ⌘W as a candidate, that race is being lost
and the liveness-probe design needs another look.

## 3. The things that make it work at all — do not undo them

1. **Signing.** `packaging/make-app.sh` signs with the self-signed **"Aloud Dev"** cert from
   the login keychain, and a missing cert is a **hard build failure** — no ad-hoc fallback.
   macOS binds the Screen Recording (TCC) grant to the designated requirement; ad-hoc signing
   re-keys that to the binary's cdhash, so every rebuild silently invalidates the grant while
   System Settings still shows the app enabled. That cost M3 a session.
2. **`NSRequiredContext`** in `Info.plist`. Without it macOS silently omits the Service from
   the Services menu — no error anywhere. That cost M3 another session.
3. **`NSApp.hide(nil)` first in `read_selection`**, before the pasteboard read and before
   every early return. It is the focus hand-back.
4. **The log file** — macOS `~/Library/Logs/Aloud/aloud.log`, Windows
   `%LOCALAPPDATA%\com.andriileso.aloud\logs\aloud.log`. `eprintln!` goes nowhere from a
   LaunchServices-launched bundle. Read this file first when something does not work.

Rebuild and reinstall (only when asked — the owner is using the installed app):

```bash
cd "Tracks/Side Projects/Aloud"
./packaging/make-app.sh
rm -rf /Applications/Aloud.app && cp -R target/Aloud.app /Applications/
open /Applications/Aloud.app
```

If anything under `dist/` changed, run `cargo clean -p aloud --release` first — `cargo` does
not notice a *new* file there and the rebuild will silently embed a stale binary
(`CLAUDE.md` hard constraint 11).

## 4. Settled questions — do not re-litigate these

- **The media key is dropped, not pending.** `MPRemoteCommandCenter` was the one sanctioned
  route, and it was implemented (F8 → `toggle_pause`, `objc2-media-player`, next/previous/seek
  disabled, no user text in `nowPlayingInfo`). The gate was always the hand-back: does
  releasing give the key back to whatever was playing before? On the owner's hands-on test it
  did not — control never returned to Music.app after a read ended. Dropped for that reason.
  The work is preserved unmerged at tag `experiment/media-key-mpremote`; its own report,
  `docs/2026-08-10-media-key-phase2.md`, exists **only on that tag**, and the programmatic
  check in it that said hand-back worked is precisely what pressing the key disproved.
- **The `CGEventTap` route to media keys** is settled against us by direct measurement
  (`docs/media-key-control-research.md` §4). Both halves of the media-key question are now
  closed, from opposite directions.
- **Launch-at-login's three unknowns** are all green — see §2.
- **`RegisterHotKey` and elevated windows on Windows — ANSWERED on the PC, 2026-08-11: the
  hotkey DOES fire.** `M3-carry-forward.md` item 3 had already demoted the old claim to a test
  item; this is the confirmation it asked for. Measured with Aloud unelevated (Medium
  integrity) and an elevated PowerShell holding the foreground: the hotkey fired and the
  overlay came up over it. The control matters — an injector at Medium integrity cannot send
  input to a High-integrity window at all (UIPI), which looks identical to the hotkey failing,
  so the test was re-run from a High-integrity injector where the keystroke is as real as a
  physical one. Item 3 in `M3-carry-forward.md` can be closed as confirmed.

## 5. The Windows prototype — BUILT, 2026-08-11

Rewritten from "what comes next" to what happened. Executed from
`BKM/PC-Queue/TASK-M6-aloud-windows-prototype.md`; the evidence, the numbered answers and the
places the brief turned out to be wrong are in the sibling `-result.md`. The stale warnings
this section used to carry — that the branch was six commits behind, and that the CI job must
stay commented out — are both resolved and are gone.

**All three prototype criteria pass.** Tray icon and menu; region capture → OCR → speech on
`Ctrl+Shift+R`; survives a restart.

**MERGED to `main` on 2026-08-16, and the gate this paragraph used to set is cleared.** It
warned that the branch touched 18 files the macOS build compiles and had never been compiled
on a Mac. It has now been: `cargo check --all-targets --locked` clean, and the full release
suite **189 passed / 0 failed / 3 ignored** with zero warnings, before the fast-forward. The
branch is redundant and can be deleted.

What landed, beyond the stubs:

| Piece | Notes |
|---|---|
| Region overlay | `src/capture/windows/overlay.rs`, raw Win32, one window per monitor (constraint 4). Snapshot-first, so it is NOT a layered window — see the module doc for why |
| OCR | `Windows.Media.Ocr`. **en-US and ru only on the dev machine**; no German. Ukrainian does not exist as a Windows OCR FOD at any price |
| Application manifest | PerMonitorV2 from process start, long paths, UTF-8, `asInvoker` |
| Log file | `%LOCALAPPDATA%\com.andriileso.aloud\logs\aloud.log` |
| Packaging | `packaging\make-win.ps1` — copies the VC++ CRT beside the exe and verifies imports with `dumpbin`. Without it the exe does not load on a machine that lacks the redistributable |
| CI | The Windows job is uncommented and green |

Still deliberately absent on Windows: **selection reading** (`src/selection/windows.rs` is an
`Err` stub — no system-mediated selection channel exists; the UI Automation design is in the
brief's Appendix S) and **launch at login** (`UnsupportedLoginItem`; the mechanism is a
product decision, not an implementation detail).

Three things the PC measured that contradict this repo's own docs, none yet folded into the
Mac-owned files:

1. **Global hotkeys DO fire against elevated windows** — see §4.
2. **Protected (DRM) windows are omitted from a capture, not blackened.** You get whatever was
   behind them. `src/capture/windows.rs` is corrected; `M6-platform-research-windows.md` is
   not.
3. **GDI `AlphaBlend` returns TRUE and silently does nothing** on that machine, with a
   stretched small source. The overlay precomputes a dimmed buffer instead.

Also settled: the MSVC runtime question (constraint conflict C7) — `STATIC_VCRUNTIME` links
against `ort` but only removes two of the four CRT imports, because `MSVCP140` comes from ONNX
Runtime's own C++. App-local deployment is the answer; `make-win.ps1`'s header records why.
- `tauri-plugin-autostart` is banned on macOS but is the **recommended** Windows mechanism
  (HKCU `Run` key, a different mechanism entirely). If M6 adopts it, `cfg`-gate the dependency
  so it cannot reach the macOS build.

## 6. Open, lower priority

- The `I'd` → `l'd` OCR misread: a normaliser fix is in place but the misread was never
  reproduced across four fonts and four resolutions. If it recurs, capture the raw helper
  output (`target/aloud-ocr <image>`) before trusting the fix.
- Sub-1.0 time-stretching is a uniformity choice, not an evidenced one — nobody has
  A/B-listened engine-native vs stretched at 0.7× on M5 (`CLAUDE.md` constraint 6).
- An OCR'd heading is welded onto the paragraph below it (prosody, not content). The fix is a
  heading heuristic in the normalizer.
- M5 (custom per-monitor overlay) is lower value than originally thought — `screencapture`
  already handles multi-monitor natively.
- Release paperwork still owed before any distribution: OpenRAIL-M Attachment A mirrored into
  a EULA, the licence shipped, attribution retained. Rektor drafts it.

## 7. Machine notes

- Disk is the binding constraint on this M1 Air — it hit 1.1 GB free at worst during M1-M3.
  **Always `cargo test --release`**; a stray debug build costs ~3 GB. Check `df -h /` before
  and after any build.
- Spotlight indexing is incomplete on this Mac, so `request_access`-style app lookups cannot
  resolve app names outside `/Applications`.
- Synthesis speed is wholly load-dependent: the same sentence takes ~7s at load 4 and ~15s at
  load 24. The design spec's "~1.5s to first word" holds only on a genuinely idle machine,
  which is why the latency guard is a ratio and the absolute check is `#[ignore]`d.
