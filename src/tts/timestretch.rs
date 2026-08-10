//! Changing how fast speech plays back, without changing what it says.
//!
//! Supertonic exposes a `speed` argument, but it implements it as
//! `duration /= speed` (`src/vendor/supertonic/mod.rs`, in `_infer`): the
//! duration predictor says how long the utterance needs to be, that number is
//! divided by the speed, and the shortened value sizes the latent the
//! flow-matching decoder renders into. Above roughly 1.1x the decoder no
//! longer has room for every phoneme and simply leaves some out — silently,
//! with no error and nothing in the log. At the 1.5x Andrii reads at, whole
//! words disappear from the middle of a sentence (2026-08-10; see
//! `docs/2026-08-10-text-drop-diagnosis.md`).
//!
//! So Aloud always synthesises at the engine's 1.0 — the only rate where the
//! decoder is guaranteed the canvas its own duration predictor asked for — and
//! changes the speed here instead, on the rendered audio.
//!
//! The method is SOLA (synchronised overlap-add): copy the waveform out in
//! overlapping segments, advancing through the input faster or slower than
//! through the output, and before each splice slide the read position a few
//! milliseconds either way to wherever it correlates best with what has
//! already been written. Aligning the splice to the waveform's own periodicity
//! is what keeps the pitch intact — resampling would have been three lines,
//! but it drops a male voice into soprano range at 1.5x. Cost is ~10 ms of CPU
//! per second of audio, off the latency path (it runs after synthesis, which
//! is three orders of magnitude slower).

/// Verbatim run copied per splice. Long enough to preserve formant structure,
/// short enough that the correlation search still tracks a moving pitch.
const SEGMENT_S: f32 = 0.040;
/// Crossfade length at each splice.
const OVERLAP_S: f32 = 0.008;
/// How far either side of the nominal read position the splice may slide to
/// find waveform-periodic alignment. Covers one full period down to ~100 Hz.
const SEEK_S: f32 = 0.010;

/// Speeds within this of 1.0 are treated as no change at all.
const UNITY_EPSILON: f32 = 1e-3;

/// Returns `samples` retimed by `speed`: 1.5 plays in two thirds of the time,
/// 0.8 takes a quarter longer. Pitch is preserved.
///
/// Returns the input unchanged at unity speed, and for clips too short to
/// splice (under ~60 ms) — there is nothing to overlap-add there.
pub fn time_stretch(samples: &[f32], sample_rate: u32, speed: f32) -> Vec<f32> {
    if !speed.is_finite() || speed <= 0.0 || (speed - 1.0).abs() < UNITY_EPSILON {
        return samples.to_vec();
    }

    let sr = sample_rate as f32;
    let segment = (SEGMENT_S * sr) as usize;
    let overlap = (OVERLAP_S * sr) as usize;
    let seek = (SEEK_S * sr) as usize;

    // Each splice reads `segment + overlap` samples starting at the chosen
    // position, and the search can push that start `seek` later.
    let block = segment + overlap;
    if segment == 0 || overlap == 0 || samples.len() < block + seek + 1 {
        return samples.to_vec();
    }

    // Output advances by `segment` per splice; the input advances by
    // `segment * speed`. That ratio is what retimes the audio.
    let analysis_hop = segment as f32 * speed;

    let mut out: Vec<f32> = samples[..block].to_vec();
    let mut nominal = 0.0f32;

    loop {
        nominal += analysis_hop;

        // Candidate window, clamped so the whole block stays in bounds.
        let centre = nominal.round();
        if centre < 0.0 {
            break;
        }
        let centre = centre as usize;
        let last_start = samples.len() - block;
        if centre > last_start {
            break;
        }
        let lo = centre.saturating_sub(seek);
        let hi = (centre + seek).min(last_start);

        let tail = &out[out.len() - overlap..];
        let start = best_alignment(samples, tail, lo, hi, overlap);

        // Crossfade the splice into the tail already written, then copy the
        // rest of the block verbatim.
        let head = out.len() - overlap;
        for i in 0..overlap {
            let w = (i as f32 + 0.5) / overlap as f32;
            out[head + i] = out[head + i] * (1.0 - w) + samples[start + i] * w;
        }
        out.extend_from_slice(&samples[start + overlap..start + block]);
    }

    out
}

/// Position in `lo..=hi` whose `overlap` samples best match `tail`, by
/// normalised cross-correlation. Normalising matters: without it the search
/// just walks to the loudest candidate instead of the best-aligned one.
fn best_alignment(samples: &[f32], tail: &[f32], lo: usize, hi: usize, overlap: usize) -> usize {
    let tail_energy: f32 = tail.iter().map(|s| s * s).sum();
    if tail_energy <= f32::EPSILON {
        // Silence correlates with nothing; keep the nominal position.
        return hi.min(lo + (hi - lo) / 2);
    }

    let mut best = lo;
    let mut best_score = f32::NEG_INFINITY;

    for start in lo..=hi {
        let cand = &samples[start..start + overlap];
        let mut dot = 0.0f32;
        let mut energy = 0.0f32;
        for i in 0..overlap {
            dot += tail[i] * cand[i];
            energy += cand[i] * cand[i];
        }
        // Equivalent to dot / sqrt(energy) up to the constant tail energy,
        // which does not vary across candidates.
        let score = if energy > f32::EPSILON {
            dot / energy.sqrt()
        } else {
            0.0
        };
        if score > best_score {
            best_score = score;
            best = start;
        }
    }

    best
}

#[cfg(test)]
mod tests {
    use super::time_stretch;

    const SR: u32 = 44_100;

    /// A steady tone, long enough to exercise many splices.
    fn tone(hz: f32, seconds: f32) -> Vec<f32> {
        let n = (SR as f32 * seconds) as usize;
        (0..n)
            .map(|i| (2.0 * std::f32::consts::PI * hz * i as f32 / SR as f32).sin())
            .collect()
    }

    /// Dominant frequency, by counting positive-going zero crossings.
    fn dominant_hz(samples: &[f32]) -> f32 {
        // Ignore the first and last 10% — splice edges live there.
        let a = samples.len() / 10;
        let b = samples.len() - samples.len() / 10;
        let w = &samples[a..b];
        let crossings = w.windows(2).filter(|p| p[0] <= 0.0 && p[1] > 0.0).count();
        crossings as f32 * SR as f32 / w.len() as f32
    }

    #[test]
    fn unity_speed_returns_the_input_untouched() {
        let x = tone(200.0, 0.5);
        assert_eq!(time_stretch(&x, SR, 1.0), x);
    }

    #[test]
    fn faster_speed_shortens_by_that_factor() {
        let x = tone(200.0, 2.0);
        let y = time_stretch(&x, SR, 1.5);
        let ratio = x.len() as f32 / y.len() as f32;
        assert!(
            (ratio - 1.5).abs() < 0.05,
            "expected ~1.5x shorter, got {ratio:.3} ({} -> {})",
            x.len(),
            y.len()
        );
    }

    #[test]
    fn slower_speed_lengthens_by_that_factor() {
        let x = tone(200.0, 2.0);
        let y = time_stretch(&x, SR, 0.75);
        let ratio = x.len() as f32 / y.len() as f32;
        assert!(
            (ratio - 0.75).abs() < 0.05,
            "expected ~0.75x, got {ratio:.3} ({} -> {})",
            x.len(),
            y.len()
        );
    }

    /// The point of the whole module. A resampler would move 200 Hz to 300 Hz
    /// at 1.5x; a time-stretch must leave it at 200 Hz.
    #[test]
    fn pitch_survives_the_speed_change() {
        let x = tone(200.0, 2.0);
        let before = dominant_hz(&x);
        let after = dominant_hz(&time_stretch(&x, SR, 1.5));
        assert!(
            (before - 200.0).abs() < 5.0,
            "test tone is not 200 Hz: {before:.1}"
        );
        assert!(
            (after - 200.0).abs() < 10.0,
            "pitch shifted: {before:.1} Hz -> {after:.1} Hz (a resampler would give ~300)"
        );
    }

    #[test]
    fn empty_and_tiny_inputs_are_returned_as_is() {
        assert!(time_stretch(&[], SR, 1.5).is_empty());
        let tiny = tone(200.0, 0.01);
        assert_eq!(time_stretch(&tiny, SR, 1.5), tiny);
    }

    #[test]
    fn silence_does_not_hang_or_produce_nonsense() {
        let x = vec![0.0f32; SR as usize];
        let y = time_stretch(&x, SR, 1.5);
        assert!(y.iter().all(|s| *s == 0.0));
        let ratio = x.len() as f32 / y.len() as f32;
        assert!((ratio - 1.5).abs() < 0.05, "got {ratio:.3}");
    }

    #[test]
    fn nonsense_speeds_are_refused_rather_than_panicking() {
        let x = tone(200.0, 0.5);
        assert_eq!(time_stretch(&x, SR, 0.0), x);
        assert_eq!(time_stretch(&x, SR, -1.0), x);
        assert_eq!(time_stretch(&x, SR, f32::NAN), x);
    }

    #[test]
    fn the_extremes_of_the_supported_range_stay_in_proportion() {
        let x = tone(200.0, 2.0);
        for speed in [0.7f32, 2.0] {
            let y = time_stretch(&x, SR, speed);
            let ratio = x.len() as f32 / y.len() as f32;
            assert!(
                (ratio - speed).abs() < 0.05,
                "speed {speed}: got {ratio:.3}"
            );
        }
    }
}
