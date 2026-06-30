//! De-reverb: light tier, tail-preserving (plan §4.1).
//!
//! A transparent light-tier dereverb that suppresses the late-reverberant tail
//! in the STFT domain. A per-bin running estimate of recent magnitude models
//! the decaying tail; the current frame has that estimate subtracted out. A
//! **direct-path protect** control keeps onsets (where the signal is far above
//! the tail estimate) untouched, and a **tail-preservation** tier scales the
//! subtraction down so sung sustains and natural tails survive — gentle by
//! default in singing mode. It auto-softens when the measured reverb time is
//! already low. Bypassed → identity.

use crate::audio::{AudioBuffer, ChannelLayout};
use crate::processor::VocalProcessor;
use rustfft::num_complex::Complex32;
use rustfft::{Fft, FftPlanner};
use std::sync::Arc;

/// Light-tier dereverb.
#[derive(Clone)]
pub struct DeReverb {
    enabled: bool,
    /// Base subtraction strength of the tail estimate.
    strength: f32,
    /// Tail preservation `[0, 1]`: higher keeps more of the tail/sustains.
    tail_preservation: f32,
    /// Direct-path protect: bins above this ratio over the tail estimate are
    /// left untouched (the onset/direct sound).
    direct_protect: f32,
    /// Spectral floor (minimum gain).
    floor: f32,
    /// Measured reverb time (RT60) for auto-soften; `0` = unknown.
    rt60: f32,
    frame: usize,
    hop: usize,
    window: Vec<f32>,
    fft: Arc<dyn Fft<f32>>,
    ifft: Arc<dyn Fft<f32>>,
    sample_rate: f32,
}

impl DeReverb {
    /// New dereverb with a base `strength` and `tail_preservation` tier.
    pub fn new(strength: f32, tail_preservation: f32) -> Self {
        let frame = 1024;
        let hop = 256;
        let window: Vec<f32> = (0..frame)
            .map(|n| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * n as f32 / frame as f32).cos())
            .collect();
        let mut planner = FftPlanner::new();
        let fft = planner.plan_fft_forward(frame);
        let ifft = planner.plan_fft_inverse(frame);
        DeReverb {
            enabled: true,
            strength,
            tail_preservation: tail_preservation.clamp(0.0, 1.0),
            direct_protect: 3.0,
            floor: 0.1,
            rt60: 0.0,
            frame,
            hop,
            window,
            fft,
            ifft,
            sample_rate: 48_000.0,
        }
    }

    /// Voiceover default: firmer dereverb (less tail preservation).
    pub fn voiceover() -> Self {
        DeReverb::new(1.2, 0.1)
    }

    /// Singing default: gentle, tail-preserving.
    pub fn singing() -> Self {
        DeReverb::new(0.8, 0.85)
    }

    /// Enable or bypass.
    pub fn set_enabled(&mut self, on: bool) {
        self.enabled = on;
    }

    /// Whether the module is active.
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Feed the measured RT60 so an already-dry input is softened automatically.
    pub fn set_rt60(&mut self, rt60_s: f32) {
        self.rt60 = rt60_s.max(0.0);
    }

    /// The measured RT60 (seconds) currently driving the auto-soften; `0.0` means
    /// unknown.
    pub fn rt60(&self) -> f32 {
        self.rt60
    }

    /// The effective subtraction after tail-preservation and the auto-soften.
    fn effective_gamma(&self) -> f32 {
        // Tail preservation backs the subtraction off (sustains survive).
        let mut gamma = self.strength * (1.0 - 0.85 * self.tail_preservation);
        // Auto-soften when the room is already dry (low RT60).
        if self.rt60 > 0.0 && self.rt60 < 0.3 {
            gamma *= self.rt60 / 0.3;
        }
        gamma.max(0.0)
    }
}

impl VocalProcessor for DeReverb {
    fn prepare(&mut self, sample_rate: u32, _max_block: usize, _layout: ChannelLayout) {
        self.sample_rate = sample_rate as f32;
    }

    fn process(&mut self, buffer: &mut AudioBuffer) {
        if !self.enabled {
            return;
        }
        let gamma = self.effective_gamma();
        if gamma <= 0.0 {
            return;
        }
        let frame = self.frame;
        let hop = self.hop;
        let nbins = frame / 2 + 1;
        // Tail preservation raises the gain floor: the tail is attenuated less,
        // so sung sustains and natural tails survive.
        let floor = (self.floor + 0.6 * self.tail_preservation).min(0.9);

        for ch in 0..buffer.channel_count() {
            let input = buffer.channel(ch).to_vec();
            let n = input.len();
            if n < frame {
                continue;
            }
            let mut out = vec![0.0f32; n];
            let mut norm = vec![0.0f32; n];
            // Per-bin slowly-decaying peak: the "recent direct level". Energy
            // that falls below it is the decaying late-reverb tail.
            let mut peak = vec![0.0f32; nbins];
            let peak_decay = (-(hop as f32) / (0.3 * self.sample_rate)).exp();
            let mut scratch = vec![Complex32::new(0.0, 0.0); frame];

            let mut pos = 0;
            while pos + frame <= n {
                for (i, sc) in scratch.iter_mut().enumerate() {
                    *sc = Complex32::new(input[pos + i] * self.window[i], 0.0);
                }
                self.fft.process(&mut scratch);

                for b in 0..nbins {
                    let mag = scratch[b].norm();
                    // Update the decaying peak first; steady/rising energy keeps
                    // mag ≈ peak (direct/sustain), a decay drops mag below it.
                    peak[b] = mag.max(peak_decay * peak[b]);
                    let excess = (peak[b] - mag).max(0.0); // the reverberant residual
                    let gain = if mag > self.direct_protect * excess.max(1e-9) {
                        1.0 // direct path / sustain — protect it
                    } else if mag > 1e-9 {
                        ((mag - gamma * excess) / mag).max(floor)
                    } else {
                        floor
                    };
                    scratch[b] *= gain;
                }
                for b in 1..nbins - 1 {
                    scratch[frame - b] = scratch[b].conj();
                }
                self.ifft.process(&mut scratch);
                let scale = 1.0 / frame as f32;
                for i in 0..frame {
                    out[pos + i] += scratch[i].re * scale * self.window[i];
                    norm[pos + i] += self.window[i] * self.window[i];
                }
                pos += hop;
            }
            let dst = buffer.channel_mut(ch);
            for i in 0..n {
                dst[i] = if norm[i] > 1e-6 {
                    out[i] / norm[i]
                } else {
                    input[i]
                };
            }
        }
    }

    fn reset(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::SampleEncoding;
    use std::f32::consts::PI;

    const SR: u32 = 48_000;

    fn buf(ch: Vec<f32>) -> AudioBuffer {
        AudioBuffer::new(SR, 16, SampleEncoding::Int, vec![ch])
    }

    fn run(mut m: DeReverb, mut b: AudioBuffer) -> AudioBuffer {
        m.prepare(SR, 4096, b.layout());
        m.process(&mut b);
        b
    }

    /// A dry tone burst (0.25 s) then silence, with a feedback-comb reverb tail
    /// added. The burst is the "direct" sound; the energy after it is reverb.
    fn reverberant(n: usize, burst: usize) -> Vec<f32> {
        let mut dry = vec![0.0f32; n];
        for (i, d) in dry.iter_mut().enumerate().take(burst) {
            *d = 0.5 * (2.0 * PI * 300.0 * i as f32 / SR as f32).sin();
        }
        let delay = 1200usize; // 25 ms
        let decay = 0.6;
        let mut wet = dry.clone();
        for i in delay..n {
            wet[i] += decay * wet[i - delay];
        }
        wet
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
    }

    #[test]
    fn reduces_the_reverb_tail() {
        let n = 48_000;
        let burst = 12_000;
        let wet = reverberant(n, burst);
        let out = run(DeReverb::voiceover(), buf(wet.clone()));
        // Tail region well after the burst.
        let tail_before = rms(&wet[burst + 4000..n]);
        let tail_after = rms(&out.channel(0)[burst + 4000..n]);
        assert!(
            tail_after < tail_before * 0.7,
            "reverb tail should drop: {tail_before} -> {tail_after}"
        );
    }

    #[test]
    fn singing_preserve_keeps_more_tail_than_voiceover() {
        let n = 48_000;
        let burst = 12_000;
        let wet = reverberant(n, burst);
        let vo = run(DeReverb::voiceover(), buf(wet.clone()));
        let sing = run(DeReverb::singing(), buf(wet.clone()));
        let tail_vo = rms(&vo.channel(0)[burst + 4000..n]);
        let tail_sing = rms(&sing.channel(0)[burst + 4000..n]);
        // Singing preserve keeps the tail well above the aggressive VO cut.
        let ratio_db = 20.0 * (tail_sing / tail_vo.max(1e-9)).log10();
        assert!(
            ratio_db >= 6.0,
            "singing tails must survive ≥6 dB above VO dereverb: {ratio_db} dB"
        );
    }

    #[test]
    fn protects_the_direct_path() {
        let n = 48_000;
        let burst = 12_000;
        let wet = reverberant(n, burst);
        let out = run(DeReverb::voiceover(), buf(wet.clone()));
        // The burst (direct sound) level is largely preserved.
        let direct_before = rms(&wet[2000..burst - 2000]);
        let direct_after = rms(&out.channel(0)[2000..burst - 2000]);
        assert!(
            direct_after > direct_before * 0.8,
            "direct path must be protected: {direct_before} -> {direct_after}"
        );
    }

    #[test]
    fn auto_softens_when_rt60_is_low() {
        let n = 48_000;
        let burst = 12_000;
        let wet = reverberant(n, burst);
        let mut dry_room = DeReverb::voiceover();
        dry_room.set_rt60(0.05); // already dry → soften hard
        let out = run(dry_room, buf(wet.clone()));
        // With auto-soften, it changes the signal far less.
        let diff = rms(&out
            .channel(0)
            .iter()
            .zip(&wet)
            .map(|(a, b)| a - b)
            .collect::<Vec<_>>());
        let full = run(DeReverb::voiceover(), buf(wet.clone()));
        let diff_full = rms(&full
            .channel(0)
            .iter()
            .zip(&wet)
            .map(|(a, b)| a - b)
            .collect::<Vec<_>>());
        assert!(
            diff < diff_full * 0.5,
            "low RT60 should soften the dereverb: {diff} vs {diff_full}"
        );
    }

    #[test]
    fn bypass_is_exact_identity() {
        let wet = reverberant(20_000, 6_000);
        let mut m = DeReverb::voiceover();
        m.set_enabled(false);
        let out = run(m, buf(wet.clone()));
        for (a, b) in out.channel(0).iter().zip(&wet) {
            assert!((a - b).abs() <= f32::EPSILON, "bypass must be identity");
        }
    }
}
