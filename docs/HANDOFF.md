# Aloud — session handoff, 2026-08-10

Rewritten at the end of the two days that built M4 (settings window, rebindable region
shortcut, live voice/speed) and then, in one run, launch-at-login, real pause/resume, and
⌘⇧A as a selection-aware play/pause toggle. It replaces the 2026-08-09 post-M4 handoff;
the parts of that file still worth having are folded in below.

**The next goal is a Windows prototype, and its brief already exists.** Skip to
"What comes next" if that is what you are here for.

Read `CLAUDE.md` first — it is short and it carries the hard constraints. Then this file,
then whichever `docs/` record matches what you are touching.

---

## 1. What works

macOS only. `main` at the time of writing is the pause-chord-removal commit; `cargo test
--release` is **186 passed, 0 failed, 3 ignored**.

| Capability | Trigger | Notes |
|---|---|---|
| Region read | `Cmd+Shift+R` (rebindable) | drag a rectangle → `screencapture` → Vision OCR → speech |
| Selection read | `Cmd+Shift+A`, or Services → Read Aloud | a macOS **Service**: the OS hands over the text. No Accessibility, no clipboard |
| Pause / resume | `Cmd+Shift+A` again, or the tray's Pause/Resume item | true mid-sentence pause; resumes at the exact sample |
| Stop | tray | clears the paused state too |
| Settings window | tray → Settings… | region shortcut, voice (F5/M5), speed (0.7-2.0), launch at login |
| Live voice/speed | settings window | applies to the running engine, no relaunch |
| Launch at login | settings window, **default off** | `SMAppService`, not a LaunchAgent |
| Logging | always | `~/Library/Logs/Aloud/aloud.log` |

**`Cmd+Shift+A` is a toggle, not a "read this."** Same text selected → pause, then resume.
Different text → stop and read the new passage. Nothing in flight → read it. The decision is
a pure function over the delivered text (`src/app/intent.rs`), which is what made every
branch unit-testable instead of testable by ear. `Cmd+Shift+R` is deliberately *not* a
toggle — it carries no text, so a re-trigger and a stray double-press are indistinguishable,
and it stays a silent no-op while a read is in flight, including a paused one.

**There is no separate pause chord.** Phase 1 shipped `Cmd+Shift+P`; it was removed the same
day, once ⌘⇧A became a toggle, as redundant surface that also collided with VS Code's
Command Palette. The tray Pause/Resume item is **not** a convenience and must not be deleted:
macOS will not invoke a Service with nothing selected, so it is the only pause control that
works once you have clicked away and lost the selection. It carries no chord in its label,
deliberately — naming ⌘⇧A there would be a lie in exactly that case.

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
  owner uses it). So: the tray Pause item and ⌘⇧A are unconfirmed *in the running app* since
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
4. **`~/Library/Logs/Aloud/aloud.log`.** `eprintln!` goes nowhere from a LaunchServices-launched
   bundle. Read this file first when something does not work.

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
- **`RegisterHotKey` and elevated windows on Windows** is *not* an established limitation; the
  old claim in `M3-carry-forward.md` was corrected. It is a test item for the PC
  (`M6-platform-research-windows.md` question 24), not a documented constraint.

## 5. What comes next — the Windows prototype

**The brief is written and ready to hand to the PC:**
`BKM/PC-Queue/TASK-M6-aloud-windows-prototype.md` (in the Second Brain repo, not this one).
It is self-contained by design — the PC has no access to this DB and no context, so the brief
*is* the interface. Read `BKM/PC-Queue/README.md` for how that channel works: the PC never
writes the DB, it writes a sibling `-result.md` and the Mac folds the result back in.

Prototype is done when three things work on Windows: a tray icon whose menu opens the
settings window; region capture → OCR → speech on a global shortcut; and survival of a
restart. Reading the *selection* is explicitly out of scope — it is a macOS Service and has
no Windows equivalent worth faking.

**Branch `m6-windows-prep` carries the prep work** — `icons/icon.ico`, the Windows-sized tray
PNGs and their generator, and `cfg`-gated seam stubs (`src/capture/windows.rs`,
`src/ocr/windows.rs`, `src/selection/windows.rs`).

⚠ **That branch is six commits behind `main` and was cut before the pause/toggle work.** Its
merge base is `175c73b`; relative to `main` it is missing pause/resume, the ⌘⇧A toggle, the
five states that toggle made reachable, and the pause-chord removal. **Rebase or merge `main`
into it before doing anything else on Windows** — otherwise the PC builds a codebase without
the app's central interaction, and the diff will read as a mass deletion.

Also relevant before M6 tasks are written:

- The **"Must be verified on the PC" checklist** (32 numbered questions at the end of
  `docs/M6-platform-research-windows.md`) settles several open recommendations with real
  answers: Ukrainian OCR availability, the UIPI/`RegisterHotKey` claim, the MSVC static-link
  question.
- The **seven constraint conflicts** in that doc's "Constraint conflicts" section are
  undecided by design — each gives options and a recommendation, not a decision. Worst-first:
  C7 the MSVC runtime, C1 WebView2, C2/C3 OCR language packs and Ukrainian, C4 the WGC yellow
  border, C5 elevation vs autostart, C6 MSIX sideloading.
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
