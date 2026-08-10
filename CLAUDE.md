# Aloud — Sub-project Instructions

Local screen-and-selection reader. One hotkey drags a rectangle and hears the text inside it read aloud; another reads the current selection. Fully local — no cloud, no account, no network. macOS first, Windows second, one codebase.

**DB project row:** `P-90212`. Status, phase and milestones live there and on the dashboard — **never in this file**.

Personal tool for Andrii's two machines. Built to product standards so that selling it later is a distribution problem, not a rewrite.

---

## Required reading by task type

| Task | Read first |
|---|---|
| Anything at all | [`../../../docs/superpowers/specs/2026-08-08-aloud-tts-reader-design.md`](../../../docs/superpowers/specs/2026-08-08-aloud-tts-reader-design.md) — the design. Seams, platform matrix, measured performance budget, licence obligations. |
| TTS / voices / engine work | [`../../../docs/capabilities.md`](../../../docs/capabilities.md) §1 (engines, measured numbers) and its licence section. |
| macOS platform work — tray, settings window, launch-at-login, TCC/signing, packaging | [`docs/M4-platform-research-macos.md`](docs/M4-platform-research-macos.md) — verified/likely/unverified findings and a traps table, built from what M3's first real use got wrong. |
| Windows-side work / M6 | [`docs/M6-platform-research-windows.md`](docs/M6-platform-research-windows.md) — the seven constraint conflicts against Aloud's hard constraints, the manual-test checklist to run on the PC, and [`../../../BKM/PC-Queue/README.md`](../../../BKM/PC-Queue/README.md) for how the PC half actually gets built (brief-driven). |
| Building, running, or verifying the app; permissions | [`README.md`](README.md) — the dev doc: what it does, how to build (`packaging/make-app.sh`), permissions, known limits. |
| Anything touching the EULA or selling | Dispatch Rektor. Do not draft licence terms unaided. |

---

## Hard constraints

1. **Zero runtime system dependencies.** Everything bundled or statically linked. No "first install espeak-ng" step, ever. This is the line between a product and a README of setup chores — it is why Natural-Voice-TTS is not shippable.
2. **No voice cloning. Permanently.** OpenRAIL-M Attachment A (g) bars non-consensual impersonation. Do not add it, do not prototype it.
3. **Licence obligations are load-bearing before any release.** Supertonic is MIT *code* + OpenRAIL-M *weights* — do not conflate them. Shipping the weights requires: Attachment A mirrored into the EULA as an enforceable provision, a copy of the licence shipped, attribution retained. Commercial use is permitted and royalty-free; the obligations are paperwork. Rektor drafts the EULA clause before money changes hands.
4. **The region overlay is one window per monitor**, each carrying its own scale factor, with coordinates normalised into a global virtual-desktop space. Never a single fullscreen window. This is [pot-app#1161](https://github.com/pot-app/pot-desktop/issues/1161) — an open bug on a 19.2k-star app — and it is cheap to design in and expensive to retrofit.
5. **First audio must arrive far sooner than whole-passage synthesis, and it is tested.** The Chunker exists solely for this. The guard is a **ratio** (`tests/latency_budget.rs`): time from `Player::speak` to the first buffer reaching the sink must be ≤ 70% of one-shot whole-paragraph synthesis, plus an append-count check. Do not "simplify" the chunker away, and do not replace the ratio with an absolute millisecond budget — an absolute number asserts a property of the *machine*, not the code, and fails on any busy laptop. Measured 2026-08-08: **synthesis speed is wholly load-dependent** — the same sentence takes ~7s at load 4 and ~15s at load 24 on this M1 Air, in both the Rust build and the Python reference. The "~1.5s to first word" figure in the design spec holds only on a genuinely idle machine. An absolute 2.0s check exists but is `#[ignore]`d for that reason.
6. **Never pass a user-facing speed to the TTS engine.** Supertonic's `speed` argument is not a playback control: it divides the duration predictor's output and that shortened number sizes the latent the decoder renders into, so above ~1.1× the decoder silently elides phonemes and then whole words — no error, nothing in the log. Andrii lost `on screen; Aloud` from mid-sentence this way at his configured 1.5× (2026-08-10). `src/tts/supertonic_engine.rs` therefore always calls the engine at `ENGINE_SPEED = 1.0`, and speed is applied to the rendered audio by `src/tts/timestretch.rs` (SOLA, pitch-preserving). Do not plumb `speed` through to `TextToSpeech::call`, and do not "simplify" the time-stretch into a resample — a resample raises a male voice seven semitones at 1.5×. Guarded by `tests/speed_preserves_words.rs` (ASR-backed, `#[ignore]`d) and the deterministic unit tests in `src/tts/timestretch.rs`; full evidence in [`docs/2026-08-10-text-drop-diagnosis.md`](docs/2026-08-10-text-drop-diagnosis.md). **The pin applies below 1.0 as a uniformity choice, not an evidenced one:** every measurement behind this constraint is at 1.0 or above, and below 1.0 the mechanism is provably benign — `duration /= 0.7` gives the decoder *more* canvas than it asked for, so word-drop cannot occur there. One code path is worth more than a speed-dependent branch, and 0.7-1.0 is the least-used part of the slider, but nobody has A/B-listened engine-native vs stretched at 0.7× on M5. Do that before treating sub-1.0 stretching as verified.
7. **The Windows half cannot be built or tested on the Mac.** WinRT bindings do not compile on macOS. `cfg`-gate per platform; finish macOS fully first; build on the PC via a `BKM/PC-Queue/` brief.
8. **Everything platform- or engine-specific lives behind a seam** (`RegionSelector`, `OcrEngine`, `SelectionGrabber`, `TtsEngine`). If a platform `#[cfg]` is leaking into pipeline logic, the seam is in the wrong place.
9. **Check `df -h /` before installing toolchains or models, and again after any build.** The M1 Air is small and has run critically low before — it hit 2.3 GB free during the M3 Tauri build.
10. **Build and test in `--release`, not debug.** `target/debug` costs ~3 GB on top of release's ~2.2 GB and offers nothing here: ONNX inference in a debug build is several times slower, so the timing-sensitive tests are misleading there anyway. A stray `cargo test` (which defaults to debug) recreates the whole 3 GB tree. If you find `target/debug` present and disk is tight, deleting it is safe.
11. **A new file under `dist/` needs a clean rebuild to actually take effect.** `dist/` is embedded at compile time by `tauri::generate_context!()`, but `cargo` does not watch it for `rerun-if-changed`, and `generate_context!()` cannot track a file that didn't exist at the previous compile. Adding a new file under `dist/` and rebuilding normally therefore silently embeds a **stale** binary — no error, no warning. If you touch `dist/`, run `cargo clean -p aloud --release` before the next build. This cost real time during M4.

---

## Stack

- **Tauri 2** (menubar shell, global-shortcut plugin, tray), Rust core, web UI. Packaging on macOS is **hand-assembled**, not the Tauri CLI bundler: `cargo install tauri-cli` drove this M1 Air to 2.0 GB free, so `packaging/make-app.sh` builds `target/Aloud.app` directly (see `README.md`). Do not reach for `cargo tauri build` here. Windows packaging (M6) is undecided — revisit then, it does not inherit this constraint.
- **Settings window (`dist/`):** hand-written HTML/CSS/JS, no bundler, no `package.json`, no build step — edit the files directly and rebuild the Rust binary (see Hard constraint 11, the `dist/` clean-rebuild trap).
- **Do not add `tauri-plugin-autostart`.** Read from its source (M4 research): on macOS it offers only LaunchAgent and AppleScript modes, and its default LaunchAgent mode writes `ProgramArguments` pointing at `Contents/MacOS/aloud` — the inner binary — which bypasses LaunchServices, the mechanism that registers the `NSServices` provider. Adopting it as-is would silently kill "Read Aloud". Launch-at-login is not yet built; see `docs/HANDOFF.md`.
- **TTS:** Supertonic 3 via its first-party Rust SDK over ONNX Runtime. Defaults **F5** (female) and **M5** (male). 31 languages; `lingua-rs` picks the tag. Kokoro stays wired as the licence-clean fallback.
- **OCR:** bundled Swift helper → Vision (macOS); `Windows.Media.Ocr` via the `windows` crate (Windows). Never Tesseract.
- **Model:** 385 MB, downloaded on first run, not shipped in the installer. All ten voice styles together are under 3 MB — ship them all.
- **Audio:** `cpal` / `rodio`.

---

## Folder map

| Folder | Purpose |
|---|---|
| `src/` | The whole Rust crate: library (text, tts, play, ocr, capture, selection) + binaries |
| `src/bin/aloud.rs` | The Tauri menubar app |
| `src/bin/aloud_say.rs` | The CLI |
| `src/vendor/` | Vendored MIT Supertonic engine — never edit |
| `dist/` | The settings window: hand-written HTML/CSS/JS, no bundler, no `package.json`. Embedded at compile time — see Hard constraint 11 for the clean-rebuild trap this creates |
| `capabilities/` | Tauri capability grants for the `settings` window. Grants only `core:default`, `core:window:allow-close`, `core:event:default` — no `global-shortcut` permission; the settings page never calls that plugin, by design, and hotkey registration happens exclusively in Rust (`src/bin/aloud.rs`). What this file actually authorises is the *core plugin* commands the page uses: `event`'s `listen` (for `aloud://probe-fired` and `aloud://voice-swapped`) and the window close. This crate's own IPC commands (`get_settings`, `set_shortcut`, `set_voice`, `set_speed`, …) are outside the ACL entirely — `build.rs` is a bare `tauri_build::build()` with no app ACL manifest, so this file has no bearing on them |
| `Info.plist` | NSServices declaration — the single source of truth for it, merged into `target/Aloud.app`'s bundle Info.plist by `packaging/make-app.sh` (PlistBuddy `Merge`), not by Tauri |
| `packaging/make-app.sh` | Hand-assembles `target/Aloud.app` (see Stack, above) — builds the release binary + the Swift OCR helper, writes the bundle Info.plist, signs with the self-signed "Aloud Dev" certificate (hard build failure if it's missing from the login keychain, since Task 10 — see `README.md`) |
| `helpers/macos-ocr/` | Swift OCR helper binary source |
| `tests/fixtures/` | Golden screenshots for OCR tests, text fixtures for the normalizer |

---

## Conventions

Deliverables register a DB pointer against `P-90212`:
`python3 memory/memory.py project --action link --project-id 90212 --link-id <N>`

Naming is not settled — "Aloud" is a working name and a product-step decision.
