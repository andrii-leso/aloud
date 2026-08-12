//! Windows region capture.
//!
//! The structure was written on macOS during the M6 preparation pass and the
//! bodies were filled in on the PC — WinRT bindings do not compile on macOS
//! (hard constraint 7), so no Windows call in here could be typechecked before
//! it reached this machine.
//!
//! This file is the thin half: the trait impl, the temp-file contract and the
//! PNG encode. The interactive UI — the per-monitor overlay windows, the drag,
//! Escape, and the `BitBlt` — lives in [`overlay`].

use super::RegionSelector;
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use ::windows::Graphics::Imaging::{BitmapAlphaMode, BitmapEncoder, BitmapPixelFormat};
use ::windows::Storage::Streams::{DataReader, InMemoryRandomAccessStream};
use ::windows::Win32::System::WinRT::{RoInitialize, RoUninitialize, RO_INIT_MULTITHREADED};

mod overlay;

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
/// # The two protection modes behave DIFFERENTLY — measured, 2026-08-12
///
/// This comment used to say that `SetWindowDisplayAffinity(hwnd,
/// WDA_EXCLUDEFROMCAPTURE)` makes DRM and banking windows "return black under
/// `BitBlt`, WGC and Desktop Duplication alike". Half right, and the wrong half
/// is the half that matters.
///
/// Measured on Windows 11 25H2 through this file's own capture path
/// (`BitBlt(SRCCOPY)` from `GetDC(NULL)`, no `CAPTUREBLT`), against a
/// magenta window that set the affinity on itself and read it back with
/// `GetWindowDisplayAffinity` to prove the value took:
///
/// | Affinity | window pixels | black | desktop behind |
/// |---|---|---|---|
/// | `WDA_NONE` (0x0) | 80.0% | 0.7% | 19.3% |
/// | `WDA_MONITOR` (0x1) | 0.0% | **92.9%** | 7.1% |
/// | `WDA_EXCLUDEFROMCAPTURE` (0x11) | 0.0% | 0.1% | **99.9%** |
///
/// So `WDA_MONITOR` **does** black the window out, and
/// `WDA_EXCLUDEFROMCAPTURE` — the mode added in Windows 10 2004, and the one
/// modern DRM uses — **omits** it: the desktop behind is captured in its place.
///
/// The `EXCLUDEFROMCAPTURE` case is the dangerous one, because there is no way
/// to notice it happened. The capture looks like an ordinary capture, so Aloud
/// will read out whatever sat *behind* the protected window with nothing
/// indicating that the thing the user pointed at was withheld.
/// [`looks_protected`] catches the `WDA_MONITOR` case and cannot catch this one.
///
/// WGC and Desktop Duplication were **not** measured. Do not restore the old
/// "black under all three" claim without measuring them.
///
/// (Recorded because both earlier attempts at this note were wrong in opposite
/// directions. The first accidentally passed `WDA_NONE` — the interop class
/// never defined `WDA_MONITOR`, so the constant resolved to null and then to
/// zero, and `SetWindowDisplayAffinity` cheerfully returned `TRUE` having
/// applied nothing. Reading the value back is what settles it; do that.)
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
        crate::log_line!("capture: starting the region overlay thread");

        // The overlay's windows must be pumped on the thread that created
        // them, so they get a thread of their own with its own `GetMessage`
        // loop, and this call blocks on `join()`. That keeps `select()` the
        // plain blocking function the trait already describes — no async, no
        // cross-thread dance, and no Win32 handle above this seam.
        //
        // `join()` is used rather than a channel because it turns a panic on
        // that thread into an `Err` here for free. No `stack_size`: a Win32
        // pump re-enters the WndProc through DWM, IME and shell hooks, and a
        // guard-page hit on Windows *aborts the process* rather than
        // surfacing as the `Err` this relies on.
        let handle = std::thread::Builder::new()
            .name("aloud-region-overlay".into())
            .spawn(overlay::run)
            .context("failed to spawn the region overlay thread")?;

        let selection = match handle.join() {
            Ok(inner) => inner?,
            Err(_) => bail!("the region overlay thread panicked"),
        };

        // The only place `Ok(None)` is produced. Everything upstream of here
        // that goes wrong is an `Err`, and the two never meet: a cancel is a
        // decision taken in the WndProc, a failure is an error returned from a
        // Win32 call.
        let Some(sel) = selection else {
            crate::log_line!("capture: the overlay was cancelled by the user");
            return Ok(None);
        };

        crate::log_line!(
            "capture: committed {}x{} at ({},{}) in virtual-desktop pixels",
            sel.width,
            sel.height,
            sel.origin_x,
            sel.origin_y
        );
        if looks_protected(&sel.bgra) {
            crate::log_line!(
                "capture: the region came back entirely black — not a permission problem \
                 (Windows has no capture permission gate). Either a WDA_MONITOR-protected \
                 window, or a display that was asleep. Note the other protection mode, \
                 WDA_EXCLUDEFROMCAPTURE, does NOT look like this: it omits the window and \
                 captures the desktop behind it, which nothing here can detect"
            );
        }

        let path = unique_capture_path();
        if let Err(e) = write_png(&path, &sel) {
            // A screenshot must never be left half-written in the temp
            // directory. The trait makes the *caller* the owner of the path in
            // `Ok(Some(path))`; on this path there is no path to hand over, so
            // nobody else can clean it up.
            let _ = std::fs::remove_file(&path);
            return Err(e).context("failed to write the captured region as PNG");
        }
        Ok(Some(path))
    }
}

/// Every pixel exactly black. Alpha is ignored because `BitBlt` does not write
/// a meaningful one.
///
/// **Catches one of the two protection modes, and it is the less common one.**
/// Measured 2026-08-12 (see the table on [`ScreenCapture`]): a `WDA_MONITOR`
/// window comes back 92.9% black, so this fires. A `WDA_EXCLUDEFROMCAPTURE`
/// window — the mode modern DRM uses — is *omitted* from the capture, so this
/// can never fire for it and nothing else can either.
///
/// Also worth one log line for the mundane cases: a display asleep mid-capture,
/// or a window that has not painted yet. It costs one pass over a small buffer.
fn looks_protected(bgra: &[u8]) -> bool {
    bgra.chunks_exact(4)
        .all(|p| p[0] == 0 && p[1] == 0 && p[2] == 0)
}

/// Puts a **freshly spawned** thread in the MTA for the life of the guard.
///
/// Simpler than `ocr::windows::Mta` on purpose: the only caller spawns the
/// thread immediately before entering, so it has never touched COM,
/// `RPC_E_CHANGED_MODE` cannot occur, and the apartment is unambiguously ours
/// to uninitialise. `RoUninitialize` on an apartment we did not create is
/// windows-rs#1169 — it unloads COM under whoever did.
struct OwnedMta;

impl OwnedMta {
    fn enter() -> Result<Self> {
        unsafe { RoInitialize(RO_INIT_MULTITHREADED) }
            .context("RoInitialize(RO_INIT_MULTITHREADED) failed on the PNG encoder thread")?;
        Ok(Self)
    }
}

impl Drop for OwnedMta {
    fn drop(&mut self) {
        unsafe { RoUninitialize() };
    }
}

/// Encodes the captured pixels as a PNG at `path`.
///
/// Uses WinRT's `BitmapEncoder` — the mirror of the `BitmapDecoder` path
/// `src/ocr/windows.rs` already takes — rather than declaring a PNG crate.
/// `image` and `png` are both in `Cargo.lock`, but only transitively via
/// tauri, and Rust will not let this crate name a dependency it does not
/// declare. `Graphics_Imaging` and `Storage_Streams` are already enabled for
/// the OCR half, so this costs no new dependency and no new feature.
fn write_png(path: &Path, sel: &overlay::Selection) -> Result<()> {
    // The WinRT half runs on its own thread, always. `IAsyncOperation::get()`
    // blocks on `WaitForSingleObject(INFINITE)` with no message pump, which is
    // a permanent deadlock on an STA — and `select()` is reachable from
    // whatever thread the region flow happens to be on. A brand-new thread is
    // apartment-free, so `RO_INIT_MULTITHREADED` always succeeds there. Same
    // reasoning and same shape as `ocr::windows::WindowsOcr::recognise`.
    let joined = std::thread::scope(|scope| scope.spawn(|| encode_png(path, sel)).join());
    match joined {
        Ok(inner) => inner,
        Err(_) => bail!("the PNG encoder thread panicked"),
    }
}

fn encode_png(path: &Path, sel: &overlay::Selection) -> Result<()> {
    let _mta = OwnedMta::enter()?;

    // `BitBlt` writes BGRX and that fourth byte is **not** a valid alpha — it
    // comes back as zero. Handing it over as-is produces a fully transparent
    // PNG and OCR silently recognises nothing. `BitmapAlphaMode::Ignore` says
    // there is no alpha here; forcing the byte to 0xFF makes that true
    // byte-for-byte rather than trusting the encoder to honour the hint.
    let mut pixels = sel.bgra.clone();
    for px in pixels.chunks_exact_mut(4) {
        px[3] = 0xFF;
    }

    let stream =
        InMemoryRandomAccessStream::new().context("InMemoryRandomAccessStream::new failed")?;
    let encoder = BitmapEncoder::CreateAsync(
        BitmapEncoder::PngEncoderId().context("BitmapEncoder::PngEncoderId failed")?,
        &stream,
    )
    .context("BitmapEncoder::CreateAsync failed to start")?
    .get()
    .context("BitmapEncoder::CreateAsync failed")?;

    encoder
        .SetPixelData(
            BitmapPixelFormat::Bgra8,
            BitmapAlphaMode::Ignore,
            sel.width as u32,
            sel.height as u32,
            96.0,
            96.0,
            &pixels,
        )
        .context("BitmapEncoder::SetPixelData failed")?;
    encoder
        .FlushAsync()
        .context("BitmapEncoder::FlushAsync failed to start")?
        .get()
        .context("BitmapEncoder::FlushAsync failed")?;

    // Pull the encoded bytes back out and write them with plain std::fs, so
    // the file is created and closed by Rust and there is no WinRT file handle
    // left open when the caller's delete-on-drop runs.
    let size = stream
        .Size()
        .context("the encoded PNG stream reported no size")?;
    let len = u32::try_from(size).context("the encoded PNG is implausibly large")?;
    let input = stream
        .GetInputStreamAt(0)
        .context("rewinding the encoded PNG stream failed")?;
    let reader =
        DataReader::CreateDataReader(&input).context("DataReader::CreateDataReader failed")?;
    reader
        .LoadAsync(len)
        .context("DataReader::LoadAsync failed to start")?
        .get()
        .context("reading the encoded PNG back failed")?;
    let mut bytes = vec![0u8; len as usize];
    reader
        .ReadBytes(&mut bytes)
        .context("DataReader::ReadBytes failed")?;

    std::fs::write(path, &bytes).with_context(|| format!("writing {} failed", path.display()))?;
    crate::log_line!("capture: wrote {} ({} bytes)", path.display(), bytes.len());
    Ok(())
}

/// Builds a fresh, unique path under the OS temp directory for one capture.
///
/// Never a fixed name: it would race a double hotkey press, and it would let a
/// stale capture from a previous run be mistaken for a fresh one.
///
/// This is a deliberate duplication of `capture::macos`'s function of the same
/// name, not an oversight — hoisting it into `capture::mod` would touch the
/// working macOS arm and its four tests for twelve lines.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unique_capture_path_never_repeats() {
        assert_ne!(unique_capture_path(), unique_capture_path());
    }

    #[test]
    fn unique_capture_path_is_a_png_in_the_temp_dir() {
        let p = unique_capture_path();
        assert!(p.starts_with(std::env::temp_dir()), "{}", p.display());
        assert_eq!(p.extension().and_then(|e| e.to_str()), Some("png"));
    }

    #[test]
    fn an_all_black_region_is_flagged() {
        assert!(looks_protected(&[0u8; 4 * 6]));
    }

    /// Only the colour channels count: `BitBlt` leaves the fourth byte
    /// undefined, so an opaque-looking alpha must not make a black region read
    /// as ordinary content.
    #[test]
    fn a_single_non_black_pixel_clears_the_flag() {
        let mut buf = vec![0u8; 4 * 6];
        buf[4 * 3 + 1] = 1; // one green pixel
        assert!(!looks_protected(&buf));

        let mut alpha_only = vec![0u8; 4 * 6];
        for px in alpha_only.chunks_exact_mut(4) {
            px[3] = 0xFF;
        }
        assert!(looks_protected(&alpha_only));
    }
}
