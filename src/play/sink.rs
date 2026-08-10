use crate::tts::Pcm;
use anyhow::{Context, Result};
use rodio::buffer::SamplesBuffer;
use rodio::{DeviceSinkBuilder, MixerDeviceSink, Player as RodioPlayer};
use std::num::NonZero;

/// Audio output. Trait-ified so the player is testable without a device,
/// and so rodio's API churn is contained to one file.
pub trait AudioSink: Send + Sync {
    fn append(&self, pcm: Pcm) -> Result<()>;
    /// Number of buffers outstanding, including the one currently playing
    /// (not just the ones still waiting behind it).
    ///
    /// **Frozen while paused** — a paused sink consumes no samples, so the
    /// depth cannot change. `Player::wait_for_drain` must not read that
    /// as a dead device; see `StallWatch` in `src/play/player.rs`.
    fn queued(&self) -> usize;
    /// Drops the queue **and clears the paused state**.
    ///
    /// The second half is not decoration. rodio's `stop()` empties the
    /// queue but leaves its pause flag exactly as it was, so a stop
    /// arriving while paused would leave a sink that is empty *and*
    /// paused — and the next `append()` would then play nothing, forever,
    /// with a queue depth that never moves. Implementations must leave
    /// the sink ready to play.
    fn stop(&self);
    /// Halts output without discarding anything already appended.
    ///
    /// Synthesis runs ahead of playback, so buffers are already queued
    /// (and one is part-played) when this lands. Neither may be dropped:
    /// `resume()` must continue from the exact sample, not restart the
    /// buffer. No effect if already paused.
    fn pause(&self);
    /// Resumes from wherever `pause()` stopped. No effect if not paused.
    fn resume(&self);
    fn is_paused(&self) -> bool;
}

pub struct RodioSink {
    // Playback stops when the device handle is dropped, so the handle
    // must be held here.
    inner: RodioInner,
}

impl RodioSink {
    pub fn new() -> Result<Self> {
        Ok(Self {
            inner: RodioInner::open()?,
        })
    }
}

impl AudioSink for RodioSink {
    fn append(&self, pcm: Pcm) -> Result<()> {
        self.inner.append(pcm)
    }
    fn queued(&self) -> usize {
        self.inner.queued()
    }
    fn stop(&self) {
        self.inner.stop()
    }
    fn pause(&self) {
        self.inner.pause()
    }
    fn resume(&self) {
        self.inner.resume()
    }
    fn is_paused(&self) -> bool {
        self.inner.is_paused()
    }
}

/// Holds the device handle (dropping it stops playback) and the rodio
/// `Player` queued sounds are appended to.
struct RodioInner {
    // Must be held for the lifetime of playback; dropping it kills audio.
    _device: MixerDeviceSink,
    player: RodioPlayer,
}

impl RodioInner {
    fn open() -> Result<Self> {
        let mut device = DeviceSinkBuilder::open_default_sink()
            .context("failed to open default audio device")?;
        device.log_on_drop(false);
        let player = RodioPlayer::connect_new(device.mixer());
        Ok(Self {
            _device: device,
            player,
        })
    }

    fn append(&self, pcm: Pcm) -> Result<()> {
        let channels = NonZero::new(1u16).expect("1 is non-zero");
        let sample_rate =
            NonZero::new(pcm.sample_rate).context("Pcm.sample_rate must not be zero")?;
        let buffer = SamplesBuffer::new(channels, sample_rate, pcm.samples);
        self.player.append(buffer);
        Ok(())
    }

    fn queued(&self) -> usize {
        self.player.len()
    }

    fn stop(&self) {
        self.player.stop();
        // rodio's `stop()` only sets its `stopped` flag; the `pause` flag
        // is a separate control it does not touch (rodio-0.22.2
        // `src/player.rs:264,301`). Without this line, Stop-while-paused
        // leaves a sink that will silently swallow the next utterance.
        // See the `AudioSink::stop` contract.
        self.player.play();
    }

    /// rodio pauses on the *output* side: the mixer wraps each source in
    /// `Pausable`, whose `next()` yields silence **without advancing the
    /// inner source** (rodio-0.22.2 `src/source/pausable.rs:85-95`). The
    /// queued `SamplesBuffer`s are therefore untouched and un-consumed —
    /// resume continues at the exact sample, mid-sentence, with nothing
    /// re-synthesised. That is what makes this a real pause rather than a
    /// stop-and-start.
    fn pause(&self) {
        self.player.pause();
    }

    /// rodio spells resume `play()`.
    fn resume(&self) {
        self.player.play();
    }

    fn is_paused(&self) -> bool {
        self.player.is_paused()
    }
}
