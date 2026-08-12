> **Moved into this repo 2026-08-12, from a loose folder on the PC's build disk
> (`X:\dev\aloud-m6-design`) that was not under version control and had no copy
> anywhere.** The M6 Windows implementation leaned on these four documents
> heavily; losing that disk would have lost the entire design rationale behind
> `src/capture/windows/overlay.rs` and `src/ocr/windows.rs`.
>
> **Status: PRE-IMPLEMENTATION. Written 2026-08-11, before any of this ran on
> Windows.** Kept as written rather than corrected, per the `docs/` rule that
> superseded findings are annotated and not rewritten. Every windows-crate API path in it was correct, down to the line numbers. Its AlphaBlend dimming design was the one thing that failed on hardware.
>
> What actually shipped, and every place reality disagreed with this document,
> is recorded in `docs/2026-08-11-windows-port.md` and in
> `BKM/PC-Queue/TASK-M6-aloud-windows-prototype-result.md` in the Second Brain
> repo. **Read this for the reasoning; read those for the outcome.**

---
# Unit 3 of 4 — interactive region-selection overlay + BitBlt capture (brief §6.3.0 and the capture half of §6.3)

## SUMMARY
Take the brief's recommended route (a): raw Win32, one borderless always-on-top window per monitor, on a dedicated thread with its own GetMessage pump, so `select(&self) -> Result<Option<PathBuf>>` stays a plain blocking call. Two deliberate refinements inside that route: (1) **snapshot-first** — BitBlt the whole virtual desktop into one top-down 32bpp DIB *before* any window exists, then paint that frozen image (dimmed, with a bright band) into each per-monitor window and crop the final rect straight out of the CPU buffer. That removes WS_EX_LAYERED entirely, removes the "overlay appears in its own screenshot" race, and removes the post-DestroyWindow repaint race, at the cost of a frozen-screen look (which is exactly Snipping Tool's behaviour). (2) **Zero scale factors in the geometry** — with PerMonitorV2 in force from the §6.2 manifest, GetSystemMetrics/GetMonitorInfoW/CreateWindowExW/mouse lParam/ClientToScreen/BitBlt are *all* already in physical virtual-desktop pixels; `GetDpiForMonitor` is called and logged but its result must touch nothing except the band's stroke width. PNG encoding is the one real cost: nothing in the current dependency set is *nameable* for PNG encoding, so either declare `png = "0.18"` (already in Cargo.lock, already compiled on Windows, zero new crates) or take the WinRT `BitmapEncoder` route which costs a new `Storage_Streams` feature plus COM init instead.

## FEATURES NEEDED
["Win32_System_LibraryLoader \u2014 GetModuleHandleW, for the HINSTANCE passed to RegisterClassExW/CreateWindowExW (verified as a valid feature name at windows-0.61.3/Cargo.toml:632). AVOIDABLE if you pass a null HINSTANCE / None instead; a null-instance window class is legal. Recommended to add.", "Win32_System_Threading \u2014 CONDITIONAL, only if SetForegroundWindow is refused and the AttachThreadInput fallback in \u00a74.5 is needed (AttachThreadInput at Win32/System/Threading/mod.rs:24, GetCurrentThreadId at :709). Valid feature name, windows-0.61.3/Cargo.toml:671. Do not add pre-emptively.", "Storage_Streams \u2014 ONLY if PNG encoding takes the WinRT BitmapEncoder route (Option B) instead of the `png` crate. BitmapEncoder::CreateAsync is #[cfg(feature = \"Storage_Streams\")] at Windows/Graphics/Imaging/mod.rs:525. Valid feature name, windows-0.61.3/Cargo.toml:308. Not needed for the recommended Option A."]

## TRAPS
- THE double-scale defect. With PerMonitorV2 in force, GetSystemMetrics(SM_*VIRTUALSCREEN), MONITORINFO.rcMonitor, CreateWindowExW x/y/w/h, mouse lParam, ClientToScreen output and BitBlt screen-DC coordinates are ALL already physical virtual-desktop pixels. The correct number of dpi/96 multiplications anywhere in the capture geometry is ZERO. GetDpiForMonitor is called and logged but must feed only the band's stroke width. Any scaling of a rect is the exact bug questions 20/21 test for.
- GET_X_LPARAM has no windows-rs equivalent and the naive extraction is wrong on multi-monitor. Mouse lParam fields are SIGNED 16-bit: you must write `(lparam.0 & 0xFFFF) as u16 as i16 as i32`. Skipping the i16 hop turns a legitimate client x of -1920 (a monitor left of primary, under SetCapture) into 63616.
- BitBlt writes BGRX, and the X byte is NOT a valid alpha — it comes back as 0. Encoding that as png::ColorType::Rgba produces a fully transparent PNG and the OCR silently sees nothing. Emit ColorType::Rgb and swizzle B<->R by hand, or force alpha to 255.
- GetMessageW returns BOOL(-1) on error. windows_core::BOOL::as_bool() is `self.0 != 0`, so -1 reads as `true` and the pump spins forever. You must check `r.0 == -1` explicitly, before the `r.0 == 0` (WM_QUIT) check.
- GetMessageW must be called with hwnd = None. With Some(hwnd) you (a) receive only that one window's messages, dropping the other monitors' input, and (b) never receive WM_QUIT, because WM_QUIT is a thread message — documented, and it hangs the overlay thread forever.
- Capturing the screen AFTER hiding the overlay is a race: the desktop underneath is not guaranteed to have been recomposited yet, so your own dim rectangle intermittently lands in the screenshot. The snapshot-first design (BitBlt the whole desktop before any window exists, crop from that buffer) removes the race by construction — there is no second BitBlt at all.
- RefCell re-entrancy in the WndProc. ReleaseCapture sends WM_CAPTURECHANGED, DestroyWindow sends WM_DESTROY, SetWindowPos sends WM_WINDOWPOSCHANGING — each re-enters wnd_proc synchronously. Holding a borrow_mut() across any of them panics with BorrowMutError. Rule: compute under the borrow, drop it, then call Win32.
- WS_EX_NOREDIRECTIONBITMAP kills a GDI-painted window — it suppresses the DWM redirection surface that WM_PAINT presents through, so the window renders nothing. It is for DirectComposition/DXGI swapchain windows only. Do not add it.
- WS_EX_TRANSPARENT makes the window click-through and the overlay then receives no mouse input at all. It is a frequent copy-paste from click-through-HUD samples. Do not add it.
- Flicker needs BOTH halves: hbrBackground = HBRUSH(null) in WNDCLASSEXW AND a WM_ERASEBKGND handler returning LRESULT(1). Either one alone still flashes. And InvalidateRect's third argument (bErase) must be false — passing true re-introduces the erase you just suppressed.
- GdiFlush() must be called after the BitBlt and before reading the DIB-section bits from the CPU. Without it the GDI batch may not have been executed and you memcpy stale/garbage rows.
- biHeight must be NEGATIVE in the BITMAPINFOHEADER to get a top-down DIB. With a positive height the buffer is bottom-up and every crop comes out vertically mirrored, at a row offset that looks like an off-by-N coordinate bug.
- Type-name gotchas in windows 0.61: ClientToScreen and InvalidateRect live in Win32::Graphics::Gdi, not Win32::UI::WindowsAndMessaging. AC_SRC_OVER is u32 but BLENDFUNCTION::BlendOp is u8 (cast). BI_RGB is BI_COMPRESSION but BITMAPINFOHEADER::biCompression is a bare u32 (use BI_RGB.0). GetStockObject returns HGDIOBJ and there is no From<HGDIOBJ> for HBRUSH (wrap manually: HBRUSH(h.0)); the From impls only go HBITMAP/HPEN/HBRUSH -> HGDIOBJ. windows_core::BOOL is #[must_use], so ignored returns need `let _ =`.
- A click with no drag must be Ok(None), not Err and not a 0x0 BitBlt. macOS `screencapture -i` treats it as a cancel and the two arms should agree. Same for a rect that clips to empty against the virtual-desktop bounds.
- If the PNG write fails, the partially-written screenshot must be deleted before returning Err. The trait makes the CALLER the owner of the path in Ok(Some(path)) — on the Err path there is no path handed over, so nobody else can clean it up, and a photograph of the user's screen is left in %TMP%.
- RegisterClassExW returning 0 with GetLastError() == ERROR_CLASS_ALREADY_EXISTS (1410) is success, not failure — the overlay is created once per hotkey press but the class survives for the process lifetime. Treating 0 as fatal breaks every capture after the first.
- Use MONITORINFO.rcMonitor, never rcWork. rcWork excludes the taskbar and the overlay would leave an unselectable strip.
- GDI handle discipline: SelectObject the ORIGINAL handle back before DeleteObject/DeleteDC, and never DeleteDC a handle obtained from GetDC (use ReleaseDC). A leak here is invisible until the process runs out of GDI objects after N captures.

## UNCERTAINTIES
- SetForegroundWindow on the overlay may be refused by the foreground lock, which would mean WM_KEYDOWN(VK_ESCAPE) never arrives. The overlay is raised from a RegisterHotKey handler in the same process (which normally grants foreground rights), but the hotkey is handled on tao's thread and ours is a different thread — I could not verify empirically that the grant survives the hop. MITIGATED three ways: log SetForegroundWindow's BOOL, right-click also cancels, and the AttachThreadInput fallback in §4.5 is specified. SETTLE IT: press the hotkey with an unrelated app focused, drag, press Escape, and check the logged BOOL and whether the overlay closed.
- AlphaBlend stretching an 8x8 source across a whole monitor: I specified 8x8 rather than 1x1 because a 1x1 source is a degenerate case I have seen reported as producing artifacts, but I could not verify either size on this machine (no cargo runs allowed). SETTLE IT: run it once; if the dim is patchy or absent, switch to the precomputed-dim-buffer fallback described in §3.4 (one extra full-desktop DIB, one SRCCOPY per paint, no msimg32 at all).
- Whether a WM_DPICHANGED can arrive during CreateWindowExW on a mixed-DPI setup and resize the overlay before it is shown. I specified two independent guards (ignore WM_DPICHANGED by returning LRESULT(0); reassert exact geometry with SetWindowPos after ShowWindow) so it should not matter either way, but I did not verify that Windows never rescales a WS_POPUP at creation time in a PMv2 process. SETTLE IT: after ShowWindow, GetWindowRect each overlay and assert it equals rcMonitor — log any mismatch. That single log line answers questions 20 and 21 before the user even drags.
- That `png 0.18.1` really is in the Windows build graph and that declaring `png = "0.18"` adds no new package. I read this off Cargo.lock (png 0.18.1 at :2907, referenced by image :1849, muda :2303, tray-icon :4681) and off tauri-2.11.5/Cargo.toml (image-png = ["image/png"], :98; optional image = "0.25", :178). I could NOT run cargo tree — a release build holds the target-dir lock. SETTLE IT: after that build finishes, `cargo tree -p png` and `cargo tree -i png --target x86_64-pc-windows-msvc`. If it resolves a second png, back it out and take the WinRT BitmapEncoder route (Option B, §6.3) instead.
- Whether GetDC(None) covers every monitor on this specific two-monitor setup. It is the documented and universally used idiom (NULL hWnd = entire screen, addressed via SM_X/CXVIRTUALSCREEN) but I have seen reports of a secondary adapter coming back black on some driver stacks. SETTLE IT: capture a region on the non-primary monitor. If it is black, swap GetDC(None) for CreateDCW(w!("DISPLAY"), None, None, None) (Graphics/Gdi/mod.rs:222) — nothing else changes.
- Whether `windows::core::w!` resolves. It is #[macro_export]ed by windows-strings-0.4.2 (src/literals.rs:11) and glob-re-exported by windows-core (src/windows.rs: `pub use windows_strings::*;`), which should make `windows::core::w!` valid — but macro re-export through a glob is exactly the kind of thing that surprises. SETTLE IT: if it fails to resolve, use a static wide literal instead: `static CLASS: [u16; 20] = [...0]; PCWSTR(CLASS.as_ptr())`.
- The per-paint cost of composing a full 4K monitor (three blits over ~8.3 Mpx) at mouse-move rate. I estimated 2-4 ms from the operation count, not from measurement. SETTLE IT: if the band lags the cursor, invalidate only union(old_sel, new_sel) inflated by border_px+1 and clip the composition to ps.rcPaint — both are one-line changes from the shape given in §3.4.
- I did NOT verify anything on the OCR side beyond reading src/ocr/windows.rs's doc comment (which says it will decode the PNG to Bgra8 via BitmapDecoder). If unit 4 concludes it would rather receive raw pixels than a file, that is a trait-level conversation, not a change to this unit — RegionSelector::select returns a PathBuf and OcrEngine::recognise takes a &Path, and neither should change for the prototype.

## SPEC
# Unit 3 — Region-selection overlay + BitBlt capture (Windows)

Everything below was checked against `%USERPROFILE%\.cargo\registry\src\index.crates.io-1949cf8c6b5b557f\windows-0.61.3\src\...` on this machine. Line numbers cited are in that source. Nothing here is from memory of windows-rs.

---

## 0. Route decision

Take route **(a)** — raw Win32, one window per monitor, dedicated thread, own `GetMessage` pump. No deviation from the brief's recommendation.

Two refinements *inside* route (a), both flagged because they change what the code looks like:

**R1 — snapshot-first, and therefore NOT `WS_EX_LAYERED`.** Capture the whole virtual desktop with one `BitBlt` **before** creating any window. Each overlay window then paints *that frozen image*, dimmed, with the selection rect drawn un-dimmed. The final crop comes out of the same CPU buffer — there is no second `BitBlt` at all.

Why this beats a layered window:
- A transparent/layered overlay means the real desktop is underneath, so the capture `BitBlt` has to happen *after* the overlay is gone. Between `DestroyWindow`/`ShowWindow(SW_HIDE)` and the `BitBlt` the desktop under it has not necessarily been recomposited yet — you get your own dim rectangle in the screenshot, intermittently, on slower machines. Every workaround (Sleep, pump-until-idle, DwmFlush) is a guess.
- `WS_EX_LAYERED` + `SetLayeredWindowAttributes(..., LWA_ALPHA)` applies **one uniform alpha to the whole window**, so you cannot have a dimmed surround *and* a bright selection interior. The only layered way to do that is `UpdateLayeredWindow` with a premultiplied per-pixel-alpha 32bpp DIB — which also means no `WM_PAINT` at all, a different presentation model, and strictly more code.
- Snapshot-first makes the overlay window a plain **opaque** `WS_POPUP`. `WM_PAINT` works normally, double-buffering works normally.

Cost: the screen appears frozen while selecting (video stops). That is Snipping Tool's behaviour and is intended, not a bug — say so in the result. Memory cost is one virtual-desktop-sized 32bpp buffer (a 3840×2160 + 2560×1440 side-by-side layout has a ~6400×2160 bounding box ⇒ ~55 MB), freed as soon as the crop is taken.

**R2 — file split.** `src/capture/windows.rs` keeps `ScreenCapture`, its existing doc comments, `select()`, the temp path and the PNG write. All the Win32 machinery goes in a private sibling module `src/capture/windows/overlay.rs`, declared as `mod overlay;` from `windows.rs` (legal in edition 2021: `foo.rs` + `foo/bar.rs`). Roughly 180 lines + 450 lines. If the Mac prefers one file, inline it — nothing depends on the split.

---

## 1. Thread + message-pump architecture

`RegionSelector::select` is already called off the main thread (`spawn_read_region` in `src/bin/aloud.rs:197` does `std::thread::spawn`), and `App::read_region` (`src/app/mod.rs:260`) already guards against a second concurrent call. So `select()` may block freely, and only one overlay can ever be up at a time.

Windows requires a window's messages be pumped **on the thread that created it**. So: create the overlays on a fresh thread, pump there, join.

Use `JoinHandle::join()` rather than a channel — it is the shorter path and it turns a panic into an `Err` for free.

```rust
// src/capture/windows.rs
use super::RegionSelector;
use anyhow::{anyhow, Context, Result};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

mod overlay;

impl RegionSelector for ScreenCapture {
    fn select(&self) -> Result<Option<PathBuf>> {
        crate::log_line!("capture: starting region overlay thread");

        let handle = std::thread::Builder::new()
            .name("aloud-region-overlay".into())
            .stack_size(256 * 1024)
            .spawn(overlay::run)
            .context("failed to spawn the region overlay thread")?;

        // Blocks until the overlay thread's GetMessage loop exits.
        // Ok(Ok(Some(sel)))  -> the user committed a rectangle
        // Ok(Ok(None))       -> the user cancelled (Escape / right-click / zero-area drag)
        // Ok(Err(e))         -> a genuine Win32 failure inside the overlay
        // Err(_)             -> the overlay thread panicked
        let selection = match handle.join() {
            Ok(inner) => inner?,
            Err(_) => return Err(anyhow!("the region overlay thread panicked")),
        };

        let Some(sel) = selection else {
            crate::log_line!("capture: overlay cancelled by the user");
            return Ok(None); // <-- the ONLY place Ok(None) is produced
        };

        crate::log_line!(
            "capture: committed rect {}x{} at ({},{}) in virtual-desktop pixels",
            sel.width, sel.height, sel.origin_x, sel.origin_y
        );

        let path = unique_capture_path();
        if let Err(e) = write_png_rgb(&path, &sel) {
            // A screenshot must never be left half-written in %TMP%. The trait
            // makes the *caller* the owner of the path in Ok(Some(path)); on the
            // Err path there is no path to hand over, so cleanup is ours.
            let _ = std::fs::remove_file(&path);
            return Err(e).context("failed to write the captured region as PNG");
        }
        Ok(Some(path))
    }
}
```

`overlay::run` has signature:

```rust
// src/capture/windows/overlay.rs
pub(super) fn run() -> anyhow::Result<Option<Selection>>;

pub(super) struct Selection {
    pub origin_x: i32,      // virtual-desktop physical px (may be negative)
    pub origin_y: i32,
    pub width: i32,         // > 0
    pub height: i32,        // > 0
    pub bgra: Vec<u8>,      // top-down, stride = width * 4, alpha byte is garbage
}
```

`Selection` is `Send` (plain ints + `Vec<u8>`), `anyhow::Error` is `Send + Sync + 'static`, so the closure's return type crosses the join boundary fine. **No `HWND`/`HDC`/`HBITMAP` ever leaves the overlay thread** — those are `*mut c_void` newtypes and are `!Send`; keeping them thread-local is what makes the seam honest.

### The pump

```rust
let mut msg = MSG::default();
loop {
    let r = unsafe { GetMessageW(&mut msg, None, 0, 0) };
    if r.0 == -1 {
        return Err(windows::core::Error::from_win32()).context("GetMessageW failed");
    }
    if r.0 == 0 {
        break; // WM_QUIT
    }
    unsafe {
        let _ = TranslateMessage(&msg);
        DispatchMessageW(&msg);
    }
}
```

`GetMessageW(lpmsg, hwnd: Option<HWND>, ..)` — `windows-0.61.3/src/Windows/Win32/UI/WindowsAndMessaging/mod.rs:997`. Pass `None` for `hwnd`, never `Some(hwnd)`: with a non-NULL hWnd you (a) only receive messages for that one window, silently dropping the other monitors', and (b) never receive `WM_QUIT`, because `WM_QUIT` is a *thread* message — the loop would hang forever.

Exit is `PostQuitMessage(0)` from the WndProc, on commit or on cancel. Never `PostQuitMessage` with a meaningful exit code — the outcome travels in the shared state, not in the quit code.

### Error propagation

`overlay::run` is structured as create → `run_inner` → teardown, so teardown runs on both the success and the failure path:

```rust
pub(super) fn run() -> anyhow::Result<Option<Selection>> {
    // Belt-and-braces over the §6.2 manifest: guarantees THIS thread reasons in
    // physical pixels even if the manifest were ever dropped. Always succeeds.
    let prev = unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    let ctx_ok = unsafe {
        AreDpiAwarenessContextsEqual(
            GetThreadDpiAwarenessContext(),
            DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        )
    }.as_bool();
    crate::log_line!("overlay: thread dpi context is PerMonitorV2 = {ctx_ok}");

    let result = run_inner();

    unsafe { SetThreadDpiAwarenessContext(prev) };
    result
}
```

`SetThreadDpiAwarenessContext` / `GetThreadDpiAwarenessContext` / `AreDpiAwarenessContextsEqual` / `DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2` are all in `windows::Win32::UI::HiDpi` (`UI/HiDpi/mod.rs:142`, `:78`, `:8`, `:252`) under the already-declared `Win32_UI_HiDpi` feature. Do **not** call `SetProcessDpiAwarenessContext` — it returns `ERROR_ACCESS_DENIED` once the manifest has set the mode, and by hotkey time Tauri already owns HWNDs.

---

## 2. Monitor enumeration

```rust
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HMONITOR, MONITORINFO,
};
use windows::Win32::Foundation::{LPARAM, RECT};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
};

pub(super) struct MonitorGeom {
    pub rc: RECT,   // virtual-desktop PHYSICAL pixels; left/top may be negative
    pub dpi: u32,   // cosmetic ONLY — see §4
}

unsafe extern "system" fn enum_monitor(
    hmon: HMONITOR,
    _hdc: windows::Win32::Graphics::Gdi::HDC,
    _clip: *mut RECT,
    data: LPARAM,
) -> windows::core::BOOL {
    let out = &mut *(data.0 as *mut Vec<MonitorGeom>);

    let mut mi = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if !GetMonitorInfoW(hmon, &mut mi).as_bool() {
        return windows::core::BOOL(1); // skip this one, keep enumerating
    }

    let mut dpi_x = 96u32;
    let mut dpi_y = 96u32;
    // Failure is non-fatal: 96 is the correct fallback and the value is cosmetic.
    let _ = GetDpiForMonitor(hmon, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y);

    out.push(MonitorGeom { rc: mi.rcMonitor, dpi: dpi_x });
    windows::core::BOOL(1) // TRUE = continue enumeration
}

fn enumerate_monitors() -> anyhow::Result<Vec<MonitorGeom>> {
    let mut list: Vec<MonitorGeom> = Vec::new();
    let ok = unsafe {
        EnumDisplayMonitors(
            None,
            None,
            Some(enum_monitor),
            LPARAM(&mut list as *mut Vec<MonitorGeom> as isize),
        )
    };
    if !ok.as_bool() || list.is_empty() {
        anyhow::bail!("EnumDisplayMonitors returned no usable monitors");
    }
    for (i, m) in list.iter().enumerate() {
        crate::log_line!(
            "overlay: monitor {i}: rc=({},{})-({},{}) dpi={} scale={:.2}",
            m.rc.left, m.rc.top, m.rc.right, m.rc.bottom, m.dpi,
            m.dpi as f32 / 96.0
        );
    }
    Ok(list)
}
```

Verified signatures:
- `EnumDisplayMonitors(hdc: Option<HDC>, lprcclip: Option<*const RECT>, lpfnenum: MONITORENUMPROC, dwdata: LPARAM) -> BOOL` — `Graphics/Gdi/mod.rs:559`
- `MONITORENUMPROC = Option<unsafe extern "system" fn(HMONITOR, HDC, *mut RECT, LPARAM) -> windows_core::BOOL>` — `Graphics/Gdi/mod.rs:5832`
- `GetMonitorInfoW(hmonitor: HMONITOR, lpmi: *mut MONITORINFO) -> BOOL` — `Graphics/Gdi/mod.rs:1102`
- `MONITORINFO { cbSize: u32, rcMonitor: RECT, rcWork: RECT, dwFlags: u32 }` — `Graphics/Gdi/mod.rs:5835`. Use `rcMonitor`, **not** `rcWork` (rcWork excludes the taskbar; the overlay must cover it).
- `GetDpiForMonitor(hmonitor, dpitype: MONITOR_DPI_TYPE, dpix: *mut u32, dpiy: *mut u32) -> Result<()>` — `UI/HiDpi/mod.rs:39`. Note it is `#[cfg(feature = "Win32_Graphics_Gdi")]` because it takes a Gdi `HMONITOR` — both features are already declared. It links `api-ms-win-shcore-scaling-l1-1-1.dll` (Win 8.1+, fine).
- `MDT_EFFECTIVE_DPI` — `UI/HiDpi/mod.rs:268`.

Virtual desktop bounds:

```rust
let vx = unsafe { GetSystemMetrics(SM_XVIRTUALSCREEN) };   // may be negative
let vy = unsafe { GetSystemMetrics(SM_YVIRTUALSCREEN) };   // may be negative
let vw = unsafe { GetSystemMetrics(SM_CXVIRTUALSCREEN) };
let vh = unsafe { GetSystemMetrics(SM_CYVIRTUALSCREEN) };
if vw <= 0 || vh <= 0 {
    anyhow::bail!("GetSystemMetrics reported an empty virtual desktop ({vw}x{vh})");
}
```

`SM_XVIRTUALSCREEN=76`, `SM_YVIRTUALSCREEN=77`, `SM_CXVIRTUALSCREEN=78`, `SM_CYVIRTUALSCREEN=79` — `UI/WindowsAndMessaging/mod.rs:6077,6078,6013,6045`.

**Negative coordinates are handled by never assuming an origin of (0,0).** Everything is stored as absolute virtual-desktop coordinates; the only place a subtraction happens is when indexing the frozen buffer, where the offset is `(x - vx, y - vy)`. Write that helper once:

```rust
#[inline]
fn to_frozen(x: i32, y: i32, vx: i32, vy: i32) -> (i32, i32) { (x - vx, y - vy) }
```

---

## 3. Window class, CreateWindowExW styles, and the drawing approach

### 3.1 Class registration (idempotent, no `Once`)

```rust
use windows::core::w;
use windows::Win32::Foundation::{ERROR_CLASS_ALREADY_EXISTS, GetLastError, HINSTANCE, HWND};
use windows::Win32::Graphics::Gdi::HBRUSH;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CS_HREDRAW, CS_VREDRAW, IDC_CROSS, LoadCursorW, RegisterClassExW, WNDCLASSEXW,
};

const CLASS_NAME: windows::core::PCWSTR = w!("AloudRegionOverlay");

fn ensure_class(hinst: HINSTANCE) -> anyhow::Result<()> {
    let cursor = unsafe { LoadCursorW(None, IDC_CROSS) }
        .context("LoadCursorW(IDC_CROSS) failed")?;

    let wc = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(wnd_proc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: hinst,
        hIcon: Default::default(),
        hCursor: cursor,
        // NULL background brush: Windows must never erase our client area.
        // This is half of the anti-flicker story (the other half is WM_ERASEBKGND).
        hbrBackground: HBRUSH(std::ptr::null_mut()),
        lpszMenuName: windows::core::PCWSTR::null(),
        lpszClassName: CLASS_NAME,
        hIconSm: Default::default(),
    };

    let atom = unsafe { RegisterClassExW(&wc) };
    if atom == 0 {
        let e = unsafe { GetLastError() };
        // The overlay is created once per hotkey press; the class survives the
        // first registration for the life of the process. Treat "already there"
        // as success rather than keeping a Once/static around.
        if e != ERROR_CLASS_ALREADY_EXISTS {
            return Err(windows::core::Error::from_win32())
                .context("RegisterClassExW for the overlay class failed");
        }
    }
    Ok(())
}
```

- `RegisterClassExW(*const WNDCLASSEXW) -> u16` — `UI/WindowsAndMessaging/mod.rs:1915`, gated `#[cfg(feature = "Win32_Graphics_Gdi")]`.
- `WNDCLASSEXW` — `:7183`, also gated on `Win32_Graphics_Gdi` (its `hbrBackground` is a Gdi `HBRUSH`). Both features already declared.
- `WNDPROC = Option<unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT>` — `:7249`.
- `LoadCursorW(hinstance: Option<HINSTANCE>, lpcursorname: P1) -> Result<HCURSOR>` — `:1415`. `IDC_CROSS: PCWSTR` — `:4146`.
- `ERROR_CLASS_ALREADY_EXISTS: WIN32_ERROR(1410)` — `Win32/Foundation/mod.rs:1374`; `GetLastError() -> WIN32_ERROR` — `:27`.

`hinst` comes from `GetModuleHandleW(None)?` → `HMODULE`, converted with the existing `impl From<HMODULE> for HINSTANCE` (`Win32/Foundation/mod.rs:5548`). That needs the **new** feature `Win32_System_LibraryLoader`. If you would rather not add it, `hInstance: HINSTANCE(std::ptr::null_mut())` and `hinstance: None` on `CreateWindowExW` both work — a null-instance class is legal. Adding the feature is the conventional choice and costs one line; either is defensible.

### 3.2 CreateWindowExW

```rust
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};

let hwnd = unsafe {
    CreateWindowExW(
        WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
        CLASS_NAME,
        w!(""),
        WS_POPUP,                       // no WS_VISIBLE: show explicitly, after all are built
        m.rc.left,                      // physical virtual-desktop px, may be negative
        m.rc.top,
        m.rc.right - m.rc.left,
        m.rc.bottom - m.rc.top,
        None,                           // hwndparent
        None,                           // hmenu
        Some(hinst),
        None,                           // lpparam — state lives in a thread_local, see §4
    )
}.context("CreateWindowExW for the overlay failed")?;
```

`CreateWindowExW(dwexstyle: WINDOW_EX_STYLE, lpclassname: P1, lpwindowname: P2, dwstyle: WINDOW_STYLE, x, y, nwidth, nheight, hwndparent: Option<HWND>, hmenu: Option<HMENU>, hinstance: Option<HINSTANCE>, lpparam: Option<*const c_void>) -> Result<HWND>` — `:430`. It already returns `Result`, so `?` + `.context(..)` is the whole error path.

Style ruling, one line each:

| Style | Use it? | Why |
|---|---|---|
| `WS_POPUP` (`:7299`) | **Yes** | No caption, no border, no non-client area ⇒ client origin == window origin == `rcMonitor` origin. |
| `WS_EX_TOPMOST` (`:7287`) | **Yes** | Always-on-top, per the brief. |
| `WS_EX_TOOLWINDOW` (`:7286`) | **Yes** | Keeps a transient overlay out of the taskbar and out of Alt-Tab. |
| `WS_EX_LAYERED` (`:7270`) | **No** | Not needed with the frozen-snapshot design (§0 R1). Uniform alpha cannot give a dimmed surround with a bright interior; the per-pixel alternative is `UpdateLayeredWindow`, a different presentation model. |
| `WS_EX_NOREDIRECTIONBITMAP` (`:7279`) | **No — actively harmful** | It suppresses the DWM redirection surface, which is exactly what a GDI `WM_PAINT` window presents through. The window would render nothing. It exists for DirectComposition/DXGI-swapchain windows. |
| `WS_EX_TRANSPARENT` (`:7288`) | **No — actively harmful** | Makes the window click-through; the overlay would receive no mouse input at all. It is a common copy-paste from click-through-HUD samples. |
| `WS_EX_NOACTIVATE` (`:7276`) | **No** | We need keyboard focus for Escape (§4.5). |

### 3.3 Show, reassert geometry, take focus

```rust
use windows::Win32::UI::WindowsAndMessaging::{
    HWND_TOPMOST, SW_SHOW, SWP_NOACTIVATE, SWP_SHOWWINDOW, SetForegroundWindow, SetWindowPos,
    ShowWindow, GetCursorPos,
};
use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;

for w in &slots {
    unsafe {
        let _ = ShowWindow(w.hwnd, SW_SHOW);            // BOOL is #[must_use]
        // Reassert exact geometry and z-order after the window has landed on
        // its monitor, so a stray WM_DPICHANGED during creation cannot leave a
        // scaled/offset overlay. Cheap, and it makes the invariant checkable.
        let _ = SetWindowPos(
            w.hwnd, Some(HWND_TOPMOST),
            w.rc.left, w.rc.top, w.rc.right - w.rc.left, w.rc.bottom - w.rc.top,
            SWP_NOACTIVATE | SWP_SHOWWINDOW,
        );
    }
}
// Focus the overlay under the cursor so WM_KEYDOWN(VK_ESCAPE) has a destination.
let focus = slot_under_cursor(&slots).unwrap_or(slots[0].hwnd);
unsafe {
    let fg = SetForegroundWindow(focus).as_bool();
    let _  = SetFocus(Some(focus));
    crate::log_line!("overlay: SetForegroundWindow={fg}");
}
```

- `ShowWindow(hwnd, ncmdshow: SHOW_WINDOW_CMD) -> BOOL` — `:2342`; `SW_SHOW` — `:6421`.
- `SetWindowPos(hwnd, hwndinsertafter: Option<HWND>, x, y, cx, cy, uflags) -> Result<()>` — `:2279`; `HWND_TOPMOST` — `:4063`; `SWP_NOACTIVATE` — `:6397`; `SWP_SHOWWINDOW` — `:6406`.
- `SetForegroundWindow(hwnd) -> BOOL` — `:2142`.
- `SetFocus(hwnd: Option<HWND>) -> Result<HWND>` — `UI/Input/KeyboardAndMouse/mod.rs:187`.
- `GetCursorPos(*mut POINT) -> Result<()>` — `:825`.

### 3.4 The drawing approach, concretely, and its flicker behaviour

**Three GDI surfaces:**

1. `frozen` — one top-down 32bpp DIB section, `vw × vh`, holding the whole virtual desktop, selected into `frozen_dc`. Created once per `select()`.
2. `back[i]` — one top-down 32bpp DIB section per monitor, monitor-sized, selected into `back_dc[i]`. The double buffer.
3. `dim` — one tiny (8×8) 32bpp DIB filled with black, selected into `dim_dc`. Stretched by `AlphaBlend` to dim arbitrary regions.

**Composition, done inside `WM_PAINT`:**

```rust
// 1. frozen slice for this monitor -> back buffer
BitBlt(back_dc, 0, 0, mw, mh, Some(frozen_dc), rc.left - vx, rc.top - vy, SRCCOPY)?;

// 2. dim the whole back buffer
let blend = BLENDFUNCTION {
    BlendOp: AC_SRC_OVER as u8,   // AC_SRC_OVER is u32, BlendOp is u8 -> cast
    BlendFlags: 0,
    SourceConstantAlpha: 120,     // ~47% black
    AlphaFormat: 0,               // no per-pixel alpha in the source
};
let _ = AlphaBlend(back_dc, 0, 0, mw, mh, dim_dc, 0, 0, 8, 8, blend);

// 3. re-blit the frozen pixels *inside* the selection, un-dimmed
if let Some(cl) = selection_clipped_to_client(sel, rc) {
    BitBlt(back_dc, cl.left, cl.top, cl.right - cl.left, cl.bottom - cl.top,
           Some(frozen_dc), (rc.left + cl.left) - vx, (rc.top + cl.top) - vy, SRCCOPY)?;

    // 4. the band outline
    let pen = CreatePen(PS_SOLID, border_px(dpi), COLORREF(0x00FF_FFFF)); // BGR: white
    let old_pen = SelectObject(back_dc, pen.into());
    let old_brush = SelectObject(back_dc, GetStockObject(NULL_BRUSH));
    let _ = Rectangle(back_dc, cl.left, cl.top, cl.right, cl.bottom);
    SelectObject(back_dc, old_pen);
    SelectObject(back_dc, old_brush);
    let _ = DeleteObject(pen.into());
}
```

**Presentation, in the same `WM_PAINT`:**

```rust
let mut ps = PAINTSTRUCT::default();
let hdc = BeginPaint(hwnd, &mut ps);
// ... composition above, clipped to ps.rcPaint if you optimise ...
BitBlt(hdc, ps.rcPaint.left, ps.rcPaint.top,
       ps.rcPaint.right - ps.rcPaint.left, ps.rcPaint.bottom - ps.rcPaint.top,
       Some(back_dc), ps.rcPaint.left, ps.rcPaint.top, SRCCOPY)?;
let _ = EndPaint(hwnd, &ps);
```

**Flicker behaviour — zero, and by construction, not by luck.** Three things together:
1. `hbrBackground = HBRUSH(null)` in the class ⇒ Windows never paints a background.
2. `WM_ERASEBKGND` returns `LRESULT(1)` ⇒ `DefWindowProcW` never erases either. (Without both of these you get a black or white flash on every mouse move.)
3. The visible surface is written exactly once per paint, by a single `BitBlt` out of a fully-composed back buffer. No intermediate state is ever presented.

Additionally `InvalidateRect(hwnd, Some(&dirty), false)` — the `false` is `bErase`; passing `true` re-introduces the erase you just suppressed.

**Repaint cost.** Composing a whole 4K monitor is three memory-to-memory blits over ~8.3 Mpx — order 2–4 ms. At mouse-move rate that is noticeable but not bad. Start simple (compose the whole client area) and, if it drags, invalidate only `union(old_sel, new_sel)` inflated by `border_px + 1` and clip the composition to `ps.rcPaint`. Both are one-line changes from the shape above.

**Fallback if `AlphaBlend` misbehaves:** precompute a *second* full-desktop DIB at capture time with every channel scaled (e.g. `× 0.5`) on the CPU, and make step 2 a plain `SRCCOPY` from it. One extra ~55 MB buffer and one ~15 ms CPU pass at start; removes msimg32, `BLENDFUNCTION` and the `u8` cast entirely. Keep it in your back pocket.

Verified: `AlphaBlend(hdcdest, xd, yd, wd, hd, hdcsrc: HDC, xs, ys, ws, hs, ftn: BLENDFUNCTION) -> BOOL` — `Graphics/Gdi/mod.rs:44`, links `msimg32.dll` (windows-link handles the import, no build-script work). `BLENDFUNCTION { BlendOp: u8, BlendFlags: u8, SourceConstantAlpha: u8, AlphaFormat: u8 }` — `:2412`. `AC_SRC_OVER: u32 = 0` — `:2175`. `CreatePen(istyle: PEN_STYLE, cwidth: i32, color: COLORREF) -> HPEN` — `:383`; `PS_SOLID` — `:6398`. `GetStockObject(GET_STOCK_OBJECT_FLAGS) -> HGDIOBJ` — `:1187`; `NULL_BRUSH` — `:5947`. `Rectangle(hdc, l, t, r, b) -> BOOL` — `:1649`. `BeginPaint(hwnd, *mut PAINTSTRUCT) -> HDC` — `:69`; `EndPaint(hwnd, *const PAINTSTRUCT) -> BOOL` — `:533`; `PAINTSTRUCT` — `:6061`. `InvalidateRect(hwnd: Option<HWND>, lprect: Option<*const RECT>, berase: bool) -> BOOL` — `:1393` (note: in **Gdi**, not WindowsAndMessaging).

---

## 4. WndProc, mouse capture, and the one coordinate space

### 4.1 State: a thread-local, not `GWLP_USERDATA`

All overlay windows live on one thread, and the drag state is **shared across monitors** (a drag can start on monitor A and end on monitor B). `GWLP_USERDATA` gives per-window state, which is the wrong shape here and would need a second shared allocation anyway. Use one thread-local:

```rust
thread_local! {
    static STATE: std::cell::RefCell<Option<OverlayState>> =
        const { std::cell::RefCell::new(None) };
}

struct OverlayState {
    virt: RECT,                 // vx, vy, vx+vw, vy+vh
    frozen: FrozenDesktop,      // hdc + hbitmap + *mut u8 bits + stride
    dim: DimSource,             // 8x8 black DIB + its dc
    slots: Vec<Slot>,           // one per monitor
    drag: Option<Drag>,         // anchor + current, BOTH in virtual-desktop px
    result: Outcome,            // Cancelled (default) | Committed(RECT)
}

struct Slot { hwnd: HWND, rc: RECT, dpi: u32, back_dc: HDC, back_bmp: HBITMAP, back_old: HGDIOBJ }
struct Drag { anchor: POINT, current: POINT }   // virtual-desktop px
enum Outcome { Cancelled, Committed(RECT) }
```

`Outcome::Cancelled` is the **default**, so every abnormal exit from the pump (window destroyed by something else, WM_CLOSE, etc.) degrades to a cancel rather than to a bogus rectangle.

**Re-entrancy discipline — this is a real panic source.** Some Win32 calls *send* messages synchronously and re-enter `wnd_proc`; if you are holding a `RefCell` borrow at that moment you get `BorrowMutError` and the overlay thread panics (which `select()` correctly turns into `Err`, but you have then lost the capture). Rule:

> Compute under the borrow. Drop the borrow. **Then** call Win32.

Safe to call while borrowed: `BitBlt`, `AlphaBlend`, `SelectObject`, `Rectangle`, `BeginPaint`/`EndPaint`, `ClientToScreen`, `InvalidateRect` (posts), `PostQuitMessage` (posts).
Must be called **after** dropping the borrow: `DestroyWindow` (sends `WM_DESTROY`), `ReleaseCapture` (sends `WM_CAPTURECHANGED`), `SetWindowPos` (sends `WM_WINDOWPOSCHANGING`), `SetForegroundWindow`, `SetFocus`.

A `with_state(|s| …) -> Option<R>` helper that takes a closure and returns a plain value makes it structurally hard to hold a borrow across a Win32 call.

### 4.2 The message handlers

```rust
unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_ERASEBKGND => LRESULT(1),        // "I erased it" — the anti-flicker half

        WM_LBUTTONDOWN => {
            let pt = lparam_to_virtual(hwnd, lparam);      // §4.3
            with_state(|s| { s.drag = Some(Drag { anchor: pt, current: pt }); });
            SetCapture(hwnd);                               // no borrow held
            invalidate_all();
            LRESULT(0)
        }

        WM_MOUSEMOVE => {
            let dragging = with_state(|s| s.drag.is_some()).unwrap_or(false);
            if dragging {
                let pt = lparam_to_virtual(hwnd, lparam);
                with_state(|s| { if let Some(d) = s.drag.as_mut() { d.current = pt; } });
                invalidate_all();
            }
            LRESULT(0)
        }

        WM_LBUTTONUP => {
            let pt = lparam_to_virtual(hwnd, lparam);
            let rect = with_state(|s| {
                let d = s.drag.take()?;
                let r = normalise_rect(d.anchor, pt);       // pure fn, §4.4
                (r.right - r.left >= 1 && r.bottom - r.top >= 1).then_some(r)
            }).flatten();

            match rect {
                Some(r) => { with_state(|s| s.result = Outcome::Committed(r)); }
                // A click with no drag is a cancel-shaped gesture (macOS
                // `screencapture -i` behaves the same). Ok(None), not Err,
                // and never a 0x0 BitBlt.
                None => { with_state(|s| s.result = Outcome::Cancelled); }
            }
            let _ = ReleaseCapture();     // borrow already dropped
            PostQuitMessage(0);
            LRESULT(0)
        }

        WM_KEYDOWN | WM_SYSKEYDOWN if wparam.0 as u16 == VK_ESCAPE.0 => {
            with_state(|s| { s.drag = None; s.result = Outcome::Cancelled; });
            let _ = ReleaseCapture();
            PostQuitMessage(0);
            LRESULT(0)
        }

        // Secondary cancel: right-click. Costs nothing and is the escape hatch
        // if SetForegroundWindow was refused and keyboard input never arrives.
        WM_RBUTTONDOWN => {
            with_state(|s| { s.drag = None; s.result = Outcome::Cancelled; });
            let _ = ReleaseCapture();
            PostQuitMessage(0);
            LRESULT(0)
        }

        WM_PAINT => { paint(hwnd); LRESULT(0) }

        // Ignore the OS's suggested rect: our geometry is authoritative and is
        // reasserted by SetWindowPos. Honouring it would resize the overlay
        // mid-drag and desynchronise the back buffer.
        WM_DPICHANGED => LRESULT(0),

        WM_DESTROY => LRESULT(0),
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}
```

`WM_ERASEBKGND=20` `:6951`, `WM_LBUTTONDOWN=513` `:7000`, `WM_LBUTTONUP=514` `:7001`, `WM_MOUSEMOVE=512` `:7028`, `WM_KEYDOWN=256` `:6994`, `WM_SYSKEYDOWN=260` `:7121`, `WM_RBUTTONDOWN=516` `:7097`, `WM_PAINT=15` `:7061`, `WM_DPICHANGED=736` `:6934`, `WM_DESTROY=2` `:6929`. `VK_ESCAPE = VIRTUAL_KEY(27)` — `UI/Input/KeyboardAndMouse/mod.rs:889`. `SetCapture(hwnd) -> HWND` — `:177`; `ReleaseCapture() -> Result<()>` — `:161`. `DefWindowProcW` — `UI/WindowsAndMessaging/mod.rs:475`. `PostQuitMessage(i32)` — `:1862`.

**Why `SetCapture` matters here specifically:** with capture held by the window where the button went down, *all* subsequent mouse messages — including moves over a different monitor — are delivered to that one window, with client coordinates that legitimately go negative or exceed the client width. That is the mechanism that makes a cross-monitor drag work at all, and it is why the conversion must be a pure translation (`ClientToScreen`), which is well-defined outside the client rect.

### 4.3 The coordinate conversion — one space, applied once

```rust
/// Mouse lParam (client-relative, PHYSICAL px) -> virtual-desktop PHYSICAL px.
/// The ONLY conversion in the file. No DPI, no scale factor, no division.
unsafe fn lparam_to_virtual(hwnd: HWND, lparam: LPARAM) -> POINT {
    // GET_X_LPARAM / GET_Y_LPARAM are C macros; windows-rs has no equivalent.
    // The cast through i16 is mandatory — the fields are SIGNED 16-bit, and a
    // monitor left of the primary produces negative client x under SetCapture.
    // `(lparam.0 & 0xFFFF) as i32` (no i16 hop) turns -1920 into 63616.
    let mut pt = POINT {
        x: (lparam.0 & 0xFFFF) as u16 as i16 as i32,
        y: ((lparam.0 >> 16) & 0xFFFF) as u16 as i16 as i32,
    };
    let _ = ClientToScreen(hwnd, &mut pt);   // pure translation, valid outside the client rect
    pt
}
```

`ClientToScreen(hwnd, lppoint: *mut POINT) -> BOOL` lives in `windows::Win32::Graphics::Gdi` (`Graphics/Gdi/mod.rs:120`), **not** in `WindowsAndMessaging` — that surprises people.

**The scale-factor rule, stated as an invariant.** With PerMonitorV2 in force (manifest, plus `SetThreadDpiAwarenessContext` belt-and-braces):

| Quantity | Space |
|---|---|
| `GetSystemMetrics(SM_*VIRTUALSCREEN)` | physical px, virtual desktop |
| `MONITORINFO.rcMonitor` | physical px, virtual desktop |
| `CreateWindowExW` x/y/w/h | physical px, virtual desktop |
| mouse `lParam` | physical px, client-relative |
| `ClientToScreen` output | physical px, virtual desktop |
| `BitBlt` source coords on the screen DC | physical px, virtual desktop |
| PNG pixel dimensions | physical px |

**Every one of those is already the same space. The correct number of scale-factor multiplications in the whole capture path is ZERO.** `GetDpiForMonitor` is called (the brief asks for it), logged, and used for exactly one thing:

```rust
/// The ONLY consumer of the per-monitor DPI. Cosmetic. If this function ever
/// gains a caller that touches a rectangle, question 20/21 will fail.
fn border_px(dpi: u32) -> i32 {
    ((2.0 * dpi as f32 / 96.0).round() as i32).max(1)
}
```

Put that comment in the code. The defect this section exists to prevent is someone "helpfully" scaling `rcMonitor` or the committed rect by `dpi/96`, which double-applies a scale that the OS already applied — producing exactly the "smaller/offset" symptom question 20 asks about.

### 4.4 `normalise_rect` — pure, and the unit under test

```rust
/// Two drag corners (any order, either may be negative) -> a well-ordered RECT.
/// Pure: no Win32, no DPI, no allocation. This is the function that questions 20
/// and 21 actually exercise, and it can be tested without any UI.
fn normalise_rect(a: POINT, b: POINT) -> RECT {
    RECT {
        left:   a.x.min(b.x),
        top:    a.y.min(b.y),
        right:  a.x.max(b.x),
        bottom: a.y.max(b.y),
    }
}
```

### 4.5 Keyboard focus — the one thing that can go wrong here

Escape needs the overlay to have keyboard focus, and `SetForegroundWindow` obeys the foreground lock: a process may only steal foreground if, among other things, it "received the last input event" or "is processing a hotkey". Aloud's overlay is raised from a `RegisterHotKey` handler in the same process, which should qualify — but the hotkey is handled on tao's thread, not ours, and I could not verify empirically that the grant survives the hop. **Mitigations, in order:**

1. Log `SetForegroundWindow(...).as_bool()` — question 20/21's report should carry it.
2. Right-click also cancels (already in the handler above), so a user is never trapped.
3. If Escape genuinely does not arrive on this machine, attach input queues around the call — the standard fix:
   ```rust
   let fg_thread = GetWindowThreadProcessId(GetForegroundWindow(), None);
   let me = GetCurrentThreadId();
   let _ = AttachThreadInput(me, fg_thread, true);
   let _ = SetForegroundWindow(focus);
   let _ = SetFocus(Some(focus));
   let _ = AttachThreadInput(me, fg_thread, false);
   ```
   `GetForegroundWindow` — `:866`; `GetWindowThreadProcessId(hwnd, Option<*mut u32>) -> u32` — `:1184`; `AttachThreadInput(idattach, idattachto, fattach: bool) -> BOOL` — `Win32/System/Threading/mod.rs:24`; `GetCurrentThreadId() -> u32` — `Win32/System/Threading/mod.rs:709`. Both need the **new** feature `Win32_System_Threading`. Only add it if step 1 shows the grant failing.

---

## 5. Where `Ok(None)` is produced and where `Err` is produced

The trait doc (`src/capture/mod.rs:16-36`) is explicit that these must never collapse. On Windows there is **no permission gate at all**, so the disambiguation is simpler than macOS's — but the discipline is the same: cancel is a *decision made in the WndProc*, failure is an *error returned from a Win32 call*. They come from different places in the code and never meet.

| Situation | Return | Produced at |
|---|---|---|
| Escape pressed | `Ok(None)` | `WM_KEYDOWN` → `Outcome::Cancelled` |
| Right-click | `Ok(None)` | `WM_RBUTTONDOWN` → `Outcome::Cancelled` |
| Click with no drag (0-width or 0-height) | `Ok(None)` | `WM_LBUTTONUP`, rect rejected |
| Pump exited without any commit | `Ok(None)` | `Outcome::Cancelled` is the default |
| Overlay thread panicked | `Err` | `handle.join()` returned `Err` |
| `EnumDisplayMonitors` gave nothing | `Err` | `enumerate_monitors` |
| `SM_CXVIRTUALSCREEN <= 0` | `Err` | virtual-bounds check |
| `RegisterClassExW == 0` and not `ERROR_CLASS_ALREADY_EXISTS` | `Err` | `ensure_class` |
| `CreateWindowExW` / `CreateDIBSection` / `BitBlt` failed | `Err` | each returns `Result` already |
| `GetMessageW` returned `-1` | `Err` | pump |
| PNG write failed | `Err` (+ temp file removed) | `select()` |
| Captured region is all black (DRM / `WDA_EXCLUDEFROMCAPTURE`) | `Ok(Some(path))` **+ a log line** | see below |

**Black captures are not errors.** `SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)` is enforced in DWM and every capture API returns black; that is a content restriction, not a failure and not a permission problem. The brief says "say so rather than going quiet", so make it visible cheaply:

```rust
fn looks_protected(bgra: &[u8]) -> bool {
    // Every pixel exactly black, alpha ignored. Fast, no false positives worth
    // caring about (a genuinely all-black region is equally worth logging).
    bgra.chunks_exact(4).all(|p| p[0] == 0 && p[1] == 0 && p[2] == 0)
}
```
Log: `capture: the region came back entirely black — this is normal for DRM/protected windows (WDA_EXCLUDEFROMCAPTURE), not a permission problem`. Still return the path; OCR will report `Outcome::Empty` downstream, which is the honest result.

**Do not port anything from `resolve_missing_capture` / `access_result` in `src/capture/macos.rs`.** There is no Windows analogue and the stub's doc comment already says so.

---

## 6. BitBlt into a DIB, and PNG encoding

### 6.1 The frozen desktop grab

```rust
use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GdiFlush, GetDC,
    ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, SRCCOPY,
};

let screen_dc = unsafe { GetDC(None) };          // NULL hwnd = the entire (virtual) screen
if screen_dc.is_invalid() { anyhow::bail!("GetDC(NULL) returned a null screen DC"); }

let mem_dc = unsafe { CreateCompatibleDC(Some(screen_dc)) };

let bmi = BITMAPINFO {
    bmiHeader: BITMAPINFOHEADER {
        biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: vw,
        biHeight: -vh,              // NEGATIVE = top-down; row 0 is the TOP row
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB.0,    // BI_RGB is BI_COMPRESSION; the field is u32
        ..Default::default()
    },
    ..Default::default()
};

let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
let hbm = unsafe { CreateDIBSection(Some(screen_dc), &bmi, DIB_RGB_COLORS, &mut bits, None, 0) }
    .context("CreateDIBSection for the frozen desktop failed")?;

let old = unsafe { SelectObject(mem_dc, hbm.into()) };   // From<HBITMAP> for HGDIOBJ

// Source coords are virtual-desktop physical px and may be negative — that is
// the whole point of SM_X/YVIRTUALSCREEN and it needs no special handling.
unsafe { BitBlt(mem_dc, 0, 0, vw, vh, Some(screen_dc), vx, vy, SRCCOPY) }
    .context("BitBlt of the virtual desktop failed")?;

unsafe { let _ = GdiFlush(); }   // MUST flush before reading `bits` from the CPU
```

Verified: `GetDC(Option<HWND>) -> HDC` `:919`; `ReleaseDC(Option<HWND>, HDC) -> i32` `:1659`; `CreateCompatibleDC(Option<HDC>) -> HDC` `:207`; `CreateDIBSection(Option<HDC>, *const BITMAPINFO, DIB_USAGE, *mut *mut c_void, Option<HANDLE>, u32) -> Result<HBITMAP>` `:242`; `BitBlt(hdc, x, y, cx, cy, hdcsrc: Option<HDC>, x1, y1, rop: ROP_CODE) -> Result<()>` `:79` (already `.ok()`-ed — `?` works directly); `SRCCOPY: ROP_CODE(13369376)` `:6639`; `DIB_RGB_COLORS` `:3007`; `BI_RGB: BI_COMPRESSION(0)` `:2402`; `GdiFlush() -> BOOL` `:759`; `BITMAPINFOHEADER` `:2330` (`biCompression` is a bare `u32`, hence `BI_RGB.0`).

Do **not** pass `SRCCOPY | CAPTUREBLT` by default. `ROP_CODE` does implement `BitOr` (`:6564`) so it compiles, but `CAPTUREBLT` (`:2433`) causes a full-screen flash on some configurations and is unnecessary under DWM, where `GetDC(NULL)` already reads the composed desktop. Keep it as a documented fallback if a layered window (a tooltip, a shaped popup) is ever missing from a capture.

If a secondary monitor comes back black under `GetDC(None)`, the fallback source DC is `CreateDCW(w!("DISPLAY"), None, None, None)` (`:222`), which is documented to span the virtual desktop. Same everything else. `BitBlt` never includes the mouse cursor, which is what we want (no crosshair baked into the OCR input).

### 6.2 Cropping — no second BitBlt

The committed rect is in virtual-desktop coordinates; the frozen buffer's (0,0) is `(vx, vy)`; the buffer is top-down with `stride = vw * 4`. So:

```rust
fn crop_bgra(bits: *const u8, stride: usize, vx: i32, vy: i32, r: RECT) -> Vec<u8> {
    let w = (r.right - r.left) as usize;
    let h = (r.bottom - r.top) as usize;
    let mut out = vec![0u8; w * h * 4];
    for row in 0..h {
        let src_y = (r.top - vy) as usize + row;
        let src_x = (r.left - vx) as usize;
        unsafe {
            std::ptr::copy_nonoverlapping(
                bits.add(src_y * stride + src_x * 4),
                out.as_mut_ptr().add(row * w * 4),
                w * 4,
            );
        }
    }
    out
}
```

Clamp `r` to `virt` before calling this (the drag can be released with the pointer parked outside the desktop bounds on some multi-monitor layouts). One helper:

```rust
fn clip(r: RECT, bounds: RECT) -> Option<RECT> {
    let c = RECT {
        left:   r.left.max(bounds.left),
        top:    r.top.max(bounds.top),
        right:  r.right.min(bounds.right),
        bottom: r.bottom.min(bounds.bottom),
    };
    (c.right > c.left && c.bottom > c.top).then_some(c)
}
```
`None` here is a cancel (`Ok(None)`), not an error.

Drop the frozen buffer (`SelectObject(mem_dc, old); DeleteObject(hbm.into()); DeleteDC(mem_dc); ReleaseDC(None, screen_dc);`) as soon as the crop is copied out — it is a photograph of the entire desktop sitting in process memory.

### 6.3 PNG encoding — the honest answer

**Nothing currently reachable from `aloud` can encode a PNG.** I checked:
- `tauri`'s `image-png` feature (`tauri-2.11.5/Cargo.toml:98`) resolves to `image-png = ["image/png"]`, i.e. it enables `tauri`'s **own optional `image` dependency** (`tauri-2.11.5/Cargo.toml:178`, `image = "0.25", default-features = false`). It is a dependency **of tauri**, not of `aloud`, and tauri does not re-export it. `tauri::image::Image` only *decodes*.
- Rust will not let you `use image::…` or `use png::…` from `aloud` unless the crate is named in `aloud`'s own `Cargo.toml`. Being in `Cargo.lock` is not enough.

So there are exactly two options, and **both are a real cost that must be flagged, not slipped in**:

**Option A (recommended) — declare `png` directly.**
```toml
[target.'cfg(windows)'.dependencies]
png = "0.18"
windows = { version = "0.61", features = [ ... ] }
```
Facts that make this the cheap one:
- `png 0.18.1` is **already in `Cargo.lock`** (`X:\dev\aloud\Cargo.lock:2907`) and already **compiled on Windows**, pulled in three ways: `image 0.25.10` (`:1849`) ← tauri's `image-png`; `muda 0.19.3` (`:2303`); `tray-icon 0.24.2` (`:4681`). Declaring it adds **no new package entry** and **no new compilation** — just an edge, exactly like the `windows` line did.
- Target-gate it so the macOS graph is untouched.
- It brings **no COM** into the capture path. GDI is not COM, so with Option A the overlay thread needs no `RoInitialize`/apartment at all.
- Verify with `cargo tree -p png` and `cargo tree -i png` after the current release build releases the target-dir lock. If it somehow resolves a *second* `png`, back it out and take Option B.

```rust
fn write_png_rgb(path: &Path, sel: &overlay::Selection) -> Result<()> {
    let file = std::fs::File::create(path)?;
    let w = std::io::BufWriter::new(file);
    let mut enc = png::Encoder::new(w, sel.width as u32, sel.height as u32);
    enc.set_color(png::ColorType::Rgb);      // NOT Rgba — see the alpha trap below
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header()?;

    // BitBlt writes BGRX; the X byte is NOT a valid alpha. Emitting Rgba with
    // that byte produces a fully-transparent PNG and the OCR sees nothing.
    let mut rgb = Vec::with_capacity((sel.width * sel.height * 3) as usize);
    for px in sel.bgra.chunks_exact(4) {
        rgb.push(px[2]); // R
        rgb.push(px[1]); // G
        rgb.push(px[0]); // B
    }
    writer.write_image_data(&rgb)?;
    writer.finish()?;
    Ok(())
}
```
API verified against `png-0.18.1/src/encoder.rs`: `Encoder::new(w, width, height)` `:165`, `set_color` `:291`, `set_depth` `:296`, `write_header() -> Result<Writer<W>>` `:282`, `Writer::write_image_data(&[u8])` `:742`, `Writer::finish()` `:1104`. `ColorType::{Rgb, Rgba}` and `BitDepth::Eight` — `png-0.18.1/src/common.rs:12,98`, re-exported at the crate root (`src/lib.rs:86,90`).

Equivalent alternative inside Option A: declare `image = { version = "0.25", default-features = false, features = ["png"] }` instead — identical zero-new-crate property (it is literally tauri's copy), but you then build an `RgbImage` (an extra full copy) and go through `save()`. `png` is the smaller surface; either is fine.

**Option B — zero new crates, one new feature: WinRT `BitmapEncoder`.**
`Windows.Graphics.Imaging.BitmapEncoder::CreateAsync(BitmapEncoder::PngEncoderId()?, stream)` exists at `windows-0.61.3/src/Windows/Graphics/Imaging/mod.rs:525` and `:494`, `SetPixelData` at `:454`, `FlushAsync` at `:475`. But `CreateAsync` is `#[cfg(feature = "Storage_Streams")]`, so this costs the new feature **`Storage_Streams`** (valid name, `windows-0.61.3/Cargo.toml:308`), plus: an `InMemoryRandomAccessStream`, blocking `.get()` on two WinRT async operations, a `DataReader` round-trip to pull the encoded bytes back out, `std::fs::write`, and `RoInitialize(RO_INIT_MULTITHREADED)` / `RoUninitialize` on the overlay thread. Roughly 40 lines instead of 15, plus COM apartment concerns in a path that otherwise has none.

**Recommendation: Option A.** State the trade in the result file and let the Mac overrule if "no new declared dependency" is the harder rule.

---

## 7. Temp file, uniqueness, and the ownership contract

Mirror `src/capture/macos.rs:168-179` **verbatim** — same function, same comment, same tests:

```rust
fn unique_capture_path() -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!(
        "aloud-capture-{}-{nanos}-{n}.png",
        std::process::id()
    ))
}
```

On Windows `std::env::temp_dir()` resolves `%TMP%`/`%TEMP%` → typically `C:\Users\<user>\AppData\Local\Temp`. Pid + nanos + counter is enough entropy; a fixed name would race a double hotkey press and would let a stale screenshot from a previous run be read as fresh.

This is a **deliberate 12-line duplication** between `macos.rs` and `windows.rs`, not an oversight. Promoting it to `src/capture/mod.rs` would be defensible but touches the working macOS arm and its four tests; per the repo's coding conduct ("every changed line must trace directly to the request") I would leave macOS alone and say so in the result, letting the Mac decide whether to hoist it later.

**Ownership.** The trait doc (`mod.rs:29-36`) makes the *caller* the owner of deleting the returned PNG, and `actions::read_region` (`src/app/actions.rs:100`) already wraps it in `DeleteOnDrop` so it dies even on a panic. So:
- On `Ok(Some(path))` — **do not delete**. Hand it over.
- On `Ok(None)` — **no file was ever created**. The `select()` sketch in §1 only calls `unique_capture_path()` after a committed rect exists.
- On `Err` **after** the file was created (only the PNG-write path) — **delete it ourselves**, as shown in §1. There is no path to hand over, so nobody else can.

Longer than `MAX_PATH` is covered by `<longPathAware>true</longPathAware>` from §6.2's manifest.

---

## 8. Teardown and GDI-handle discipline

Every path — success, cancel, error, and the early returns inside `run_inner` — must run the same teardown. Structure it so it cannot be forgotten:

```
run_inner():
  build state (frozen desktop, dim source, per-monitor back buffers, windows)
  let pumped = pump();                 // Result<()>
  teardown(state);                     // ALWAYS, before returning
  pumped?;
  Ok(state.result -> Option<Selection>)
```

Teardown order, per surface:
1. `SelectObject(dc, old_handle)` — restore the original bitmap/pen/brush **before** deleting.
2. `DeleteObject(hbitmap.into())`.
3. `DeleteDC(dc)`.
4. `DestroyWindow(hwnd)` for each slot (outside any `RefCell` borrow — it sends `WM_DESTROY`).
5. `ReleaseDC(None, screen_dc)` for the DC obtained with `GetDC` (never `DeleteDC` a `GetDC` handle).
6. Clear the thread-local: `STATE.with(|s| *s.borrow_mut() = None)`.

`DeleteObject(HGDIOBJ) -> BOOL` `:463`; `DeleteDC(HDC) -> BOOL` `:448`; `DestroyWindow(HWND) -> Result<()>` `:534`.

Do **not** call `UnregisterClassW` — the class is process-wide and cheap to keep; unregistering while another overlay could exist is a race for no benefit.

---

## 9. Tests worth writing (all pure, no UI, no OS)

The interactive parts cannot be automated, but the parts that actually break can. Put these in a `#[cfg(test)] mod tests` in `src/capture/windows.rs`, and keep the pure helpers `pub(super)` or move them to `windows.rs` so the test module can reach them without any Win32 type in scope:

1. `normalise_rect` for all four drag directions, including a drag that starts at `(-1920, -100)` and ends at `(200, 800)` — asserts the negative-coordinate case directly.
2. `clip(rect, bounds)` returns `None` for a zero-area or fully-outside rect, and the exact intersection otherwise.
3. **The anti-double-scale test:** assert `normalise_rect` output dimensions equal `|dx|`/`|dy|` exactly, for a monitor whose `dpi` is 144 and 192. It must not consult DPI at all — the test passes trivially today and *fails loudly* the moment someone threads a scale factor through it. That is the regression guard for questions 20 and 21.
4. `border_px(96) == 2`, `border_px(144) == 3`, `border_px(192) == 4`, `border_px(0) >= 1`.
5. `crop_bgra` against a synthetic buffer with a known pattern and a negative `(vx, vy)`.
6. BGRA→RGB channel order on a 2-pixel fixture.
7. `looks_protected` — all-zero buffer true, one non-black pixel false.
8. Copy the four `unique_capture_path` tests from `src/capture/macos.rs:186-215` (they are platform-neutral).

Per Aloud constraint 10, run these with `cargo test --release`.

---

## 10. Cargo.toml delta (exact)

```toml
[target.'cfg(windows)'.dependencies]
# PNG encoding for the captured region. Already in Cargo.lock (0.18.1) and
# already compiled on Windows via image<-tauri "image-png", muda and tray-icon;
# declaring it makes it nameable from this crate and adds no new package.
png = "0.18"

windows = { version = "0.61", features = [
    "Foundation",
    "Graphics",
    "Graphics_Imaging",
    "Media",
    "Media_Ocr",
    "Globalization",
    "Win32_Foundation",
    "Win32_Graphics_Gdi",
    "Win32_System_Com",
    "Win32_System_WinRT",
    "Win32_UI_WindowsAndMessaging",
    "Win32_UI_HiDpi",
    "Win32_UI_Input_KeyboardAndMouse",
    # --- added for §6.3.0's overlay ---
    "Win32_System_LibraryLoader",   # GetModuleHandleW -> HINSTANCE for the window class
    # "Win32_System_Threading",     # ONLY if AttachThreadInput is needed (§4.5)
] }
```

`Win32_System_LibraryLoader` and `Win32_System_Threading` were both checked against `windows-0.61.3/Cargo.toml` (lines 632 and 671) and exist. The `windows` **version stays at 0.61** — nothing here needs anything newer.
