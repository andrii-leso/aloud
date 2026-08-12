> **Moved into this repo 2026-08-12, from a loose folder on the PC's build disk
> (`X:\dev\aloud-m6-design`) that was not under version control and had no copy
> anywhere.** The M6 Windows implementation leaned on these four documents
> heavily; losing that disk would have lost the entire design rationale behind
> `src/capture/windows/overlay.rs` and `src/ocr/windows.rs`.
>
> **Status: PRE-IMPLEMENTATION. Written 2026-08-11, before any of this ran on
> Windows.** Kept as written rather than corrected, per the `docs/` rule that
> superseded findings are annotated and not rewritten. Its corrections were folded in before implementation.
>
> What actually shipped, and every place reality disagreed with this document,
> is recorded in `docs/2026-08-11-windows-port.md` and in
> `BKM/PC-Queue/TASK-M6-aloud-windows-prototype-result.md` in the Second Brain
> repo. **Read this for the reasoning; read those for the outcome.**

---
VERDICT: NEEDS_FIXES

### BLOCKING DEFECTS
- FALSE-PREMISE ESCAPE HATCH THAT REINTRODUCES THE DEADLOCK. The spec offers: 'If the thread-per-call ever shows up in tests/latency_budget.rs, the fallback is to drop the scope and call recognise_blocking inline.' tests/latency_budget.rs does not touch OCR at all — it imports only aloud::play::player::Player, aloud::play::sink::AudioSink, aloud::text::chunk::split_sentences and aloud::tts::{supertonic_engine::SupertonicEngine, Pcm, TtsEngine}, and measures Supertonic synthesis. The contingency can never fire, but if someone acts on it, recognise_blocking runs on whatever thread the caller is on, and IAsyncOperation::get() on an STA thread is the permanent WaitForSingleObject(INFINITE) deadlock the spec itself documents. DELETE the escape hatch; the scoped thread is unconditional.
- PANIC IN THE ONE FUNCTION ADVERTISED AS PURE AND OS-FREE. upscale_dimensions ends with .clamp(1, max_dim). u32::clamp asserts min <= max, so max_dim == 0 panics ('assertion failed: min <= max') rather than returning anything. The spec's own never_returns_zero test proves the author intended a floor of 1, but the floor and the ceiling are taken from different values (1 vs max_dim). Fix: bind `let ceiling = max_dim.max(1);` once and use it for BOTH the scale divisor and the clamp upper bound.
- THE NULL-FALLBACK CONTRADICTS THE STUB AND THE BRIEF VERBATIM, AND THE SPEC SHIPS IT ANYWAY. src/ocr/windows.rs:38-39 says 'Treat a null from either as a real, reported error here — never as "recognised no text".' Brief §6.3 repeats it: 'treat a `null` from either as a real reported error.' The spec's `Err(e) if is_null_return(&e) => ... falling back to tags[0]` arm is a deliberate deviation from both. It is defensible (new()'s own doc says 'fail loudly when no recognizer can be constructed **at all**'), but brief §8 is explicit that product calls get reported, not picked. Ship the fallback so the app starts, but the log line must be loud and the result MUST quote both readings and name the consequence of the literal one: with a hard bail!, `Ocr::new()?` at src/bin/aloud.rs:1096 is inside Tauri's setup(), so a hard-fail means Aloud does not start at all on a machine with no matching OCR pack.
- IVectorView IS NOT NAMEABLE FROM ALOUD, AND THE SPEC ONLY WARNS ABOUT IAsyncOperation. Verified: windows 0.61.3's src/lib.rs re-exports only `windows_core as core`; there is no `pub use windows_future` or `pub use windows_collections` anywhere in its src tree, and windows::Foundation contains no IAsyncOperation. Both crates are unconditional *dependencies of windows* (Cargo.toml lines 743, 751) but not of `aloud`, so neither type has any importable path. The spec states this for IAsyncOperation and then writes `IVectorView<Language>` in prose without the same warning. Rule: NEVER write a type annotation containing IVectorView or IAsyncOperation. Use `let x = ...;` inference everywhere. Adding windows-collections/windows-future to Cargo.toml to name them would be a new version-drift surface — do not.

### API PATH ERRORS
- NONE. Every windows-crate API path, type, method signature, constant value and feature gate the spec names was checked against C:\Users\Andrew\.cargo\registry\src\index.crates.io-1949cf8c6b5b557f\windows-0.61.3 (and windows-core-0.61.2, windows-result-0.3.4, windows-strings-0.4.2, windows-collections-0.2.0, windows-future-0.2.1) and ALL of them exist as written. Confirmed individually: RoInitialize(RO_INIT_TYPE)->Result<()> at Win32/System/WinRT/mod.rs:233; RoUninitialize()->() at :333; RO_INIT_MULTITHREADED=RO_INIT_TYPE(1) at :2567; CreateRandomAccessStreamOnFile<P0: Param<PCWSTR>, T: Interface>(filepath, accessmode: u32)->Result<T> at :68 and NOT cfg-gated; CoIncrementMTAUsage()->Result<CO_MTA_USAGE_COOKIE> at Win32/System/Com/mod.rs:321 (returns the cookie, no out-param); CO_MTA_USAGE_COOKIE(pub *mut c_void) at :1961 with a Free impl and NO Drop (so !Send and leak-on-drop, both as claimed); RPC_E_CHANGED_MODE at Win32/Foundation/mod.rs:6517 and S_OK at :9912; BitmapDecoder::CreateAsync at Graphics/Imaging/mod.rs:224 gated #[cfg(feature="Storage_Streams")] at :223; GetSoftwareBitmapTransformedAsync with the exact 5-arg signature; PixelWidth/PixelHeight via required_hierarchy!(BitmapDecoder, IBitmapFrame, IBitmapFrameWithSoftwareBitmap) at :137; BitmapTransform::new/SetScaledWidth/SetScaledHeight/SetInterpolationMode at :985/1000/1011/1022; Bgra8=87, Premultiplied=0, Fant=3, Cubic=2, IgnoreExifOrientation=0, DoNotColorManage=0, ColorManageToSRgb=1; OcrEngine::{AvailableRecognizerLanguages :106, TryCreateFromUserProfileLanguages :132, TryCreateFromLanguage :123, MaxImageDimension :99 (static), RecognizerLanguage :92, RecognizeAsync :81}; OcrResult::{Lines :192, Text :206}, OcrLine::{Words :160, Text :167}, OcrWord::{BoundingRect :231, Text :238}; Language::{CreateLanguage(&HSTRING) :2778, LanguageTag :2729, DisplayName :2736}; IRandomAccessStream at Storage/Streams/mod.rs:2266 via define_interface!; IVectorView::{GetAt, Size, First} at windows-collections bindings.rs:1734/1747/1792 with IntoIterator for &IVectorView doing self.First().unwrap() at :1811; IAsyncOperation<T: RuntimeType>::get() inherent at windows-future get.rs:20 and Waiter::drop -> WaitForSingleObject(h, 0xFFFFFFFF) at waiter.rs:35; Type::from_abi -> Err(Error::empty()) on null at windows-core type.rs:43-51; Error::empty() at windows-result error.rs:82 with code() remapping S_EMPTY_ERROR to HRESULT(0) at :131; HRESULT::ok() at hresult.rs:34; HRESULT derives PartialEq (hresult.rs:5); windows_core re-exports windows_result::* (lib.rs:54) so windows::core::{Error, HRESULT, Result} resolve; AgileReference::{new, resolve} + unsafe impl Send/Sync at windows-core agile_reference.rs:11/27/31-32; From<&std::path::Path> for HSTRING at windows-strings hstring.rs:148 behind #[cfg(feature="std")]; Display for HSTRING at :113; unsafe impl Send/Sync for HSTRING at :110-111; impl Param<PCWSTR> for &HSTRING at windows-core windows.rs:29; unsafe impl Send/Sync for ErrorInfo at windows-result error.rs:358-359 and impl std::error::Error for Error at :157.
- Two citation nits, neither an API error: the spec's fallback cites `StorageFile::OpenAsync` at Storage/mod.rs:1534 — that line belongs to a different class; StorageFile::OpenAsync is at Storage/mod.rs:4331 (GetFileFromPathAsync at :4446 is correct). And BitmapPixelFormat::Bgra8 is line 768, not 769. Neither changes any code.

### CORRECTIONS
## Unit 4 — `src/ocr/windows.rs`: verified spec with corrections merged

The spec's API surface is **clean** — zero wrong windows-crate paths across ~45 items (see `api_path_errors` for the full ledger). Its two best findings hold up under source inspection:

* **`Storage_Streams` is genuinely missing and genuinely fails as a missing *method*.** Verified: `Graphics/Imaging/mod.rs:223` gates `BitmapDecoder::CreateAsync` and `:233` gates `CreateWithIdAsync`; with the feature off, `BitmapDecoder` has **no constructor at all**. Error text will be `no function or associated item named 'CreateAsync' found for struct 'BitmapDecoder'`, not an unresolved import. `Storage_Streams = ["Storage"]` (Cargo.toml:308), `Storage = ["Foundation"]` (:299) — one name, two namespaces, no other feature needed.
* **The null-arrives-as-an-`Err`-that-says-success trap is real and exactly as described.** `Type::from_abi` → `Err(Error::empty())` on a null interface (windows-core `type.rs:43-51`); `Error::empty()` stores `S_EMPTY_ERROR` (windows-result `error.rs:82`) which `code()` remaps back to `HRESULT(0)` (`:131`); `Display` is `"{message} ({code})"` (`:221`) → *"The operation completed successfully. (0x00000000)"*. Intercepting on `e.code() == S_OK` is exact: a real failure short-circuits inside `and_then` with a negative HRESULT and never reaches `from_abi`, so `code() == S_OK` uniquely means "returned S_OK with a null pointer."

Apply the four blocking fixes plus the improvements below; everything else ships unchanged.

---

### FIX 1 — `Cargo.toml`, unchanged from the spec

Add `"Storage_Streams"` to the `[target.'cfg(windows)'.dependencies] windows` feature list (14 features). Update the comment: *"All thirteen were checked…"* → *"All fourteen…"*, and add: *"`Storage_Streams` is not optional and does not surface as an unresolved import — every `BitmapDecoder::Create*Async` is `#[cfg(feature = \"Storage_Streams\")]` (windows 0.61.3, `Graphics/Imaging/mod.rs:223`), so without it `BitmapDecoder` has no constructor. It pulls `Storage` transitively."*

### FIX 2 — `upscale_dimensions`: one ceiling, used twice

```rust
/// PowerToys' 1.5x pre-upscale, generalised to guard BOTH dimensions.
///
/// Windows OCR returns nothing at all on an image that is too small, and a
/// dragged rectangle is Aloud's primary gesture — the small case is the common
/// case. PowerToys (`PowerOCR/Helpers/OcrExtensions.cs:77`) tests only
/// `bmp.Width * 1.5 > OcrEngine.MaxImageDimension`; a tall, narrow selection
/// (a sidebar, a code gutter) then blows the ceiling on HEIGHT. This tests both
/// and clamps to the tighter, and scales an oversized source DOWN.
fn upscale_dimensions(w: u32, h: u32, max_dim: u32) -> (u32, u32) {
    const TARGET: f64 = 1.5;
    // One ceiling, used as both the divisor and the clamp bound. Taking the
    // floor from `1` and the bound from `max_dim` (as an earlier draft did)
    // panics inside `u32::clamp` when max_dim == 0: clamp asserts min <= max.
    let ceiling = max_dim.max(1);
    let longest = w.max(h).max(1) as f64;
    let scale = TARGET.min(ceiling as f64 / longest);
    let dst_w = ((w as f64 * scale).round() as u32).clamp(1, ceiling);
    let dst_h = ((h as f64 * scale).round() as u32).clamp(1, ceiling);
    (dst_w, dst_h)
}

#[cfg(test)]
mod tests {
    use super::upscale_dimensions;
    #[test] fn small_region_is_upscaled_1_5x()     { assert_eq!(upscale_dimensions(100, 50, 10_000), (150, 75)); }
    #[test] fn wide_source_clamps_to_the_ceiling() { assert_eq!(upscale_dimensions(8_000, 100, 10_000), (10_000, 125)); }
    #[test] fn tall_source_clamps_on_height()      { assert_eq!(upscale_dimensions(100, 8_000, 10_000), (125, 10_000)); }
    #[test] fn oversized_source_is_scaled_down()   { let (w, h) = upscale_dimensions(12_000, 100, 10_000); assert!(w <= 10_000 && h >= 1); }
    #[test] fn never_returns_zero()                { assert_eq!(upscale_dimensions(1, 1, 10_000), (2, 2)); }
    #[test] fn zero_ceiling_does_not_panic()       { assert_eq!(upscale_dimensions(100, 50, 0), (1, 1)); }
}
```

`tall_source_clamps_on_height` is the case PowerToys gets wrong — name it in the result as a deliberate deviation.

### FIX 3 — the worker thread is unconditional; delete the inline escape hatch

`tests/latency_budget.rs` measures TTS only (`SupertonicEngine`, `Player::speak`, `split_sentences`) and never constructs an `OcrEngine`. The scoped thread cannot appear in that budget, so there is no condition under which inlining is warranted — and inlining is the deadlock. Ship this, with no alternative offered:

```rust
impl OcrEngine for WindowsOcr {          // <- Aloud's trait, `super::OcrEngine`
    fn recognise(&self, image_path: &Path) -> Result<String> {
        let path = HSTRING::from(image_path);
        let tag = self.language_tag.as_str();
        let max_dim = self.max_image_dimension;

        // The WinRT half runs on its own thread, ALWAYS — not conditionally.
        // `IAsyncOperation::get()` blocks on WaitForSingleObject(INFINITE) with
        // no message pump (windows-future-0.2.1 get.rs:20 + waiter.rs:35); on an
        // STA that is a permanent deadlock, and `recognise` is reachable from
        // `Arc<Runtime>` on any thread. tao puts the Tauri main thread in an STA
        // (`CoInitializeEx(None, COINIT_APARTMENTTHREADED)`, tao 0.35.3
        // platform_impl/windows/window.rs:1450; wry does the same,
        // wry 0.55.1 webview2/mod.rs:115). A fresh thread is apartment-free, so
        // RoInitialize(RO_INIT_MULTITHREADED) succeeds and completions land on
        // an MTA pool thread. Cost is one spawn (~50us) against tens of ms of OCR.
        // Do NOT "optimise" this away on the grounds that spawn_read_region
        // (src/bin/aloud.rs:197) already spawns — that is a caller detail this
        // seam must not depend on.
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

Type-checks: `HSTRING: Send + Sync` (hstring.rs:110-111) so `&HSTRING` crosses the scope; `anyhow::Error: Send + Sync`; the handle is joined explicitly so `thread::scope` does not re-panic.

### FIX 4 — deterministic stream release, not just `drop`

`IRandomAccessStream::Close()` is an inherent method (windows 0.61.3, `Storage/Streams/mod.rs:2330`, reached through `required_hierarchy!(IRandomAccessStream, Foundation::IClosable, IInputStream, IOutputStream)` at `:2271`). Prefer it over relying on the refcount: WIC can hold an internal reference that Rust's `drop` cannot see, and the consequence is not cosmetic. `read_region`'s `DeleteOnDrop` (src/app/actions.rs:59-66) is `let _ = std::fs::remove_file(self.0);` — on Windows an open handle makes that fail with a sharing violation, and the failure is **swallowed**, leaving a photograph of the user's screen in the temp directory. Order matters: release the decoder's reference first, then close.

```rust
    drop(decoder);                 // releases the decoder's reference on the stream
    let _ = stream.Close();        // IClosable::Close — deterministic handle release
    drop(stream);
```

Never cache the stream or the decoder on `self`.

### FIX 5 — the null-on-`TryCreateFromUserProfileLanguages` arm: ship it, but report it as a deviation

Keep the spec's report-and-fall-back, with a louder log, and put the choice in the result rather than deciding it silently. Both readings, quoted, for the result file:

* Stub `src/ocr/windows.rs:38-39` and brief §6.3: *"treat a null from either as a real, reported error"*.
* Stub `new()` doc, src/ocr/windows.rs:122-124: *"fail loudly when no recognizer can be constructed **at all**"*.

State the consequence of the literal reading plainly: `Ocr::new()?` is at `src/bin/aloud.rs:1096`, inside Tauri's `setup()`, so a hard `bail!` means **Aloud does not launch at all** on a machine whose profile language has no OCR pack, even with `en-US` installed. That is why the fallback is the shipped default. To switch to hard-fail, delete one match arm.

One extra thing to report, free: **the order `AvailableRecognizerLanguages` returns.** It is undocumented. On this box the list is `en-US` and `ru`. If `ru` comes back first, `tags[0]` sends English screenshots through the Russian model — a real finding that changes the fallback. Paste the order.

### `new()` — unchanged except the log line and the `pin_process_mta` call site

```rust
pub struct WindowsOcr {
    /// BCP-47 tag of the recognizer chosen at startup. A plain `String`, not a
    /// live `OcrEngine`: `WindowsOcr` sits inside `Arc<Runtime>`
    /// (src/bin/aloud.rs:111-114) and is reached from every thread, and holding
    /// only data removes the cross-apartment question entirely. Two plain
    /// fields also make `OcrEngine: Send + Sync` trivially true.
    language_tag: String,
    /// `OcrEngine.MaxImageDimension`, read once. A **static** property
    /// (Media/Ocr/mod.rs:99), not per-engine. 10000 on the M6 dev box.
    max_image_dimension: u32,
}
```

`new()` body is the spec's, verbatim, with `pin_process_mta();` first and `let _mta = Mta::enter()?;` second (on the Tauri main thread that takes the `RPC_E_CHANGED_MODE` arm — harmless, because everything `new()` calls is synchronous WinRT, no `.get()`). Add `pin_process_mta();` at the top of `recognise_blocking` too — it is a `OnceLock`, so it costs nothing and removes the ordering assumption.

The two guards are correct as specified and must not be simplified:

* `Mta { owned: bool }` — `RoInitialize` returning **`S_FALSE`** (already MTA) maps to `Ok(())` via `HRESULT::ok()` (`self.0 >= 0`, hresult.rs:34) and **did** increment, so it needs balancing. Only the `Err(RPC_E_CHANGED_MODE)` arm must **never** be balanced: tao's `ComInitialized` has a `Drop` calling `CoUninitialize()` (tao window.rs:1438-1444), so a stray `RoUninitialize` decrements tao's refcount and unloads COM under wry.
* `pin_process_mta()` — `CoIncrementMTAUsage()` returns `Result<CO_MTA_USAGE_COOKIE>` in 0.61.3 (Com/mod.rs:321); do **not** write `CoIncrementMTAUsage(&mut cookie)`. The cookie is `CO_MTA_USAGE_COOKIE(pub *mut c_void)` (`:1961`) — `!Send`, so it can never be a field of `WindowsOcr`; it has a `Free` impl but **no `Drop`** (`:1967`), so dropping it leaks the usage count for the process lifetime, which is the intent.

Aside, verified but not to be relied on: windows-rs self-heals `CO_E_NOTINITIALIZED` inside its factory cache by calling `CoIncrementMTAUsage` and retrying `RoGetActivationFactory` (windows-core `imp/factory_cache.rs:87-95`). That covers *activation* only, never async completion routing. It is why sloppy WinRT-from-Rust appears to work and why the failure this design prevents is intermittent.

### RESOLVED — `CreateRandomAccessStreamOnFile`'s `accessmode`, and a better fallback than the spec's

`0` is correct and unambiguous: `FileAccessMode::Read == 0` is verified at `Storage/mod.rs:815`, and `STGM_READ == 0` under the other reading. The function is **not** `#[cfg]`-gated beyond `Win32_System_WinRT` (`Win32/System/WinRT/mod.rs:68`) and links against the `api-ms-win-shcore-stream-winrt-l1-1-0.dll` API set.

**Replace the spec's two-call `StorageFile` fallback with the one-call, typed-enum route** — verified, needs no feature beyond the same `Storage_Streams`:

```rust
// Fallback if CreateRandomAccessStreamOnFile ever returns E_INVALIDARG (0x80070057).
// Storage/Streams/mod.rs:586 — typed FileAccessMode, one static, one .get().
let stream: IRandomAccessStream =
    FileRandomAccessStream::OpenAsync(path, FileAccessMode::Read)?.get()?;
```

`windows::Storage::Streams::FileRandomAccessStream` + `windows::Storage::FileAccessMode`. It goes through the storage broker so it is slower; take it only if the fast path fails. `StorageFile::GetFileFromPathAsync` (`Storage/mod.rs:4446`) → `.OpenAsync(FileAccessMode::Read)` (`:4331`, **not** `:1534` as the spec cited) remains a second fallback, but there is no reason to reach for it.

### Everything else in the spec ships as written

`recognise_blocking`'s decode-transform-recognise body (with FIX 4's release block), `assemble()`, and the imports block are all correct. Load-bearing points confirmed against the repo, not just the crate:

* **`"\n"` between lines is required, not taste.** `helpers/macos-ocr/ocr.swift:19` is `print(lines.joined(separator: "\n"))`; `src/ocr/macos.rs:65` does `.trim_end()`. `src/text/normalize.rs` keys on it: `re_hyphen_break` = `(\w)-\n(?:[ \t]*)(\w)` (:7), `re_soft_break` = `(?m)([^\n])\n([^\n])` (:13). A space join silently disables both.
* **Empty recognition is `Ok(String::new())`, never `Err`.** `src/app/actions.rs:106-109` turns an empty normalization into `Outcome::Empty`.
* **The `use windows::Media::Ocr::{OcrEngine as WinRtOcr, ...}` alias is mandatory** — `use super::OcrEngine;` is already in the stub (line 9); unaliased is an immediate E0252.
* **No `?` glue needed** — `windows_core::Error` is `Send + Sync + std::error::Error + 'static`, so anyhow's blanket `From` applies.
* **`AvailableRecognizerLanguages` fails the other way**: a real non-null `IVectorView<Language>` that is merely empty, so `Ok(view)` with `view.Size()? == 0`. Testing for `Err` silently passes on a zero-pack machine. Use the explicit `for i in 0..view.Size()?` loop — `IntoIterator for &IVectorView<T>` does `self.First().unwrap()` (windows-collections bindings.rs:1811) and panics on a failed QI.
* **`.get()` has no timeout.** A wedged engine hangs the read thread forever, and `App::read_region`'s `acquire_within(Current::Region, Duration::ZERO, false)` guard (src/app/mod.rs:265) then silently drops every subsequent hotkey press. Not fixable in the prototype; log immediately before and after each `.get()` so the timestamps in `aloud.log` show the hang.

### `tests/ocr_windows.rs` — weaken the assertion further than the spec does

Whole-file `#![cfg(target_os = "windows")]`, same reason as `tests/ocr_macos.rs`'s gate. `tests/fixtures/german_form.png` is the **only** fixture in the repo, so it is the one you get. This box has `en-US` and `ru` only — no `de-DE`. The spec drops the umlaut assertions, which is right, but keeping `assert!(text.contains("Antragsteller"))` is still a coin flip: Windows OCR's English model is lexicon-influenced and "Antragsteller" is a long non-English token. Assert only what is structural, and print the rest:

```rust
#[test]
fn recognises_something_from_the_german_fixture() {
    let ocr = WindowsOcr::new().expect("an OCR recognizer should be constructible");
    let text = ocr.recognise(Path::new("tests/fixtures/german_form.png")).expect("ocr should succeed");
    // No umlaut / no word-level assertions. This box has en-US and ru only
    // (no de-DE recognizer), so `ausgefüllte` and `läuft` from the macOS
    // sibling would fail for a language-pack reason and read as a code bug.
    // What is being tested here is the seam: new() constructs, recognise()
    // returns Ok, and the engine produced text at all.
    assert!(!text.trim().is_empty(), "recognised nothing — got: {text:?}");
    eprintln!("--- windows OCR of german_form.png ---\n{text}\n---");
}
```

Run with `--nocapture` and paste the output in the result: it is free evidence for how the English model handles German, which the Mac cannot get any other way.

### What to log so the brief's questions answer themselves

* **Q17** (which languages come back on this machine) — the `ocr: recognizer available: <tag> (<name>)` lines from `new()`, **in order**. Expected `en-US`, `ru`. This answers Q17 from the app itself, without the PowerShell in the question text; paste both if you run the PowerShell too.
* **Q22** (smallest region that still returns text; `MaxImageDimension`) — the `ocr: decoded WxH -> WxH (MaxImageDimension=N)` line plus `recognised text length=N chars`. Drag progressively smaller rectangles; report the smallest `src_w x src_h` with non-zero length.
* Report which of `ocr: process MTA pinned` / `ocr: this thread is already in a single-threaded apartment` appeared at startup — a free datapoint about whether Tauri's `EventLoop`/webview COM init has run before `setup()` reaches line 1096.
* Report the `TryCreateFromLanguage` cost (wrap it in `Instant::now()`). If it exceeds ~20 ms per call, cache it as `OnceLock<windows::core::AgileReference<WinRtOcr>>` — `AgileReference::new(&engine)` / `.resolve()` (windows-core `agile_reference.rs:11/27`), which is `unsafe impl Send + Sync` (`:31-32`) and legal across apartments. Never cache a bare `OcrEngine` field.
* Say which of `Fant` vs `Cubic` and `DoNotColorManage` vs `ColorManageToSRgb` you shipped; both compile, neither is measured.

### One framing note for the result

The `bail!` on an empty recognizer list names `Add-WindowsCapability`. That is a **diagnostic string on a failure path**, not a supported install step — say so explicitly in the result, next to the §7.7 / §12 language-pack gap, so it does not read as Aloud shipping a "first install X" requirement against Aloud constraint 1.