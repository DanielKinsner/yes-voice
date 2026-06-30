//! Band-limited resample island for the 48 kHz-only neural engine (plan §2.2).
//!
//! MossFormer2_SE_48K runs at 48 kHz only, so any other input rate is brought
//! to 48 kHz for the neural pass and carried back to the source rate afterwards.
//! This is a self-contained Kaiser-windowed-sinc resampler (Whittaker–Shannon
//! reconstruction with an anti-alias cutoff at the lower of the two Nyquists),
//! pure Rust with no new dependencies. Each output sample is the
//! running-normalized sum of windowed-sinc-weighted input taps, which gives a
//! flat passband and clean edges (truncated taps renormalize rather than
//! rolling off).
//!
//! [`to_48k`]/[`from_48k`] are the engine's entry points; `from_48k` fits the
//! result to an exact length so the whole-app same-length guarantee holds.

/// Zero crossings of the sinc kept on each side (kernel quality vs. cost).
const ZERO_CROSSINGS: f64 = 32.0;
/// Kaiser β — ~−90 dB stopband, transparent for speech.
const KAISER_BETA: f64 = 9.0;
/// Kernel-table samples per unit of input-sample lag (linear-interpolated).
const TABLE_DENSITY: f64 = 512.0;

/// A fixed-rate band-limited resampler from `in_sr` to `out_sr`.
pub struct Resampler {
    ratio: f64, // out_sr / in_sr
    half_width: f64,
    table: Vec<f32>, // windowed sinc for lag τ ∈ [0, half_width]
    identity: bool,
}

/// Modified Bessel function of the first kind, order 0 (series form).
fn bessel_i0(x: f64) -> f64 {
    let mut term = 1.0;
    let mut sum = 1.0;
    let half_x = x / 2.0;
    for m in 1..64 {
        term *= (half_x / m as f64).powi(2);
        sum += term;
        if term < 1e-12 * sum {
            break;
        }
    }
    sum
}

fn sinc(x: f64) -> f64 {
    if x.abs() < 1e-12 {
        1.0
    } else {
        let px = std::f64::consts::PI * x;
        px.sin() / px
    }
}

impl Resampler {
    /// Build a resampler from `in_sr` to `out_sr` (Hz). Equal rates are an exact
    /// pass-through.
    pub fn new(in_sr: u32, out_sr: u32) -> Self {
        let ratio = out_sr as f64 / in_sr as f64;
        if in_sr == out_sr {
            return Resampler {
                ratio: 1.0,
                half_width: 0.0,
                table: Vec::new(),
                identity: true,
            };
        }
        // Anti-alias cutoff (cycles/input-sample) at the lower Nyquist.
        let two_fc = ratio.min(1.0); // = 2·fc
        let half_width = ZERO_CROSSINGS / two_fc;

        let len = (half_width * TABLE_DENSITY).ceil() as usize + 2;
        let i0_beta = bessel_i0(KAISER_BETA);
        let table: Vec<f32> = (0..len)
            .map(|i| {
                let tau = i as f64 / TABLE_DENSITY; // lag in input samples
                let norm = (tau / half_width).min(1.0); // ∈ [0, 1]
                let kaiser = bessel_i0(KAISER_BETA * (1.0 - norm * norm).max(0.0).sqrt()) / i0_beta;
                (sinc(two_fc * tau) * kaiser) as f32
            })
            .collect();

        Resampler {
            ratio,
            half_width,
            table,
            identity: false,
        }
    }

    /// Resample `input`, returning ≈`round(input.len() · ratio)` samples.
    pub fn process(&self, input: &[f32]) -> Vec<f32> {
        if self.identity {
            return input.to_vec();
        }
        let n = input.len();
        let n_out = (n as f64 * self.ratio).round() as usize;
        let mut out = Vec::with_capacity(n_out);
        for j in 0..n_out {
            let center = j as f64 / self.ratio; // input position
            let k_lo = (center - self.half_width).ceil() as i64;
            let k_hi = (center + self.half_width).floor() as i64;
            let mut acc = 0.0f32;
            let mut wsum = 0.0f32;
            for k in k_lo..=k_hi {
                if k < 0 || k as usize >= n {
                    continue;
                }
                let h = self.weight((center - k as f64).abs());
                acc += input[k as usize] * h;
                wsum += h;
            }
            out.push(if wsum.abs() > 1e-12 { acc / wsum } else { 0.0 });
        }
        out
    }

    /// Windowed-sinc weight at lag `tau` (input samples), linear-interpolated.
    fn weight(&self, tau: f64) -> f32 {
        let idx = tau * TABLE_DENSITY;
        let i = idx.floor() as usize;
        if i + 1 >= self.table.len() {
            return 0.0;
        }
        let frac = (idx - i as f64) as f32;
        self.table[i] * (1.0 - frac) + self.table[i + 1] * frac
    }
}

/// Bring a signal up/down to 48 kHz for the neural pass.
pub fn to_48k(signal: &[f32], in_sr: u32) -> Vec<f32> {
    Resampler::new(in_sr, 48_000).process(signal)
}

/// Carry a 48 kHz signal back to `out_sr`, fitted to exactly `target_len`
/// samples (trim or zero-pad) so the same-length guarantee holds.
pub fn from_48k(signal_48k: &[f32], out_sr: u32, target_len: usize) -> Vec<f32> {
    let mut v = Resampler::new(48_000, out_sr).process(signal_48k);
    v.resize(target_len, 0.0);
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    fn tone(freq: f32, sr: f32, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| 0.5 * (2.0 * PI * freq * i as f32 / sr).sin())
            .collect()
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
    }

    #[test]
    fn equal_rates_are_exact_passthrough() {
        let x = tone(1000.0, 48_000.0, 4096);
        assert_eq!(Resampler::new(48_000, 48_000).process(&x), x);
    }

    #[test]
    fn output_length_follows_ratio() {
        let x = tone(1000.0, 24_000.0, 1000);
        assert_eq!(Resampler::new(24_000, 48_000).process(&x).len(), 2000);
        let y = tone(1000.0, 48_000.0, 2000);
        assert_eq!(Resampler::new(48_000, 16_000).process(&y).len(), 667);
    }

    #[test]
    fn coincident_samples_are_preserved_on_integer_upsample() {
        // 2× upsample: even outputs must equal the source samples (sinc zeros
        // land on the other integer taps).
        let x = tone(500.0, 24_000.0, 512);
        let y = Resampler::new(24_000, 48_000).process(&x);
        for i in 64..448 {
            assert!(
                (y[2 * i] - x[i]).abs() < 1e-4,
                "y[{}]={} x[{}]={}",
                2 * i,
                y[2 * i],
                i,
                x[i]
            );
        }
    }

    #[test]
    fn passband_tone_survives_round_trip() {
        // 1 kHz tone, 48k → 44.1k → 48k. Interior amplitude preserved (Kaiser
        // passband is flat) and waveform error small.
        let sr = 48_000.0;
        let x = tone(1000.0, sr, 48_000);
        let down = Resampler::new(48_000, 44_100).process(&x);
        let back = Resampler::new(44_100, 48_000).process(&down);
        let lo = 2000;
        let hi = back.len().min(x.len()) - 2000;
        // Goertzel power at 1 kHz preserved within ~0.5 dB.
        let p_in = crate::dsp::goertzel_power(&x[lo..hi], sr, 1000.0);
        let p_out = crate::dsp::goertzel_power(&back[lo..hi], sr, 1000.0);
        let d_db = 10.0 * (p_out.max(1e-12) / p_in.max(1e-12)).log10();
        assert!(d_db.abs() < 0.5, "passband level drifted {d_db} dB");
    }

    #[test]
    fn above_nyquist_tone_is_rejected_on_downsample() {
        // 12 kHz tone at 48k, downsample to 16k (Nyquist 8k). The anti-alias
        // lowpass kills it instead of folding it back as an alias.
        let x = tone(12_000.0, 48_000.0, 16_000);
        let down = Resampler::new(48_000, 16_000).process(&x);
        let interior = &down[200..down.len() - 200];
        assert!(
            rms(interior) < 0.05,
            "stopband tone should be ≥20 dB down, rms {}",
            rms(interior)
        );
    }

    #[test]
    fn from_48k_fits_exact_target_length() {
        let x = tone(440.0, 48_000.0, 5000);
        // Exactly the source length, regardless of resampler edge counts.
        assert_eq!(from_48k(&x, 44_100, 4096).len(), 4096);
        assert_eq!(from_48k(&x, 48_000, 9000).len(), 9000); // pad path
                                                            // to_48k/from_48k preserve a tone end-to-end at the original length.
        let src = tone(1000.0, 44_100.0, 22_050);
        let up = to_48k(&src, 44_100);
        let back = from_48k(&up, 44_100, src.len());
        assert_eq!(back.len(), src.len());
        let lo = 2000;
        let hi = src.len() - 2000;
        let p_in = crate::dsp::goertzel_power(&src[lo..hi], 44_100.0, 1000.0);
        let p_out = crate::dsp::goertzel_power(&back[lo..hi], 44_100.0, 1000.0);
        let d_db = 10.0 * (p_out.max(1e-12) / p_in.max(1e-12)).log10();
        assert!(d_db.abs() < 0.6, "round-trip level drifted {d_db} dB");
    }
}
