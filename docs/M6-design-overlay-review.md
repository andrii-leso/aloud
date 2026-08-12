> **Moved into this repo 2026-08-12, from a loose folder on the PC's build disk
> (`X:\dev\aloud-m6-design`) that was not under version control and had no copy
> anywhere.** The M6 Windows implementation leaned on these four documents
> heavily; losing that disk would have lost the entire design rationale behind
> `src/capture/windows/overlay.rs` and `src/ocr/windows.rs`.
>
> **Status: PRE-IMPLEMENTATION. Written 2026-08-11, before any of this ran on
> Windows.** Kept as written rather than corrected, per the `docs/` rule that
> superseded findings are annotated and not rewritten. All five blocking defects it found were real, and all five were fixed before the first compile. Its verdict of NEEDS_FIXES was correct.
>
> What actually shipped, and every place reality disagreed with this document,
> is recorded in `docs/2026-08-11-windows-port.md` and in
> `BKM/PC-Queue/TASK-M6-aloud-windows-prototype-result.md` in the Second Brain
> repo. **Read this for the reasoning; read those for the outcome.**

---
VERDICT: NEEDS_FIXES

### BLOCKING DEFECTS
- §8 teardown ordering frees the pixels it is about to return. The sequence given is `pump(); teardown(state); Ok(state.result)` — but `Selection.bgra` is cropped out of the frozen DIB, and teardown does `DeleteObject(hbm)` on exactly that DIB, and it also reads `state.result` after `state` was consumed. As written this is either a use-after-free or does not compile. Correct order: pump → take the state out of the thread_local → clip + crop into an owned `Vec<u8>` → THEN destroy every GDI object and window → return.
- STATE/window-creation ordering is unspecified, and the obvious reading deadlocks the pump. `wnd_proc` and `paint()` look the slot up by HWND in the thread_local, but §8 says "build state (frozen desktop, dim source, per-monitor back buffers, windows)" as one step. The thread_local must be populated (with `slots: Vec::new()`) BEFORE the first `CreateWindowExW`, and each slot pushed as its window is created — otherwise the first WM_PAINT after `ShowWindow` finds `STATE == None` or a slot that is not registered yet.
- `paint()` must call BeginPaint/EndPaint on EVERY path, including the "no state" and "unknown hwnd" early-returns. WM_PAINT is level-triggered: if the update region is never validated, Windows re-posts WM_PAINT immediately and forever — 100% CPU on the overlay thread, a black overlay, and the drag never completes. The spec's `WM_PAINT => { paint(hwnd); LRESULT(0) }` has no such guarantee, and defect 2 above is precisely the condition that triggers it.
- WM_CAPTURECHANGED is not handled, so a capture stolen mid-drag strands the overlay. A UAC prompt, an Alt-Tab, or any other process calling SetCapture silently takes the mouse capture away; WM_LBUTTONUP then never reaches our window, `drag` stays `Some(..)` forever, and the user is left staring at a frozen full-screen snapshot with no way out except Escape/right-click. Add `WM_CAPTURECHANGED => clear drag, set Outcome::Cancelled, PostQuitMessage(0)`.
- `.stack_size(256 * 1024)` on the overlay thread is an unrequested change that converts a recoverable error into a process abort. A Win32 pump re-enters wnd_proc through DWM, IME and shell hooks; 256 KB is well under the 1 MB Windows default and far under Rust's 2 MB. A guard-page hit on Windows aborts the process — it does NOT surface as the `Err` that `handle.join()` is relied on to produce. Delete the `stack_size` line; keep `.name("aloud-region-overlay")`.

### API PATH ERRORS

### CORRECTIONS
## Verdict

**NEEDS_FIXES** — but the fixes are all in the *logic*, not the bindings.

**Zero windows-crate API path errors.** I checked every single symbol the spec names against
`%USERPROFILE%\.cargo\registry\src\index.crates.io-1949cf8c6b5b557f\windows-0.61.3\` — every
module path, every signature, every constant, and every cited line number is correct, including
the four counter-intuitive ones (`ClientToScreen` and `InvalidateRect` in `Graphics::Gdi`;
`biCompression: u32` vs `BI_RGB: BI_COMPRESSION`; `AC_SRC_OVER: u32` vs `BLENDFUNCTION::BlendOp: u8`;
no `From<HGDIOBJ> for HBRUSH`). All three feature names are real and correctly gate what is claimed.
The three uncertainties I could settle from disk are settled below. Type the API calls as written.

The five blocking defects are ordering/lifetime bugs in the composition. Everything below is the
delta — apply it on top of the spec under review, which is otherwise sound and can be used as-is.

---

## 1. Verification results (evidence on disk)

### 1.1 API paths — all confirmed, nothing to change

| Symbol | Path / line verified | Signature notes |
|---|---|---|
| `GetMessageW` | `UI/WindowsAndMessaging/mod.rs:997` | `(*mut MSG, Option<HWND>, u32, u32) -> windows_core::BOOL` |
| `TranslateMessage` / `DispatchMessageW` | `:2402` / `:575` | `*const MSG` |
| `DefWindowProcW` / `PostQuitMessage` | `:475` / `:1862` | |
| `CreateWindowExW` | `:430` | returns `Result<HWND>`; `hwndparent/hmenu/hinstance/lpparam` all `Option<_>` |
| `RegisterClassExW` | `:1915` | `-> u16`, `#[cfg(feature = "Win32_Graphics_Gdi")]` |
| `WNDCLASSEXW` | `:7183` | Gdi-gated; field list matches the spec's literal exactly |
| `WNDPROC` | `:7249` | `Option<unsafe extern "system" fn(HWND,u32,WPARAM,LPARAM)->LRESULT>` |
| `LoadCursorW` / `IDC_CROSS` | `:1415` / `:4146` | `-> Result<HCURSOR>` |
| `ShowWindow` / `SetWindowPos` / `SetForegroundWindow` | `:2342` / `:2279` / `:2142` | |
| `DestroyWindow` / `GetCursorPos` / `GetWindowRect` | `:534` / `:825` / `:1159` | all `Result<..>` |
| `GetSystemMetrics` | `:1086` | takes `SYSTEM_METRICS_INDEX` |
| `SM_{X,Y,CX,CY}VIRTUALSCREEN` | `:6077,:6078,:6013,:6045` | |
| `HWND_TOPMOST`/`SW_SHOW`/`SWP_NOACTIVATE`/`SWP_SHOWWINDOW` | `:4063`/`:6421`/`:6397`/`:6406` | |
| `WS_POPUP` / `WS_EX_TOPMOST` / `WS_EX_TOOLWINDOW` | `:7299` / `:7287` / `:7286` | `BitOr` impls exist for all flag newtypes |
| `WS_EX_LAYERED/NOREDIRECTIONBITMAP/TRANSPARENT/NOACTIVATE` | `:7270/:7279/:7288/:7276` | (rejected, correctly) |
| every `WM_*` cited | `:6951,:7000,:7001,:7028,:6994,:7121,:7097,:7061,:6934,:6929` | exact |
| `MSG` | `:5096` | derives `Default` — `MSG::default()` is fine |
| `EnumDisplayMonitors` / `MONITORENUMPROC` | `Graphics/Gdi/mod.rs:559` / `:5832` | callback sig in the spec matches byte-for-byte |
| `GetMonitorInfoW` / `MONITORINFO` | `:1102` / `:5835` | derives `Default`; `rcMonitor` correct |
| `GetDC`/`ReleaseDC`/`CreateCompatibleDC`/`CreateDIBSection` | `:919`/`:1659`/`:207`/`:242` | `HDC::is_invalid()` exists (`:5338`) |
| `BitBlt` / `SRCCOPY` / `CAPTUREBLT` | `:79` / `:6639` / `:2433` | `hdcsrc: Option<HDC>`, returns `Result<()>` |
| `AlphaBlend` / `BLENDFUNCTION` / `AC_SRC_OVER` | `:44` / `:2412` / `:2175` | `hdcsrc` is bare `HDC` (**not** `Option`) — pass `dim_dc`, not `Some(dim_dc)` |
| `CreatePen`/`PS_SOLID`/`GetStockObject`/`NULL_BRUSH`/`Rectangle` | `:383`/`:6398`/`:1187`/`:5947`/`:1649` | |
| `BeginPaint`/`EndPaint`/`PAINTSTRUCT` | `:69`/`:533`/`:6061` | `PAINTSTRUCT` has a zeroed `Default` |
| `ClientToScreen` / `InvalidateRect` | `:120` / `:1393` | **in Gdi**, confirmed absent from WindowsAndMessaging |
| `SelectObject`/`DeleteObject`/`DeleteDC`/`GdiFlush`/`CreateDCW` | `:1756`/`:463`/`:448`/`:759`/`:222` | `From<HBITMAP|HPEN|HBRUSH> for HGDIOBJ` at `:5300/:5525/:5330`; no reverse impl |
| `BITMAPINFO`/`BITMAPINFOHEADER`/`BI_RGB`/`DIB_RGB_COLORS` | `:2319`/`:2330`/`:2402`/`:3007` | `biCompression: u32` confirmed → `BI_RGB.0` is required |
| `SetThreadDpiAwarenessContext`/`Get…`/`AreDpiAwarenessContextsEqual` | `UI/HiDpi/mod.rs:142`/`:78`/`:8` | ungated within the feature |
| `DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2` / `MDT_EFFECTIVE_DPI` | `:252` / `:268` | |
| `GetDpiForMonitor` | `:39` | `#[cfg(feature = "Win32_Graphics_Gdi")]`, links `api-ms-win-shcore-scaling-l1-1-1.dll` |
| `SetCapture`/`ReleaseCapture`/`SetFocus`/`VK_ESCAPE` | `KeyboardAndMouse:177`/`:161`/`:187`/`:889` | |
| `GetLastError`/`ERROR_CLASS_ALREADY_EXISTS`/`From<HMODULE> for HINSTANCE` | `Foundation:27`/`:1374`/`:5548` | |
| `GetModuleHandleW` | `System/LibraryLoader/mod.rs:224` | `Param<PCWSTR>`; `None` infers to `Option<&PCWSTR>` — the only applicable impl |
| `AttachThreadInput`/`GetCurrentThreadId` | `System/Threading/mod.rs:24`/`:709` | |
| `BitmapEncoder::{SetPixelData,FlushAsync,PngEncoderId,CreateAsync}` | `Graphics/Imaging/mod.rs:454/:475/:494/:525` | `CreateAsync` gated `Storage_Streams` |

`windows_core::BOOL` is `#[must_use]`, `pub struct BOOL(pub i32)`, `as_bool() == (self.0 != 0)`
(`windows-result-0.3.4/src/bool.rs:7`). **The GetMessageW `-1` trap the spec names is real** —
keep the explicit `r.0 == -1` check ahead of the `r.0 == 0` check.

### 1.2 Cargo features — all three valid, none gate anything they shouldn't

`windows-0.61.3/Cargo.toml`: `Win32_System_LibraryLoader` **:632**, `Win32_System_Threading` **:671**,
`Storage_Streams` **:308**. Add only `Win32_System_LibraryLoader`. Leave `Win32_System_Threading`
commented out (§4.5 fallback) and skip `Storage_Streams` entirely under Option A.

### 1.3 Hard constraints — no violations

- Zero runtime system deps: gdi32/user32/msimg32/shcore are inbox and imported via
  `windows_link::link!`, which emits `kind = "raw-dylib", modifiers = "+verbatim"`
  (`windows-link-0.2.1/src/lib.rs`) — **no import library, no build-script work, nothing to ship.**
- No accessibility permission, no Tesseract, no version bumps. Clean.
- Seam: every `#[cfg]` stays at `src/capture/mod.rs`'s `pub mod windows;`. Nothing leaks upward.
- Aloud constraint 4 (one window per monitor) is preserved by the snapshot-first design.

**One reporting obligation:** the brief's route (a) is worded "raw Win32 **layered** windows", and
this design deliberately drops `WS_EX_LAYERED`. §6.3.0 says *"if you deviate, say which and why in
the result."* The §0-R1 reasoning is correct and sufficient — copy it verbatim into `-result.md`.

### 1.4 Settled uncertainties

- **`png 0.18.1` really is in the Windows graph — CONFIRMED, no cargo run needed.**
  `Cargo.lock:2907` is png 0.18.1, referenced by `image 0.25.10` (`:1849`), `muda 0.19.3` (`:2303`)
  and `tray-icon 0.24.2` (`:4681`). `tauri-2.11.5/Cargo.toml:98` is `image-png = ["image/png"]` and
  `[dependencies.image]` at `:178` is `optional = true` with **no `?` in the feature**, so enabling
  `image-png` (which `aloud/Cargo.toml:11` does) pulls `image` in. muda and tray-icon are Windows
  crates and are enabled by tauri's `tray-icon` feature. Declaring `png = "0.18"` adds an edge, not
  a package. (Note there is also a png **0.17.16** in the lock, from `ico` and `tauri-codegen` —
  harmless, it already coexists.) `png-0.18.1` API confirmed exactly as cited: `Encoder::new`
  `src/encoder.rs:165`, `set_color` `:291`, `set_depth` `:296`, `write_header` `:282`,
  `write_image_data` `:742`, `finish` `:1104`; `ColorType::Rgb` / `BitDepth::Eight`
  `src/common.rs:12,98`, re-exported at the root via `pub use crate::common::*` (`src/lib.rs:86`).
  Still run `cargo tree -i png` afterwards for the record, but do not gate the design on it.
- **`windows::core::w!` resolves — CONFIRMED.** `windows-0.61.3/src/lib.rs` has
  `pub use windows_core as core;`; `windows-core-0.61.2/src/windows.rs` has
  `pub use windows_strings::*;`; `w!` is `#[macro_export]` at `windows-strings-0.4.2/src/literals.rs:11`
  and its `$crate::utf16_len` / `$crate::decode_utf8_char` helpers are `pub` (`:146`, `:69`).
  Drop the static-`[u16; N]` fallback.
- **`SetForegroundWindow` across the thread hop — very likely fine, still log it.** The
  foreground-lock conditions are documented **per process**, not per thread ("the process … is the
  foreground process / is processing input / received the last input event"), so raising the overlay
  from a worker thread of the same process that just took the hotkey should be granted. Keep the
  logged BOOL. See §4 below for a better fallback than `AttachThreadInput`.
- **AlphaBlend 8×8 stretch** and **`GetDC(None)` on the secondary monitor** genuinely cannot be
  settled without running. Keep both fallbacks as written.

---

## 2. Blocking fixes

### 2.1 Fix the run/teardown sequence (defects 1, 2, 3)

Replace §8's sketch with this exact shape. It is the only ordering that is both memory-safe and
paint-safe.

```rust
pub(super) fn run() -> anyhow::Result<Option<Selection>> {
    let prev = unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    let out = run_inner();
    // Restoring a NULL context is a no-op error; skip it so it does not muddy GetLastError.
    if !prev.0.is_null() { unsafe { SetThreadDpiAwarenessContext(prev); } }
    out
}

fn run_inner() -> anyhow::Result<Option<Selection>> {
    // 1. Geometry + the frozen desktop. No window exists yet.
    let virt = virtual_bounds()?;                 // GetSystemMetrics, bails on <= 0
    let monitors = enumerate_monitors()?;         // EnumDisplayMonitors + GetMonitorInfoW
    let frozen = FrozenDesktop::capture(virt)?;   // GetDC(None) -> CreateDIBSection -> BitBlt -> GdiFlush
    let dim = DimSource::new()?;                  // 8x8 black DIB

    // 2. Install the state BEFORE any HWND exists, with an EMPTY slot list.
    //    Every window message from here on finds a live state.
    STATE.with(|s| *s.borrow_mut() = Some(OverlayState {
        virt, frozen, dim, slots: Vec::new(), drag: None, result: Outcome::Cancelled,
    }));

    // 3. Create windows one at a time; push each slot the moment it exists.
    //    Any failure here jumps to the teardown at the bottom via `res`.
    let res = (|| -> anyhow::Result<()> {
        ensure_class(hinst)?;
        for m in &monitors {
            let (back_dc, back_bmp, back_old) = make_back_buffer(m)?;
            let hwnd = create_overlay_window(hinst, m)?;
            with_state(|s| s.slots.push(Slot { hwnd, rc: m.rc, dpi: m.dpi, back_dc, back_bmp, back_old }));
        }
        show_and_focus();      // ShowWindow + SetWindowPos + SetForegroundWindow + SetFocus
        pump()                 // GetMessageW loop; returns Err only on GetMessageW == -1
    })();

    // 4. Take the state OUT of the thread_local. From here nothing re-enters wnd_proc,
    //    so there is no borrow question left, and the thread_local is already clean.
    let mut st = STATE.with(|s| s.borrow_mut().take())
        .ok_or_else(|| anyhow::anyhow!("overlay state vanished"))?;

    // 5. Crop FIRST — st.frozen is still alive at this point. This is the step the
    //    original ordering destroyed.
    let selection = match st.result {
        Outcome::Committed(r) => clip(r, st.virt).map(|c| Selection {
            origin_x: c.left,
            origin_y: c.top,
            width:  c.right - c.left,
            height: c.bottom - c.top,
            bgra: crop_bgra(st.frozen.bits, st.frozen.stride, st.virt.left, st.virt.top, c),
        }),
        Outcome::Cancelled => None,
    };

    // 6. NOW release every GDI object and window.
    teardown(&mut st);

    res?;                 // a pump failure outranks a committed rect
    Ok(selection)
}
```

Two invariants that fall out and must be written as comments in the code:

- **`STATE` is `Some` for the entire lifetime of every HWND.** Installed before the first
  `CreateWindowExW`, taken only after `pump()` has returned (i.e. after `WM_QUIT`, i.e. after the
  last message any of these windows will ever process). Nothing in `wnd_proc` needs to handle a
  missing state as a *normal* case — but `paint()` still must not livelock if it happens (below).
- **`teardown` runs on every path**, including the `?` inside the closure, because the closure's
  `Err` is captured in `res` and only re-raised after teardown.

`teardown(&mut st)` order, per slot then globally:

1. `SelectObject(slot.back_dc, slot.back_old)` — restore before deleting.
2. `DeleteObject(slot.back_bmp.into())`, `DeleteDC(slot.back_dc)`.
3. `DestroyWindow(slot.hwnd)` — **not** under any `RefCell` borrow (it sends `WM_DESTROY`); here
   the state is already a plain local, so there is no borrow to hold.
4. `SelectObject(frozen.dc, frozen.old)`, `DeleteObject(frozen.bmp.into())`, `DeleteDC(frozen.dc)`.
5. Same three for `dim`.
6. `ReleaseDC(None, frozen.screen_dc)` — **never** `DeleteDC` a handle from `GetDC`.

Do not `UnregisterClassW` (correct in the original).

### 2.2 `paint()` never returns without validating

```rust
unsafe fn paint(hwnd: HWND) {
    let mut ps = PAINTSTRUCT::default();
    let hdc = BeginPaint(hwnd, &mut ps);
    // Everything below is best-effort. BeginPaint/EndPaint bracket it unconditionally:
    // if the update region is not validated, Windows re-posts WM_PAINT immediately and
    // forever — 100% CPU on this thread, a black overlay, and no way to finish the drag.
    if !hdc.is_invalid() {
        let _ = compose_and_present(hwnd, hdc, &ps);   // no-ops on unknown hwnd / missing state
    }
    let _ = EndPaint(hwnd, &ps);
}
```

### 2.3 Handle `WM_CAPTURECHANGED`

```rust
// Someone else took the mouse capture (a UAC prompt, an Alt-Tab, another app's
// SetCapture). WM_LBUTTONUP will never arrive at this window, so a drag left in
// place here strands the user in front of a frozen full-screen snapshot.
WM_CAPTURECHANGED => {
    with_state(|s| { s.drag = None; s.result = Outcome::Cancelled; });
    PostQuitMessage(0);
    LRESULT(0)
}
```

`WM_CAPTURECHANGED = 533` — `UI/WindowsAndMessaging/mod.rs:6902`. Do **not** call `ReleaseCapture()`
in this handler: capture is already gone, and `ReleaseCapture` sends `WM_CAPTURECHANGED`, so you
would re-enter this arm.

Note this also fixes the spec's re-entrancy list, which correctly names `ReleaseCapture` as a
sender but never says what the resulting message does.

### 2.4 Thread spawn

```rust
let handle = std::thread::Builder::new()
    .name("aloud-region-overlay".into())
    // NO stack_size. A Win32 pump re-enters wnd_proc through DWM/IME/shell hooks;
    // 256 KB is under the 1 MB Windows default, and a guard-page hit on Windows
    // ABORTS the process — it does not become the Err that handle.join() relies on.
    .spawn(overlay::run)
    .context("failed to spawn the region overlay thread")?;
```

---

## 3. Non-blocking corrections

**3.1 Use `::windows::…` in both new files.** `src/capture/windows.rs` and
`src/capture/windows/overlay.rs` are modules literally named `windows`. Uniform-path resolution only
becomes ambiguous when the *current* module owns an item of that name — neither does, so plain
`use windows::…` compiles today. But one future `pub use` or a moved item turns it into E0659 in the
most confusing possible file. Write `use ::windows::Win32::…` in both; it costs two characters and
removes the class.

**3.2 Do not read `GetLastError()` twice around a class-registration failure.**

```rust
let atom = unsafe { RegisterClassExW(&wc) };
if atom == 0 {
    let e = unsafe { GetLastError() };
    if e != ERROR_CLASS_ALREADY_EXISTS {
        // Build the error from the code already read. Calling Error::from_win32()
        // here re-reads the thread's last-error, and `crate::log_line!` does file
        // I/O (WriteFile), which clobbers it — so a log line inserted between the
        // two would silently turn this into a bogus error.
        anyhow::bail!("RegisterClassExW for the overlay class failed (WIN32_ERROR {})", e.0);
    }
}
```

(If you want a real `windows::core::Error`: `Error::from_hresult(HRESULT::from_win32(e.0))` —
`windows-result-0.3.4/src/error.rs:110`, `hresult.rs:117`.)

**3.3 Nail down `selection_clipped_to_client`'s space.** The spec uses `cl.left` both as a
destination in the back buffer *and* as `rc.left + cl.left` for the frozen source, which only works
if it is **client-relative**. State it in the signature:

```rust
/// Intersect the selection (virtual-desktop px) with this monitor, then translate
/// into CLIENT coordinates of that monitor's overlay. Returns None when the
/// selection does not touch this monitor. WS_POPUP has no non-client area, so
/// client origin == window origin == rc.left/rc.top — this is a pure subtraction.
fn selection_clipped_to_client(sel: RECT, rc: RECT) -> Option<RECT>;
```

**3.4 The band never contaminates the crop — say why, in a comment.** `Rectangle()` strokes centred
on the path, so half the pen width falls inside the selection. That is harmless *only* because the
crop is taken from `frozen`, which is never drawn on. Anyone who later "optimises" this into a
second `BitBlt` off the screen DC would bake the white band into the OCR input. One comment on
`crop_bgra` prevents that.

**3.5 Do the PNG buffer arithmetic in `usize`.** `(sel.width * sel.height * 3) as usize` multiplies
`i32`s before widening. Write `sel.width as usize * sel.height as usize * 3`. Same for the
`crop_bgra` allocation. Costs nothing, removes an overflow class on a wide virtual desktop.

**3.6 Soften the flicker claim.** With `hbrBackground = HBRUSH(null)`, `DefWindowProcW` already
returns 0 for `WM_ERASEBKGND` without painting — the null brush alone suppresses the erase. Keep
the `WM_ERASEBKGND => LRESULT(1)` handler (it also covers an external `RedrawWindow(..., RDW_ERASE)`),
but the load-bearing half is the null brush, not both independently. The `InvalidateRect(.., false)`
point stands as written.

**3.7 `invalidate_all()` / `slot_under_cursor()` — spell them out.** Both are referenced and never
defined, and both are borrow-discipline traps:

```rust
fn invalidate_all() {
    // Collect under the borrow, call outside it. InvalidateRect only marks the
    // update region (WM_PAINT is generated later by GetMessage), so it is safe
    // under a borrow — but keeping ONE rule for every Win32 call is what makes
    // the discipline checkable. Never UpdateWindow/RedrawWindow(RDW_UPDATENOW)
    // here: those send WM_PAINT synchronously and would re-enter.
    let hwnds: Vec<HWND> = with_state(|s| s.slots.iter().map(|w| w.hwnd).collect())
        .unwrap_or_default();
    for h in hwnds { unsafe { let _ = InvalidateRect(Some(h), None, false); } }
}

fn slot_under_cursor() -> Option<HWND> {
    let mut pt = POINT::default();
    if unsafe { GetCursorPos(&mut pt) }.is_err() { return None; }
    with_state(|s| s.slots.iter()
        .find(|w| pt.x >= w.rc.left && pt.x < w.rc.right && pt.y >= w.rc.top && pt.y < w.rc.bottom)
        .map(|w| w.hwnd)).flatten()
}
```

**3.8 Tests live in `overlay.rs`, not `windows.rs`.** The spec's reason for hoisting the pure helpers
("so the test module can reach them without any Win32 type in scope") is confused — `RECT` and
`POINT` *are* Win32 types and the helpers take them. The whole file only compiles on Windows anyway.
Put `#[cfg(test)] mod tests` inside `overlay.rs` and keep the helpers private. The eight tests listed
are all worth writing, especially #3 (the anti-double-scale guard). Run with
`cargo test --release` (Aloud constraint 10).

---

## 4. A better Escape fallback than `AttachThreadInput`

Keep the §4.5 ladder, but replace step 3. If the logged `SetForegroundWindow` BOOL comes back false,
the robust fix is a **scoped hotkey**, not input-queue attachment — it does not depend on focus at all:

```rust
// After the windows exist, before the pump. Escape becomes a thread-scoped grab
// for exactly as long as the overlay is up.
const ESC_ID: i32 = 0xA10D;
let esc_hotkey_ok = unsafe {
    RegisterHotKey(Some(focus_hwnd), ESC_ID, HOT_KEY_MODIFIERS(0), VK_ESCAPE.0 as u32)
}.is_ok();
// ... and in teardown, unconditionally paired:
if esc_hotkey_ok { unsafe { let _ = UnregisterHotKey(Some(focus_hwnd), ESC_ID); } }
```

Then add to `wnd_proc`:

```rust
WM_HOTKEY if wparam.0 as i32 == ESC_ID => {
    with_state(|s| { s.drag = None; s.result = Outcome::Cancelled; });
    let _ = ReleaseCapture();
    PostQuitMessage(0);
    LRESULT(0)
}
```

Verified: `RegisterHotKey(Option<HWND>, i32, HOT_KEY_MODIFIERS, u32) -> Result<()>` at
`UI/Input/KeyboardAndMouse/mod.rs:156`; `UnregisterHotKey` `:233`; `HOT_KEY_MODIFIERS(pub u32)`
`:344`; `WM_HOTKEY = 786` at `UI/WindowsAndMessaging/mod.rs:6970`. All under the already-declared
`Win32_UI_Input_KeyboardAndMouse` — **no new feature, unlike `AttachThreadInput`.** Because `hwnd`
is `Some(..)`, `WM_HOTKEY` is posted to that window and reaches `wnd_proc` normally (with `None` it
would be a *thread* message that `DispatchMessageW` never routes to a WndProc — you would have to
handle it in the pump). Registration failure is non-fatal: log it and fall through to the existing
`WM_KEYDOWN` path.

Both of these stay behind the "only if step 1 shows the grant failing" gate. Do not add either
pre-emptively.

---

## 5. Two things to carry into `-result.md`

**5.1 The import table changes, and question 34 should say so.** `windows_link` uses `raw-dylib`, so
calling `AlphaBlend` adds **`msimg32.dll`** and calling `GetDpiForMonitor` adds
**`api-ms-win-shcore-scaling-l1-1-1.dll`** to what `dumpbin /dependents` reports. Both are inbox on
Windows 10/11 and neither is a runtime dependency in the constraint-1 sense — but §10 q34 lists an
expected import set, and these two will look like a contradiction if you do not name them.
(If you would rather not import shcore at all: `GetDpiForWindow(hwnd)` — `UI/HiDpi/mod.rs:49`,
user32 — gives the same number for the same purpose, since the DPI is cosmetic here. The brief names
`GetDpiForMonitor`, so keep it unless you have a reason; just report the import.)

**5.2 Hand the small/large-region problem to the OCR unit explicitly.** The spec's `>= 1` commit
threshold is right — a click-with-no-drag is a cancel, matching macOS `screencapture -i`. But the
brief (§6.3) says *"Windows OCR returns **nothing** on images that are too small"* and *"clamp to
`OcrEngine.MaxImageDimension`"*, and this machine reports `MaxImageDimension = 10000`. A capture
unit that commits a 3×2 px rect, or a rect wider than 10000 px across a three-monitor desktop, is
behaving correctly and the OCR unit is where both get handled (upscale ~1.5× guarded as PowerToys
does; downscale past 10000). **Do not clamp in the capture unit** — that would silently change what
the user selected. Do log the committed `width×height` on every capture, so question 22's answer
falls out of the log rather than needing a separate experiment.

---

## 6. Cargo.toml delta (unchanged from the spec, re-verified)

```toml
[target.'cfg(windows)'.dependencies]
# PNG encoding for the captured region. Already in Cargo.lock (0.18.1) and already
# compiled on Windows via image<-tauri "image-png", muda and tray-icon; declaring
# it makes it nameable from this crate and adds no new package.
png = "0.18"

windows = { version = "0.61", features = [
    "Foundation", "Graphics", "Graphics_Imaging", "Media", "Media_Ocr", "Globalization",
    "Win32_Foundation", "Win32_Graphics_Gdi", "Win32_System_Com", "Win32_System_WinRT",
    "Win32_UI_WindowsAndMessaging", "Win32_UI_HiDpi", "Win32_UI_Input_KeyboardAndMouse",
    # --- added for §6.3.0's overlay ---
    "Win32_System_LibraryLoader",   # GetModuleHandleW -> HINSTANCE for the window class
    # "Win32_System_Threading",     # ONLY if the AttachThreadInput fallback is needed (§4.5)
] }
```

Version stays 0.61. Everything else in the spec under review — the snapshot-first design, the
route-(a) decision, the coordinate invariant table, the "zero scale-factor multiplications" rule,
the `Ok(None)` vs `Err` table, the GDI-handle discipline, the `i16` lParam extraction, the top-down
`biHeight`, the `GdiFlush` before reading bits, the BGRX/alpha trap, the temp-file ownership rule —
is correct as written and should be implemented unchanged.