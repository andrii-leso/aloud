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

1. **RESOLVED, 2026-08-09 (M4 platform research) — `model_dir()`'s third branch drops the model outside any app folder on Windows, and the fix is not "pick a path", it's "route through Tauri instead of raw `dirs`".** The original framing here was to "decide deliberately where the model lives" as if any of the three branches were viable; the M6 Windows research (`docs/M6-platform-research-windows.md` §3.6) inverts that. `dirs` 5.0 on Windows collapses cache and local-data into the same folder with no `Caches` subdirectory, so `dirs::cache_dir()` returns `%LOCALAPPDATA%` **itself** — not a subfolder of it. `model_dir()`'s third branch (`dirs::cache_dir()/supertonic3`, the only branch that applies on Windows, since the middle branch is a macOS-specific existing-path check) would therefore drop the 385 MB model directly in `%LOCALAPPDATA%\supertonic3`, outside `%LOCALAPPDATA%\<bundle identifier>\`. Tauri's NSIS uninstaller only deletes folders named after the bundle identifier, so that model directory is never cleaned up by anything — 385 MB orphaned on every uninstall, forever. **M6 must route `model_dir()` through Tauri's `app_cache_dir()` / `app_local_data_dir()` on Windows, not raw `dirs::cache_dir()`.**
2. **`tests/engine_smoke.rs` pins synthesis duration to 6.566s ±0.01, baselined on aarch64 macOS.** x86-64 Windows may legitimately differ. Re-baseline per platform; do not send an agent chasing it as a regression.
3. **CORRECTED, 2026-08-09 (M4 platform research) — not established that global hotkeys fail against elevated windows on Windows, at least not for the API Aloud uses.** The original claim here was written as verified fact; it isn't. The strong evidence (UIPI blocking cross-process window-message sends, and AutoHotkey's own FAQ) concerns `SetWindowsHookEx` and journal hooks — a different mechanism from the `RegisterHotKey` API `tauri-plugin-global-shortcut` actually calls. At least one well-sourced Windows-internals source states the opposite for `RegisterHotKey` specifically: that UIPI did not prevent it from triggering across the privilege boundary, because Explorer relies on it working that way for its own registered hotkeys. `RegisterHotKey`'s own documentation says nothing about integrity levels. **This is now a test item, not a documented limitation:** `docs/M6-platform-research-windows.md` §3.3 and question 24 give the exact manual test — run the app normally (not elevated), focus an elevated Command Prompt, and press the global shortcut; then confirm it still fires against a normal window. Do this before writing anything about the limitation into a PC-Queue brief, and correct or confirm this landmine in the same change set as the test result.
4. **`cargo fmt` and the vendored file.** `rustfmt.toml`'s `ignore` key is nightly-only and inert on stable. The exclusion is instead `#[rustfmt::skip]` on the `pub mod supertonic;` declaration in `src/vendor/mod.rs`, which keeps the vendored bytes untouched. Do not "tidy" that attribute away.

## Performance, honestly

Synthesis speed is **entirely load-dependent** and the Rust build matches the Python reference at every load measured:

| Machine load | Rust | Python reference |
|---|---|---|
| ~24 (8-core M1 Air) | 14.9s | 13.7–15.7s |
| ~4–6 | 7.1s | 7.1–9.8s |

The design spec's "~1.5s to first word" comes from a near-idle measurement and has **not** been reproduced on a genuinely idle machine since. Treat it as unverified until it is. This is why the latency guard is a ratio and the absolute check is `#[ignore]`d.

`speed` must stay clamped to 0.7–2.0 (the CLI does this): chunks cap at 120 chars, and below ~0.27x one chunk exceeds 30s of audio and false-positives the player's stall watchdog.

**CORRECTED, 2026-08-10 — that clamp was never the real hazard, and the upper half of the range was actively broken.** Supertonic's `speed` is not a playback control: `_infer` does `duration /= speed` and the result sizes the decoder's latent, so any value above ~1.1x hands the decoder less room than its own duration predictor asked for and it silently elides phonemes, then whole words. Measured across six texts: at 1.5x, four of six lost content; at 2.0x, all six did; short utterances degrade first (a two-word heading breaks at 1.25x). Aloud now always synthesises at 1.0 and retimes the rendered audio in `src/tts/timestretch.rs`. Evidence and method: [`2026-08-10-text-drop-diagnosis.md`](2026-08-10-text-drop-diagnosis.md).

## Deferred minors (none blocking)

- Digit-only paragraph blocks are stripped as page numbers, so a standalone year or an OCR'd numeric table column is dropped when it sits between blank lines. Judged an acceptable trade — page numbers are common in target input, standalone numeric paragraphs are not. Single-block input is exempt, so a bare-number selection still speaks.
- An OCR'd heading gets welded onto the paragraph below it. Vision separates a heading from the following line with a single `\n`, and `normalize_ocr`'s soft-break rule turns any single newline into a space, so `Read Region` + hint becomes one run-on utterance with no pause between them. Cosmetic (prosody, not content) and explicitly *not* the cause of the 2026-08-10 word-drop, which was the speed knob — but a real defect, and the fix is a heading heuristic in the normalizer rather than anything in the chunker.
- A sentence terminator followed by a closing quote is not treated as a boundary, so `He said "Stop." Then left.` stays one chunk. Harmless (still under the 120-char cap); revisit if reading selected prose from articles becomes primary.
- The long-Cyrillic chunker test infers the comma-preference path from tracing rather than asserting `ends_with(',')`.
- Stop-test timing margins are generous rather than handshake-synchronised; no flake observed.
- A `stop()` arriving in the window just before `speak()` begins is swallowed by the flag reset. Caller-layer concern; the CLI is single-shot.
- `Player::speak` is documented single-caller/serialized and not enforced with locking. The Tauri shell **must** respect that — a hotkey pressed twice quickly must not produce two concurrent `speak()` calls.

## Release paperwork still owed (before publishing, per CLAUDE.md constraint 3)

Aloud has no LICENSE/NOTICE of its own yet, and the OpenRAIL-M weights statement lives only in `CLAUDE.md`. Shipping the weights requires Attachment A mirrored into the EULA, a copy of the licence, and attribution. Rektor drafts that before money changes hands.

---

# First-use findings, 2026-08-09 (Andrii testing on his own Mac)

Four real bugs, none caught by 80 passing tests. All fixed. The pattern in three of
the four is the same and worth naming: **I researched crate APIs exhaustively before
planning M3 and did essentially no research on macOS platform integration.** Do that
research before M4/M6.

## 1. The Services menu entry never appeared — missing `NSRequiredContext`

`pbs` listed the service, the UTI was correct (`public.utf8-plain-text`),
`NSServicesStatus` showed it was not disabled, the cache was flushed, the app was in
/Applications and LaunchServices-registered. macOS still refused to show it.

**Cause:** the `NSServices` dict lacked `NSRequiredContext`. macOS silently omits any
service without that key — no error, no log, nothing in Services Settings. An empty
dict means "offer in every context".

I had hypothesised ad-hoc signing was suppressing it. **That was wrong.** Andrii asked
for research before redesigning; the real cause surfaced in one search.

## 2. Screen Recording grants died on every rebuild — ad-hoc signing

Ad-hoc (`--sign -`) gives no certificate, so the designated requirement falls back to
the binary's cdhash. Every rebuild = new hash = grant silently invalid, **while System
Settings still shows the app enabled**. Toggling it does nothing because the entry
belongs to a binary that no longer exists.

**Fix:** a self-signed code-signing certificate ("Aloud Dev", created once in Keychain
Access). The DR becomes
`identifier "com.andriileso.aloud" and certificate leaf = H"e83bf2a9..."` — stable
across rebuilds. `packaging/make-app.sh` uses it. (At the time of this writing it fell
back to ad-hoc with a loud warning if the certificate was absent; M4 Task 10 replaced
that fallback with a hard build failure, since a silent ad-hoc fallback reintroduces
exactly this bug — see `README.md`.) The cert does **not** need to be trusted; codesign
accepts it. **Verified by deliberately rebuilding + reinstalling: the grant survived.**

A stable signing identity is a **development** requirement on macOS, not a distribution
one. Notarization/Developer ID is the distribution concern. Do not conflate them again.

## 3. `CGPreflightScreenCaptureAccess` false-negatives and blocked working captures

It was used as a gate before capture and returned false while capture would have
succeeded, so the app reported a permission problem that did not exist.

**Fix:** never gate on it. Run `screencapture` unconditionally; only consult the
preflight *after* a failed capture, to decide whether the failure was a permission
problem or a user cancel.

## 4. Ordinary sentences were chopped mid-clause

Reported as an audible pause before the final word of a 125-char sentence. The
120-char valve was applied to **every** chunk; it cut at char 115.

**Fix:** two budgets. `FIRST_CHUNK_CHARS = 120` (latency-critical, unchanged) and
`LATER_CHUNK_CHARS = 300` (only to keep one buffer under the 30s stall watchdog).
Later chunks are synthesised while earlier audio plays, so capping them bought nothing
and cost natural phrasing.

## Also added

`~/Library/Logs/Aloud/aloud.log` — the app was undebuggable before this, because
`eprintln!` goes nowhere from a LaunchServices-launched bundle. Every diagnosis above
came from this file. Keep it.

## Still open

- Region shortcut is not rebindable from the tray (Andrii asked for this).
- The "Read Selection" tray item is disabled/informational and reads as broken; relabel
  it now that the Service demonstrably works.
- Tray icon: square + "A" read well at 22pt; the crosshair and speaker corners do not.
- The `I'd` → `l'd` OCR misread has a normaliser fix in place but was never reproduced;
  if it recurs, capture the raw helper output before blaming the fix.
