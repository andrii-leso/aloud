//! Real-audio sanity check for `RodioSink`. The faked player tests prove
//! orchestration but not that sound actually comes out of the device.
//!
//! Run with: cargo run --release --example beep

use aloud::play::sink::{AudioSink, RodioSink};
use aloud::tts::Pcm;
use std::f32::consts::PI;

fn main() -> anyhow::Result<()> {
    let sample_rate = 44_100u32;
    let freq = 440.0f32; // A4
    let duration_s = 1.0f32;
    let n = (sample_rate as f32 * duration_s) as usize;

    let samples: Vec<f32> = (0..n)
        .map(|i| {
            let t = i as f32 / sample_rate as f32;
            0.2 * (2.0 * PI * freq * t).sin()
        })
        .collect();

    let pcm = Pcm {
        samples,
        sample_rate,
        duration_s,
    };

    let sink = RodioSink::new()?;
    sink.append(pcm)?;

    println!("playing 1s sine wave at {freq} Hz...");
    while sink.queued() > 0 {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    // Give the device a moment to flush the final buffer before the sink
    // (and its stream handle) is dropped at the end of main.
    std::thread::sleep(std::time::Duration::from_millis(150));
    println!("done.");

    Ok(())
}
