> **Moved into this repo 2026-08-12, from a loose folder on the PC's build disk
> (`X:\dev\aloud-m6-design`) that was not under version control and had no copy
> anywhere.** The M6 Windows implementation leaned on these four documents
> heavily; losing that disk would have lost the entire design rationale behind
> `src/capture/windows/overlay.rs` and `src/ocr/windows.rs`.
>
> **Status: PRE-IMPLEMENTATION. Written 2026-08-11, before any of this ran on
> Windows.** Kept as written rather than corrected, per the `docs/` rule that
> superseded findings are annotated and not rewritten. Implemented substantially as written; see the commit 3614b9f body for the one judgement call that departed from it.
>
> What actually shipped, and every place reality disagreed with this document,
> is recorded in `docs/2026-08-11-windows-port.md` and in
> `BKM/PC-Queue/TASK-M6-aloud-windows-prototype-result.md` in the Second Brain
> repo. **Read this for the reasoning; read those for the outcome.**

---
# Unit 4 of 4 — OCR through Windows.Media.Ocr (brief §6.3, OCR half)

## SUMMARY
`WindowsOcr::new()` enumerates `AvailableRecognizerLanguages`, resolves a default engine, and stores only plain data (a BCP-47 tag string + `MaxImageDimension`) — no WinRT object is ever held across a thread, which sidesteps every apartment question by construction. `recognise()` runs the whole WinRT pipeline on a freshly spawned scoped thread that owns an MTA, because `IAsyncOperation::get()` in windows-future 0.2.1 blocks on `WaitForSingleObject(INFINITE)` with no message pump and would deadlock forever on the STA that tao puts the Tauri main thread into. The PNG is opened with `CreateRandomAccessStreamOnFile`, then decoded, upscaled ~1.5x and converted to Bgra8/Premultiplied in a single `BitmapDecoder::GetSoftwareBitmapTransformedAsync` call driven by a `BitmapTransform`. The single biggest concrete finding: the declared 13-feature list **cannot compile this at all** — `BitmapDecoder`'s only constructors are `#[cfg(feature = "Storage_Streams")]`, so `Storage_Streams` must be added, and it fails as a *missing method*, not the "plain unresolved import" the brief promises.

## FEATURES NEEDED
["Storage_Streams"]

## TRAPS
- **`Storage_Streams` is missing from the declared 13 features, and it fails as a MISSING METHOD, not an unresolved import.** Every `BitmapDecoder::Create*Async` is `#[cfg(feature = "Storage_Streams")]` (Graphics/Imaging/mod.rs:223,233), so without it `BitmapDecoder` has literally no constructor. Error text will be `no function or associated item named 'CreateAsync' found for struct 'BitmapDecoder'`. The brief's claim that a missing feature always shows up as a plain unresolved import is true for modules and false for gated methods.
- **Name collision: `windows::Media::Ocr::OcrEngine` vs Aloud's own `crate::ocr::OcrEngine` trait.** The stub already does `use super::OcrEngine;`. Importing the WinRT class unaliased is an immediate E0252. Alias it (`as WinRtOcr`).
- **The null return is an `Err` whose message says the operation SUCCEEDED.** `Type::from_abi` for interfaces returns `Err(Error::empty())` on a null pointer (windows-core-0.61.2/src/type.rs:43-51), and `Error::empty().code()` is remapped back to `HRESULT(0)` (windows-result-0.3.4/src/error.rs:82,131), so `Display` renders `"The operation completed successfully. (0x00000000)"`. Never let that reach the user — intercept with `e.code() == S_OK`.
- **`AvailableRecognizerLanguages` fails the OTHER way.** It returns a real non-null `IVectorView<Language>` that is merely empty, so it is `Ok(view)` and only `view.Size()? == 0` reveals it. Testing for `Err` there will silently pass on a machine with zero OCR packs.
- **Never `RoUninitialize` after `RPC_E_CHANGED_MODE`.** tao 0.35.3 puts the Tauri main thread in an STA (`CoInitializeEx(None, COINIT_APARTMENTTHREADED)`, platform_impl/windows/window.rs:1450) and wry does the same (webview2/mod.rs:115). Balancing an apartment you did not create decrements tao's refcount and unloads COM under wry — the exact segfault in windows-rs#1169 the stub's doc cites. Note `RoInitialize` returning S_FALSE *does* map to `Ok(())` via `HRESULT::ok()` and *does* need balancing; only the `Err(RPC_E_CHANGED_MODE)` arm must not be balanced.
- **`IAsyncOperation::get()` on an STA thread deadlocks permanently.** Its wait is `WaitForSingleObject(handle, 0xFFFFFFFF)` inside `Waiter::drop` (windows-future-0.2.1/src/waiter.rs:37) with no message pump and no timeout. Run the WinRT half on a thread you own that is in the MTA.
- **`.get()` has no timeout at all.** A wedged OCR engine hangs the read thread forever, and `App::read_region`'s `Duration::ZERO` busy-guard then drops every subsequent hotkey press silently. Not fixable in the prototype without restructuring; log before and after so the log shows the hang.
- **`windows_future` is NOT re-exported by `windows` 0.61.3.** There is no `windows::Foundation::IAsyncOperation` (grep the whole src tree — Foundation/mod.rs has no such type and no `pub use`). Never annotate the intermediate; just chain `.get()`, which is an inherent method and needs no import.
- **Release the `IRandomAccessStream` and `BitmapDecoder` before returning.** `read_region`'s `DeleteOnDrop` (src/app/actions.rs:59) does `let _ = std::fs::remove_file(...)` — on Windows that fails with a sharing violation while a handle is open, and the failure is swallowed. Result: a screenshot of the user's screen (possibly a bank statement — the trait doc says so) survives in the temp directory. Never cache the stream or decoder on `self`.
- **`CO_MTA_USAGE_COOKIE` is `!Send`** (a raw-pointer newtype, Win32/System/Com/mod.rs:1961) so it cannot be a field of `WindowsOcr`, which must be `Send + Sync`. It also has no `Drop` — only a `windows_core::Free` impl that only `Owned<T>` consumes — so dropping it leaks the usage count, which is exactly what you want. Also: 0.61.3's signature is `CoIncrementMTAUsage() -> Result<CO_MTA_USAGE_COOKIE>`, not an out-param.
- **Set BOTH `SetScaledWidth` and `SetScaledHeight`.** `BitmapTransform` does not derive the missing one from aspect ratio; an unset dimension stays 0 and you get either an unchanged or a rejected decode.
- **PowerToys' guard only checks Width** (`if (bmp.Width * 1.5 > OcrEngine.MaxImageDimension)`, OcrExtensions.cs:77). A tall, narrow selection — a sidebar, a code gutter, a phone-shaped column — blows the ceiling on height. Guard both dimensions; the deviation is deliberate and should be named in the result.
- **Do not mirror `tests/ocr_macos.rs`'s umlaut assertions.** This box has en-US and ru only; `tests/fixtures/german_form.png` would go through the English recognizer and lose `ü`/`ä`. The test would fail for a language-pack reason and be misread as a code bug.
- **`\n`, not a space, between lines.** `normalize_ocr` (src/text/normalize.rs) keys hyphen rejoin on `(\w)-\n(\w)`, soft-break joining on a single `\n`, and page-number stripping on `\n\n`. Joining with a space silently disables all three and breaks parity with the macOS helper, which does `lines.joined(separator: "\n")`.
- **Empty recognition must be `Ok(String::new())`, not `Err`.** The pipeline has `Outcome::Empty` for exactly this and turning it into an error produces a bogus notification on any drag over blank space.

## UNCERTAINTIES
- **`CreateRandomAccessStreamOnFile`'s `accessmode: u32`.** windows-rs types it as a bare `u32` because the Win32 metadata carries no enum, and Microsoft's page documents it only as "the access mode". `0` is safe under BOTH candidate interpretations (`FileAccessMode::Read == 0`, Storage/mod.rs:815; `STGM_READ == 0`), so read-only is unambiguous — but I could not verify the mapping for non-zero values. **Settle it:** if the call returns `E_INVALIDARG` (0x80070057), switch to the fully-documented fallback, which needs no extra feature beyond the same `Storage_Streams` (it pulls `Storage` transitively): `StorageFile::GetFileFromPathAsync(&HSTRING::from(image_path))?.get()?` then `.OpenAsync(FileAccessMode::Read)?.get()?` -> `IRandomAccessStream` (Storage/mod.rs:4446 and :1534). That route goes through the broker and is slower, so only take it if the fast one fails.
- **Cost of `TryCreateFromLanguage` per call.** This design re-creates the `OcrEngine` on every `recognise()` so that no WinRT object ever crosses a thread. PowerToys does the same per extraction, so it is the mainstream pattern, but I have no measurement. **Settle it:** log `Instant::now()` around the `TryCreateFromLanguage` call in the first build. If it exceeds ~20 ms, cache it as `OnceLock<windows::core::AgileReference<WinRtOcr>>` — `AgileReference::new(&engine)` / `.resolve()` (windows-core-0.61.2/src/agile_reference.rs, reachable as `windows::core::AgileReference` via the `#[cfg(windows)] include!("windows.rs")` in that crate's lib.rs) — which is `Send + Sync` and legal across apartments. Do not cache a bare `OcrEngine` field just because windows-rs marks it `Send`.
- **Order of `OcrResult::Lines`.** Undocumented; the RecognizeAsync reference page has no remarks section at all. Single-column screenshots come back top-to-bottom in practice and PowerToys relies on that. **Settle it:** OCR a two-column PDF page or a newspaper-layout screenshot and read the log. If it interleaves, sort lines by `line.Words()?.GetAt(0)?.BoundingRect()?.Y` (Foundation::Rect { X,Y,Width,Height: f32 }) — and remember those coordinates are in the UPSCALED bitmap space.
- **`BitmapInterpolationMode::Fant` vs `Cubic` for the 1.5x upscale.** Fant is WIC's highest-quality mode and my default recommendation, but PowerToys upscales through System.Drawing rather than WIC so there is no like-for-like precedent, and I have no OCR-accuracy measurement either way. **Settle it:** run brief question 22 (smallest region that still returns text) twice, once with each mode, and report both numbers. Changing it is a one-line edit.
- **`ColorManagementMode::DoNotColorManage` vs `ColorManageToSRgb`.** Screen pixels from `BitBlt` are already sRGB so colour management should be a no-op cost; Microsoft's own OCR sample uses `ColorManageToSRgb`. Untested, both compile, neither should change recognition. Low stakes — mention which you shipped.
- **Whether the Tauri main thread is already STA at the exact moment `Ocr::new()` runs.** tao's `CoInitializeEx` is lazy, inside a `thread_local` fired by `com_initialized()` on window creation; wry's fires in webview construction. Whether either has run before `setup()` reaches line 1096 depends on Tauri's internal ordering, which I did not trace. It does not matter — `Mta::enter()` handles both arms — but it means you may see either "process MTA pinned" alone or that plus the "already in a single-threaded apartment" line. Report which appeared; it is a free datapoint about Tauri's startup order.
- **Whether the `report-and-fall-back` behaviour on a null `TryCreateFromUserProfileLanguages` is what the owner wants.** The stub's doc says "treat a null from either as a real, reported error"; its `new()` doc says "fail loudly when no recognizer can be constructed **at all**". I read those as report-loudly-then-fall-back-to-the-first-available-recognizer, and hard-fail only when the available list is empty. If the owner reads it as hard-fail-on-null, delete one match arm. Flag the choice in the result rather than deciding it silently.
- **`std::thread::scope` per call vs inline.** I could not measure the spawn cost against `tests/latency_budget.rs`'s budget on this machine (no cargo runs permitted this session). The scoped thread is the safe default; if it shows up in the budget, inline `recognise_blocking` is correct *only* because `spawn_read_region` (src/bin/aloud.rs:198) already spawns — and that becomes a caller dependency the seam previously did not have. Say which you shipped and why.

## SPEC
## Unit 4 — `src/ocr/windows.rs`, full implementation spec

Everything below was checked by reading `C:\Users\Andrew\.cargo\registry\src\index.crates.io-1949cf8c6b5b557f\windows-0.61.3\` and its sibling crates (`windows-core-0.61.2`, `windows-result-0.3.4`, `windows-strings-0.4.2`, `windows-collections-0.2.0`, `windows-future-0.2.1`), not from memory. File+line citations are given for the load-bearing claims so they can be re-checked without a compile.

---

### 0. THE ONE THING THAT BLOCKS THE BUILD — a 14th Cargo feature

`Cargo.toml` must gain **`Storage_Streams`** to the `[target.'cfg(windows)'.dependencies] windows` feature list.

Reason, verified: in `windows-0.61.3/src/Windows/Graphics/Imaging/mod.rs`, **every** `BitmapDecoder` constructor is feature-gated:

```rust
// line 223
#[cfg(feature = "Storage_Streams")]
pub fn CreateAsync<P0>(stream: P0) -> windows_core::Result<windows_future::IAsyncOperation<BitmapDecoder>>
where P0: windows_core::Param<super::super::Storage::Streams::IRandomAccessStream>
// line 233
#[cfg(feature = "Storage_Streams")]
pub fn CreateWithIdAsync<P1>(decoderid: windows_core::GUID, stream: P1) -> ...
```

Without `Storage_Streams` there is **no way to construct a `BitmapDecoder`**, and therefore no way to produce a `SoftwareBitmap` from a file. `Storage_Streams = ["Storage"]` (Cargo.toml line 308), so it transitively enables `Storage` too — one name, two namespaces.

**Correction to the brief.** §6.3 says "a missing feature shows up as a plain 'unresolved import', not as anything subtle". That is true for *modules*, false for `#[cfg]`-gated **methods**. This one surfaces as `no function or associated item named 'CreateAsync' found for struct 'BitmapDecoder'` — which reads like a wrong API path and will send you hunting through docs. Record this in the result.

Also update the `Cargo.toml` comment: "All thirteen were checked…" → fourteen, and say why the fourteenth exists.

No other feature is needed. Audit of every API named in this spec against the declared list:

| API | Feature | Status |
|---|---|---|
| `windows::core::{HSTRING, Error, HRESULT, AgileReference}` | none (windows-core is unconditional) | ✓ |
| `windows::Globalization::Language` | `Globalization` | ✓ declared |
| `windows::Graphics::Imaging::{BitmapDecoder, BitmapTransform, SoftwareBitmap, BitmapPixelFormat, BitmapAlphaMode, BitmapInterpolationMode, ExifOrientationMode, ColorManagementMode}` | `Graphics_Imaging` | ✓ declared |
| `windows::Media::Ocr::{OcrEngine, OcrResult, OcrLine, OcrWord}` | `Media_Ocr` | ✓ declared |
| `windows::Storage::Streams::IRandomAccessStream` | `Storage_Streams` | ✗ **NEW** |
| `windows::Win32::Foundation::{RPC_E_CHANGED_MODE, S_OK}` | `Win32_Foundation` | ✓ declared |
| `windows::Win32::System::Com::CoIncrementMTAUsage` | `Win32_System_Com` | ✓ declared |
| `windows::Win32::System::WinRT::{RoInitialize, RoUninitialize, RO_INIT_MULTITHREADED, CreateRandomAccessStreamOnFile}` | `Win32_System_WinRT` | ✓ declared |
| `IVectorView<T>`, `IAsyncOperation<T>` | none — `windows-collections` and `windows-future` are **unconditional** deps of `windows` (Cargo.toml lines 743, 751) | ✓ |

---

### 1. WinRT/COM init — where exactly, and RPC_E_CHANGED_MODE

**Signature, verified** (`Win32/System/WinRT/mod.rs:233`):

```rust
pub unsafe fn RoInitialize(inittype: RO_INIT_TYPE) -> windows_core::Result<()>   // impl is `RoInitialize(inittype).ok()`
pub unsafe fn RoUninitialize()                                                   // line 333, returns ()
pub const RO_INIT_MULTITHREADED: RO_INIT_TYPE = RO_INIT_TYPE(1i32);              // line 2567
```

`HRESULT::ok()` (`windows-result-0.3.4/src/hresult.rs:35`) is `if self.0 >= 0 { Ok(()) } else { Err(self.into()) }`. So:

* `S_OK` → `Ok(())` — we created the apartment reference.
* `S_FALSE` (already MTA on this thread) → **also `Ok(())`**, and it *did* increment. Both must be balanced by exactly one `RoUninitialize`. You cannot tell them apart through this signature and you do not need to.
* `RPC_E_CHANGED_MODE` (0x80010106) → `Err`. **Never fatal, and never followed by `RoUninitialize`.**

**Detecting it:** `windows::Win32::Foundation::RPC_E_CHANGED_MODE` (`Win32/Foundation/mod.rs:6517`) and `Error::code()` (`windows-result-0.3.4/src/error.rs:131`, a `const fn` returning `HRESULT`, which is `PartialEq`).

**Why it will actually happen here — evidence, not speculation.** tao 0.35.3 puts the Tauri main thread into an **STA**:

```
tao-0.35.3/src/platform_impl/windows/window.rs:1450
    ComInitialized(match CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok() { ... })
tao-0.35.3/src/platform_impl/windows/window.rs:104
    if let Err(error) = OleInitialize(None) { ... RPC_E_CHANGED_MODE => panic!(...) }
wry-0.55.1/src/webview2/mod.rs:115
    let _ = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
```

`Ocr::new()?` is called at `src/bin/aloud.rs:1096`, inside Tauri's `setup()`, i.e. on that same main thread. So `new()` **will** see `RPC_E_CHANGED_MODE` and must sail straight past it.

And note the second half of the trap: tao's `ComInitialized` has a `Drop` that calls `CoUninitialize()`. If you "balanced" a `RPC_E_CHANGED_MODE` with a `RoUninitialize`, you would decrement *tao's* apartment refcount and unload COM out from under wry — which is precisely the segfault in windows-rs#1169 that the stub's doc comment cites.

**Where it is called:** three places, one guard type.

```rust
/// Puts the calling thread in the MTA for as long as the guard lives.
///
/// `RPC_E_CHANGED_MODE` means another crate already put this thread in a
/// *different* apartment — tao does exactly that to the Tauri main thread
/// (`CoInitializeEx(None, COINIT_APARTMENTTHREADED)`,
/// tao 0.35.3 `platform_impl/windows/window.rs:1450`). That is not fatal:
/// `Windows.Media.Ocr.OcrEngine` is `ThreadingModel.Both` + agile, so the
/// calls still work from an STA. What we must never do is `RoUninitialize`
/// an apartment we did not create — that decrements someone else's refcount
/// and unloads COM under them (windows-rs#1169, the segfault the stub cites).
struct Mta { owned: bool }

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
```

**Plus one process-wide pin, once, in `new()`.** Without it, the per-call `RoInitialize`/`RoUninitialize` pair on a short-lived worker thread is the *only* MTA in the process, so combase and the OCR model DLLs are loaded and unloaded on every hotkey press — slow, and it is the DLL-unload-under-a-live-WinRT-object shape of #1169 again.

```rust
/// Keeps a process-wide MTA alive from first construction to exit.
///
/// The cookie is deliberately never handed to `CoDecrementMTAUsage`:
/// `CO_MTA_USAGE_COOKIE` is a raw-pointer newtype (`Win32/System/Com/mod.rs:1961`)
/// with **no `Drop`** — only a `windows_core::Free` impl that `Owned<T>` consumes —
/// so dropping it here leaks the usage count for the process lifetime, which is
/// exactly the intent. It is also `!Send`, so it must NOT become a field of
/// `WindowsOcr` (which has to be `Send + Sync` for the `OcrEngine` trait).
fn pin_process_mta() {
    static ONCE: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    ONCE.get_or_init(|| match unsafe { CoIncrementMTAUsage() } {
        Ok(_cookie) => crate::log_line!("ocr: process MTA pinned"),
        Err(e) => crate::log_line!("ocr: CoIncrementMTAUsage failed: {e} — continuing"),
    });
}
```

`CoIncrementMTAUsage` in 0.61.3 has the ergonomic signature `pub unsafe fn CoIncrementMTAUsage() -> windows_core::Result<CO_MTA_USAGE_COOKIE>` (`Win32/System/Com/mod.rs:321`) — it returns the cookie, it does not take an out-param. (This differs from older windows-rs; do not write `CoIncrementMTAUsage(&mut cookie)`.)

**Worth knowing but not relied on:** windows-rs already self-heals `CO_E_NOTINITIALIZED` inside its factory cache — `windows-core-0.61.2/src/imp/factory_cache.rs:89-95` calls `CoIncrementMTAUsage` and retries `RoGetActivationFactory`. That is why sloppy WinRT-from-Rust code "just works" and why the failure this design prevents is intermittent rather than immediate. Do not lean on it; it only covers *activation*, not the async completion routing that actually deadlocks.

---

### 2. Enumeration, the default engine, and the null-not-exception trap

**Signatures, verified** (`Windows/Media/Ocr/mod.rs`):

```rust
// line 106
pub fn AvailableRecognizerLanguages() -> windows_core::Result<windows_collections::IVectorView<Language>>
// line 132
pub fn TryCreateFromUserProfileLanguages() -> windows_core::Result<OcrEngine>
// line 123
pub fn TryCreateFromLanguage<P0: Param<Language>>(language: P0) -> windows_core::Result<OcrEngine>
// line 99  — a STATIC, not a per-engine property
pub fn MaxImageDimension() -> windows_core::Result<u32>
// line 92
pub fn RecognizerLanguage(&self) -> windows_core::Result<Language>
```

**How windows-rs surfaces the null. This is the answer, and it is neither `Result<Option<T>>` nor a null interface.**

Every one of these ends in `.and_then(|| windows_core::Type::from_abi(result__))`. `Type::from_abi` for interface types is (`windows-core-0.61.2/src/type.rs:43-51`):

```rust
unsafe fn from_abi(abi: Self::Abi) -> Result<Self> {
    if !abi.is_null() { Ok(core::mem::transmute_copy(&abi)) } else { Err(Error::empty()) }
}
```

So **`TryCreateFromUserProfileLanguages()` returning `S_OK` + a null pointer becomes `Err(windows_core::Error::empty())`.** And `Error::empty()` (`windows-result-0.3.4/src/error.rs:82`) stores the sentinel `S_EMPTY_ERROR`, which `Error::code()` (line 131) maps **back to `HRESULT(0)` — S_OK**. Its `Display` (line 221) is `"{message} ({code})"`, and `HRESULT(0).message()` is `FormatMessageW(0)`:

> `The operation completed successfully. (0x00000000)`

That is what reaches the user if you just `?` it. **The null trap in windows-rs is not that it is swallowed — it is that it arrives as an `Err` whose text says the operation succeeded.** Intercept it:

```rust
/// windows-rs turns "returned S_OK with a **null** interface" into
/// `Err(Error::empty())` — an error whose `code()` is `S_OK` and whose
/// `Display` is the useless "The operation completed successfully.
/// (0x00000000)". (`windows-core-0.61.2/src/type.rs:43`,
/// `windows-result-0.3.4/src/error.rs:82,131`.) Every "returns null on
/// failure" WinRT API lands here and must be given a real message.
fn is_null_return(e: &windows::core::Error) -> bool {
    e.code() == S_OK        // windows::Win32::Foundation::S_OK, Win32/Foundation/mod.rs:9912
}
```

**`AvailableRecognizerLanguages` is the opposite shape and must be handled differently.** It returns a real, non-null `IVectorView<Language>` object that is simply *empty*. So it is `Ok(view)` and **`view.Size()? == 0`** — an `Err` never appears. `IVectorView` API, verified in `windows-collections-0.2.0/src/bindings.rs:1733-1795`: `GetAt(u32) -> Result<T>`, `Size() -> Result<u32>`, `First() -> Result<IIterator<T>>`, plus `IntoIterator for &IVectorView<T>` (line 1810) — but that impl does `self.First().unwrap()`, i.e. it **panics** on a failed QI, so prefer the explicit `for i in 0..view.Size()?` loop.

**`Language` accessors** (`Windows/Globalization/mod.rs:2728+`): `LanguageTag() -> Result<HSTRING>`, `DisplayName() -> Result<HSTRING>`, `NativeName()`, and the constructor `Language::CreateLanguage(languagetag: &HSTRING) -> Result<Language>`. `HSTRING::to_string_lossy() -> String` (`windows-strings-0.4.2/src/hstring.rs:23`).

**`new()` in full:**

```rust
pub struct WindowsOcr {
    /// BCP-47 tag of the recognizer chosen at startup. Stored as a plain
    /// `String`, not as a live `OcrEngine`: `WindowsOcr` is held inside
    /// `Arc<Runtime>` (src/bin/aloud.rs) and reached from every thread, and
    /// keeping only data removes the cross-apartment question entirely.
    language_tag: String,
    /// `OcrEngine.MaxImageDimension`, read once. A **static** property, not
    /// per-engine. 10000 on the M6 dev box.
    max_image_dimension: u32,
}

impl WindowsOcr {
    pub fn new() -> Result<Self> {
        pin_process_mta();
        let _mta = Mta::enter()?;   // on the Tauri main thread this is the RPC_E_CHANGED_MODE arm

        // 1. Enumerate and LOG. An empty list is `Ok(view)` with Size()==0,
        //    never an Err — only Size() tells the truth.
        let available = WinRtOcr::AvailableRecognizerLanguages()
            .context("OcrEngine.AvailableRecognizerLanguages failed")?;
        let count = available.Size().context("AvailableRecognizerLanguages.Size failed")?;

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
                 > Optical character recognition, or from an elevated PowerShell: \
                 Get-WindowsCapability -Online | Where-Object Name -like 'Language.OCR*' | \
                 Add-WindowsCapability -Online"
            );
        }

        // 2. Default via the user profile. A null here is Err-with-code-S_OK.
        let engine = match WinRtOcr::TryCreateFromUserProfileLanguages() {
            Ok(e) => e,
            Err(e) if is_null_return(&e) => {
                // Reported loudly, but not fatal: the stub's contract is "fail
                // loudly when no recognizer can be constructed **at all**", and
                // a non-empty AvailableRecognizerLanguages proves one can. This
                // is the case where the user's display language (e.g. uk-UA) has
                // no OCR FOD while en-US sits right there installed.
                crate::log_line!(
                    "ocr: TryCreateFromUserProfileLanguages returned NULL — no profile \
                     language has an OCR recognizer. Falling back to {}", tags[0]
                );
                let lang = Language::CreateLanguage(&HSTRING::from(tags[0].as_str()))
                    .with_context(|| format!("Language::CreateLanguage(\"{}\") failed", tags[0]))?;
                WinRtOcr::TryCreateFromLanguage(&lang).map_err(|e| {
                    if is_null_return(&e) {
                        anyhow!(
                            "OcrEngine.TryCreateFromLanguage(\"{}\") returned NULL even though \
                             that tag is listed in AvailableRecognizerLanguages", tags[0]
                        )
                    } else {
                        anyhow::Error::from(e)
                    }
                })?
            }
            Err(e) => return Err(e).context("OcrEngine.TryCreateFromUserProfileLanguages failed"),
        };

        let language_tag = engine.RecognizerLanguage()?.LanguageTag()?.to_string_lossy();
        let max_image_dimension =
            WinRtOcr::MaxImageDimension().context("OcrEngine.MaxImageDimension failed")?;

        crate::log_line!(
            "ocr: Windows.Media.Ocr ready, language={language_tag}, \
             MaxImageDimension={max_image_dimension}, {} recognizer(s) installed",
            tags.len()
        );

        // `engine` is dropped here on purpose. Nothing WinRT survives this call.
        Ok(Self { language_tag, max_image_dimension })
    }
}
```

**The fallback is a judgement call — flag it in the result.** A strict reading of "treat a null from either as a real, reported error" would `bail!` instead. I chose report-and-fall-back because the stub's own wording is *"fail loudly when no recognizer can be constructed at all"*, and here one demonstrably can. If the owner prefers hard-fail, delete the `Err(e) if is_null_return(&e)` arm and let it propagate. Either way the log line is mandatory — this must never look like "recognised no text".

**This log answers brief question 17 without PowerShell.** On the dev box expect exactly two lines: `en-US` and `ru`. No `de-DE`. No `uk` at any tag.

---

### 3 + 4. Loading the PNG, and the small-region upscale — one call, not two

**The resize does not happen after decoding. It happens *inside* the decode**, via a `BitmapTransform` handed to `BitmapDecoder::GetSoftwareBitmapTransformedAsync`. There is never an intermediate full-size `SoftwareBitmap`, and there is no separate scaling type.

**Signatures, verified** (`Windows/Graphics/Imaging/mod.rs`):

```rust
// line 224 — REQUIRES the Storage_Streams feature
BitmapDecoder::CreateAsync<P0: Param<IRandomAccessStream>>(stream: P0)
    -> Result<windows_future::IAsyncOperation<BitmapDecoder>>

// lines 298/305 — resolved through `required_hierarchy!(BitmapDecoder, IBitmapFrame, ...)`
// at line 137, so these are callable directly on the decoder (= frame 0).
decoder.PixelWidth()  -> Result<u32>
decoder.PixelHeight() -> Result<u32>

// line 357 — the whole job in one call
decoder.GetSoftwareBitmapTransformedAsync<P2: Param<BitmapTransform>>(
    pixelformat: BitmapPixelFormat,
    alphamode:   BitmapAlphaMode,
    transform:   P2,
    exiforientationmode: ExifOrientationMode,
    colormanagementmode: ColorManagementMode,
) -> Result<windows_future::IAsyncOperation<SoftwareBitmap>>

// line 985 + 999/1010/1021
BitmapTransform::new() -> Result<BitmapTransform>
transform.SetScaledWidth(u32)  -> Result<()>
transform.SetScaledHeight(u32) -> Result<()>
transform.SetInterpolationMode(BitmapInterpolationMode) -> Result<()>
```

Constant values, verified: `BitmapPixelFormat::Bgra8 = 87` (line 769), `BitmapAlphaMode::Premultiplied = 0` (line 5), `BitmapInterpolationMode::{NearestNeighbor 0, Linear 1, Cubic 2, Fant 3}` (line 747), `ExifOrientationMode::IgnoreExifOrientation = 0` (line 1133), `ColorManagementMode::DoNotColorManage = 0` (line 1120).

**Getting an `IRandomAccessStream` from a `&Path`.** Primary route — one synchronous call, no `StorageFile` broker round-trip:

```rust
// Win32/System/WinRT/mod.rs:68 — note: NOT #[cfg]-gated, plain Win32_System_WinRT
pub unsafe fn CreateRandomAccessStreamOnFile<P0: Param<PCWSTR>, T: Interface>(
    filepath: P0, accessmode: u32
) -> windows_core::Result<T>
```

`HSTRING::from(&Path)` exists (`windows-strings-0.4.2/src/hstring.rs:148`, `#[cfg(feature = "std")]` — and `std` is on: `windows` defaults to `std` → `windows-core/std` → `windows-strings/std`, chain verified in the three Cargo.tomls). `impl Param<PCWSTR> for &HSTRING` is in `windows-core-0.61.2/src/windows.rs`.

`accessmode` is a bare `u32` in the bindings because the Win32 metadata carries no enum for it. **Pass `0`** — that is `FileAccessMode::Read` (`Windows/Storage/mod.rs:815`) *and* `STGM_READ`, so `0` is correct under either reading. See "Uncertainties" for the fallback if it ever returns `E_INVALIDARG`.

**The scale math — a pure function, unit-testable with no OS:**

```rust
/// PowerToys' 1.5x pre-upscale, generalised to guard BOTH dimensions.
///
/// Windows OCR returns *nothing at all* on an image that is too small, and a
/// dragged rectangle is Aloud's primary gesture — so the small case is the
/// common case, not an edge case. PowerToys
/// (`PowerOCR/Helpers/OcrExtensions.cs:77`) tests only
/// `bmp.Width * 1.5 > OcrEngine.MaxImageDimension`; a tall, narrow selection
/// would then blow the ceiling on height. This tests both and clamps to the
/// tighter. It also handles the reverse case — a source already larger than
/// the ceiling is scaled DOWN, because RecognizeAsync rejects an oversized
/// bitmap outright.
fn upscale_dimensions(w: u32, h: u32, max_dim: u32) -> (u32, u32) {
    const TARGET: f64 = 1.5;
    let max = max_dim.max(1) as f64;
    let scale = TARGET.min(max / w.max(h) as f64);
    let dst_w = ((w as f64 * scale).round() as u32).clamp(1, max_dim);
    let dst_h = ((h as f64 * scale).round() as u32).clamp(1, max_dim);
    (dst_w, dst_h)
}

#[cfg(test)]
mod tests {
    use super::upscale_dimensions;
    #[test] fn small_region_is_upscaled_1_5x()      { assert_eq!(upscale_dimensions(100, 50, 10_000), (150, 75)); }
    #[test] fn wide_source_clamps_to_the_ceiling()  { assert_eq!(upscale_dimensions(8_000, 100, 10_000), (10_000, 125)); }
    #[test] fn tall_source_clamps_on_height()       { assert_eq!(upscale_dimensions(100, 8_000, 10_000), (125, 10_000)); }
    #[test] fn oversized_source_is_scaled_down()    { let (w, h) = upscale_dimensions(12_000, 100, 10_000); assert!(w <= 10_000 && h >= 1); }
    #[test] fn never_returns_zero()                 { assert_eq!(upscale_dimensions(1, 1, 10_000), (2, 2)); }
}
```

Note `tall_source_clamps_on_height` is the case PowerToys gets wrong — it is the reason for deviating, and it should be named in the result as a deliberate deviation.

---

### 5. `.get()` — the blocking pattern, and why it must not run on the calling thread

**`RecognizeAsync` returns `windows_future::IAsyncOperation<OcrResult>`** (`Windows/Media/Ocr/mod.rs:81`), and **`windows_future` is NOT re-exported by the `windows` crate in 0.61.3** — verified by grepping the whole `src/` tree: `Windows/Foundation/mod.rs` has no `IAsyncOperation` and no `pub use`, and `src/lib.rs` re-exports only `windows_core as core`. So `windows::Foundation::IAsyncOperation` **does not exist** and will not compile.

You do not need it. `get()` is an **inherent** method (`windows-future-0.2.1/src/get.rs:20-35`), so it resolves without importing a trait and without naming the type. Never annotate the intermediate:

```rust
let decoder = BitmapDecoder::CreateAsync(&stream)?.get()?;   // -> BitmapDecoder
let bitmap  = decoder.GetSoftwareBitmapTransformedAsync(..)?.get()?;  // -> SoftwareBitmap
let result  = engine.RecognizeAsync(&bitmap)?.get()?;        // -> OcrResult
```

Two `?` per await: the first on `Result<IAsyncOperation<T>>` (the call started), the second on `Result<T>` (the operation finished).

**Is `.get()` safe to call on the calling thread? Only in the MTA — and this is not a style preference.** Its implementation:

```rust
// windows-future-0.2.1/src/get.rs
pub fn get(&self) -> Result<T> {
    if self.Status()? == AsyncStatus::Started {
        let (_waiter, signaler) = Waiter::new()?;                    // CreateEventW
        self.SetCompleted(&AsyncOperationCompletedHandler::new(move |_, _| {
            unsafe { signaler.signal(); } Ok(())
        }))?;
    }   // <- `_waiter` drops HERE, and Waiter::drop is
        //    WaitForSingleObject(handle, 0xFFFFFFFF)  (waiter.rs:37)
    self.GetResults()
}
```

A raw kernel wait, **INFINITE, with no message pump and no timeout**. On an STA the completion delegate is marshalled back through the apartment's message queue, which this wait is blocking → **permanent deadlock**, not a slow path. In the MTA the completion fires on a threadpool thread and the event is signalled.

**Therefore: run the whole WinRT half on a dedicated thread, always.** `recognise()` is reachable from `Arc<Runtime>` on any thread including Tauri's main STA; today `spawn_read_region` (`src/bin/aloud.rs:198`) happens to spawn a fresh thread, but that is a caller detail this seam must not depend on. A fresh thread is guaranteed apartment-free, so `Mta::enter()` gets the `Ok` arm.

```rust
impl OcrEngine for WindowsOcr {          // <- Aloud's trait, `super::OcrEngine`
    fn recognise(&self, image_path: &Path) -> Result<String> {
        let path = HSTRING::from(image_path);
        let tag = self.language_tag.as_str();
        let max_dim = self.max_image_dimension;

        // The WinRT half runs on its own thread, always. `IAsyncOperation::get()`
        // blocks on WaitForSingleObject(INFINITE) with no message pump
        // (windows-future-0.2.1 get.rs + waiter.rs); on an STA that is a
        // permanent deadlock. A fresh thread is apartment-free, so
        // RoInitialize(RO_INIT_MULTITHREADED) succeeds and completions land on
        // an MTA pool thread. Cost is one thread spawn (~50us) against tens of
        // ms of OCR.
        let joined = std::thread::scope(|scope| {
            scope.spawn(|| recognise_blocking(&path, tag, max_dim)).join()
        });

        let text = match joined {
            Ok(inner) => inner?,
            Err(_) => bail!("ocr: the Windows.Media.Ocr worker thread panicked"),
        };

        crate::log_line!("ocr: recognised text length={} chars", text.chars().count());
        Ok(text)
    }
}
```

`std::thread::scope` is stable since 1.63 (toolchain is 1.97.1). `HSTRING` is `Send + Sync` (`windows-strings-0.4.2/src/hstring.rs:110-111`) so `&HSTRING` crosses the scope; `anyhow::Error` is `Send + Sync` so the return type satisfies the scoped-thread bound.

*If* the thread-per-call ever shows up in `tests/latency_budget.rs`, the fallback is to drop the scope and call `recognise_blocking` inline — correct **only** because `spawn_read_region` already spawns. Say which you shipped.

**The body:**

```rust
fn recognise_blocking(path: &HSTRING, language_tag: &str, max_image_dimension: u32) -> Result<String> {
    let _mta = Mta::enter()?;

    // ---- open + decode + scale + convert -------------------------------
    let stream: IRandomAccessStream =
        unsafe { CreateRandomAccessStreamOnFile(path, 0) }   // 0 == FileAccessMode::Read == STGM_READ
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

    let transform = BitmapTransform::new()?;
    transform.SetScaledWidth(dst_w)?;      // BOTH must be set — see traps
    transform.SetScaledHeight(dst_h)?;
    transform.SetInterpolationMode(BitmapInterpolationMode::Fant)?;

    let bitmap: SoftwareBitmap = decoder
        .GetSoftwareBitmapTransformedAsync(
            BitmapPixelFormat::Bgra8,
            BitmapAlphaMode::Premultiplied,
            &transform,
            ExifOrientationMode::IgnoreExifOrientation,   // BitBlt PNGs carry no EXIF; be deterministic
            ColorManagementMode::DoNotColorManage,        // screen pixels are already sRGB
        )?
        .get()
        .context("decoding the captured PNG failed")?;

    crate::log_line!(
        "ocr: decoded {src_w}x{src_h} -> {dst_w}x{dst_h} (MaxImageDimension={max_image_dimension})"
    );

    // The file handle must be released before `read_region`'s DeleteOnDrop runs
    // (src/app/actions.rs:59) or `remove_file` fails with a sharing violation —
    // and that Drop is `let _ = ...`, so the failure is SILENT and a photograph
    // of the user's screen survives in the temp directory. SoftwareBitmap owns
    // its own pixels (deep copy), so both of these are safe to release now.
    drop(decoder);
    drop(stream);

    // ---- recognise ------------------------------------------------------
    let lang = Language::CreateLanguage(&HSTRING::from(language_tag))?;
    let engine = WinRtOcr::TryCreateFromLanguage(&lang).map_err(|e| {
        if is_null_return(&e) {
            anyhow!("OcrEngine.TryCreateFromLanguage(\"{language_tag}\") returned NULL — \
                     the recognizer that existed at startup is gone")
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
```

---

### 6. `OcrResult` → `String` — lines, words, and the separator

**Signatures, verified** (`Windows/Media/Ocr/mod.rs`):

```rust
OcrResult::Lines(&self)  -> Result<IVectorView<OcrLine>>   // line 192
OcrResult::Text(&self)   -> Result<HSTRING>                // line 206
OcrLine::Words(&self)    -> Result<IVectorView<OcrWord>>   // line 160
OcrLine::Text(&self)     -> Result<HSTRING>                // line 167
OcrWord::Text(&self)     -> Result<HSTRING>                // line 238
OcrWord::BoundingRect(&self) -> Result<Foundation::Rect>   // line 231; Rect { X,Y,Width,Height: f32 }
```

**The separator is `"\n"` between lines, and this is not a taste call — it is required for parity.** The macOS sibling (`helpers/macos-ocr/ocr.swift`) does exactly:

```swift
let lines = (req.results ?? []).compactMap { $0.topCandidates(1).first?.string }
print(lines.joined(separator: "\n"))
```

and `src/ocr/macos.rs:65` then `.trim_end()`s it. Downstream, `normalize_ocr` (`src/text/normalize.rs`) is built entirely around `\n`: `re_hyphen_break` matches `(\w)-\n(\w)`, `re_soft_break` matches a single `\n`, and page-number stripping splits on `"\n\n"`. Joining with a space instead would silently disable hyphen rejoin and page-number stripping, and the two platforms would stop sounding identical — which `src/app/actions.rs`'s module doc names as an explicit goal.

**Do not use `OcrResult::Text()`.** Its line separator is undocumented (the `RecognizeAsync` reference page has no remarks section at all), so it is exactly the kind of "convention, not contract" the research flagged. Build the string yourself.

**Use `OcrLine::Text()` rather than re-joining `Words()`.** `OcrLine.Text` already joins the line's words with a single space and is what PowerToys uses. `Words()` is the fallback if `Text()` ever comes back empty on a line that has words (worth a defensive check, not worth a code path).

```rust
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
                for w in 0..n { parts.push(words.GetAt(w)?.Text()?.to_string_lossy()); }
                text = parts.join(" ");
            }
        }

        if !text.trim().is_empty() { lines.push(text); }
    }

    // Matches helpers/macos-ocr/ocr.swift exactly: lines joined with "\n",
    // trailing whitespace trimmed (src/ocr/macos.rs:65 does the same trim_end).
    Ok(lines.join("\n").trim_end().to_string())
}
```

**Reading order.** The order of `OcrResult.Lines` is not documented. For a single-column screenshot it is top-to-bottom in practice (PowerToys relies on it). If a two-column test shows interleaving, sort by the first word's box — `line.Words()?.GetAt(0)?.BoundingRect()?.Y` — but note the coordinates are in the **upscaled** bitmap space, so anything that maps back to screen pixels must divide by the scale factor. Do not add the sort speculatively; test first (see "Uncertainties").

**An empty recognition is `Ok(String::new())`, never `Err`.** `read_region` (`src/app/actions.rs:106`) turns an empty normalization into `Outcome::Empty` and the tray says "no text found". Returning `Err` there would produce a bogus error notification for a perfectly ordinary drag over a blank area. The `Err` cases are: no recognizer, decode failure, RecognizeAsync failure — not "found nothing".

---

### 7. Imports block (exact, copy-paste-able)

```rust
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
```

`use windows::…` inside the module `crate::ocr::windows` resolves to the **extern crate** (2018-edition `use` paths are absolute, and `windows` is not a crate-root module of `aloud`), so no `::windows::` prefix is needed. `src/capture/windows.rs` and `src/login_item/windows.rs` will do the same thing.

No `?` conversion glue is needed: `windows_core::Error` is `Send + Sync + std::error::Error + 'static` (its `ErrorInfo` carries `unsafe impl Send/Sync`, `windows-result-0.3.4/src/error.rs:358-359`; the `std::error::Error` impl is at line 157 behind the `std` feature, which is on by default), so `anyhow`'s blanket `From` applies directly.

### 8. Call site — nothing to change

`src/bin/aloud.rs:35-37` already `#[cfg]`-swaps `aloud::ocr::windows::WindowsOcr as Ocr`, and line 1096 is `ocr: Ocr::new()?`. `new() -> Result<Self>` is unchanged. `WindowsOcr` becomes a struct with two `String`/`u32` fields — trivially `Send + Sync`, which the `OcrEngine: Send + Sync` supertrait requires now that it lives in `Arc<Runtime>`.

### 9. Tests

* Inline `#[cfg(test)] mod tests` for `upscale_dimensions` (above) — pure, no OS, matches the `src/ocr/confusions.rs` convention.
* A new `tests/ocr_windows.rs` mirroring `tests/ocr_macos.rs`'s structure, gated `#![cfg(target_os = "windows")]` for the same reason its sibling is gated.
  **Do NOT mirror its assertions.** `reads_german_text_with_umlauts` asserts on `ausgefüllte` / `läuft` from `tests/fixtures/german_form.png`. This box has **en-US and ru only** — no `de-DE` recognizer — so that fixture will be read by the English model and the umlauts will be mangled. It would fail for a language-pack reason and read as a code bug. Assert instead: `new()` succeeds, `recognise()` returns `Ok`, and the result is non-empty and contains `Antragsteller` (ASCII-only, survives the English model). Add a comment saying why the umlaut assertions are absent.

### 10. What to log so the brief's questions answer themselves

* Brief **Q17** ("which languages come back on this machine") — the `ocr: recognizer available: <tag> (<name>)` lines from `new()`. Expected: `en-US`, `ru`. Paste them.
* Brief **Q22** ("smallest region that still returns text; MaxImageDimension") — the `ocr: decoded WxH -> WxH (MaxImageDimension=N)` line plus the `recognised text length=N chars` line. Drag progressively smaller rectangles and read the smallest `src_w x src_h` whose length is non-zero.
