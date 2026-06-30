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
use crate::quality::{Engine, EngineBinding};
use std::path::Path;
use std::sync::Arc;

/// A denoiser built for a [`crate::quality::QualityMode`] binding, tagged with
/// the engine that was *actually* constructed.
pub struct BuiltEngine {
    /// The constructed denoiser, ready for [`Denoise::with_engine`].
    pub denoiser: Arc<dyn Denoiser>,
    /// What was actually built. May fall back to [`Engine::Light`] when the
    /// neural engine is unavailable (feature off, or no model) — a
    /// neural-unavailable fallback distinct from the slow-hardware downshift in
    /// [`crate::quality::resolve`]. The UI/CLI logs this when it differs from the
    /// binding's intended engine.
    pub actual: Engine,
}

/// Build the denoiser an [`EngineBinding`] calls for.
///
/// The light engine ([`SpectralDenoiser`]) is always available. The neural
/// engine ([`crate::neural::MossformerEngine`]) needs the `neural` feature **and**
/// a `neural_model` path; when either is missing it transparently falls back to
/// the light engine (reported via [`BuiltEngine::actual`]).
pub fn build_denoiser(binding: EngineBinding, neural_model: Option<&Path>) -> BuiltEngine {
    match binding.engine {
        Engine::Light => BuiltEngine {
            denoiser: Arc::new(SpectralDenoiser::default()),
            actual: Engine::Light,
        },
        Engine::Neural => build_neural(neural_model),
    }
}

#[cfg(feature = "neural")]
fn build_neural(model: Option<&Path>) -> BuiltEngine {
    use crate::neural::{MossformerEngine, OrtMaskModel, WindowedMask};
    if let Some(path) = model {
        if let Ok(mask) = OrtMaskModel::from_file(path) {
            let engine = MossformerEngine::new(Box::new(WindowedMask::mossformer(Box::new(mask))));
            return BuiltEngine {
                denoiser: Arc::new(engine),
                actual: Engine::Neural,
            };
        }
    }
    light_fallback()
}

#[cfg(not(feature = "neural"))]
fn build_neural(_model: Option<&Path>) -> BuiltEngine {
    light_fallback()
}

fn light_fallback() -> BuiltEngine {
    BuiltEngine {
        denoiser: Arc::new(SpectralDenoiser::default()),
        actual: Engine::Light,
    }
}

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
    fn factory_builds_light_and_falls_back_without_a_model() {
        use crate::quality::{Engine, QualityMode};
        // Natural always binds the light engine.
        let nat = build_denoiser(QualityMode::Natural.binding(), None);
        assert_eq!(nat.actual, Engine::Light);
        // Balanced wants the neural engine, but with no model (and no feature in
        // the default build) it transparently falls back to light.
        let bal = build_denoiser(QualityMode::Balanced.binding(), None);
        assert_eq!(bal.actual, Engine::Light);
        // The built denoiser actually runs and preserves length.
        let x: Vec<f32> = (0..8192)
            .map(|i| 0.2 * (2.0 * PI * 200.0 * i as f32 / SR as f32).sin())
            .collect();
        let y = bal.denoiser.denoise(&x, SR as f32);
        assert_eq!(y.len(), x.len());
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
