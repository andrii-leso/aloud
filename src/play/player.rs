use crate::play::sink::AudioSink;
use crate::text::chunk::split_sentences;
use crate::tts::TtsEngine;
use anyhow::Result;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Poll interval for the drain loops below.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// If `queued()` reports the same depth for this long with no stop
/// requested, the audio device is presumed stalled (see `wait_for_drain`).
/// Only the *first* chunk is capped at ~120 chars; every later one is capped
/// at `LATER_CHUNK_CHARS` = 300 (`src/text/chunk.rs`), which is roughly 18-20s
/// of audio at speed 1.0 — so the margin here is thinner than the old ~8s
/// figure implied. At the 0.7 speed floor a 300-char chunk runs ~28s against
/// this 30s timeout. That is pre-existing and unchanged by the 2026-08-10
/// speed fix (0.7 produced an equally long buffer before it), but it is only
/// 1-4s of headroom on a content-dependent estimate: do not raise
/// `LATER_CHUNK_CHARS` or lower the speed floor without revisiting this.
const STALL_TIMEOUT: Duration = Duration::from_secs(30);

pub struct Player {
    engine: Arc<dyn TtsEngine>,
    sink: Arc<dyn AudioSink>,
    stop_flag: Arc<AtomicBool>,
    speaking: Arc<AtomicBool>,
}

impl Player {
    pub fn new(engine: Arc<dyn TtsEngine>, sink: Arc<dyn AudioSink>) -> Self {
        Self {
            engine,
            sink,
            stop_flag: Arc::new(AtomicBool::new(false)),
            speaking: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn is_speaking(&self) -> bool {
        self.speaking.load(Ordering::SeqCst)
    }

    /// Drops the queue. Returns immediately.
    ///
    /// Stops the sink directly (in addition to setting the internal stop
    /// flag) because the thread running `speak()` is very often blocked
    /// inside `engine.synthesize()` — a call that can take over a second —
    /// and would not notice the flag until it returns. `AudioSink::stop()`
    /// is required to be safe to call from any thread and non-blocking, so
    /// calling it here silences audio immediately regardless of what the
    /// speaking thread is doing.
    pub fn stop(&self) {
        self.stop_flag.store(true, Ordering::SeqCst);
        self.sink.stop();
    }

    /// Synthesises sentence-by-sentence and plays each as it is ready, so
    /// audio starts after the first sentence rather than the whole text.
    /// Blocks until finished or stopped.
    ///
    /// Not reentrant: intended for a single caller at a time. Concurrent
    /// calls would interleave `sink.append()` calls and race on the shared
    /// `speaking`/`stop_flag` state; callers must serialize their own calls
    /// rather than relying on internal locking here.
    pub fn speak(&self, text: &str, lang: &str, speed: f32) -> Result<()> {
        let sentences = split_sentences(text);
        if sentences.is_empty() {
            return Ok(());
        }

        self.stop_flag.store(false, Ordering::SeqCst);
        self.speaking.store(true, Ordering::SeqCst);
        let result = self.run(&sentences, lang, speed);
        self.speaking.store(false, Ordering::SeqCst);
        result
    }

    fn run(&self, sentences: &[String], lang: &str, speed: f32) -> Result<()> {
        for sentence in sentences {
            if self.stopped() {
                self.sink.stop();
                return Ok(());
            }
            let pcm = self.engine.synthesize(sentence, lang, speed)?;
            if self.stopped() {
                self.sink.stop();
                return Ok(());
            }
            self.sink.append(pcm)?;

            // Keep at most one sentence buffered ahead, so stop stays
            // responsive and memory stays flat on long documents.
            if self.wait_for_drain(2)? {
                return Ok(());
            }
        }

        self.wait_for_drain(1)?;
        Ok(())
    }

    /// Blocks until `sink.queued()` drops below `threshold`, a stop is
    /// requested, or the device is presumed stalled.
    ///
    /// Returns `Ok(true)` if a stop landed while waiting (caller should
    /// unwind), `Ok(false)` if the threshold was reached normally, or
    /// `Err` if `queued()` did not change for `STALL_TIMEOUT` — the
    /// pragmatic guard against a lost/frozen audio device, which rodio
    /// surfaces no direct signal for through this API.
    fn wait_for_drain(&self, threshold: usize) -> Result<bool> {
        let mut last_queued = self.sink.queued();
        let mut last_changed = Instant::now();

        loop {
            if self.stopped() {
                self.sink.stop();
                return Ok(true);
            }

            let queued = self.sink.queued();
            if queued < threshold {
                return Ok(false);
            }

            if queued != last_queued {
                last_queued = queued;
                last_changed = Instant::now();
            } else if last_changed.elapsed() >= STALL_TIMEOUT {
                self.sink.stop();
                anyhow::bail!(
                    "audio device appears stalled: queue depth stuck at {queued} for {STALL_TIMEOUT:?}"
                );
            }

            std::thread::sleep(POLL_INTERVAL);
        }
    }

    fn stopped(&self) -> bool {
        self.stop_flag.load(Ordering::SeqCst)
    }
}
