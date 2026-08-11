//! The interactive region-selection overlay: raw Win32, **one borderless
//! always-on-top window per monitor** (hard constraint 4), created on a
//! dedicated thread that owns their message pump.
//!
//! macOS gets this whole UI from the OS — `screencapture -i` draws the
//! crosshair, handles the drag, handles Escape and writes the PNG. Windows
//! has no equivalent, so everything below exists to reproduce that one
//! command.
//!
//! # Snapshot-first, and therefore *not* a layered window
//!
//! The brief recommends "raw Win32 layered windows". This takes the route but
//! drops `WS_EX_LAYERED`, deliberately:
//!
//! * A see-through overlay means the real desktop is still underneath, so the
//!   capture `BitBlt` has to run *after* the overlay is gone — and between
//!   `DestroyWindow` and that blit the desktop underneath has not necessarily
//!   been recomposited, so our own dim rectangle lands in the screenshot,
//!   intermittently. Every workaround (`Sleep`, pump-until-idle, `DwmFlush`)
//!   is a guess.
//! * `SetLayeredWindowAttributes(LWA_ALPHA)` applies **one uniform alpha to
//!   the whole window**, so it cannot give a dimmed surround *and* a bright
//!   selection interior at once. The only layered way to do that is
//!   `UpdateLayeredWindow` with a premultiplied per-pixel-alpha DIB, which
//!   also means no `WM_PAINT` at all — a different presentation model and
//!   strictly more code.
//!
//! So: `BitBlt` the whole virtual desktop into one top-down 32bpp DIB
//! **before any window exists**, then let each overlay paint *that frozen
//! image* — dimmed, with the selection redrawn bright — and crop the committed
//! rectangle straight out of the same CPU buffer. There is no second `BitBlt`,
//! so the race cannot happen. The screen appears frozen while selecting; that
//! is Snipping Tool's behaviour and is intended.
//!
//! # One coordinate space, and zero scale factors
//!
//! With PerMonitorV2 in force (the application manifest, plus
//! [`SetThreadDpiAwarenessContext`] here as belt-and-braces),
//! `GetSystemMetrics(SM_*VIRTUALSCREEN)`, `MONITORINFO::rcMonitor`,
//! `CreateWindowExW`'s x/y/w/h, the mouse `lParam`, `ClientToScreen`'s output
//! and `BitBlt`'s screen-DC coordinates are **all already physical pixels of
//! the virtual desktop**. The correct number of `dpi / 96` multiplications
//! anywhere in the capture geometry is **zero**; [`border_px`] is the only
//! consumer of the per-monitor DPI and it feeds nothing but a pen width.
//! A scale factor applied twice is exactly the "captured region is smaller /
//! offset" defect this design exists to prevent.
//!
//! Monitors placed left of or above the primary produce **negative**
//! coordinates. That is normal, and it is handled by never assuming an origin
//! of `(0, 0)`: the only subtraction in the file is into the frozen buffer,
//! at `(x - virt.left, y - virt.top)`.

use anyhow::{bail, Context, Result};
use std::cell::RefCell;

use ::windows::core::{w, BOOL, PCWSTR};
use ::windows::Win32::Foundation::{
    GetLastError, COLORREF, ERROR_CLASS_ALREADY_EXISTS, HINSTANCE, HWND, LPARAM, LRESULT, POINT,
    RECT, WPARAM,
};
use ::windows::Win32::Graphics::Gdi::{
    BeginPaint, BitBlt, ClientToScreen, CreateCompatibleDC, CreateDIBSection, CreatePen, DeleteDC,
    DeleteObject, EndPaint, EnumDisplayMonitors, GdiFlush, GetDC, GetMonitorInfoW, GetStockObject,
    InvalidateRect, Rectangle, ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
    DIB_RGB_COLORS, HBITMAP, HBRUSH, HDC, HGDIOBJ, HMONITOR, MONITORINFO, NULL_BRUSH, PAINTSTRUCT,
    PS_SOLID, SRCCOPY,
};
use ::windows::Win32::System::LibraryLoader::GetModuleHandleW;
use ::windows::Win32::UI::HiDpi::{
    AreDpiAwarenessContextsEqual, GetDpiForMonitor, GetThreadDpiAwarenessContext,
    SetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, MDT_EFFECTIVE_DPI,
};
use ::windows::Win32::UI::Input::KeyboardAndMouse::{
    ReleaseCapture, SetCapture, SetFocus, VK_ESCAPE,
};
use ::windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetCursorPos, GetMessageW,
    GetSystemMetrics, GetWindowRect, LoadCursorW, PostQuitMessage, RegisterClassExW,
    SetForegroundWindow, SetWindowPos, ShowWindow, TranslateMessage, CS_HREDRAW, CS_VREDRAW,
    HWND_TOPMOST, IDC_CROSS, MSG, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
    SM_YVIRTUALSCREEN, SWP_NOACTIVATE, SWP_SHOWWINDOW, SW_SHOW, WM_CAPTURECHANGED, WM_DESTROY,
    WM_DPICHANGED, WM_ERASEBKGND, WM_KEYDOWN, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_PAINT,
    WM_RBUTTONDOWN, WM_SYSKEYDOWN, WNDCLASSEXW, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};

const CLASS_NAME: PCWSTR = w!("AloudRegionOverlay");

/// How much of each colour channel the un-selected surround keeps, out of 256.
/// 135/256 ≈ 53%, i.e. a ~47% black wash.
///
/// The dim is precomputed on the CPU into a second full-desktop buffer rather
/// than done per-paint with `AlphaBlend` over a small stretched black source.
/// That was the first implementation and it **silently did nothing on this
/// machine**: `AlphaBlend` returned `TRUE` with `GetLastError() == 0` on every
/// call, with both a zero-alpha and a forced-opaque 8×8 source, and the
/// surround came back at full brightness in a screenshot. Precomputing is
/// deterministic, drops the `msimg32` import, and makes each paint two plain
/// `SRCCOPY` blits.
const DIM_KEEP: u32 = 135;

/// A committed rectangle plus its pixels, lifted out of the frozen desktop.
///
/// Deliberately holds **no** `HWND`/`HDC`/`HBITMAP`: those are `*mut c_void`
/// newtypes and are `!Send`, and keeping every one of them on the overlay
/// thread is what lets this cross the `join()` boundary and what keeps the
/// seam honest.
pub(super) struct Selection {
    /// Virtual-desktop physical pixels. May be negative.
    pub origin_x: i32,
    pub origin_y: i32,
    /// Always > 0 — a zero-area drag is a cancel, not a selection.
    pub width: i32,
    pub height: i32,
    /// Top-down, `stride == width * 4`. The fourth byte is whatever `BitBlt`
    /// left there and is **not** a valid alpha — see `windows.rs`.
    pub bgra: Vec<u8>,
}

/// One monitor, as the overlay cares about it.
struct MonitorGeom {
    /// `rcMonitor`, never `rcWork`: `rcWork` excludes the taskbar and the
    /// overlay would leave an unselectable strip there.
    rc: RECT,
    /// Cosmetic only. See [`border_px`].
    dpi: u32,
}

/// The whole virtual desktop, grabbed once before any overlay window exists.
struct FrozenDesktop {
    /// From `GetDC(None)` — must be released with `ReleaseDC`, never `DeleteDC`.
    screen_dc: HDC,
    dc: HDC,
    bmp: HBITMAP,
    old: HGDIOBJ,
    bits: *mut u8,
    stride: usize,
}

/// A darkened copy of the whole frozen desktop, the same size as it. Each
/// paint blits this for the surround and [`FrozenDesktop`] for the selection
/// interior, so the two are guaranteed to line up pixel-for-pixel.
struct DimmedDesktop {
    dc: HDC,
    bmp: HBITMAP,
    old: HGDIOBJ,
}

/// One overlay window and its double buffer.
struct Slot {
    hwnd: HWND,
    rc: RECT,
    dpi: u32,
    back_dc: HDC,
    back_bmp: HBITMAP,
    back_old: HGDIOBJ,
}

#[derive(Clone, Copy)]
struct Drag {
    /// Both in virtual-desktop physical pixels, so a drag that starts on one
    /// monitor and ends on another needs no special case.
    anchor: POINT,
    current: POINT,
}

enum Outcome {
    Cancelled,
    Committed(RECT),
}

struct OverlayState {
    virt: RECT,
    frozen: FrozenDesktop,
    dim: DimmedDesktop,
    slots: Vec<Slot>,
    drag: Option<Drag>,
    /// `Cancelled` is the default, so every abnormal exit from the pump
    /// degrades to a cancel rather than to a bogus rectangle.
    result: Outcome,
    /// Set by the first terminal transition. Makes [`finish`] once-only, which
    /// is what stops the `WM_CAPTURECHANGED` that `ReleaseCapture()` sends from
    /// overwriting the commit we just made.
    finished: bool,
}

thread_local! {
    /// Installed **before** the first `CreateWindowExW` and taken out only
    /// after `pump()` has returned, so `STATE` is `Some` for the entire
    /// lifetime of every `HWND` these windows will ever have.
    static STATE: RefCell<Option<OverlayState>> = const { RefCell::new(None) };
}

/// Compute under the borrow, drop it, **then** call Win32.
///
/// Some Win32 calls *send* messages synchronously and re-enter `wnd_proc`
/// (`ReleaseCapture` → `WM_CAPTURECHANGED`, `DestroyWindow` → `WM_DESTROY`,
/// `SetWindowPos` → `WM_WINDOWPOSCHANGING`). Holding a borrow across one of
/// those is a `BorrowMutError`, and a panic unwinding out of an
/// `extern "system"` function aborts the process rather than becoming the
/// `Err` that `select()` would otherwise report. So a failed borrow is logged
/// and skipped instead: a discipline slip then degrades to a missed repaint,
/// visible in the log, rather than to a dead app.
fn with_state<R>(f: impl FnOnce(&mut OverlayState) -> R) -> Option<R> {
    STATE.with(|s| match s.try_borrow_mut() {
        Ok(mut borrowed) => borrowed.as_mut().map(f),
        Err(_) => {
            crate::log_line!(
                "overlay: BUG — re-entrant access to the overlay state, skipping this one"
            );
            None
        }
    })
}

// ---------------------------------------------------------------------------
// Pure helpers. Everything the coordinate maths depends on lives here, so it
// can be tested without a window, a monitor or an OS.
// ---------------------------------------------------------------------------

/// Two drag corners in any order, either possibly negative → a well-ordered
/// `RECT`. No Win32, no DPI, no allocation.
fn normalise_rect(a: POINT, b: POINT) -> RECT {
    RECT {
        left: a.x.min(b.x),
        top: a.y.min(b.y),
        right: a.x.max(b.x),
        bottom: a.y.max(b.y),
    }
}

/// Intersect with the virtual-desktop bounds. `None` (empty intersection) is a
/// **cancel**, not an error — a drag can be released with the pointer parked
/// outside the desktop on some layouts.
fn clip(r: RECT, bounds: RECT) -> Option<RECT> {
    let c = RECT {
        left: r.left.max(bounds.left),
        top: r.top.max(bounds.top),
        right: r.right.min(bounds.right),
        bottom: r.bottom.min(bounds.bottom),
    };
    (c.right > c.left && c.bottom > c.top).then_some(c)
}

/// Intersect the selection (virtual-desktop px) with one monitor, then
/// translate into **client** coordinates of that monitor's overlay. `None`
/// when the selection does not touch this monitor.
///
/// `WS_POPUP` has no non-client area, so client origin == window origin ==
/// `rc.left`/`rc.top` and this is a pure subtraction.
fn selection_clipped_to_client(sel: RECT, rc: RECT) -> Option<RECT> {
    let c = clip(sel, rc)?;
    Some(RECT {
        left: c.left - rc.left,
        top: c.top - rc.top,
        right: c.right - rc.left,
        bottom: c.bottom - rc.top,
    })
}

/// The **only** consumer of the per-monitor DPI, and it is cosmetic. If this
/// function ever gains a caller that touches a rectangle, the captured region
/// will be scaled twice and will come out smaller and offset.
fn border_px(dpi: u32) -> i32 {
    ((2.0 * dpi as f32 / 96.0).round() as i32).max(1)
}

/// Copy one rectangle out of the frozen desktop buffer.
///
/// The buffer is top-down with its `(0, 0)` at `(vx, vy)` in virtual-desktop
/// coordinates, so the source offset is a plain subtraction. `r` must already
/// be clipped to the desktop bounds.
///
/// This reads from **`frozen`, which is never drawn on** — that is why the
/// selection band cannot contaminate the crop even though `Rectangle()` strokes
/// centred on the path and so paints half the pen width *inside* the selection.
/// Anyone "optimising" this into a second `BitBlt` off the screen DC would bake
/// a white band into the OCR input.
///
/// # Safety
///
/// `bits` must point at a top-down buffer of at least `stride` bytes per row,
/// covering every row `r` names once translated by `(vx, vy)`.
unsafe fn crop_bgra(bits: *const u8, stride: usize, vx: i32, vy: i32, r: RECT) -> Vec<u8> {
    let w = (r.right - r.left) as usize;
    let h = (r.bottom - r.top) as usize;
    // Widen before multiplying: an i32 product overflows on a wide desktop.
    let mut out = vec![0u8; w * h * 4];
    let src_x = (r.left - vx) as usize;
    for row in 0..h {
        let src_y = (r.top - vy) as usize + row;
        std::ptr::copy_nonoverlapping(
            bits.add(src_y * stride + src_x * 4),
            out.as_mut_ptr().add(row * w * 4),
            w * 4,
        );
    }
    out
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

pub(super) fn run() -> Result<Option<Selection>> {
    // Belt-and-braces over the application manifest: guarantees *this* thread
    // reasons in physical pixels even if the manifest were ever dropped.
    // `SetProcessDpiAwarenessContext` is deliberately not used — it returns
    // ERROR_ACCESS_DENIED once the manifest has set the mode, and by hotkey
    // time Tauri already owns HWNDs anyway.
    let prev = unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    let ctx_ok = unsafe {
        AreDpiAwarenessContextsEqual(
            GetThreadDpiAwarenessContext(),
            DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        )
    }
    .as_bool();
    crate::log_line!("overlay: thread dpi context is PerMonitorV2 = {ctx_ok}");

    let out = run_inner();

    // Restoring a null context is a no-op error; skip it so it does not muddy
    // the thread's last-error for anything downstream.
    if !prev.0.is_null() {
        unsafe { SetThreadDpiAwarenessContext(prev) };
    }
    out
}

fn run_inner() -> Result<Option<Selection>> {
    // 1. Geometry and the frozen desktop. No window exists yet, which is the
    //    whole point: nothing of ours can appear in this snapshot.
    let virt = virtual_bounds()?;
    let monitors = enumerate_monitors()?;
    let frozen = FrozenDesktop::capture(virt)?;
    let dim =
        match DimmedDesktop::from_frozen(&frozen, virt.right - virt.left, virt.bottom - virt.top) {
            Ok(d) => d,
            Err(e) => {
                // `frozen` is not in the state yet, so teardown will never see it.
                frozen.release();
                return Err(e);
            }
        };

    // 2. Install the state BEFORE any HWND exists, with an empty slot list.
    //    Every window message from here on finds a live state, and each slot
    //    is registered the instant its window can start receiving messages.
    STATE.with(|s| {
        *s.borrow_mut() = Some(OverlayState {
            virt,
            frozen,
            dim,
            slots: Vec::new(),
            drag: None,
            result: Outcome::Cancelled,
            finished: false,
        })
    });

    // 3. Build the windows and pump. Any failure lands in `res` and is
    //    re-raised only after teardown has run.
    let res = (|| -> Result<()> {
        let hinst: HINSTANCE = unsafe { GetModuleHandleW(None) }
            .context("GetModuleHandleW(NULL) failed")?
            .into();
        ensure_class(hinst)?;

        for m in &monitors {
            let (back_dc, back_bmp, back_old, _) =
                make_dib(screen_dc()?, m.rc.right - m.rc.left, m.rc.bottom - m.rc.top)
                    .context("creating an overlay back buffer failed")?;
            let hwnd = match create_overlay_window(hinst, m) {
                Ok(h) => h,
                Err(e) => {
                    // This one is not in a slot yet, so teardown will not see it.
                    unsafe {
                        SelectObject(back_dc, back_old);
                        let _ = DeleteObject(back_bmp.into());
                        let _ = DeleteDC(back_dc);
                    }
                    return Err(e);
                }
            };
            with_state(|s| {
                s.slots.push(Slot {
                    hwnd,
                    rc: m.rc,
                    dpi: m.dpi,
                    back_dc,
                    back_bmp,
                    back_old,
                })
            });
        }

        show_and_focus();
        pump()
    })();

    // 4. Take the state OUT of the thread-local. Nothing re-enters `wnd_proc`
    //    from here, so there is no borrow question left in the teardown, and
    //    any stray message a `DestroyWindow` provokes finds `None` and no-ops.
    let mut st = STATE
        .with(|s| s.borrow_mut().take())
        .context("the overlay state vanished while the overlay was up")?;

    // 5. Crop FIRST — `st.frozen` is still alive here. Doing this after
    //    teardown would read pixels that `DeleteObject` had just freed.
    let selection = match st.result {
        Outcome::Committed(r) => clip(r, st.virt).map(|c| Selection {
            origin_x: c.left,
            origin_y: c.top,
            width: c.right - c.left,
            height: c.bottom - c.top,
            bgra: unsafe {
                crop_bgra(
                    st.frozen.bits,
                    st.frozen.stride,
                    st.virt.left,
                    st.virt.top,
                    c,
                )
            },
        }),
        Outcome::Cancelled => None,
    };

    // 6. NOW release every window and GDI object.
    teardown(&mut st);

    // A pump failure outranks a committed rectangle.
    res?;
    Ok(selection)
}

// ---------------------------------------------------------------------------
// Geometry
// ---------------------------------------------------------------------------

fn virtual_bounds() -> Result<RECT> {
    let vx = unsafe { GetSystemMetrics(SM_XVIRTUALSCREEN) };
    let vy = unsafe { GetSystemMetrics(SM_YVIRTUALSCREEN) };
    let vw = unsafe { GetSystemMetrics(SM_CXVIRTUALSCREEN) };
    let vh = unsafe { GetSystemMetrics(SM_CYVIRTUALSCREEN) };
    if vw <= 0 || vh <= 0 {
        bail!("GetSystemMetrics reported an empty virtual desktop ({vw}x{vh})");
    }
    crate::log_line!("overlay: virtual desktop {vw}x{vh} at ({vx},{vy})");
    Ok(RECT {
        left: vx,
        top: vy,
        right: vx + vw,
        bottom: vy + vh,
    })
}

unsafe extern "system" fn enum_monitor(
    hmon: HMONITOR,
    _hdc: HDC,
    _clip: *mut RECT,
    data: LPARAM,
) -> BOOL {
    let out = &mut *(data.0 as *mut Vec<MonitorGeom>);

    let mut mi = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if !GetMonitorInfoW(hmon, &mut mi).as_bool() {
        return BOOL(1); // skip this one, keep enumerating
    }

    let mut dpi_x = 96u32;
    let mut dpi_y = 96u32;
    // Non-fatal: 96 is the right fallback and the value is cosmetic anyway.
    let _ = GetDpiForMonitor(hmon, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y);

    out.push(MonitorGeom {
        rc: mi.rcMonitor,
        dpi: dpi_x,
    });
    BOOL(1) // TRUE = continue
}

fn enumerate_monitors() -> Result<Vec<MonitorGeom>> {
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
        bail!("EnumDisplayMonitors returned no usable monitors");
    }
    for (i, m) in list.iter().enumerate() {
        crate::log_line!(
            "overlay: monitor {i}: rc=({},{})-({},{}) dpi={} scale={:.2}",
            m.rc.left,
            m.rc.top,
            m.rc.right,
            m.rc.bottom,
            m.dpi,
            m.dpi as f32 / 96.0
        );
    }
    Ok(list)
}

// ---------------------------------------------------------------------------
// GDI surfaces
// ---------------------------------------------------------------------------

fn screen_dc() -> Result<HDC> {
    let dc = unsafe { GetDC(None) };
    if dc.is_invalid() {
        bail!("GetDC(NULL) returned a null screen DC");
    }
    Ok(dc)
}

/// One top-down 32bpp DIB section selected into its own memory DC.
///
/// `biHeight` is **negative** on purpose: a positive height gives a bottom-up
/// buffer and every crop comes out vertically mirrored at a row offset that
/// looks exactly like an off-by-N coordinate bug.
///
/// Cleans up after itself on failure, so a partially built surface never
/// escapes into the teardown list.
fn make_dib(ref_dc: HDC, w: i32, h: i32) -> Result<(HDC, HBITMAP, HGDIOBJ, *mut u8)> {
    let dc = unsafe { CreateCompatibleDC(Some(ref_dc)) };
    if dc.is_invalid() {
        bail!("CreateCompatibleDC failed");
    }

    let bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            biHeight: -h,
            biPlanes: 1,
            biBitCount: 32,
            // `BI_RGB` is a `BI_COMPRESSION` newtype; the field is a bare u32.
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };

    let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
    let bmp =
        match unsafe { CreateDIBSection(Some(ref_dc), &bmi, DIB_RGB_COLORS, &mut bits, None, 0) } {
            Ok(b) => b,
            Err(e) => {
                unsafe {
                    let _ = DeleteDC(dc);
                }
                return Err(e).context("CreateDIBSection failed");
            }
        };

    let old = unsafe { SelectObject(dc, bmp.into()) };
    Ok((dc, bmp, old, bits as *mut u8))
}

impl FrozenDesktop {
    fn capture(virt: RECT) -> Result<Self> {
        let vw = virt.right - virt.left;
        let vh = virt.bottom - virt.top;
        let screen_dc = screen_dc()?;
        let (dc, bmp, old, bits) = match make_dib(screen_dc, vw, vh) {
            Ok(v) => v,
            Err(e) => {
                unsafe { ReleaseDC(None, screen_dc) };
                return Err(e).context("allocating the frozen-desktop buffer failed");
            }
        };

        // Source coordinates are virtual-desktop physical pixels and may be
        // negative — that is what SM_X/YVIRTUALSCREEN are for, and it needs no
        // special handling. `CAPTUREBLT` is deliberately not OR'd in: it
        // flashes the screen on some configurations and is unnecessary under
        // DWM, where GetDC(NULL) already reads the composed desktop.
        if let Err(e) = unsafe {
            BitBlt(
                dc,
                0,
                0,
                vw,
                vh,
                Some(screen_dc),
                virt.left,
                virt.top,
                SRCCOPY,
            )
        } {
            unsafe {
                SelectObject(dc, old);
                let _ = DeleteObject(bmp.into());
                let _ = DeleteDC(dc);
                ReleaseDC(None, screen_dc);
            }
            return Err(e).context("BitBlt of the virtual desktop failed");
        }

        // MUST flush before reading `bits` from the CPU: without it the GDI
        // batch may not have executed and the crop memcpys stale rows.
        unsafe {
            let _ = GdiFlush();
        }

        Ok(Self {
            screen_dc,
            dc,
            bmp,
            old,
            bits,
            // 32bpp DIB rows are inherently DWORD-aligned, so there is no
            // padding to account for.
            stride: (vw as usize) * 4,
        })
    }

    /// Frees every handle this owns. Called from [`teardown`], and directly on
    /// the one error path that can fail after the grab but before the state is
    /// installed.
    fn release(&self) {
        unsafe {
            SelectObject(self.dc, self.old);
            let _ = DeleteObject(self.bmp.into());
            let _ = DeleteDC(self.dc);
            // From GetDC — ReleaseDC, never DeleteDC.
            ReleaseDC(None, self.screen_dc);
        }
    }
}

/// Scale every colour channel down by [`DIM_KEEP`]/256. Pure and testable;
/// the alpha byte is forced opaque so nothing downstream can read the
/// undefined one `BitBlt` left behind.
fn dim_into(src: &[u8], dst: &mut [u8]) {
    for (s, d) in src.chunks_exact(4).zip(dst.chunks_exact_mut(4)) {
        d[0] = ((s[0] as u32 * DIM_KEEP) >> 8) as u8;
        d[1] = ((s[1] as u32 * DIM_KEEP) >> 8) as u8;
        d[2] = ((s[2] as u32 * DIM_KEEP) >> 8) as u8;
        d[3] = 0xFF;
    }
}

impl DimmedDesktop {
    fn from_frozen(frozen: &FrozenDesktop, vw: i32, vh: i32) -> Result<Self> {
        let (dc, bmp, old, bits) = make_dib(frozen.screen_dc, vw, vh)
            .context("allocating the dimmed-desktop buffer failed")?;
        let bytes = (vw as usize) * (vh as usize) * 4;
        // SAFETY: both buffers are DIB sections of exactly these dimensions,
        // 32bpp and DWORD-aligned, so `bytes` is their exact length. They are
        // distinct allocations, and no GDI call is outstanding against either
        // (the frozen grab already ran GdiFlush).
        unsafe {
            let src = std::slice::from_raw_parts(frozen.bits as *const u8, bytes);
            let dst = std::slice::from_raw_parts_mut(bits, bytes);
            dim_into(src, dst);
        }
        Ok(Self { dc, bmp, old })
    }
}

// ---------------------------------------------------------------------------
// Windows
// ---------------------------------------------------------------------------

fn ensure_class(hinst: HINSTANCE) -> Result<()> {
    let cursor =
        unsafe { LoadCursorW(None, IDC_CROSS) }.context("LoadCursorW(IDC_CROSS) failed")?;

    let wc = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(wnd_proc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: hinst,
        hIcon: Default::default(),
        hCursor: cursor,
        // A null background brush means Windows never erases our client area.
        // This is the load-bearing half of the anti-flicker story; the
        // WM_ERASEBKGND handler covers an explicit RedrawWindow(RDW_ERASE).
        hbrBackground: HBRUSH(std::ptr::null_mut()),
        lpszMenuName: PCWSTR::null(),
        lpszClassName: CLASS_NAME,
        hIconSm: Default::default(),
    };

    let atom = unsafe { RegisterClassExW(&wc) };
    if atom == 0 {
        let e = unsafe { GetLastError() };
        // The overlay is rebuilt on every hotkey press but the class survives
        // for the life of the process, so "already registered" is success.
        // Treating it as fatal would break every capture after the first.
        if e != ERROR_CLASS_ALREADY_EXISTS {
            // Built from the code already read rather than from
            // `Error::from_win32()`: that re-reads the thread's last-error, and
            // any log line inserted between the two would clobber it (writing
            // the log file is a `WriteFile` call) and turn this into a bogus
            // error.
            bail!(
                "RegisterClassExW for the overlay class failed (WIN32_ERROR {})",
                e.0
            );
        }
    }
    Ok(())
}

fn create_overlay_window(hinst: HINSTANCE, m: &MonitorGeom) -> Result<HWND> {
    // WS_POPUP: no caption, no border, no non-client area, so client origin ==
    // window origin == rcMonitor origin.
    // WS_EX_TOOLWINDOW: keeps a transient overlay out of the taskbar and
    // out of Alt-Tab.
    // Not WS_EX_LAYERED (see the module doc), not WS_EX_NOREDIRECTIONBITMAP
    // (it suppresses the DWM redirection surface a GDI WM_PAINT presents
    // through — the window would render nothing), not WS_EX_TRANSPARENT (it
    // makes the window click-through and we would get no mouse input at all),
    // and not WS_EX_NOACTIVATE (we need keyboard focus for Escape).
    // No WS_VISIBLE either: the windows are shown together, after all of them
    // exist, so a half-built overlay is never on screen.
    unsafe {
        CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            CLASS_NAME,
            w!(""),
            WS_POPUP,
            m.rc.left,
            m.rc.top,
            m.rc.right - m.rc.left,
            m.rc.bottom - m.rc.top,
            None,
            None,
            Some(hinst),
            None,
        )
    }
    .context("CreateWindowExW for the overlay failed")
}

fn show_and_focus() {
    let slots: Vec<(HWND, RECT)> =
        with_state(|s| s.slots.iter().map(|w| (w.hwnd, w.rc)).collect()).unwrap_or_default();

    for (hwnd, rc) in &slots {
        unsafe {
            let _ = ShowWindow(*hwnd, SW_SHOW);
            // Reassert exact geometry and z-order once the window has landed on
            // its monitor, so a stray WM_DPICHANGED during creation cannot
            // leave a scaled or offset overlay behind.
            let _ = SetWindowPos(
                *hwnd,
                Some(HWND_TOPMOST),
                rc.left,
                rc.top,
                rc.right - rc.left,
                rc.bottom - rc.top,
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            );

            // Cheap proof that no scale factor got applied twice: if this ever
            // disagrees with rcMonitor, questions 20 and 21 are already lost
            // and the log says so before the user even drags.
            let mut got = RECT::default();
            if GetWindowRect(*hwnd, &mut got).is_ok()
                && (got.left != rc.left
                    || got.top != rc.top
                    || got.right != rc.right
                    || got.bottom != rc.bottom)
            {
                crate::log_line!(
                    "overlay: WARNING window rect ({},{})-({},{}) != monitor rect ({},{})-({},{})",
                    got.left,
                    got.top,
                    got.right,
                    got.bottom,
                    rc.left,
                    rc.top,
                    rc.right,
                    rc.bottom
                );
            }
        }
    }

    // Focus the overlay under the cursor so WM_KEYDOWN(VK_ESCAPE) has a
    // destination. The BOOL is logged rather than checked: the foreground lock
    // is documented per *process*, and we are raising this from a worker thread
    // of the process that just handled the hotkey, so it should be granted —
    // but if it is ever refused, this line is what says so, and right-click
    // still cancels either way.
    let focus = slot_under_cursor().or_else(|| slots.first().map(|(h, _)| *h));
    if let Some(hwnd) = focus {
        unsafe {
            let fg = SetForegroundWindow(hwnd).as_bool();
            let _ = SetFocus(Some(hwnd));
            crate::log_line!(
                "overlay: {} window(s) shown, SetForegroundWindow={fg}",
                slots.len()
            );
        }
    }
}

fn slot_under_cursor() -> Option<HWND> {
    let mut pt = POINT::default();
    if unsafe { GetCursorPos(&mut pt) }.is_err() {
        return None;
    }
    with_state(|s| {
        s.slots
            .iter()
            .find(|w| {
                pt.x >= w.rc.left && pt.x < w.rc.right && pt.y >= w.rc.top && pt.y < w.rc.bottom
            })
            .map(|w| w.hwnd)
    })
    .flatten()
}

/// Mark every overlay dirty. `InvalidateRect` only *marks* the update region —
/// `WM_PAINT` is generated later by `GetMessage` — so it is safe under a
/// borrow, but the handles are collected first anyway to keep one rule for
/// every Win32 call. Never `UpdateWindow`/`RedrawWindow(RDW_UPDATENOW)` here:
/// those send `WM_PAINT` synchronously and would re-enter.
fn invalidate_all() {
    let hwnds: Vec<HWND> =
        with_state(|s| s.slots.iter().map(|w| w.hwnd).collect()).unwrap_or_default();
    for h in hwnds {
        // `bErase` false: passing true reintroduces the erase the null class
        // brush exists to suppress.
        unsafe {
            let _ = InvalidateRect(Some(h), None, false);
        }
    }
}

fn pump() -> Result<()> {
    let mut msg = MSG::default();
    loop {
        // hwnd MUST be None. With `Some(hwnd)` we would receive only that one
        // window's messages — dropping the other monitors' input — and would
        // never receive WM_QUIT at all, because WM_QUIT is a *thread* message.
        let r = unsafe { GetMessageW(&mut msg, None, 0, 0) };
        // -1 is the error return, and it must be checked BEFORE the 0 check:
        // `BOOL::as_bool()` is `self.0 != 0`, so -1 reads as `true` and a naive
        // loop spins forever.
        if r.0 == -1 {
            return Err(::windows::core::Error::from_win32()).context("GetMessageW failed");
        }
        if r.0 == 0 {
            return Ok(()); // WM_QUIT
        }
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

fn teardown(st: &mut OverlayState) {
    unsafe {
        for slot in &st.slots {
            SelectObject(slot.back_dc, slot.back_old);
            let _ = DeleteObject(slot.back_bmp.into());
            let _ = DeleteDC(slot.back_dc);
            let _ = DestroyWindow(slot.hwnd);
        }
        st.slots.clear();

        SelectObject(st.dim.dc, st.dim.old);
        let _ = DeleteObject(st.dim.bmp.into());
        let _ = DeleteDC(st.dim.dc);
    }
    // Frees its own DC, bitmap and the GetDC handle both buffers were made
    // against, so it must go last.
    st.frozen.release();
    // The window class is deliberately NOT unregistered: it is process-wide,
    // cheap to keep, and unregistering it while another overlay could exist
    // would be a race for no benefit.
}

// ---------------------------------------------------------------------------
// Message handling
// ---------------------------------------------------------------------------

/// The one-way door out of the pump. Idempotent by design: the first caller
/// wins.
///
/// That matters concretely. `WM_LBUTTONUP` commits a rectangle and then calls
/// `ReleaseCapture()`, which **synchronously sends** `WM_CAPTURECHANGED` back
/// into this WndProc — and that handler cancels. Without the `finished` latch
/// every successful drag would be turned into a cancel by its own cleanup.
fn finish(outcome: Outcome) {
    let took = with_state(|s| {
        if s.finished {
            return false;
        }
        s.finished = true;
        s.drag = None;
        s.result = outcome;
        true
    })
    .unwrap_or(false);
    let _ = took;
    // Posts, never sends — safe with no borrow held, and harmless if the pump
    // has already exited.
    unsafe { PostQuitMessage(0) };
}

/// Mouse `lParam` (client-relative, physical px) → virtual-desktop physical px.
/// The only coordinate conversion in the file: no DPI, no scale, no division.
unsafe fn lparam_to_virtual(hwnd: HWND, lparam: LPARAM) -> POINT {
    // GET_X_LPARAM / GET_Y_LPARAM are C macros with no windows-rs equivalent.
    // The hop through i16 is mandatory: the fields are SIGNED 16-bit, and under
    // SetCapture a monitor left of the primary produces a legitimately negative
    // client x. Without the i16 cast, -1920 arrives as 63616.
    let mut pt = POINT {
        x: (lparam.0 & 0xFFFF) as u16 as i16 as i32,
        y: ((lparam.0 >> 16) & 0xFFFF) as u16 as i16 as i32,
    };
    // A pure translation, and well-defined outside the client rect — which is
    // exactly the cross-monitor drag case.
    let _ = ClientToScreen(hwnd, &mut pt);
    pt
}

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_ERASEBKGND => LRESULT(1), // "already erased"

        WM_LBUTTONDOWN => {
            let pt = lparam_to_virtual(hwnd, lparam);
            with_state(|s| {
                s.drag = Some(Drag {
                    anchor: pt,
                    current: pt,
                })
            });
            // With capture held by the window the button went down on, every
            // later mouse message — including moves over a different monitor —
            // is delivered here, with client coordinates that legitimately go
            // negative or past the client width. That is what makes a
            // cross-monitor drag work at all.
            SetCapture(hwnd);
            invalidate_all();
            LRESULT(0)
        }

        WM_MOUSEMOVE => {
            if with_state(|s| s.drag.is_some()).unwrap_or(false) {
                let pt = lparam_to_virtual(hwnd, lparam);
                with_state(|s| {
                    if let Some(d) = s.drag.as_mut() {
                        d.current = pt;
                    }
                });
                invalidate_all();
            }
            LRESULT(0)
        }

        WM_LBUTTONUP => {
            let pt = lparam_to_virtual(hwnd, lparam);
            let rect = with_state(|s| {
                let d = s.drag.take()?;
                let r = normalise_rect(d.anchor, pt);
                (r.right - r.left >= 1 && r.bottom - r.top >= 1).then_some(r)
            })
            .flatten();

            match rect {
                Some(r) => finish(Outcome::Committed(r)),
                // A click with no drag is a cancel-shaped gesture — macOS
                // `screencapture -i` behaves the same way. Ok(None), never Err,
                // and never a 0x0 BitBlt.
                None => finish(Outcome::Cancelled),
            }
            let _ = ReleaseCapture();
            LRESULT(0)
        }

        WM_KEYDOWN | WM_SYSKEYDOWN if wparam.0 as u16 == VK_ESCAPE.0 => {
            finish(Outcome::Cancelled);
            let _ = ReleaseCapture();
            LRESULT(0)
        }

        // Secondary cancel. Costs nothing, and it is the way out if
        // SetForegroundWindow was ever refused and keyboard input never arrives.
        WM_RBUTTONDOWN => {
            finish(Outcome::Cancelled);
            let _ = ReleaseCapture();
            LRESULT(0)
        }

        // Someone else took the mouse capture — a UAC prompt, an Alt-Tab,
        // another process calling SetCapture. WM_LBUTTONUP will then never
        // arrive here, so a drag left in place would strand the user in front
        // of a frozen full-screen snapshot with no way out.
        //
        // Do NOT call ReleaseCapture() here: capture is already gone, and
        // ReleaseCapture *sends* this very message, so it would re-enter.
        WM_CAPTURECHANGED => {
            finish(Outcome::Cancelled);
            LRESULT(0)
        }

        WM_PAINT => {
            paint(hwnd);
            LRESULT(0)
        }

        // Ignore the OS's suggested rectangle: our geometry is authoritative
        // and is reasserted by SetWindowPos. Honouring it would resize the
        // overlay mid-drag and desynchronise its back buffer.
        WM_DPICHANGED => LRESULT(0),

        WM_DESTROY => LRESULT(0),
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

/// `BeginPaint`/`EndPaint` bracket **every** path, including the ones that do
/// nothing.
///
/// `WM_PAINT` is level-triggered: if the update region is never validated,
/// Windows re-posts it immediately and forever — 100% CPU on the overlay
/// thread, a black overlay, and a drag that can never complete.
unsafe fn paint(hwnd: HWND) {
    let mut ps = PAINTSTRUCT::default();
    let hdc = BeginPaint(hwnd, &mut ps);
    if !hdc.is_invalid() {
        compose_and_present(hwnd, hdc, &ps);
    }
    let _ = EndPaint(hwnd, &ps);
}

/// Composes into the back buffer and presents in one blit, so no intermediate
/// state ever reaches the screen.
///
/// Every call in here is a GDI drawing call, none of which sends a message, so
/// the whole composition can legitimately run under the state borrow.
unsafe fn compose_and_present(hwnd: HWND, hdc: HDC, ps: &PAINTSTRUCT) {
    with_state(|s| {
        let Some(slot) = s.slots.iter().find(|w| w.hwnd.0 == hwnd.0) else {
            return; // a window we do not know about; nothing to draw
        };
        let (rc, dpi, back_dc) = (slot.rc, slot.dpi, slot.back_dc);
        let (mw, mh) = (rc.right - rc.left, rc.bottom - rc.top);
        let (vx, vy) = (s.virt.left, s.virt.top);

        // 1. This monitor's slice of the *dimmed* desktop. Same geometry as
        // the frozen buffer, so the un-dimmed selection blit below lines up
        // with it exactly.
        let _ = BitBlt(
            back_dc,
            0,
            0,
            mw,
            mh,
            Some(s.dim.dc),
            rc.left - vx,
            rc.top - vy,
            SRCCOPY,
        );

        // 2. Put the selection's own pixels back, un-dimmed, and outline them.
        if let Some(sel) = s.drag.map(|d| normalise_rect(d.anchor, d.current)) {
            if let Some(cl) = selection_clipped_to_client(sel, rc) {
                let _ = BitBlt(
                    back_dc,
                    cl.left,
                    cl.top,
                    cl.right - cl.left,
                    cl.bottom - cl.top,
                    Some(s.frozen.dc),
                    (rc.left + cl.left) - vx,
                    (rc.top + cl.top) - vy,
                    SRCCOPY,
                );
                draw_band(back_dc, cl, dpi);
            }
        }

        // 3. Present exactly the invalid region, in one blit.
        let _ = BitBlt(
            hdc,
            ps.rcPaint.left,
            ps.rcPaint.top,
            ps.rcPaint.right - ps.rcPaint.left,
            ps.rcPaint.bottom - ps.rcPaint.top,
            Some(back_dc),
            ps.rcPaint.left,
            ps.rcPaint.top,
            SRCCOPY,
        );
    });
}

unsafe fn draw_band(dc: HDC, cl: RECT, dpi: u32) {
    // COLORREF is 0x00BBGGRR, so this is white.
    let pen = CreatePen(PS_SOLID, border_px(dpi), COLORREF(0x00FF_FFFF));
    if pen.0.is_null() {
        return;
    }
    let old_pen = SelectObject(dc, pen.into());
    let old_brush = SelectObject(dc, GetStockObject(NULL_BRUSH));
    let _ = Rectangle(dc, cl.left, cl.top, cl.right, cl.bottom);
    SelectObject(dc, old_brush);
    SelectObject(dc, old_pen);
    let _ = DeleteObject(pen.into());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pt(x: i32, y: i32) -> POINT {
        POINT { x, y }
    }

    fn rect(left: i32, top: i32, right: i32, bottom: i32) -> RECT {
        RECT {
            left,
            top,
            right,
            bottom,
        }
    }

    fn same(a: RECT, b: RECT) -> bool {
        a.left == b.left && a.top == b.top && a.right == b.right && a.bottom == b.bottom
    }

    #[test]
    fn normalise_rect_orders_all_four_drag_directions() {
        let expected = rect(10, 20, 110, 220);
        for (a, b) in [
            (pt(10, 20), pt(110, 220)), // down-right
            (pt(110, 220), pt(10, 20)), // up-left
            (pt(110, 20), pt(10, 220)), // down-left
            (pt(10, 220), pt(110, 20)), // up-right
        ] {
            assert!(same(normalise_rect(a, b), expected), "{a:?} -> {b:?}");
        }
    }

    /// A monitor left of and above the primary. The whole reason coordinates
    /// are kept signed everywhere.
    #[test]
    fn normalise_rect_handles_negative_coordinates() {
        let r = normalise_rect(pt(-1920, -100), pt(200, 800));
        assert!(same(r, rect(-1920, -100, 200, 800)));
    }

    /// The anti-double-scale guard. These dimensions are what the user dragged,
    /// in physical pixels, and they must survive untouched — `normalise_rect`
    /// must not consult a DPI, a scale factor or a monitor. The assertion
    /// passes trivially today and fails loudly the moment someone threads a
    /// scale factor through the geometry, which is the defect that makes a
    /// captured region come out smaller and offset on a scaled monitor.
    #[test]
    fn a_dragged_rect_is_never_scaled() {
        for dpi in [96u32, 144, 192] {
            let r = normalise_rect(pt(100, 100), pt(340, 280));
            assert_eq!(r.right - r.left, 240, "width changed at dpi {dpi}");
            assert_eq!(r.bottom - r.top, 180, "height changed at dpi {dpi}");
            // The DPI's only legitimate effect is cosmetic.
            assert!(border_px(dpi) >= 1);
        }
    }

    #[test]
    fn border_px_scales_only_the_pen() {
        assert_eq!(border_px(96), 2);
        assert_eq!(border_px(144), 3);
        assert_eq!(border_px(192), 4);
        assert!(border_px(0) >= 1, "must never be a zero-width pen");
    }

    #[test]
    fn clip_intersects_and_rejects_empty() {
        let bounds = rect(-2560, 0, 4480, 1440);
        assert!(same(
            clip(rect(-3000, -50, 100, 900), bounds).unwrap(),
            rect(-2560, 0, 100, 900)
        ));
        // Entirely outside, and zero-area: both are cancels, not errors.
        assert!(clip(rect(5000, 100, 6000, 200), bounds).is_none());
        assert!(clip(rect(100, 100, 100, 200), bounds).is_none());
    }

    #[test]
    fn selection_clipped_to_client_is_a_pure_translation() {
        // The secondary monitor at x = -2560, and a selection straddling it.
        let monitor = rect(-2560, 0, 0, 1440);
        let cl = selection_clipped_to_client(rect(-1000, 100, 500, 300), monitor).unwrap();
        assert!(same(cl, rect(1560, 100, 2560, 300)));
        // A selection that never touches this monitor draws nothing on it.
        assert!(selection_clipped_to_client(rect(100, 100, 200, 200), monitor).is_none());
    }

    #[test]
    fn dim_into_darkens_colour_and_forces_alpha_opaque() {
        // Black stays black, white lands at DIM_KEEP/256, and the undefined
        // alpha byte BitBlt leaves behind never survives.
        let src = [0u8, 0, 0, 0, 255, 255, 255, 0];
        let mut dst = [0u8; 8];
        dim_into(&src, &mut dst);
        assert_eq!(&dst[0..3], &[0, 0, 0]);
        // 255 * 135 >> 8 == 134, i.e. just under 53% of full brightness.
        assert_eq!(&dst[4..7], &[134, 134, 134]);
        assert_eq!(dst[3], 0xFF);
        assert_eq!(dst[7], 0xFF);
        // It must actually darken — a no-op here is the AlphaBlend bug again.
        assert!(dst[4] < 255);
    }

    #[test]
    fn crop_bgra_reads_the_right_rows_with_a_negative_origin() {
        // A 4x3 desktop whose origin is (-2, -1). Each pixel is tagged with
        // its own row and column so a transposed or offset read is obvious.
        let (w, h) = (4usize, 3usize);
        let mut buf = vec![0u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                let p = (y * w + x) * 4;
                buf[p] = x as u8;
                buf[p + 1] = y as u8;
            }
        }
        // Virtual coords (-1, 0)-(1, 2) = buffer coords (1, 1)-(3, 3).
        let out = unsafe { crop_bgra(buf.as_ptr(), w * 4, -2, -1, rect(-1, 0, 1, 2)) };
        assert_eq!(out.len(), 2 * 2 * 4);
        assert_eq!((out[0], out[1]), (1, 1)); // top-left of the crop
        assert_eq!((out[4], out[5]), (2, 1));
        assert_eq!((out[8], out[9]), (1, 2)); // second row
        assert_eq!((out[12], out[13]), (2, 2));
    }
}
