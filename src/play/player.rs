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
/// at `LATER_CHUNK_CHARS` = 300 (`src/text/chunk.rs`), which is roughly 20s
/// of audio at speed 1.0 — so the margin here is thinner than the old ~8s
/// figure implied. At the 0.7 speed floor a 300-char chunk runs ~28s against
/// this 30s timeout. That is pre-existing and unchanged by the 2026-08-10
/// speed fix (0.7 produced an equally long buffer before it), but it is only
/// 1-4s of headroom on a content-dependent estimate: do not raise
/// `LATER_CHUNK_CHARS` or lower the speed floor without revisiting this.
const STALL_TIMEOUT: Duration = Duration::from_secs(30);

/// The stall watchdog's state, extracted from `wait_for_drain` so the one
/// decision that matters — "is this frozen queue a dead device, or a
/// paused one?" — is unit-testable against a synthetic clock instead of
/// requiring a test to sit through `STALL_TIMEOUT` of wall time.
///
/// Same pattern as `shortcut::plan_apply` and `aloud.rs`'s
/// `apply_shortcut_with`/`take_probe`: seam out the decision, leave the
/// I/O loop around it thin.
pub struct StallWatch {
    last_queued: usize,
    last_changed: Instant,
}

impl StallWatch {
    pub fn new(queued: usize, now: Instant) -> Self {
        Self {
            last_queued: queued,
            last_changed: now,
        }
    }

    /// Records one poll and reports whether the device is presumed dead.
    ///
    /// While `paused`, the deadline is carried forward on every tick, so
    /// time spent paused does not accumulate toward `STALL_TIMEOUT` at
    /// all. Without this the guard cannot tell a dead device from a
    /// deliberately paused one — a paused sink's depth is frozen *by
    /// definition* — and any pause longer than 30s aborted the read with
    /// "audio device appears stalled".
    ///
    /// Note what is deliberately not done here: the timeout is not
    /// raised. Raising it would weaken the guard for a real dead device
    /// and still break on a long enough pause. Resuming starts a fresh
    /// full budget, which is the honest reading — a stall is 30s of no
    /// progress *while playback is expected*.
    pub fn observe(&mut self, queued: usize, paused: bool, now: Instant) -> bool {
        if paused {
            self.last_queued = queued;
            self.last_changed = now;
            return false;
        }
        if queued != self.last_queued {
            self.last_queued = queued;
            self.last_changed = now;
            return false;
        }
        now.duration_since(self.last_changed) >= STALL_TIMEOUT
    }
}

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
        // Also clears the paused state — see the `AudioSink::stop`
        // contract. Stop-while-paused must not leave a sink that
        // swallows the next utterance.
        self.sink.stop();
    }

    /// Whether audio output is currently halted mid-utterance.
    ///
    /// Delegates to the sink rather than mirroring the state in a local
    /// flag. There is exactly one source of truth, so no tray label,
    /// hotkey, or future Now Playing state can drift from what the audio
    /// device is actually doing — the defect class this app has spent the
    /// week removing.
    pub fn is_paused(&self) -> bool {
        self.sink.is_paused()
    }

    /// Halts output, keeping every already-synthesised buffer.
    ///
    /// **No-op unless something is speaking.** Pausing an idle sink would
    /// park a paused-and-empty audio device that silently eats the next
    /// read — the tray item and the hotkey are both reachable at any
    /// time, so this has to be refused here rather than assumed away.
    ///
    /// Like `stop()`, this takes effect even while the speaking thread is
    /// blocked inside `engine.synthesize()`: it acts on the sink, not on
    /// the loop. Returns the resulting paused state.
    pub fn pause(&self) -> bool {
        if !self.is_speaking() {
            return false;
        }
        self.sink.pause();
        true
    }

    /// Resumes from the exact sample `pause()` stopped at. Returns the
    /// resulting paused state (always `false`).
    pub fn resume(&self) -> bool {
        self.sink.resume();
        false
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
        // A new utterance never begins paused. `pause()` refuses when
        // nothing is speaking, but that check and the end of the previous
        // utterance can interleave: a pause landing just as `run()` drains
        // its last buffer would leave the sink paused with nothing
        // playing, and this read would then be silent with a queue depth
        // that never moves. Clearing it here makes that unrepresentable
        // rather than merely unlikely.
        self.sink.resume();
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
        let mut watch = StallWatch::new(self.sink.queued(), Instant::now());

        loop {
            if self.stopped() {
                self.sink.stop();
                return Ok(true);
            }

            let queued = self.sink.queued();
            if queued < threshold {
                return Ok(false);
            }

            if watch.observe(queued, self.sink.is_paused(), Instant::now()) {
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
