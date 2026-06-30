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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::SampleEncoding;

    fn buf(samples: Vec<f32>) -> AudioBuffer {
        AudioBuffer::new(48_000, 24, SampleEncoding::Int, vec![samples])
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
