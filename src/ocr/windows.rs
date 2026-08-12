//! Windows OCR via `Windows.Media.Ocr`.
//!
//! In-process, unlike the macOS path: `Windows.Media.Ocr` has usable Rust
//! bindings through the `windows` crate, so there is no equivalent of the
//! Swift `aloud-ocr` helper subprocess to ship.
//!
//! # The engine is inbox; the *language models* are not
//!
//! `OcrEngine` ships in `windows.dll` on every Windows 10/11 SKU — no Windows
//! App SDK, no redistributable. But each language is a **Feature on Demand**
//! the user installs (`Language.OCR~~~<tag>~0.0.1.0`). This is the sharpest
//! collision with hard constraint 1 ("zero runtime system dependencies") in
//! the whole port, and the prototype's answer is to constrain the feature to
//! what the machine already has rather than instruct the user to install
//! anything.
//!
//! Measured on the development machine, 2026-08-11: **`en-US` and `ru` only**.
//! No `de-DE`. No Ukrainian at any tag — `uk-UA` does not exist as a Windows
//! OCR FOD at all, which is verified rather than assumed.
//!
//! # `TryCreateFromLanguage` fails by returning **null**, silently
//!
//! Not an exception, not an `Err` — a null. `AvailableRecognizerLanguages`
//! likewise returns an *empty list* rather than throwing. windows-rs turns
//! that null into an `Err` whose `code()` is `S_OK` and whose `Display` reads
//! *"The operation completed successfully. (0x00000000)"* — an error that
//! claims success. [`is_null_return`] is the discriminator.
//!
//! # COM/WinRT initialisation
//!
//! windows-rs does **not** initialise COM. See [`Mta`] and [`pin_process_mta`].
//! Relying on implicit initialisation has produced real segfaults when another
//! crate called `CoUninitialize` (windows-rs#1169).

use super::OcrEngine;
use anyhow::{anyhow, bail, Context, Result};
use std::path::Path;

use windows::core::HSTRING;
use windows::Globalization::Language;
use windows::Graphics::Imaging::{
    BitmapAlphaMode, BitmapDecoder, BitmapInterpolationMode, BitmapPixelFormat, BitmapTransform,
    ColorManagementMode, ExifOrientationMode, SoftwareBitmap,
};
// The WinRT class shares its name with THIS crate's trait (`super::OcrEngine`).
// The alias is mandatory, not style — without it the file will not compile.
use windows::Media::Ocr::{OcrEngine as WinRtOcr, OcrResult};
use windows::Storage::Streams::IRandomAccessStream;
use windows::Win32::Foundation::{RPC_E_CHANGED_MODE, S_OK};
use windows::Win32::System::Com::CoIncrementMTAUsage;
use windows::Win32::System::WinRT::{
    CreateRandomAccessStreamOnFile, RoInitialize, RoUninitialize, RO_INIT_MULTITHREADED,
};

/// A null WinRT return, disguised as an `Err` that reports success.
///
/// `Type::from_abi` maps a null interface pointer to `Err(Error::empty())`,
/// and `Error::empty()`'s `code()` is `HRESULT(0)` — i.e. `S_OK`. A *real*
/// failure short-circuits earlier with a negative HRESULT and never reaches
/// `from_abi`, so `code() == S_OK` uniquely means "returned S_OK with a null
/// pointer": the documented failure mode of `TryCreateFrom*`.
fn is_null_return(e: &windows::core::Error) -> bool {
    e.code() == S_OK
}

/// Puts the calling thread in the MTA for as long as the guard lives.
///
/// `RPC_E_CHANGED_MODE` means another crate already put this thread in a
/// *different* apartment — tao does exactly that to the Tauri main thread
/// (`CoInitializeEx(None, COINIT_APARTMENTTHREADED)`). That is not fatal:
/// `Windows.Media.Ocr.OcrEngine` is `ThreadingModel.Both` and agile, so the
/// calls still work from an STA.
///
/// What must never happen is `RoUninitialize` on an apartment we did not
/// create — that decrements someone else's refcount and unloads COM under
/// them, which is windows-rs#1169. Hence `owned`.
struct Mta {
    owned: bool,
}

impl Mta {
    fn enter() -> Result<Self> {
        match unsafe { RoInitialize(RO_INIT_MULTITHREADED) } {
            // S_OK and S_FALSE both land here, and both incremented.
            Ok(()) => Ok(Self { owned: true }),
            Err(e) if e.code() == RPC_E_CHANGED_MODE => {
                crate::log_line!(
                    "ocr: this thread is already in a single-threaded apartment; \
                     continuing without taking one (OcrEngine is agile)"
                );
                Ok(Self { owned: false })
            }
            Err(e) => Err(e).context("RoInitialize(RO_INIT_MULTITHREADED) failed"),
        }
    }
}

impl Drop for Mta {
    fn drop(&mut self) {
        if self.owned {
            unsafe { RoUninitialize() };
        }
    }
}

/// Keeps a process-wide MTA alive from first construction to exit.
///
/// Without it, the per-call `RoInitialize`/`RoUninitialize` pair on a
/// short-lived worker thread is the *only* MTA in the process, so combase and
/// the OCR model DLLs are loaded and unloaded on every hotkey press — slow,
/// and it is the DLL-unload-under-a-live-WinRT-object shape of #1169 again.
///
/// The cookie is deliberately never handed to `CoDecrementMTAUsage`:
/// `CO_MTA_USAGE_COOKIE` is a raw-pointer newtype with no `Drop`, so dropping
/// it leaks the usage count for the process lifetime, which is the intent. It
/// is also `!Send`, so it must not become a field of [`WindowsOcr`].
fn pin_process_mta() {
    static ONCE: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    ONCE.get_or_init(|| match unsafe { CoIncrementMTAUsage() } {
        Ok(_cookie) => crate::log_line!("ocr: process MTA pinned"),
        Err(e) => crate::log_line!("ocr: CoIncrementMTAUsage failed: {e} — continuing"),
    });
}

/// PowerToys' 1.5x pre-upscale, generalised to guard BOTH dimensions.
///
/// Windows OCR returns *nothing at all* on an image that is too small, and a
/// dragged rectangle is Aloud's primary gesture — so the small case is the
/// common case, not an edge case. PowerToys tests only
/// `bmp.Width * 1.5 > OcrEngine.MaxImageDimension`; a tall, narrow selection
/// (a sidebar, a code gutter) would then blow the ceiling on *height*. This
/// tests both and clamps to the tighter of the two. It also handles the
/// reverse case — a source already larger than the ceiling is scaled DOWN,
/// because `RecognizeAsync` rejects an oversized bitmap outright.
///
/// # 1.5× is not enough, and the floor is a hard 40 px — measured 2026-08-12
///
/// A flat 1.5× leaves the common case broken. `Windows.Media.Ocr` needs **at
/// least 40 px on each side of the image it is handed**, and that threshold is
/// sharp rather than gradual — measured on this machine against identical text:
///
/// | region dragged | after 1.5× | recognised |
/// |---|---|---|
/// | 360 × 26 | 540 × **39** | **nothing** |
/// | 360 × 27 | 540 × **41** | 20 chars |
///
/// One pixel of source height is the difference between silence and text. A
/// single line of UI text is ~13–15 px tall, so *every* single-line drag —
/// which is the gesture this app exists for — landed under the floor and came
/// back empty. On 30 tight crops of real screen text, a flat 1.5× recognised
/// **1**.
///
/// So the scale is now whatever it takes to clear the floor on the *smaller*
/// side, or 1.5×, whichever is larger.
///
/// **The floor is exclusive, hence 48 rather than 40.** Aiming at exactly 40
/// was tried first and did not work: regions scaled to land on 40 exactly still
/// recognised nothing (49×21 → 93×40 → empty, 23×13 → 71×40 → empty). Overshoot
/// is free, so there is margin rather than a fight over the boundary.
///
/// Measured before and after, same text, same coordinates:
///
/// | dragged | before | after |
/// |---|---|---|
/// | 360 × 21 (one line) | 540 × 32 → **0 chars** | — |
/// | 360 × 19 (one line) | — | 909 × 48 → **39 chars**, 3 runs of 3 |
/// | 360 × 26 (one line) | 540 × 39 → **0 chars** | 665 × 48 → **39 chars** |
/// | 360 × 64 (two lines) | — | **53 chars** |
///
/// **What this does not fix**, so nobody expects more of it than it gives:
/// a pixel-tight crop with no margin at all around the glyphs still returns
/// nothing (360 × 13, the exact glyph box, is empty even at 1329 × 48), and so
/// does a single short word (49 × 21, 53 × 23). Clearing the floor is necessary
/// and not sufficient — below roughly a line-with-margin, `Windows.Media.Ocr`
/// wants more than pixels. A bundled engine is the answer to that residue if it
/// is ever worth the ~20 MB; it measured 30/30 on exactly these crops.
///
/// **Scaling, not padding.** The obvious alternative is to paste the crop onto a
/// 40×40 canvas, which preserves glyph size. It is rejected: padding a region
/// smaller than the floor means pulling in the neighbouring screen pixels, and
/// in a dense UI that means OCR reads text the user did not select — Aloud
/// speaking something it was not pointed at. Scaling touches no pixel outside
/// the selection, and enlarging the glyphs helps recognition rather than
/// hurting it.
///
/// The ceiling still wins where the two conflict: an extremely elongated region
/// (say 9000 × 10) cannot clear the floor without blowing `MaxImageDimension`,
/// so it is left under it and will recognise nothing. That is unavoidable and
/// is not new.
///
/// Pure: no OS, no WinRT, unit-testable. `ceiling` is bound once and used for
/// both the scale divisor and the clamp bound, so a `max_dim` of 0 cannot make
/// `clamp` panic on `min > max`.
fn upscale_dimensions(w: u32, h: u32, max_dim: u32) -> (u32, u32) {
    const TARGET: f64 = 1.5;
    /// The measured floor, per side, of the image handed to `RecognizeAsync`,
    /// plus margin. Aiming at exactly 40 was tried and did not work: regions
    /// scaled to land on 40 exactly still recognised nothing, so the floor is
    /// not inclusive. The extra pixels cost nothing.
    const MIN_DIM: f64 = 48.0;

    let ceiling = max_dim.max(1);
    // Enough to lift the SHORTER side over the floor — that is the side that
    // fails first, and for a dragged line of text it is always the height.
    let to_clear_floor = MIN_DIM / w.min(h).max(1) as f64;
    let wanted = TARGET.max(to_clear_floor);
    let scale = wanted.min(ceiling as f64 / w.max(h).max(1) as f64);

    let dst_w = ((w as f64 * scale).round() as u32).clamp(1, ceiling);
    let dst_h = ((h as f64 * scale).round() as u32).clamp(1, ceiling);
    (dst_w, dst_h)
}

/// OCR through the inbox `Windows.Media.Ocr` engine.
///
/// Holds no WinRT object. Both fields are plain data resolved once at
/// construction, which is what makes this trivially `Send + Sync` as the
/// `OcrEngine` supertrait requires — a WinRT interface pointer is neither, and
/// smuggling one in here would need an `AgileReference` for no benefit, since
/// `TryCreateFromLanguage` is cheap.
pub struct WindowsOcr {
    language_tag: String,
    max_image_dimension: u32,
}

impl WindowsOcr {
    /// Fails loudly when no recognizer can be constructed at all, so the
    /// condition surfaces at startup rather than as an empty transcription on
    /// the first hotkey press.
    ///
    /// Note this runs inside Tauri's `setup()` via `Ocr::new()?`, so an `Err`
    /// here means the app does not start.
    pub fn new() -> Result<Self> {
        pin_process_mta();
        // On the Tauri main thread this takes the RPC_E_CHANGED_MODE arm.
        let _mta = Mta::enter()?;

        // Enumerate and log. An empty list is `Ok(view)` with Size() == 0,
        // never an Err — only Size() tells the truth.
        let available = WinRtOcr::AvailableRecognizerLanguages()
            .context("OcrEngine.AvailableRecognizerLanguages failed")?;
        let count = available
            .Size()
            .context("AvailableRecognizerLanguages.Size failed")?;

        let mut tags: Vec<String> = Vec::with_capacity(count as usize);
        for i in 0..count {
            let lang = available.GetAt(i)?;
            let tag = lang.LanguageTag()?.to_string_lossy();
            let name = lang.DisplayName()?.to_string_lossy();
            crate::log_line!("ocr: recognizer available: {tag} ({name})");
            tags.push(tag);
        }
        if tags.is_empty() {
            bail!(
                "Windows.Media.Ocr has no recognizer languages installed. Add one in \
                 Settings > Time & language > Language & region > (language) > Language options \
                 > Optical character recognition."
            );
        }

        // Default via the user profile. A null here is Err-with-code-S_OK.
        let engine = match WinRtOcr::TryCreateFromUserProfileLanguages() {
            Ok(e) => e,
            Err(e) if is_null_return(&e) => {
                // Reported loudly, but not fatal. The strict reading of the
                // brief ("treat a null from either as a real reported error")
                // would bail! here; this falls back instead, because the
                // contract is "fail loudly when no recognizer can be
                // constructed *at all*" and a non-empty enumeration proves one
                // can. This is the case where the user's display language has
                // no OCR FOD while an installed recognizer sits right there.
                // A hard bail! would mean Aloud refuses to start on such a box.
                crate::log_line!(
                    "ocr: TryCreateFromUserProfileLanguages returned NULL — no profile \
                     language has an OCR recognizer. Falling back to {}",
                    tags[0]
                );
                let lang = Language::CreateLanguage(&HSTRING::from(tags[0].as_str()))
                    .with_context(|| format!("Language::CreateLanguage({}) failed", tags[0]))?;
                WinRtOcr::TryCreateFromLanguage(&lang).map_err(|e| {
                    if is_null_return(&e) {
                        anyhow!(
                            "OcrEngine.TryCreateFromLanguage({}) returned NULL even though \
                             that tag is listed in AvailableRecognizerLanguages",
                            tags[0]
                        )
                    } else {
                        anyhow::Error::from(e)
                    }
                })?
            }
            Err(e) => return Err(e).context("OcrEngine.TryCreateFromUserProfileLanguages failed"),
        };

        let language_tag = engine
            .RecognizerLanguage()?
            .LanguageTag()?
            .to_string_lossy();
        let max_image_dimension =
            WinRtOcr::MaxImageDimension().context("OcrEngine.MaxImageDimension failed")?;

        crate::log_line!(
            "ocr: Windows.Media.Ocr ready, language={language_tag}, \
             MaxImageDimension={max_image_dimension}, {} recognizer(s) installed",
            tags.len()
        );

        // `engine` is dropped here on purpose. Nothing WinRT survives this call.
        Ok(Self {
            language_tag,
            max_image_dimension,
        })
    }
}

/// The whole WinRT half, run on a thread that is guaranteed apartment-free.
fn recognise_blocking(
    path: &HSTRING,
    language_tag: &str,
    max_image_dimension: u32,
) -> Result<String> {
    let _mta = Mta::enter()?;

    // 0 == FileAccessMode::Read == STGM_READ, correct under either reading.
    let stream: IRandomAccessStream = unsafe { CreateRandomAccessStreamOnFile(path, 0) }
        .with_context(|| format!("cannot open {path} for OCR"))?;

    let decoder = BitmapDecoder::CreateAsync(&stream)
        .context("BitmapDecoder::CreateAsync failed to start")?
        .get()
        .context("BitmapDecoder::CreateAsync failed — is the capture a valid PNG?")?;

    let (src_w, src_h) = (decoder.PixelWidth()?, decoder.PixelHeight()?);
    if src_w == 0 || src_h == 0 {
        bail!("captured image is {src_w}x{src_h} — nothing to recognise");
    }
    let (dst_w, dst_h) = upscale_dimensions(src_w, src_h, max_image_dimension);

    // The scale happens INSIDE the decode, so there is never an intermediate
    // full-size SoftwareBitmap. Both scaled dimensions must be set.
    let transform = BitmapTransform::new()?;
    transform.SetScaledWidth(dst_w)?;
    transform.SetScaledHeight(dst_h)?;
    transform.SetInterpolationMode(BitmapInterpolationMode::Fant)?;

    let bitmap: SoftwareBitmap = decoder
        .GetSoftwareBitmapTransformedAsync(
            BitmapPixelFormat::Bgra8,
            BitmapAlphaMode::Premultiplied,
            &transform,
            // BitBlt PNGs carry no EXIF; be deterministic.
            ExifOrientationMode::IgnoreExifOrientation,
            // Screen pixels are already sRGB.
            ColorManagementMode::DoNotColorManage,
        )?
        .get()
        .context("decoding the captured PNG failed")?;

    crate::log_line!(
        "ocr: decoded {src_w}x{src_h} -> {dst_w}x{dst_h} (MaxImageDimension={max_image_dimension})"
    );

    // The file handle must be released before the caller's delete-on-drop runs,
    // or `remove_file` fails with a sharing violation — and that Drop discards
    // its error, so the failure is SILENT and a photograph of the user's screen
    // survives in the temp directory. SoftwareBitmap owns its own pixels, so
    // both of these are safe to release now.
    drop(decoder);
    drop(stream);

    let lang = Language::CreateLanguage(&HSTRING::from(language_tag))?;
    let engine = WinRtOcr::TryCreateFromLanguage(&lang).map_err(|e| {
        if is_null_return(&e) {
            anyhow!(
                "OcrEngine.TryCreateFromLanguage({language_tag}) returned NULL — \
                 the recognizer that existed at startup is gone"
            )
        } else {
            anyhow::Error::from(e)
        }
    })?;

    let result = engine
        .RecognizeAsync(&bitmap)
        .context("OcrEngine.RecognizeAsync failed to start")?
        .get()
        .context("OcrEngine.RecognizeAsync failed")?;

    assemble(&result)
}

/// `OcrResult` -> `String`, joined with `"\n"`.
///
/// The separator is not a taste call. The macOS sibling joins Vision's lines
/// with `"\n"`, and `text::normalize`'s hyphen-rejoin, soft-break and
/// page-number rules are all built around `\n`. Joining with a space would
/// silently disable them and the two platforms would stop sounding identical.
///
/// `OcrResult::Text()` is deliberately unused: its line separator is
/// undocumented, which makes it convention rather than contract.
fn assemble(result: &OcrResult) -> Result<String> {
    let view = result.Lines().context("OcrResult.Lines failed")?;
    let count = view.Size()?;
    let mut lines: Vec<String> = Vec::with_capacity(count as usize);

    for i in 0..count {
        let line = view.GetAt(i)?;
        let mut text = line.Text()?.to_string_lossy();

        // Defensive: OcrLine.Text is the documented way to get a line, but if
        // it ever comes back empty on a line that has words, rebuild it.
        if text.trim().is_empty() {
            let words = line.Words()?;
            let n = words.Size()?;
            if n > 0 {
                let mut parts = Vec::with_capacity(n as usize);
                for w in 0..n {
                    parts.push(words.GetAt(w)?.Text()?.to_string_lossy());
                }
                text = parts.join(" ");
            }
        }

        if !text.trim().is_empty() {
            lines.push(text);
        }
    }

    Ok(lines.join("\n").trim_end().to_string())
}

impl OcrEngine for WindowsOcr {
    /// An empty recognition is `Ok(String::new())`, never `Err` — the caller
    /// turns that into "no text found". The `Err` cases are: no recognizer,
    /// decode failure, `RecognizeAsync` failure. Not "found nothing".
    fn recognise(&self, image_path: &Path) -> Result<String> {
        let path = HSTRING::from(image_path);
        let tag = self.language_tag.as_str();
        let max_dim = self.max_image_dimension;

        // The WinRT half runs on its own thread, ALWAYS. `IAsyncOperation::get()`
        // blocks on WaitForSingleObject(INFINITE) with no message pump; on an STA
        // that is a permanent deadlock, and this method is reachable from the
        // Tauri main thread, which tao has put in an STA. A fresh thread is
        // apartment-free, so RoInitialize(RO_INIT_MULTITHREADED) succeeds and
        // completions land on an MTA pool thread. Cost is one thread spawn
        // (~50us) against tens of ms of OCR.
        let joined = std::thread::scope(|scope| {
            scope
                .spawn(|| recognise_blocking(&path, tag, max_dim))
                .join()
        });

        let text = match joined {
            Ok(inner) => inner?,
            Err(_) => bail!("ocr: the Windows.Media.Ocr worker thread panicked"),
        };

        crate::log_line!("ocr: recognised text length={} chars", text.chars().count());
        Ok(text)
    }
}

#[cfg(test)]
mod tests {
    use super::upscale_dimensions;

    #[test]
    fn small_region_is_upscaled_1_5x() {
        assert_eq!(upscale_dimensions(100, 50, 10_000), (150, 75));
    }

    #[test]
    fn wide_source_clamps_to_the_ceiling() {
        assert_eq!(upscale_dimensions(8_000, 100, 10_000), (10_000, 125));
    }

    /// The case PowerToys gets wrong: it guards only width, so a tall narrow
    /// selection blows the ceiling on height.
    #[test]
    fn tall_source_clamps_on_height() {
        assert_eq!(upscale_dimensions(100, 8_000, 10_000), (125, 10_000));
    }

    #[test]
    fn oversized_source_is_scaled_down() {
        let (w, h) = upscale_dimensions(12_000, 100, 10_000);
        assert!(w <= 10_000 && h >= 1);
    }

    /// Was `(2, 2)` under a flat 1.5×, which is far under the 40 px floor and
    /// so recognised nothing. Clearing the floor is the point of the change.
    #[test]
    fn never_returns_zero() {
        assert_eq!(upscale_dimensions(1, 1, 10_000), (48, 48));
    }

    /// The regression guard for the defect this floor exists to fix: a single
    /// dragged line of UI text. 13 px is a realistic line height at 100%.
    #[test]
    fn a_single_line_of_text_clears_the_ocr_floor() {
        for (w, h) in [(360, 26), (360, 13), (620, 20), (200, 15)] {
            let (dw, dh) = upscale_dimensions(w, h, 10_000);
            assert!(
                dw >= 40 && dh >= 40,
                "{w}x{h} -> {dw}x{dh}, under the 40 px floor: OCR returns nothing"
            );
        }
    }

    /// Both sides count, not just the shorter one at the time of writing: a
    /// narrow *column* fails on width exactly as a line fails on height.
    #[test]
    fn a_narrow_column_clears_the_floor_on_width() {
        let (dw, dh) = upscale_dimensions(26, 300, 10_000);
        assert!(
            dw >= 40,
            "26x300 -> {dw}x{dh}, still under the floor on width"
        );
    }

    /// Anything already comfortably over the floor keeps the plain 1.5×, so the
    /// floor cannot quietly become a magnifier for ordinary selections.
    #[test]
    fn a_comfortable_region_is_untouched_by_the_floor() {
        assert_eq!(upscale_dimensions(400, 200, 10_000), (600, 300));
    }

    /// The ceiling still wins. An extremely elongated region cannot clear the
    /// floor without blowing MaxImageDimension; it is left under it rather than
    /// producing an oversized bitmap `RecognizeAsync` would reject outright.
    #[test]
    fn the_ceiling_outranks_the_floor() {
        let (dw, dh) = upscale_dimensions(9_000, 10, 10_000);
        assert!(
            dw <= 10_000,
            "must never exceed MaxImageDimension, got {dw}"
        );
        assert!(dh < 40, "this case genuinely cannot clear the floor: {dh}");
    }

    /// A zero ceiling must not make `clamp` panic on `min > max`.
    #[test]
    fn zero_max_dimension_does_not_panic() {
        assert_eq!(upscale_dimensions(100, 50, 0), (1, 1));
    }
}
