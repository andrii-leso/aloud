# Carry-forward into M3 (Tauri shell) and M6 (Windows)

Findings from building M1–M2 that the next plan must not rediscover. Recorded 2026-08-08,
when the engine and pipeline landed (22 commits, 40 tests).

## Verified facts

**`rodio` 0.22.2** — verified from crate source, not docs. Three things bite:
- `SampleRate` is `NonZero<u32>`, `ChannelCount` is `NonZero<u16>`. `SamplesBuffer::new(1, 44100, v)` does not compile.
- `MixerDeviceSink` prints to stderr on every drop unless `log_on_drop(false)` is called.
- `rodio::Player` collides with our `Player`; import qualified.
API: `DeviceSinkBuilder::open_default_sink()` → `MixerDeviceSink::mixer()` → `rodio::Player::connect_new(&Mixer)`. `Sample` is `f32`.

**ONNX Runtime links statically.** `otool -L` shows no `onnxruntime` entry; `ort-sys` pulls `libonnxruntime.a`. The zero-runtime-dependencies constraint holds — there is no dylib to bundle.
**But `ort`'s `download-binaries` fetches that archive at BUILD time.** The Windows build on the PC needs network on first build; an offline or reproducible build needs `ORT_LIB_LOCATION` set or the cache pre-seeded. **Copy this verbatim into the PC-Queue brief** — the brief is the only interface the PC gets.

**Pinned versions are forced, not preferences.** `ort` and `ort-sys` both `=2.0.0-rc.7` (ort's internal dep is loose and otherwise resolves an ABI-incompatible rc.13); `ndarray` 0.16 because ort rc.7 links against it and two 0.x majors cannot coexist for trait resolution. Do not bump any of the three casually.

**Supertonic has no G2P.** It is character-level (`text.chars().map(|c| c as usize)`), which is why there is no espeak dependency and why 31 languages come free.

**`TextToSpeech::call` takes `&mut self` and returns one finished buffer** having already chunked internally. Its internal chunking buys no latency; streaming requires per-sentence calls, which is what `Player::speak` does.

## Landmines for the Windows port

1. **`model_dir()` resolution order is `$ALOUD_MODEL_DIR` → `~/.cache/supertonic3` if it exists → `dirs::cache_dir()/supertonic3`.** The middle branch exists because the model is already at `~/.cache/supertonic3` on the Mac, while `dirs::cache_dir()` on macOS is `~/Library/Caches`. On Windows only the third branch applies. Decide deliberately where the 385 MB model lives on the PC rather than letting the two machines drift.
2. **`tests/engine_smoke.rs` pins synthesis duration to 6.566s ±0.01, baselined on aarch64 macOS.** x86-64 Windows may legitimately differ. Re-baseline per platform; do not send an agent chasing it as a regression.
3. **Global hotkeys silently fail against elevated windows on Windows.** Unfixable OS behaviour. Document it; do not chase it.
4. **`cargo fmt` and the vendored file.** `rustfmt.toml`'s `ignore` key is nightly-only and inert on stable. The exclusion is instead `#[rustfmt::skip]` on the `pub mod supertonic;` declaration in `src/vendor/mod.rs`, which keeps the vendored bytes untouched. Do not "tidy" that attribute away.

## Performance, honestly

Synthesis speed is **entirely load-dependent** and the Rust build matches the Python reference at every load measured:

| Machine load | Rust | Python reference |
|---|---|---|
| ~24 (8-core M1 Air) | 14.9s | 13.7–15.7s |
| ~4–6 | 7.1s | 7.1–9.8s |

The design spec's "~1.5s to first word" comes from a near-idle measurement and has **not** been reproduced on a genuinely idle machine since. Treat it as unverified until it is. This is why the latency guard is a ratio and the absolute check is `#[ignore]`d.

`speed` must stay clamped to 0.7–2.0 (the CLI does this): chunks cap at 120 chars, and below ~0.27x one chunk exceeds 30s of audio and false-positives the player's stall watchdog.

## Deferred minors (none blocking)

- Digit-only paragraph blocks are stripped as page numbers, so a standalone year or an OCR'd numeric table column is dropped when it sits between blank lines. Judged an acceptable trade — page numbers are common in target input, standalone numeric paragraphs are not. Single-block input is exempt, so a bare-number selection still speaks.
- A sentence terminator followed by a closing quote is not treated as a boundary, so `He said "Stop." Then left.` stays one chunk. Harmless (still under the 120-char cap); revisit if reading selected prose from articles becomes primary.
- The long-Cyrillic chunker test infers the comma-preference path from tracing rather than asserting `ends_with(',')`.
- Stop-test timing margins are generous rather than handshake-synchronised; no flake observed.
- A `stop()` arriving in the window just before `speak()` begins is swallowed by the flag reset. Caller-layer concern; the CLI is single-shot.
- `Player::speak` is documented single-caller/serialized and not enforced with locking. The Tauri shell **must** respect that — a hotkey pressed twice quickly must not produce two concurrent `speak()` calls.

## Release paperwork still owed (before publishing, per CLAUDE.md constraint 3)

Aloud has no LICENSE/NOTICE of its own yet, and the OpenRAIL-M weights statement lives only in `CLAUDE.md`. Shipping the weights requires Attachment A mirrored into the EULA, a copy of the licence, and attribution. Rektor drafts that before money changes hands.
