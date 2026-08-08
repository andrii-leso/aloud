use aloud::tts::{supertonic_engine::SupertonicEngine, TtsEngine};
use std::sync::Arc;

#[test]
fn engine_is_usable_from_multiple_threads() {
    let engine = Arc::new(SupertonicEngine::spawn("F5").expect("engine should spawn"));
    let mut handles = Vec::new();

    for i in 0..3 {
        let e = Arc::clone(&engine);
        handles.push(std::thread::spawn(move || {
            let pcm = e
                .synthesize(&format!("Sentence number {i}."), "en", 1.0)
                .expect("synthesis should succeed");
            assert!(!pcm.samples.is_empty());
            assert!(pcm.sample_rate > 0);
        }));
    }
    for h in handles {
        h.join().expect("worker thread should not panic");
    }
}
