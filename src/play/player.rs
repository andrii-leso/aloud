use crate::play::sink::AudioSink;
use crate::text::chunk::split_sentences;
use crate::tts::TtsEngine;
use anyhow::Result;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

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
    pub fn stop(&self) {
        self.stop_flag.store(true, Ordering::SeqCst);
    }

    /// Synthesises sentence-by-sentence and plays each as it is ready, so
    /// audio starts after the first sentence rather than the whole text.
    /// Blocks until finished or stopped.
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
            while self.sink.queued() > 1 {
                if self.stopped() {
                    self.sink.stop();
                    return Ok(());
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        }

        while self.sink.queued() > 0 {
            if self.stopped() {
                self.sink.stop();
                return Ok(());
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        Ok(())
    }

    fn stopped(&self) -> bool {
        self.stop_flag.load(Ordering::SeqCst)
    }
}
