//! Regression: raising the speech speed must never silently drop words.
//!
//! Reported by Andrii 2026-08-10. Reading a captured region of Aloud's own
//! settings window at his configured 1.5x, the spoken output skipped
//! `on screen; Aloud` out of the middle of a sentence — no error, no log
//! entry, and nothing wrong with the text: OCR, `normalize_ocr` and
//! `split_sentences` all carry the full 80 characters through (see
//! `docs/2026-08-10-text-drop-diagnosis.md`). The loss happened inside
//! synthesis, because `speed` was passed to the vendored Supertonic engine,
//! which implements it as `duration /= speed` — it shrinks the canvas the
//! flow-matching decoder renders into, and the decoder elides whatever no
//! longer fits.
//!
//! The invariant this pins is deliberately relative: whatever the engine says
//! at speed 1.0 is the reference, and a faster rendering of the same text must
//! not contain less. Comparing against the literal source string instead would
//! make the test hostage to the transcriber's own errors (Whisper hears
//! "Aloud" as "allowed" at every speed).
//!
//! Content is checked by transcribing the rendered audio — there is no cheaper
//! honest oracle for "was this word actually spoken". It uses the local Whisper
//! install; if that is absent the test skips loudly rather than passing
//! vacuously.

use aloud::tts::supertonic_engine::SupertonicEngine;
use aloud::tts::{Pcm, TtsEngine};
use std::path::{Path, PathBuf};
use std::process::Command;

/// The owner's exact sentence, as `target/aloud-ocr` returns it from the
/// "Read Region" section of Aloud's own settings window.
const OWNER_SENTENCE: &str = "Drag a rectangle anywhere on screen; Aloud reads the text inside it.";

fn whisper_bin() -> Option<PathBuf> {
    let p = dirs::home_dir()?.join("whisper/venv/bin/whisper");
    p.is_file().then_some(p)
}

fn write_wav(path: &Path, pcm: &Pcm) {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: pcm.sample_rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut w = hound::WavWriter::create(path, spec).expect("create wav");
    for s in &pcm.samples {
        w.write_sample(*s).expect("write sample");
    }
    w.finalize().expect("finalize wav");
}

/// Lowercased alphanumeric word list, so punctuation and casing differences
/// between transcriptions do not register as content loss.
fn words(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| w.to_lowercase())
        .collect()
}

fn transcribe(whisper: &Path, dir: &Path, stem: &str, pcm: &Pcm) -> Vec<String> {
    let wav = dir.join(format!("{stem}.wav"));
    write_wav(&wav, pcm);

    let out = Command::new(whisper)
        .arg(&wav)
        .args(["--model", "base", "--language", "en"])
        .args(["--output_format", "txt", "--fp16", "False"])
        .arg("--output_dir")
        .arg(dir)
        .output()
        .expect("run whisper");
    assert!(
        out.status.success(),
        "whisper failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let txt = std::fs::read_to_string(dir.join(format!("{stem}.txt"))).expect("whisper txt");
    println!("  {stem}: {}", txt.trim());
    words(&txt)
}

#[test]
fn raising_the_speed_does_not_drop_words() {
    let Some(whisper) = whisper_bin() else {
        eprintln!("SKIPPED: no whisper at ~/whisper/venv/bin/whisper, cannot verify audio content");
        return;
    };

    let dir = std::env::temp_dir().join("aloud-speed-regression");
    std::fs::create_dir_all(&dir).expect("scratch dir");

    // The owner's configured voice.
    let engine = SupertonicEngine::spawn("M5").expect("engine");

    let reference = transcribe(
        &whisper,
        &dir,
        "reference_1p0",
        &engine.synthesize(OWNER_SENTENCE, "en", 1.0).expect("1.0"),
    );
    assert!(
        reference.len() >= 10,
        "reference rendering at speed 1.0 is itself degenerate: {reference:?}"
    );

    // Synthesis is stochastic (the decoder samples a noisy latent), so a single
    // clean trial would not prove much.
    for trial in 0..2 {
        let pcm = engine.synthesize(OWNER_SENTENCE, "en", 1.5).expect("1.5");
        let fast = transcribe(&whisper, &dir, &format!("fast_1p5_t{trial}"), &pcm);

        let missing: Vec<&String> = reference.iter().filter(|w| !fast.contains(w)).collect();
        assert!(
            missing.is_empty(),
            "speed 1.5 dropped words that speed 1.0 spoke (trial {trial}): missing {missing:?}\n\
             reference: {reference:?}\n     fast: {fast:?}"
        );
    }
}
