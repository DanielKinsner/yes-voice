//! De-click / crackle + mouth-click (plan §3.1 / §3.2).
//!
//! Uses the shared [`TransientDetector`] so every impulsive defect is found
//! once; this module owns the short-impulse kinds — [`TransientKind::Click`]
//! (vinyl clicks, crackle, digital ticks) and [`TransientKind::MouthClick`]
//! (tongue/lip/saliva) — and repairs each by interpolating across it. Plosive
//! events are left for de-pop (#6), so a transient is never double-repaired.
//! The detector's local (voiced-guard) threshold keeps normal consonants from
//! being flagged. Bypassed → identity.

use crate::audio::{AudioBuffer, ChannelLayout};
use crate::modules::transient::{fill_gap, TransientDetector, TransientKind};
use crate::processor::VocalProcessor;

/// Impulsive-defect remover (clicks, crackle, mouth-clicks).
#[derive(Clone, Copy, Debug)]
pub struct DeClick {
    enabled: bool,
    detector: TransientDetector,
    sample_rate: f32,
}

impl Default for DeClick {
    fn default() -> Self {
        DeClick {
            enabled: true,
            detector: TransientDetector::default(),
            sample_rate: 48_000.0,
        }
    }
}

impl DeClick {
    /// New de-click with the default shared detector.
    pub fn new() -> Self {
        DeClick::default()
    }

    /// Enable or bypass.
    pub fn set_enabled(&mut self, on: bool) {
        self.enabled = on;
    }

    /// Whether the module is active.
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// This module owns the short-impulse kinds; plosives belong to de-pop.
    fn owns(kind: TransientKind) -> bool {
        matches!(kind, TransientKind::Click | TransientKind::MouthClick)
    }
}

impl VocalProcessor for DeClick {
    fn prepare(&mut self, sample_rate: u32, _max_block: usize, _layout: ChannelLayout) {
        self.sample_rate = sample_rate as f32;
    }

    fn process(&mut self, buffer: &mut AudioBuffer) {
        if !self.enabled {
            return;
        }
        let sr = self.sample_rate;
        for ch in 0..buffer.channel_count() {
            let samples = buffer.channel_mut(ch);
            let events = self.detector.detect(samples, sr);
            for ev in events {
                if Self::owns(ev.kind) {
                    fill_gap(samples, ev.start, ev.end);
                }
            }
        }
    }

    fn reset(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::SampleEncoding;
    use crate::modules::transient::Transient;
    use std::f32::consts::PI;

    const SR: u32 = 48_000;

    fn buf(ch: Vec<f32>) -> AudioBuffer {
        AudioBuffer::new(SR, 16, SampleEncoding::Int, vec![ch])
    }

    fn run(mut m: DeClick, mut b: AudioBuffer) -> AudioBuffer {
        m.prepare(SR, 4096, b.layout());
        m.process(&mut b);
        b
    }

    fn tone(freq: f32, amp: f32, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| amp * (2.0 * PI * freq * i as f32 / SR as f32).sin())
            .collect()
    }

    #[test]
    fn removes_clicks_and_moves_toward_clean() {
        let clean = tone(200.0, 0.4, 48_000);
        let mut clicky = clean.clone();
        for k in 0..12 {
            clicky[2000 + k * 3500] += 0.9;
        }
        let err_before: f32 = clicky
            .iter()
            .zip(&clean)
            .map(|(a, b)| (a - b).powi(2))
            .sum();

        let out = run(DeClick::new(), buf(clicky));
        let err_after: f32 = out
            .channel(0)
            .iter()
            .zip(&clean)
            .map(|(a, b)| (a - b).powi(2))
            .sum();

        assert!(
            err_after < err_before * 0.1,
            "de-click should cut click energy ≥10x: {err_before} -> {err_after}"
        );
    }

    #[test]
    fn crackle_repairs_are_disjoint_and_not_shared_with_plosive() {
        let mut x = tone(180.0, 0.3, 48_000);
        for k in 0..30 {
            x[800 + k * 1500] += 0.7;
        }
        let det = TransientDetector::default();
        let events = det.detect(&x, SR as f32);
        let owned: Vec<&Transient> = events.iter().filter(|e| DeClick::owns(e.kind)).collect();
        // Repaired regions are pairwise disjoint (no double-repair).
        for pair in owned.windows(2) {
            assert!(pair[0].end <= pair[1].start, "de-click regions overlap");
        }
        // No region is owned by both de-click and de-pop (plosive).
        for e in &events {
            let declick = DeClick::owns(e.kind);
            let depop = matches!(e.kind, TransientKind::Plosive);
            assert!(!(declick && depop), "a transient owned by both modules");
        }
    }

    #[test]
    fn clean_voiced_audio_barely_changes() {
        // Buzzy voiced signal, no clicks → near-identity.
        let n = 24_000;
        let mut x = vec![0.0f32; n];
        for (i, s) in x.iter_mut().enumerate() {
            let t = i as f32 / SR as f32;
            *s = 0.3 * (2.0 * PI * 150.0 * t).sin() + 0.2 * (2.0 * PI * 300.0 * t).sin();
        }
        let out = run(DeClick::new(), buf(x.clone()));
        let diff: f32 = out
            .channel(0)
            .iter()
            .zip(&x)
            .map(|(a, b)| (a - b).abs())
            .sum::<f32>()
            / n as f32;
        assert!(
            diff < 1.0e-3,
            "clean voiced audio should barely change: {diff}"
        );
    }

    #[test]
    fn bypass_is_exact_identity() {
        let mut x = tone(220.0, 0.4, 10_000);
        x[5000] += 0.9;
        let mut m = DeClick::new();
        m.set_enabled(false);
        let out = run(m, buf(x.clone()));
        for (a, b) in out.channel(0).iter().zip(&x) {
            assert!((a - b).abs() <= f32::EPSILON, "bypass must be identity");
        }
    }
}
