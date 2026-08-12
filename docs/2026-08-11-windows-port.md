# The Windows port (M6) — what shipped, and what the machine disagreed with

**2026-08-11 / 2026-08-12, on the PC.** Branch `m6-windows-prep`, commits
`5a44d26..32e5fdf`.

Every macOS fix on 2026-08-10 left a dated record in this folder. The two PC
sessions left none, which is the gap this file closes. It is the implementation
record; the evidence pack, the numbered answers to the brief's 45 questions, and
the raw measurements live in `BKM/PC-Queue/TASK-M6-aloud-windows-prototype-result.md`
in the Second Brain repo. The design rationale is in the four `M6-design-*.md`
files beside this one.

---

## 1. Where it got to

All three prototype criteria pass: a tray icon whose menu opens the settings
window; region capture → OCR → speech on a global shortcut; survival of a
restart. Suite on Windows: **198 passed / 0 failed / 3 ignored**.

| | macOS | Windows |
|---|---|---|
| Region read | `screencapture -i` + Vision | custom Win32 overlay + `Windows.Media.Ocr` |
| Selection read | NSServices, ⌘⇧A | **absent** — `Err` stub, deliberate |
| Pause / resume | chord + tray | tray only |
| Launch at login | `SMAppService` | **absent** — `UnsupportedLoginItem`, deliberate |
| OCR languages | en / de / uk / ru | **en-US + ru** on the dev machine |
| Packaging | `packaging/make-app.sh` | `packaging/make-win.ps1` |

The language row is the real parity gap, and it is worse than the platform
research predicted. See §4.

## 2. The overlay

`src/capture/windows/overlay.rs`, ~1,250 lines. Raw Win32, **one borderless
always-on-top window per monitor** (hard constraint 4), created on a dedicated
thread with its own `GetMessage` pump so `select()` stays a plain blocking call.

Two deliberate departures from the brief, both flagged rather than slipped in:

**Snapshot-first, therefore NOT `WS_EX_LAYERED`.** The brief's route (a) said
"layered". A see-through overlay forces the capture `BitBlt` to run *after* the
overlay is gone, and the desktop underneath is not guaranteed to have
recomposited by then — so the app's own dim rectangle lands in the screenshot,
intermittently, and every workaround (`Sleep`, pump-until-idle, `DwmFlush`) is a
guess. Separately, `SetLayeredWindowAttributes` applies one uniform alpha to the
whole window and so cannot give a dimmed surround with a bright selection.
Instead the whole virtual desktop is grabbed into one top-down 32bpp DIB
**before any window exists**, each overlay paints that frozen image, and the crop
comes out of the same CPU buffer. There is no second `BitBlt`, so the race
cannot happen. Cost: the screen looks frozen while selecting, which is Snipping
Tool's behaviour and is intended.

**Precomputed dim, not `AlphaBlend`.** See §5 — this one was forced by hardware.

**Zero scale-factor multiplications in the geometry.** With PerMonitorV2 in
force from the application manifest, every quantity is already physical
virtual-desktop pixels. `GetDpiForMonitor` is called, logged, and feeds exactly
one thing: the selection band's pen width. Verified on a three-monitor desktop
with one monitor at x = −2560 and mixed 100%/150% scaling: a drag of 456×80 at
(920,175) committed as exactly 456×80 at (920,175), and drags straddling the
boundary between two differently-scaled monitors came back exact.

A test, `a_dragged_rect_is_never_scaled`, pins this. It passes trivially today
and fails loudly the moment anyone threads a scale factor through the geometry.

## 3. Constraint 1 was being violated, invisibly

`cargo build --release` alone produces an exe that hard-imports `MSVCP140.dll`,
`MSVCP140_1.dll`, `VCRUNTIME140.dll` and `VCRUNTIME140_1.dll`. Those are the
VC++ redistributable, **not** OS components — present on a dev box only because
the C++ workload installed them. On a clean Windows install the exe fails at
load with no useful message, and no dev machine can ever observe that.

`STATIC_VCRUNTIME=true` was tried first, as the brief allowed. **It does link
against `ort`** — which had been unverified — and it removes `VCRUNTIME140*`.
But `MSVCP140*` remain, because those are the C++ standard library ONNX
Runtime's own C++ pulls in, which `static_vcruntime` does not cover. Worse, it
leaves a static ucrt inside the exe while `msvcp140.dll` drags `vcruntime140.dll`
and `ucrtbase.dll` in behind it: **two CRT instances, two heaps**, and an
allocation crossing between them is corruption that will not reproduce on
demand. Rejected on that basis, not on the link result.

`packaging/make-win.ps1` therefore does app-local deployment (Microsoft sanctions
it verbatim), and then **verifies with `dumpbin` that every non-OS import is
satisfied in the output folder** — the DLL list is a claim until it is checked.

## 4. The language gap is worse than the research predicted

`M6-platform-research-windows.md` §7.7 calls Windows "a three-language design"
(en/de/ru). **On the dev machine it is two: `en-US` and `ru`.** `de-DE` OCR is
`NotPresent`, and obtaining it means `Add-WindowsCapability` — exactly the
"first install X" step constraint 1 forbids. Ukrainian does not exist as a
Windows OCR feature-on-demand at any price.

A follow-up spike measured what a bundled engine would buy (PP-OCRv5, Apache-2.0
for both code and weights, ~20.8 MB). The headline was not German:

- **Small regions return nothing today.** On 30 tight crops of real screen text,
  `Windows.Media.Ocr` returned text 0/30 raw and 1/30 through Aloud's current
  1.5× upscale. PP-OCRv5: 30/30 exact, smallest correct read 52×10 px. Dragging
  a rectangle is Aloud's primary gesture, so this is the practical gap.
- Ukrainian: 0.0000 CER vs 0.3859. German: an upgrade (≈11.5% → ≈2.4% word
  error), not a rescue — the stock en-US recognizer already reads ß/ä/ö/ü.

**Nothing was built.** Bundling an OCR engine is Andrii's call and remains open.
One cheap mitigation is available independently of that decision: pad the capture
canvas to ≥40 px rather than upscaling 1.5×.

## 5. Three things that only running could find

**`AlphaBlend` returns `TRUE` and does nothing.** The overlay's dimming was
designed as a per-paint `AlphaBlend` stretching a small black source over each
monitor. On this machine it returned `TRUE` with `GetLastError() == 0` on every
call and dimmed nothing — tested with both a zero-alpha and a forced-opaque
source. A silent success is the hardest failure shape to notice and is invisible
in a log; it was caught by screenshotting the overlay mid-drag and measuring
pixel brightness. Replaced with a precomputed dimmed copy of the frozen desktop:
deterministic, drops the `msimg32` import, two plain `SRCCOPY` blits per paint.
Verified numerically at 0.5259 of original brightness against a designed 135/256
= 0.527.

**Global hotkeys DO fire against elevated windows.** `M3-carry-forward.md` item 3
had already demoted this from a documented limitation to a test item on
2026-08-09 and asked for the result to be written back. This is that
confirmation. Measured with Aloud unelevated (Medium integrity) and an elevated
PowerShell holding the foreground: the hotkey fired and the overlay came up over
it. **The control is the load-bearing part** — an injector at Medium integrity
cannot send input to a High-integrity window at all (UIPI), which looks exactly
like the hotkey failing, so the test only means anything when run from a
High-integrity injector where the keystroke is as real as a physical one.

**The two capture-protection modes are not equivalent.** Readback-confirmed with
`GetWindowDisplayAffinity`:

| Affinity | window pixels | black | desktop behind |
|---|---|---|---|
| `WDA_NONE` (0x0) | 80.0% | 0.7% | 19.3% |
| `WDA_MONITOR` (0x1) | 0.0% | **92.9%** | 7.1% |
| `WDA_EXCLUDEFROMCAPTURE` (0x11) | 0.0% | 0.1% | **99.9%** |

`WDA_MONITOR` blacks the window out; `WDA_EXCLUDEFROMCAPTURE` — the mode modern
DRM uses — omits it and the desktop behind is captured. The old "black under
`BitBlt`, WGC and Desktop Duplication alike" claim is right for the first and
wrong for the second. WGC and Desktop Duplication were **not** measured.

The `EXCLUDEFROMCAPTURE` case is undetectable: the capture looks ordinary, so
Aloud reads out whatever sat behind the protected window with nothing indicating
the thing the user pointed at was withheld. That is worth a product decision.

> **Two wrong answers preceded that table**, and both were silent-success bugs
> of the same family as the `AlphaBlend` one. The first test never defined
> `WDA_MONITOR` on its interop class, so the constant resolved to null → 0 →
> `WDA_NONE`, and `SetWindowDisplayAffinity` returned `TRUE` having applied no
> protection; it measured "unprotected" and reported "`WDA_MONITOR` does not
> work". An independent audit then measured both modes as omitted. **Reading the
> value back is what settles it.** Anything calling a Win32 setter whose
> argument is a magic constant should read the value back and assert it.

## 6. What is deliberately not built

- **`src/selection/windows.rs`** — `Err` stub. Windows has no system-mediated
  selection channel; the clipboard route is permanently rejected. A UI Automation
  `TextPattern` design exists in the brief's Appendix S. Not started.
- **`src/login_item/windows.rs`** — `UnsupportedLoginItem`. HKCU `Run` key vs
  Startup folder vs Task Scheduler is a product decision.
- **An installer.** A portable folder is the intended shape.
- **Code signing.** Windows has no TCC analogue; signing here is purely
  distributional.
- **A bundled OCR engine.** See §4.

## 7. Still owed

- **This branch has never been compiled on a Mac.** It touches 18 files the
  macOS build compiles. `main` is an ancestor, so merging is a fast-forward with
  no conflicts — but build it there first.
- Six Tier-2 questions in the brief need a human at the tray or the settings
  window; two of them (`Ctrl+Alt+Del`, `Win+L`) are secure-desktop chords no
  injector can reach by design.
- Question 38, the offline rebuild, was not run — it needs the network
  disconnected, which would have killed the session doing the work.
- `tests/engine_speed_pin.rs` is flaky on Windows: 1 fail in 4, a different
  delta each time, on a test whose comment asserts the duration predictor is
  deterministic. It guards constraint 6, and CI cannot run it. The Mac decides
  whether to seed, widen, or `#[ignore]` it on Windows.
- Corrections owed to Mac-owned docs are listed in §13.5 of the result file.
