//! ITU-R BS.1770-4 loudness + true-peak (plan §1.1, §4.4).
//!
//! Shared by the analyzer (#2) and the finish stage (#14). Integrated loudness
//! is K-weighted, block-gated LUFS; true peak is estimated from 4× oversampling.
//! The K-weighting coefficients are designed at the buffer's own sample rate via
//! the bilinear transform, so this is correct at 44.1/48/88.2/96/176.4/192 kHz.

use crate::audio::AudioBuffer;
use crate::dsp::Biquad;

/// BS.1770 stage-1 high-shelf ("pre-filter"), designed at `sample_rate`.
fn k_pre_filter(sample_rate: f32) -> Biquad {
    // Analog prototype constants from the standard (as used by pyloudnorm).
    let f0 = 1681.9745_f32;
    let g = 3.9998438_f32;
    let q = 0.70717525_f32;
    let k = (std::f32::consts::PI * f0 / sample_rate).tan();
    let vh = 10f32.powf(g / 20.0);
    let vb = vh.powf(0.49966678_f32);
    let a0 = 1.0 + k / q + k * k;
    Biquad::from_coeffs(
        (vh + vb * k / q + k * k) / a0,
        2.0 * (k * k - vh) / a0,
        (vh - vb * k / q + k * k) / a0,
        1.0,
        2.0 * (k * k - 1.0) / a0,
        (1.0 - k / q + k * k) / a0,
    )
}

/// BS.1770 stage-2 "RLB" high-pass, designed at `sample_rate`.
fn k_rlb_filter(sample_rate: f32) -> Biquad {
    let f0 = 38.13547_f32;
    let q = 0.50032705_f32;
    let k = (std::f32::consts::PI * f0 / sample_rate).tan();
    let a0 = 1.0 + k / q + k * k;
    Biquad::from_coeffs(1.0, -2.0, 1.0, a0, 2.0 * (k * k - 1.0), 1.0 - k / q + k * k)
}

/// K-weight one channel (pre-filter then RLB high-pass).
fn k_weight(channel: &[f32], sample_rate: f32) -> Vec<f32> {
    let mut pre = k_pre_filter(sample_rate);
    let mut rlb = k_rlb_filter(sample_rate);
    channel
        .iter()
        .map(|&x| rlb.process(pre.process(x)))
        .collect()
}

/// Integrated loudness in LUFS per BS.1770-4 (400 ms blocks, 75 % overlap,
/// −70 LUFS absolute gate then −10 LU relative gate). Returns `None` for
/// signals with no gated content (e.g. silence).
pub fn integrated_lufs(buffer: &AudioBuffer) -> Option<f32> {
    let sr = buffer.sample_rate() as f32;
    let weighted: Vec<Vec<f32>> = buffer.channels().iter().map(|c| k_weight(c, sr)).collect();

    let block = (0.400 * sr).round() as usize;
    let step = (0.100 * sr).round() as usize;
    if block == 0 || buffer.frames() < block {
        return None;
    }

    // Per-block mean-square energy z (channel weights G = 1.0 for mono / L / R).
    let mut zs: Vec<f32> = Vec::new();
    let frames = buffer.frames();
    let mut start = 0;
    while start + block <= frames {
        let mut z = 0.0f32;
        for ch in &weighted {
            let seg = &ch[start..start + block];
            z += seg.iter().map(|x| x * x).sum::<f32>() / block as f32;
        }
        zs.push(z);
        start += step;
    }
    if zs.is_empty() {
        return None;
    }

    let loudness = |z: f32| -0.691 + 10.0 * (z.max(1e-12)).log10();

    // Absolute gate at −70 LUFS.
    let gated_abs: Vec<f32> = zs
        .iter()
        .copied()
        .filter(|&z| loudness(z) >= -70.0)
        .collect();
    if gated_abs.is_empty() {
        return None;
    }

    // Relative gate at (integrated of abs-gated − 10 LU).
    let mean_abs = gated_abs.iter().sum::<f32>() / gated_abs.len() as f32;
    let rel_thresh = loudness(mean_abs) - 10.0;
    let gated: Vec<f32> = gated_abs
        .into_iter()
        .filter(|&z| loudness(z) >= rel_thresh)
        .collect();
    if gated.is_empty() {
        return None;
    }

    let mean = gated.iter().sum::<f32>() / gated.len() as f32;
    Some(loudness(mean))
}

/// True-peak in dBTP, estimated by 4× oversampling each channel with a
/// windowed-sinc polyphase interpolator and taking the max absolute value.
pub fn true_peak_dbtp(buffer: &AudioBuffer) -> f32 {
    let mut peak = 0.0f32;
    for ch in buffer.channels() {
        peak = peak.max(oversample_peak(ch));
    }
    20.0 * peak.max(1e-9).log10()
}

const OS: usize = 4;
const TAPS_PER_PHASE: usize = 8;

/// Peak absolute value of a channel after 4× polyphase sinc upsampling. The
/// inter-sample peak can only exceed the sample peak in the neighborhood of an
/// already-large sample, so interpolation is restricted to centers above half
/// the sample peak — exact for the maximum, far cheaper than upsampling all.
fn oversample_peak(x: &[f32]) -> f32 {
    let sample_peak = x.iter().fold(0.0f32, |m, &v| m.max(v.abs()));
    let mut peak = sample_peak;
    if x.len() < TAPS_PER_PHASE || sample_peak == 0.0 {
        return peak;
    }
    let gate = 0.5 * sample_peak;
    let half = TAPS_PER_PHASE as isize / 2;
    for center in half..(x.len() as isize - half) {
        if x[center as usize].abs() < gate {
            continue;
        }
        for p in 1..OS {
            let frac = p as f32 / OS as f32;
            let mut acc = 0.0f32;
            for t in -half..half {
                let idx = (center + t) as usize;
                let m = t as f32 - frac;
                acc += x[idx] * sinc_window(m);
            }
            peak = peak.max(acc.abs());
        }
    }
    peak
}

/// Hann-windowed sinc sample at offset `m` (in samples) within ±`TAPS_PER_PHASE/2`.
fn sinc_window(m: f32) -> f32 {
    let half = TAPS_PER_PHASE as f32 / 2.0;
    if m.abs() >= half {
        return 0.0;
    }
    let sinc = if m.abs() < 1e-6 {
        1.0
    } else {
        (std::f32::consts::PI * m).sin() / (std::f32::consts::PI * m)
    };
    let w = 0.5 * (1.0 + (std::f32::consts::PI * m / half).cos());
    sinc * w
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::SampleEncoding;

    fn sine_buf(amp: f32, freq: f32, sr: u32, secs: f32) -> AudioBuffer {
        let n = (sr as f32 * secs) as usize;
        let ch: Vec<f32> = (0..n)
            .map(|i| amp * (2.0 * std::f32::consts::PI * freq * i as f32 / sr as f32).sin())
            .collect();
        AudioBuffer::new(sr, 16, SampleEncoding::Int, vec![ch])
    }

    #[test]
    fn integrated_lufs_of_known_sine_is_in_band() {
        // A 1 kHz sine at −20 dBFS amplitude reads ≈ −23 LUFS (K-weight ~0 dB
        // at 1 kHz, −3 dB RMS-vs-peak, −0.691 offset).
        let amp = 10f32.powf(-20.0 / 20.0);
        let buf = sine_buf(amp, 1000.0, 48_000, 3.0);
        let l = integrated_lufs(&buf).expect("loudness");
        assert!((l - -23.0).abs() < 1.5, "expected ≈ −23 LUFS, got {l}");
    }

    #[test]
    fn silence_has_no_integrated_loudness() {
        let buf = AudioBuffer::new(48_000, 16, SampleEncoding::Int, vec![vec![0.0; 48_000]]);
        assert!(integrated_lufs(&buf).is_none());
    }

    #[test]
    fn true_peak_of_full_scale_sine_is_near_zero_dbtp() {
        let buf = sine_buf(1.0, 997.0, 48_000, 0.5);
        let tp = true_peak_dbtp(&buf);
        // Inter-sample peaks push a full-scale sine slightly above 0 dBFS.
        assert!(tp > -0.5 && tp < 1.5, "true peak {tp} dBTP out of range");
    }

    #[test]
    fn louder_signal_reads_higher() {
        let quiet = sine_buf(0.1, 1000.0, 48_000, 2.0);
        let loud = sine_buf(0.4, 1000.0, 48_000, 2.0);
        let lq = integrated_lufs(&quiet).unwrap();
        let ll = integrated_lufs(&loud).unwrap();
        assert!(ll > lq + 8.0, "4× amplitude ≈ +12 LU: {lq} -> {ll}");
    }
}
