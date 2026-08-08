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
    fn queued(&self) -> usize;
    fn stop(&self);
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
    }
}
