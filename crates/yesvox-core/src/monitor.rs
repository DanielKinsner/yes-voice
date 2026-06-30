//! A/B compare, the difference monitor, and reblend (plan §5.5) — the
//! "what was removed" surface, as pure buffer ops the [`crate::Session`] and the
//! UI share.
//!
//! The whole-chain difference monitor is the net residual `original − processed`
//! ("what was removed"); reblend mixes that removed signal back in as a single
//! global post-rack wet/dry; and loudness-matched A/B compares dry vs processed
//! at a common loudness so the comparison is about *character*, not level.

use crate::audio::AudioBuffer;

/// Apply a per-sample binary op across the common channels/length of two buffers,
/// inheriting `a`'s format.
fn binary_op(a: &AudioBuffer, b: &AudioBuffer, f: impl Fn(f32, f32) -> f32) -> AudioBuffer {
    let nch = a.channel_count().min(b.channel_count()).max(1);
    let channels: Vec<Vec<f32>> = (0..nch)
        .map(|ch| {
            a.channel(ch)
                .iter()
                .zip(b.channel(ch))
                .map(|(x, y)| f(*x, *y))
                .collect()
        })
        .collect();
    AudioBuffer::new(a.sample_rate(), a.bits_per_sample(), a.encoding(), channels)
}

/// "What was removed": the per-sample residual `original − processed`. Silent
/// where the chain changed nothing.
pub fn difference(original: &AudioBuffer, processed: &AudioBuffer) -> AudioBuffer {
    binary_op(original, processed, |o, p| o - p)
}

/// Reblend `amount` of the removed signal back in:
/// `(1 − amount)·processed + amount·original`. `amount` 0 = fully processed,
/// 1 = the original restored; clamped to `[0, 1]`.
pub fn reblend(processed: &AudioBuffer, original: &AudioBuffer, amount: f32) -> AudioBuffer {
    let t = amount.clamp(0.0, 1.0);
    binary_op(processed, original, |p, o| (1.0 - t) * p + t * o)
}

/// Linear gain that brings a signal at `from_lufs` to `to_lufs` — the offset for
/// loudness-matched A/B (plan §5.5).
pub fn loudness_match_gain(from_lufs: f32, to_lufs: f32) -> f32 {
    10f32.powf((to_lufs - from_lufs) / 20.0)
}

/// Scale every sample of `buffer` by `gain`, inheriting its format — the buffer op
/// behind "Volume Match" (audition the dry A at the processed B's loudness).
pub fn apply_gain(buffer: &AudioBuffer, gain: f32) -> AudioBuffer {
    let channels: Vec<Vec<f32>> = (0..buffer.channel_count().max(1))
        .map(|ch| buffer.channel(ch).iter().map(|s| s * gain).collect())
        .collect();
    AudioBuffer::new(
        buffer.sample_rate(),
        buffer.bits_per_sample(),
        buffer.encoding(),
        channels,
    )
}

/// Linear amplitude → dBFS, with a −120 dB floor for (near-)silence.
fn lin_to_db(x: f32) -> f32 {
    if x <= 1e-6 {
        -120.0
    } else {
        20.0 * x.log10()
    }
}

/// Per-bucket `(min, max)` peaks for drawing a waveform without plotting every
/// sample. `buckets` is the target horizontal resolution; the result has
/// `min(buckets, frames)` pairs (empty for an empty buffer or `buckets == 0`).
/// `ch` clamps into range.
pub fn waveform_peaks(buffer: &AudioBuffer, ch: usize, buckets: usize) -> Vec<(f32, f32)> {
    let ch = ch.min(buffer.channel_count().saturating_sub(1));
    let samples = buffer.channel(ch);
    let n = samples.len();
    if n == 0 || buckets == 0 {
        return Vec::new();
    }
    let buckets = buckets.min(n);
    (0..buckets)
        .map(|b| {
            let start = b * n / buckets;
            let end = (((b + 1) * n / buckets).max(start + 1)).min(n);
            let mut lo = f32::INFINITY;
            let mut hi = f32::NEG_INFINITY;
            for &s in &samples[start..end] {
                lo = lo.min(s);
                hi = hi.max(s);
            }
            (lo, hi)
        })
        .collect()
}

/// Peak level of channel `ch` in dBFS (the static IN/OUT L-R peak meter). `ch`
/// clamps into range.
pub fn channel_peak_db(buffer: &AudioBuffer, ch: usize) -> f32 {
    let ch = ch.min(buffer.channel_count().saturating_sub(1));
    let peak = buffer
        .channel(ch)
        .iter()
        .fold(0.0f32, |m, &s| m.max(s.abs()));
    lin_to_db(peak)
}

/// Whole-buffer RMS level of channel `ch` in dBFS (the static meter's average
/// bar). A moving readout additionally needs the playback clock (deferred).
pub fn channel_rms_db(buffer: &AudioBuffer, ch: usize) -> f32 {
    let ch = ch.min(buffer.channel_count().saturating_sub(1));
    let s = buffer.channel(ch);
    if s.is_empty() {
        return -120.0;
    }
    let mean_sq = s.iter().map(|x| x * x).sum::<f32>() / s.len() as f32;
    lin_to_db(mean_sq.sqrt())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::SampleEncoding;
    use std::f32::consts::PI;

    fn buf(samples: Vec<f32>) -> AudioBuffer {
        AudioBuffer::new(48_000, 24, SampleEncoding::Int, vec![samples])
    }

    #[test]
    fn waveform_peaks_bucket_and_channel_meters_read_levels() {
        // A full-scale ramp −1..1: the bucketed peaks span the whole range.
        let n = 48_000;
        let ramp: Vec<f32> = (0..n)
            .map(|i| -1.0 + 2.0 * i as f32 / (n - 1) as f32)
            .collect();
        let b = buf(ramp);
        let peaks = waveform_peaks(&b, 0, 100);
        assert_eq!(peaks.len(), 100, "one (min,max) pair per requested bucket");
        assert!(peaks.iter().all(|p| p.0 <= p.1), "each bucket min ≤ max");
        let gmin = peaks.iter().map(|p| p.0).fold(f32::INFINITY, f32::min);
        let gmax = peaks.iter().map(|p| p.1).fold(f32::NEG_INFINITY, f32::max);
        assert!(
            (gmin + 1.0).abs() < 1e-3 && (gmax - 1.0).abs() < 1e-3,
            "global peaks span the full ±1 range: {gmin}..{gmax}"
        );
        // More buckets than samples clamps gracefully (no empty/blown buckets).
        let short = buf(vec![0.1, -0.2, 0.3]);
        assert!(waveform_peaks(&short, 0, 100).len() <= 3);
        assert!(waveform_peaks(&short, 0, 0).is_empty());

        // −6 dBFS sine on L, silence on R.
        let amp = 10f32.powf(-6.0 / 20.0);
        let sine: Vec<f32> = (0..n)
            .map(|i| amp * (2.0 * PI * 1000.0 * i as f32 / 48_000.0).sin())
            .collect();
        let stereo = AudioBuffer::new(48_000, 24, SampleEncoding::Int, vec![sine, vec![0.0; n]]);
        assert!(
            (channel_peak_db(&stereo, 0) - (-6.0)).abs() < 0.2,
            "L peak ≈ −6 dBFS, got {}",
            channel_peak_db(&stereo, 0)
        );
        assert!(channel_peak_db(&stereo, 1) < -100.0, "R is silent");
        // RMS of a sine is ~3 dB below its peak.
        assert!((channel_rms_db(&stereo, 0) - (-9.0)).abs() < 0.3);
    }

    #[test]
    fn difference_is_original_minus_processed() {
        let original = buf(vec![1.0, 0.5, -0.4, 0.2]);
        let processed = buf(vec![0.8, 0.5, -0.2, 0.1]);
        let d = difference(&original, &processed);
        assert_eq!(d.channel(0), &[1.0 - 0.8, 0.0, -0.4 - -0.2, 0.2 - 0.1]);
        // Identical processing → silence.
        let z = difference(&original, &original);
        assert!(z.channel(0).iter().all(|&v| v == 0.0));
    }

    #[test]
    fn reblend_spans_processed_to_original() {
        let original = buf(vec![1.0, 0.0, -1.0]);
        let processed = buf(vec![0.4, 0.0, -0.4]);
        assert_eq!(
            reblend(&processed, &original, 0.0).channel(0),
            &[0.4, 0.0, -0.4]
        );
        assert_eq!(
            reblend(&processed, &original, 1.0).channel(0),
            &[1.0, 0.0, -1.0]
        );
        let mid = reblend(&processed, &original, 0.5);
        assert_eq!(mid.channel(0), &[0.7, 0.0, -0.7]);
        // Clamped.
        assert_eq!(
            reblend(&processed, &original, 2.0).channel(0),
            &[1.0, 0.0, -1.0]
        );
    }

    #[test]
    fn loudness_match_gain_offsets_in_db() {
        // Bring -20 LUFS up to -16 LUFS → +4 dB ≈ 1.585×.
        let g = loudness_match_gain(-20.0, -16.0);
        assert!((g - 1.5849).abs() < 1e-3, "gain {g}");
        // Same loudness → unity.
        assert!((loudness_match_gain(-14.0, -14.0) - 1.0).abs() < 1e-6);
    }
}
