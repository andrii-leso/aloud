use crate::tts::Pcm;
use anyhow::{Context, Result};
use rodio::buffer::SamplesBuffer;
use rodio::{DeviceSinkBuilder, MixerDeviceSink, Player as RodioPlayer};
use std::num::NonZero;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// How long `RodioInner::append` will wait for a stopped rodio queue to
/// actually empty before declaring the device gone. Generous next to the
/// ~5-15ms a healthy device takes, and far short of the player's 30s stall
/// watchdog, which cannot help here — see `RodioInner::append`.
const STOP_DRAIN_TIMEOUT: Duration = Duration::from_secs(2);

/// Poll interval while waiting for that drain.
const STOP_DRAIN_POLL: Duration = Duration::from_millis(2);

/// Polls `depth` until it reports zero or `timeout` elapses; reports
/// whether it drained.
///
/// Seamed out as a free function taking a closure so the decision is
/// testable without an audio device — the same pattern as `StallWatch` in
/// `src/play/player.rs`.
fn wait_until_drained(depth: impl Fn() -> usize, timeout: Duration, poll: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if depth() == 0 {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(poll);
    }
}

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
    ///
    /// Implementations must also leave the sink ready to *accept* — a
    /// `stop()` immediately followed by an `append()` is the ordinary
    /// shape of a ⌘⇧A takeover, and it must not block indefinitely. See
    /// `RodioInner::append` for why that is not free.
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
    /// Set by `stop()`, cleared by the next `append()`. See `append`.
    stop_pending: AtomicBool,
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
            stop_pending: AtomicBool::new(false),
        })
    }

    /// Queues a buffer for playback.
    ///
    /// **The third rodio contract**, after Phase 1's two. `Player::append`
    /// begins with an *unbounded* blocking wait when a stop is outstanding
    /// and buffers are still counted (rodio-0.22.2 `src/player.rs:109-115`
    /// calls `sleep_until_end()`, a bare `Receiver::recv()` with no
    /// timeout at `player.rs:313-316`). `stop()` there only sets a flag
    /// (`player.rs:301`); the queue is actually emptied on the audio
    /// output thread, in the `periodic_access` callback, and `sound_count`
    /// falls only as the mixer pulls the queued samples. On a healthy
    /// device that is 5-15ms and invisible.
    ///
    /// On a device that has stopped consuming — a Bluetooth headset
    /// dropping mid-read, a DAC unplugged — it never returns at all. The
    /// thread then never reaches `Player::wait_for_drain`, so the 30s
    /// stall watchdog, the app's only guard for exactly this condition,
    /// never runs: `App`'s busy flag is held by a parked thread and Aloud
    /// is silently bricked until it is relaunched.
    ///
    /// ⌘⇧A's takeover turned stop→append from a rare manual sequence into
    /// an automatic one on every different-selection press, which is what
    /// puts this on the ordinary path. Doing the wait ourselves, bounded,
    /// turns a permanent hang back into an ordinary `Err` that unwinds the
    /// read, releases the busy flag and surfaces in the tray.
    fn append(&self, pcm: Pcm) -> Result<()> {
        if self.stop_pending.swap(false, Ordering::SeqCst)
            && !wait_until_drained(|| self.player.len(), STOP_DRAIN_TIMEOUT, STOP_DRAIN_POLL)
        {
            anyhow::bail!(
                "audio device appears to have gone away: the stopped queue did not clear \
                 within {STOP_DRAIN_TIMEOUT:?}"
            );
        }

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
        // And the queue is not emptied here either — only marked. The
        // next `append` has to reckon with that; see its doc comment.
        self.stop_pending.store(true, Ordering::SeqCst);
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

#[cfg(test)]
mod tests {
    use super::{wait_until_drained, STOP_DRAIN_POLL, STOP_DRAIN_TIMEOUT};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    #[test]
    fn an_already_empty_queue_drains_immediately() {
        assert!(wait_until_drained(
            || 0,
            STOP_DRAIN_TIMEOUT,
            STOP_DRAIN_POLL
        ));
    }

    #[test]
    fn a_queue_that_clears_shortly_after_the_stop_is_waited_for() {
        // The healthy case: rodio's audio thread empties the stopped
        // queue a few polls later, and `append` must go ahead once it has.
        let depth = AtomicUsize::new(3);
        assert!(wait_until_drained(
            || depth.fetch_sub(1, Ordering::SeqCst).saturating_sub(1),
            STOP_DRAIN_TIMEOUT,
            Duration::from_millis(1)
        ));
    }

    #[test]
    fn a_queue_that_never_clears_gives_up_instead_of_blocking_forever() {
        // The device-gone case. rodio's own `append` would sit in
        // `sleep_until_end()` here with no timeout at all, taking the
        // busy flag and the stall watchdog down with it.
        let started = Instant::now();
        assert!(!wait_until_drained(
            || 2,
            Duration::from_millis(60),
            Duration::from_millis(2)
        ));
        assert!(
            started.elapsed() >= Duration::from_millis(60),
            "it gave up before the timeout"
        );
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "it did not give up anywhere near the timeout"
        );
    }
}
