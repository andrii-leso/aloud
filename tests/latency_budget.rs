use aloud::text::chunk::split_sentences;
use aloud::tts::{supertonic_engine::SupertonicEngine, TtsEngine};
use std::time::Instant;

const BUDGET_MS: u128 = 2000;

/// The model must already be resident; that is the whole point of the
/// worker thread. We spawn, do one throwaway synthesis to warm any lazy
/// init, then measure the first sentence of a realistic paragraph.
#[test]
fn first_sentence_synthesises_within_budget() {
    let engine = SupertonicEngine::spawn("F5").expect("engine should spawn");
    let _ = engine.synthesize("Warm up.", "en", 1.0).expect("warmup");

    let paragraph = "The applicant must submit the completed form together with proof of \
                     residence within four weeks. Incomplete submissions will be returned \
                     without processing. If you require an extension, contact the office in \
                     writing before the deadline expires.";
    let sentences = split_sentences(paragraph);
    assert!(!sentences.is_empty());

    let t0 = Instant::now();
    let pcm = engine
        .synthesize(&sentences[0], "en", 1.0)
        .expect("synthesis should succeed");
    let elapsed = t0.elapsed().as_millis();

    assert!(!pcm.samples.is_empty());
    assert!(
        elapsed <= BUDGET_MS,
        "time-to-first-audio was {elapsed}ms, over the {BUDGET_MS}ms budget. \
         Measured baseline on an M1 Air was ~1500ms. Something regressed."
    );
}
