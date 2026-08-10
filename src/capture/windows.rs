//! Windows region capture — **stub, body not implemented.**
//!
//! Written on macOS as part of the M6 preparation pass, so the PC fills in
//! bodies rather than inventing structure. It deliberately references no
//! WinRT/Win32 type: WinRT bindings do not compile on macOS (hard constraint
//! 7), so anything that needed one could not have been typechecked before
//! being handed over. Everything below that *is* a Windows fact lives in the
//! doc comments, sourced from `docs/M6-platform-research-windows.md`.

use super::RegionSelector;
use anyhow::{bail, Result};
use std::path::PathBuf;

/// Interactive rectangle capture on Windows.
///
/// Named and shaped to mirror `capture::macos::ScreenCapture` exactly —
/// `new()` returns `Self`, not `Result<Self>` — so wiring it up in
/// `src/bin/aloud.rs` is an import swap behind a `#[cfg]`, not a change to
/// the call site.
///
/// # Capture API: GDI `BitBlt`, not `Windows.Graphics.Capture`
///
/// `Windows.Graphics.Capture` draws a **system yellow border** around whatever
/// it captures. Turning that off requires `GraphicsCaptureSession.IsBorderRequired
/// = false`, which requires the `graphicsCaptureWithoutBorder` capability, which
/// requires a **package manifest** — i.e. MSIX. A non-packaged app can set the
/// property; the value is silently ignored. Aloud grabs one still frame, which
/// is `BitBlt`'s exact shape: one call, no border, no packaging pressure, and it
/// natively spans the whole virtual desktop including negative coordinates.
/// DXGI Desktop Duplication is built for frame-by-frame streaming and is a worse
/// fit again.
///
/// # There is no permission gate here — do not port the macOS one
///
/// Windows has no TCC analogue for screen capture. `BitBlt`, WGC and Desktop
/// Duplication all work unpackaged, unelevated, with no prompt and no Settings
/// toggle. The `CGPreflightScreenCaptureAccess` false-negative handling, the
/// "grant died on rebuild" handling and the restart-after-granting flow in
/// `capture::macos` have **no Windows equivalent** and must not be copied over.
///
/// # Coordinate space
///
/// The virtual desktop origin is the *primary* monitor, so a monitor placed
/// left of or above it produces **negative** coordinates — normal, and must be
/// designed for. `GetSystemMetrics(SM_XVIRTUALSCREEN / SM_YVIRTUALSCREEN /
/// SM_CXVIRTUALSCREEN / SM_CYVIRTUALSCREEN)` gives origin and extent in physical
/// pixels *once the process is PerMonitorV2 DPI-aware*. Per hard constraint 4
/// the overlay is **one window per monitor**, each carrying its own scale
/// factor, normalised into that global space — never one fullscreen window.
///
/// # The DPI ordering trap
///
/// tao sets `PER_MONITOR_AWARE_V2` at `EventLoop` creation, but the mode is
/// locked once *any* `HWND` exists in the process. Creating an overlay window
/// before `EventLoop::new()` would permanently pin the process to DPI-*unaware*,
/// silently, and every capture rectangle on a scaled monitor would then be wrong.
/// The application manifest fixes this from process start; keep the ordering
/// rule anyway.
///
/// # A capture can legitimately come back black
///
/// `SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE)` is enforced in DWM,
/// so DRM and banking windows return black under `BitBlt`, WGC and Desktop
/// Duplication alike. That is a content restriction, not a permission problem
/// and not a bug. Say so rather than going quiet.
pub struct ScreenCapture;

impl ScreenCapture {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ScreenCapture {
    fn default() -> Self {
        Self::new()
    }
}

impl RegionSelector for ScreenCapture {
    /// Contract is the trait's, unchanged and load-bearing: `Ok(None)` means
    /// **the user cancelled** and the caller silently does nothing; anything
    /// that stops a capture for any other reason is `Err`. Do not let the two
    /// collapse. The returned `PathBuf` is a freshly written PNG under the temp
    /// directory — a photograph of the user's screen — and the caller owns
    /// deleting it on every path including error paths.
    fn select(&self) -> Result<Option<PathBuf>> {
        bail!("region capture is not implemented on Windows yet")
    }
}
