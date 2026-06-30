//! Offline denoise: engine seam + worker pool + long-file streaming (plan
//! §1.4, §1.5, §2.1).
//!
//! `Denoise` is the [`crate::processor::VocalProcessor`] that wires it together:
//! the file is split into overlapping chunks, the chunks are denoised on a
//! [`WorkerPool`] (so heavy inference never runs on the audio path), and the
//! results are equal-power crossfaded back together. A RAM preflight refuses to
//! blow up memory on very long files. The denoiser itself is hidden behind the
//! [`Denoiser`] seam so the neural DFN3 engine can replace the v1 spectral
//! engine without touching any of this.

pub mod engine;
pub mod pool;
pub mod stream;

pub use engine::{Denoiser, SpectralDenoiser};
pub use pool::WorkerPool;
pub use stream::{ChunkPolicy, RamPreflight};

use crate::audio::{AudioBuffer, ChannelLayout};
use crate::processor::VocalProcessor;
use std::sync::Arc;

/// The offline denoise processor.
pub struct Denoise {
    enabled: bool,
    denoiser: Arc<dyn Denoiser>,
    pool: WorkerPool,
    policy: ChunkPolicy,
    sample_rate: f32,
}

impl Denoise {
    /// New denoise stage with the v1 spectral engine and `workers` threads.
    pub fn new(workers: usize) -> Self {
        Denoise {
            enabled: true,
            denoiser: Arc::new(SpectralDenoiser::default()),
            pool: WorkerPool::new(workers),
            policy: ChunkPolicy::default(),
            sample_rate: 48_000.0,
        }
    }

    /// New denoise stage with a custom engine behind the [`Denoiser`] seam.
    pub fn with_engine(workers: usize, denoiser: Arc<dyn Denoiser>) -> Self {
        Denoise {
            enabled: true,
            denoiser,
            pool: WorkerPool::new(workers),
            policy: ChunkPolicy::default(),
            sample_rate: 48_000.0,
        }
    }

    /// Enable or bypass.
    pub fn set_enabled(&mut self, on: bool) {
        self.enabled = on;
    }

    /// Whether the module is active.
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Estimate memory and check it against a budget before processing.
    pub fn preflight(&self, frames: usize, channels: usize, available_bytes: u64) -> RamPreflight {
        self.policy
            .preflight(frames, channels, self.sample_rate, available_bytes)
    }
}

impl VocalProcessor for Denoise {
    fn prepare(&mut self, sample_rate: u32, _max_block: usize, _layout: ChannelLayout) {
        self.sample_rate = sample_rate as f32;
    }

    fn process(&mut self, buffer: &mut AudioBuffer) {
        if !self.enabled {
            return;
        }
        let sr = self.sample_rate;
        for ch in 0..buffer.channel_count() {
            let input = buffer.channel(ch).to_vec();
            let denoised =
                stream::process_streamed(&input, sr, &self.policy, &self.pool, &self.denoiser);
            buffer.channel_mut(ch).copy_from_slice(&denoised);
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

    #[test]
    fn bypass_is_exact_identity() {
        let n = 20_000;
        let ch: Vec<f32> = (0..n)
            .map(|i| 0.3 * (2.0 * PI * 200.0 * i as f32 / SR as f32).sin())
            .collect();
        let mut b = AudioBuffer::new(SR, 16, SampleEncoding::Int, vec![ch.clone()]);
        let mut d = Denoise::new(2);
        d.set_enabled(false);
        d.prepare(SR, 4096, b.layout());
        d.process(&mut b);
        for (a, c) in b.channel(0).iter().zip(&ch) {
            assert!((a - c).abs() <= f32::EPSILON, "bypass must be identity");
        }
    }

    #[test]
    fn same_length_after_denoise() {
        let n = 50_000;
        let ch: Vec<f32> = (0..n)
            .map(|i| 0.3 * (2.0 * PI * 200.0 * i as f32 / SR as f32).sin())
            .collect();
        let mut b = AudioBuffer::new(SR, 16, SampleEncoding::Int, vec![ch]);
        let mut d = Denoise::new(2);
        d.prepare(SR, 4096, b.layout());
        d.process(&mut b);
        assert_eq!(b.frames(), n, "denoise must preserve length");
    }
}
