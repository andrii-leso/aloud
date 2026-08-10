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
/// Crossfade length at each splice, and the window the correlation search
/// matches on. It must comfortably exceed one pitch period of the voices Aloud
/// ships, or the search locks onto a sub-period feature and the splices beat
/// against the pitch as a faint warble. M5 (male) sits at roughly 100-120 Hz,
/// an 8-10 ms period, so 8 ms — the original value — was inside the failure
/// condition for the voice Andrii actually uses. 20 ms covers two periods at
/// 100 Hz.
const OVERLAP_S: f32 = 0.020;
/// How far either side of the nominal read position the splice may slide to
/// find waveform-periodic alignment. Covers one full period down to ~100 Hz.
const SEEK_S: f32 = 0.010;

/// Speeds within this of 1.0 are treated as no change at all.
const UNITY_EPSILON: f32 = 1e-3;

/// Returns `samples` retimed by `speed`: 1.5 plays in two thirds of the time,
/// 0.8 takes a quarter longer. Pitch is preserved.
///
/// Returns the input unchanged at unity speed, and for clips too short to
/// splice — there is nothing to overlap-add there. That is anything under
/// ~70 ms, and anything under one block plus one analysis hop (~180 ms at
/// 2.0x), where the loop cannot run even once and the tail flush hands the
/// whole input straight back.
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
    // One past the last input sample already written out. Every splice ends
    // with a verbatim copy, so `out` always ends exactly on the input sample at
    // `consumed_end - 1` — which is what lets the flush below be a plain append.
    let mut consumed_end = block;
    let mut nominal = 0.0f32;
    let last_start = samples.len() - block;

    loop {
        // `nominal` is an independent accumulator: the correlation result below
        // is never fed back into it, so search deviation cannot accumulate into
        // drift. `speed > 0`, so this only ever grows.
        nominal += analysis_hop;

        // Candidate window, clamped so the whole block stays in bounds.
        let centre = nominal.round() as usize;
        if centre > last_start {
            break;
        }
        let lo = centre.saturating_sub(seek);
        let hi = (centre + seek).min(last_start);

        let tail = &out[out.len() - overlap..];
        let start = best_alignment(samples, tail, lo, hi, overlap, centre);

        // Crossfade the splice into the tail already written, then copy the
        // rest of the block verbatim.
        let head = out.len() - overlap;
        for i in 0..overlap {
            let w = (i as f32 + 0.5) / overlap as f32;
            out[head + i] = out[head + i] * (1.0 - w) + samples[start + i] * w;
        }
        out.extend_from_slice(&samples[start + overlap..start + block]);
        consumed_end = start + block;
    }

    // Flush the tail. The loop stops as soon as the next splice would read past
    // the end, which strands up to one analysis hop of input — ~90 ms at 2.0x.
    // Aloud synthesises one chunk per sentence, so without this every sentence
    // loses its last breath, and a final plosive ("...inside it.") loses its
    // consonant. That is the same defect class this module exists to remove.
    //
    // `out` ends exactly on input sample `consumed_end - 1`, so the remainder is
    // a seamless continuation of the same waveform: appended verbatim, there is
    // no splice and nothing to align, and no artifact is possible. It does mean
    // the last few milliseconds play at 1.0 rather than at `speed` — the same
    // is true of the first `block` samples, which are also emitted verbatim.
    // Both are fixed costs of a few tens of milliseconds, independent of how
    // long the input is.
    if consumed_end < samples.len() {
        out.extend_from_slice(&samples[consumed_end..]);
    }

    out
}

/// Position in `lo..=hi` whose `overlap` samples best match `tail`, by
/// normalised cross-correlation. Normalising matters: without it the search
/// just walks to the loudest candidate instead of the best-aligned one.
fn best_alignment(
    samples: &[f32],
    tail: &[f32],
    lo: usize,
    hi: usize,
    overlap: usize,
    nominal_centre: usize,
) -> usize {
    let tail_energy: f32 = tail.iter().map(|s| s * s).sum();
    if tail_energy <= f32::EPSILON {
        // Silence correlates with nothing, so there is no alignment to find.
        // Stay on the nominal position rather than drifting off it.
        return nominal_centre.clamp(lo, hi);
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
        let x = tone(200.0, 1.0);
        assert_eq!(time_stretch(&x, SR, 1.0), x);
    }

    /// Clips are 4 s because the head and tail are each emitted verbatim at 1.0
    /// (see `time_stretch`), which biases the ratio by a fixed number of
    /// samples — tens of milliseconds — regardless of length. On a 4 s clip
    /// that is well inside the tolerance; on a 0.2 s clip it would not be, and
    /// the tolerance would have to be loosened to hide it rather than the test
    /// being honest about what it measures.
    const RATIO_CLIP_S: f32 = 4.0;

    #[test]
    fn faster_speed_shortens_by_that_factor() {
        let x = tone(200.0, RATIO_CLIP_S);
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
        let x = tone(200.0, RATIO_CLIP_S);
        let y = time_stretch(&x, SR, 0.75);
        let ratio = x.len() as f32 / y.len() as f32;
        assert!(
            (ratio - 0.75).abs() < 0.05,
            "expected ~0.75x, got {ratio:.3} ({} -> {})",
            x.len(),
            y.len()
        );
    }

    /// The loop can only splice while a whole block remains ahead of it, which
    /// strands up to one analysis hop of input — ~90 ms at 2.0x. Aloud
    /// synthesises one chunk per sentence, so that is the end of every sentence
    /// the owner hears, and a final plosive loses its consonant.
    #[test]
    fn the_end_of_the_input_is_never_clipped() {
        for speed in [0.7f32, 1.25, 1.5, 2.0] {
            let x = tone(200.0, RATIO_CLIP_S);
            let y = time_stretch(&x, SR, speed);
            let n = 256;
            assert!(
                y.ends_with(&x[x.len() - n..]),
                "speed {speed}: output does not end on the input's final {n} samples"
            );
        }
    }

    /// A speech-shaped check on the same property: a burst of energy right at
    /// the end (a final consonant) must survive.
    #[test]
    fn a_final_burst_is_not_swallowed() {
        for speed in [1.5f32, 2.0] {
            let mut x = tone(200.0, RATIO_CLIP_S);
            let burst = x.len() - (0.030 * SR as f32) as usize;
            for (i, s) in x[burst..].iter_mut().enumerate() {
                *s = if i % 2 == 0 { 0.9 } else { -0.9 };
            }
            let y = time_stretch(&x, SR, speed);
            let tail_peak = y[y.len() - (0.030 * SR as f32) as usize..]
                .iter()
                .fold(0.0f32, |m, s| m.max(s.abs()));
            assert!(
                tail_peak > 0.8,
                "speed {speed}: final burst missing from the output (peak {tail_peak:.3})"
            );
        }
    }

    /// The point of the whole module. A resampler would move 200 Hz to 300 Hz
    /// at 1.5x; a time-stretch must leave it at 200 Hz.
    #[test]
    fn pitch_survives_the_speed_change() {
        let x = tone(200.0, RATIO_CLIP_S);
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

    /// 200 Hz is a 5 ms period and sits comfortably inside any sane crossfade,
    /// so it does not exercise the shipped voice's worst case. M5 — the voice
    /// Andrii reads with — is male, around 100-120 Hz, an 8-10 ms period. That
    /// is the case that punishes a correlation window shorter than one period.
    #[test]
    fn pitch_survives_at_a_male_fundamental() {
        for hz in [105.0f32, 120.0] {
            for speed in [1.5f32, 2.0] {
                let x = tone(hz, RATIO_CLIP_S);
                let after = dominant_hz(&time_stretch(&x, SR, speed));
                assert!(
                    (after - hz).abs() < hz * 0.05,
                    "{hz} Hz at {speed}x came out at {after:.1} Hz"
                );
            }
        }
    }

    #[test]
    fn empty_and_tiny_inputs_are_returned_as_is() {
        assert!(time_stretch(&[], SR, 1.5).is_empty());
        let tiny = tone(200.0, 0.01);
        assert_eq!(time_stretch(&tiny, SR, 1.5), tiny);
    }

    #[test]
    fn silence_does_not_hang_or_produce_nonsense() {
        let x = vec![0.0f32; (SR as f32 * RATIO_CLIP_S) as usize];
        let y = time_stretch(&x, SR, 1.5);
        assert!(y.iter().all(|s| *s == 0.0));
        let ratio = x.len() as f32 / y.len() as f32;
        assert!((ratio - 1.5).abs() < 0.05, "got {ratio:.3}");
    }

    #[test]
    fn nonsense_speeds_are_refused_rather_than_panicking() {
        let x = tone(200.0, 1.0);
        assert_eq!(time_stretch(&x, SR, 0.0), x);
        assert_eq!(time_stretch(&x, SR, -1.0), x);
        assert_eq!(time_stretch(&x, SR, f32::NAN), x);
    }

    #[test]
    fn the_extremes_of_the_supported_range_stay_in_proportion() {
        let x = tone(200.0, RATIO_CLIP_S);
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
