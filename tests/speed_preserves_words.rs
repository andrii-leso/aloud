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
//! honest oracle for "was this word actually spoken".
//!
//! # Why this is `#[ignore]`d
//!
//! Each case is an ONNX synthesis plus a Whisper invocation. Run inside a plain
//! `cargo test --release`, they load the machine alongside `latency_budget.rs`,
//! whose ratio guard is the enforcement of hard constraint 5 and which has
//! already been observed at 0.82 against its own 0.70 gate under concurrency.
//! A guard that flakes gets loosened by the next person, so this test stays out
//! of the default run rather than putting pressure on it. The mechanism itself
//! has fast deterministic coverage in `src/tts/timestretch.rs`'s unit tests
//! (retiming ratio, pitch preservation at a male fundamental, and the tail
//! flush); this is the end-to-end content check on top.
//!
//! ```text
//! cargo test --release --test speed_preserves_words -- --ignored --nocapture
//! ```

use aloud::tts::supertonic_engine::SupertonicEngine;
use aloud::tts::{Pcm, TtsEngine};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

/// The owner's exact sentence, as `target/aloud-ocr` returns it from the
/// "Read Region" section of Aloud's own settings window.
const OWNER_SENTENCE: &str = "Drag a rectangle anywhere on screen; Aloud reads the text inside it.";

/// A second, unrelated input. One fixture proves one sentence; the defect was
/// general, so the guard should be too.
const SECOND_SENTENCE: &str = "The quick brown fox jumps over the lazy dog.";

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
    println!(
        "  {stem}: {}",
        txt.split_whitespace().collect::<Vec<_>>().join(" ")
    );
    words(&txt)
}

/// Words present at 1.0 that are absent at `speed`.
///
/// This is *set* recall: a word spoken twice at 1.0 and once at `speed` would
/// pass. Aloud's inputs are prose where a dropped duplicate is not the failure
/// mode observed, and requiring multiset equality would fail on the
/// transcriber's own word-boundary wobble rather than on lost audio.
fn missing_against_reference(reference: &[String], fast: &[String]) -> Vec<String> {
    reference
        .iter()
        .filter(|w| !fast.contains(w))
        .cloned()
        .collect()
}

fn scratch_dir() -> PathBuf {
    // Unique per run: the stems below would otherwise collide between two
    // concurrent invocations sharing one directory.
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("aloud-speed-regression-{nonce}"));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

#[test]
#[ignore = "expensive (ONNX + Whisper per case); would load the latency ratio guard — see module docs"]
fn raising_the_speed_does_not_drop_words() {
    let Some(whisper) = whisper_bin() else {
        eprintln!("SKIPPED: no whisper at ~/whisper/venv/bin/whisper, cannot verify audio content");
        return;
    };
    let dir = scratch_dir();

    // The owner's configured voice.
    let engine = SupertonicEngine::spawn("M5").expect("engine");

    // (label, text, speed, trials, min_recall). Synthesis is stochastic — the
    // decoder samples a noisy latent — so the headline case runs more than once.
    //
    // The budget is a token count, not a fraction, because the oracle's noise
    // is a token count: Whisper substitutes about one word per utterance
    // regardless of how long it is ("Aloud" as "allowed"/"allow", "brown" as
    // "round", and in one run "dog" as "door" in the 1.0 reference itself);
    // occasionally two adjacent words go together ("Aloud reads" as "allow
    // read"), which is why the budget is 2 and not 1.
    // A fraction would therefore be strict on a long sentence and slack on a
    // short one, which is backwards. These are substitutions — same token
    // count, same slot — not the deletions this test exists to catch.
    //
    // The budget still discriminates, which is the point: on these same inputs
    // the broken engine dropped 5 of 12 and 3 of 9 words at 1.5x, and 9 of 12
    // and 7 of 9 at 2.0x — against budgets of 2 and 3. A regression that only
    // bit at 2.0x would blow its budget three times over. Calibrated over
    // seven consecutive runs of this test.
    //
    // Residual 2.0x word errors are transcription, not lost audio, and that is
    // measured rather than assumed: retiming one 1.0 rendering with a plain
    // resampler — which cannot drop content, it only interpolates — transcribes
    // at 2.0x as "Try to rectangle anyone on screen…", while the SOLA path
    // gives the sentence verbatim. The provably-lossless control scores *worse*
    // than the shipped path.
    //
    // Sub-second utterances are deliberately NOT asserted here, even though the
    // diagnosis identifies them as the worst case. They cannot be: Whisper
    // `base` transcribed a *clean* 1.5x rendering of the two-word heading
    // "Read Region" as "Read Readin" on one run in three, from audio the
    // resampler control shows is intact. An assertion at that length measures
    // the transcriber and would flake, and a flaky guard gets loosened by the
    // next person. Short-utterance behaviour is covered deterministically
    // instead, without an oracle, by the tail-flush, final-burst and
    // male-fundamental tests in `src/tts/timestretch.rs`.
    // (label, text, speed, trials, max_missing_tokens)
    let cases: &[(&str, &str, f32, usize, usize)] = &[
        ("owner_1p5", OWNER_SENTENCE, 1.5, 2, 2),
        ("second_1p5", SECOND_SENTENCE, 1.5, 1, 2),
        ("owner_2p0", OWNER_SENTENCE, 2.0, 1, 3),
    ];

    for (label, text, speed, trials, max_missing) in cases {
        let reference = transcribe(
            &whisper,
            &dir,
            &format!("{label}__reference_1p0"),
            &engine.synthesize(text, "en", 1.0).expect("1.0"),
        );
        assert!(
            !reference.is_empty(),
            "{label}: reference rendering at speed 1.0 is itself degenerate"
        );

        for trial in 0..*trials {
            let pcm = engine.synthesize(text, "en", *speed).expect("fast");
            let fast = transcribe(&whisper, &dir, &format!("{label}__t{trial}"), &pcm);

            let missing = missing_against_reference(&reference, &fast);
            assert!(
                missing.len() <= *max_missing,
                "{label}: speed {speed} lost {} of {} words that speed 1.0 spoke \
                 (budget {max_missing}, trial {trial}): missing {missing:?}\n \
                 reference: {reference:?}\n      fast: {fast:?}",
                missing.len(),
                reference.len()
            );
        }
    }

    let _ = std::fs::remove_dir_all(&dir);
}
