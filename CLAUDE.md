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
| Windows-side work | [`../../../BKM/PC-Queue/README.md`](../../../BKM/PC-Queue/README.md) — the Windows half is built on the PC, brief-driven. |
| Anything touching the EULA or selling | Dispatch Rektor. Do not draft licence terms unaided. |

---

## Hard constraints

1. **Zero runtime system dependencies.** Everything bundled or statically linked. No "first install espeak-ng" step, ever. This is the line between a product and a README of setup chores — it is why Natural-Voice-TTS is not shippable.
2. **No voice cloning. Permanently.** OpenRAIL-M Attachment A (g) bars non-consensual impersonation. Do not add it, do not prototype it.
3. **Licence obligations are load-bearing before any release.** Supertonic is MIT *code* + OpenRAIL-M *weights* — do not conflate them. Shipping the weights requires: Attachment A mirrored into the EULA as an enforceable provision, a copy of the licence shipped, attribution retained. Commercial use is permitted and royalty-free; the obligations are paperwork. Rektor drafts the EULA clause before money changes hands.
4. **The region overlay is one window per monitor**, each carrying its own scale factor, with coordinates normalised into a global virtual-desktop space. Never a single fullscreen window. This is [pot-app#1161](https://github.com/pot-app/pot-desktop/issues/1161) — an open bug on a 19.2k-star app — and it is cheap to design in and expensive to retrofit.
5. **Time-to-first-audio ≤ 2.0s warm, and it is tested.** The Chunker exists solely for this: unchunked means 5.8s of silence before the first word. Do not "simplify" it away.
6. **The Windows half cannot be built or tested on the Mac.** WinRT bindings do not compile on macOS. `cfg`-gate per platform; finish macOS fully first; build on the PC via a `BKM/PC-Queue/` brief.
7. **Everything platform- or engine-specific lives behind a seam** (`RegionSelector`, `OcrEngine`, `SelectionGrabber`, `TtsEngine`). If a platform `#[cfg]` is leaking into pipeline logic, the seam is in the wrong place.
8. **Check `df -h /` before installing toolchains or models.** The M1 Air is small and has run critically low before.

---

## Stack

- **Tauri 2**, Rust core, web UI. `.dmg` and `.msi` from the Tauri bundler.
- **TTS:** Supertonic 3 via its first-party Rust SDK over ONNX Runtime. Defaults **F5** (female) and **M5** (male). 31 languages; `lingua-rs` picks the tag. Kokoro stays wired as the licence-clean fallback.
- **OCR:** bundled Swift helper → Vision (macOS); `Windows.Media.Ocr` via the `windows` crate (Windows). Never Tesseract.
- **Model:** 385 MB, downloaded on first run, not shipped in the installer. All ten voice styles together are under 3 MB — ship them all.
- **Audio:** `cpal` / `rodio`.

---

## Folder map

| Folder | Purpose |
|---|---|
| `src-tauri/` | Rust core: seams, pipeline, hotkeys, tray |
| `src/` | Web UI: pill, settings window, region overlay |
| `helpers/macos-ocr/` | Swift OCR helper binary source |
| `tests/fixtures/` | Golden screenshots for OCR tests, text fixtures for the normalizer |

---

## Conventions

Deliverables register a DB pointer against `P-90212`:
`python3 memory/memory.py project --action link --project-id 90212 --link-id <N>`

Naming is not settled — "Aloud" is a working name and a product-step decision.
