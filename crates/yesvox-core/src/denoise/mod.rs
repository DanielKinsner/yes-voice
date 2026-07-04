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
    /// A user-presentable reason when the neural engine was requested but a
    /// light fallback happened (feature off, no model, load/integrity failure —
    /// #33). `None` when the actual engine is what was asked for.
    pub fallback_reason: Option<String>,
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
            // Not a fallback — the light engine was the chosen binding.
            fallback_reason: None,
        },
        Engine::Neural => build_neural(neural_model),
    }
}

#[cfg(feature = "neural")]
fn build_neural(model: Option<&Path>) -> BuiltEngine {
    use crate::neural::{MossformerEngine, OrtMaskModel, WindowedMask};
    let Some(path) = model else {
        return light_fallback(Some(
            "no neural model configured — set YESVOX_MODEL_PATH".to_string(),
        ));
    };
    // Weight integrity gate (#33): reject a corrupt/tampered model before it ever
    // reaches ONNX Runtime.
    if let Err(reason) = crate::neural::verify_model_integrity(path) {
        return light_fallback(Some(reason));
    }
    match OrtMaskModel::from_file(path) {
        Ok(mask) => {
            let engine = MossformerEngine::new(Box::new(WindowedMask::mossformer(Box::new(mask))));
            BuiltEngine {
                denoiser: Arc::new(engine),
                actual: Engine::Neural,
                fallback_reason: None,
            }
        }
        Err(e) => light_fallback(Some(format!(
            "neural model failed to load ({e}) — reinstall or clear YESVOX_MODEL_PATH"
        ))),
    }
}

#[cfg(not(feature = "neural"))]
fn build_neural(_model: Option<&Path>) -> BuiltEngine {
    light_fallback(Some(
        "neural engine not built — rebuild with `--features neural`".to_string(),
    ))
}

fn light_fallback(reason: Option<String>) -> BuiltEngine {
    BuiltEngine {
        denoiser: Arc::new(SpectralDenoiser::default()),
        actual: Engine::Light,
        fallback_reason: reason,
    }
}

/// The offline denoise processor.
pub struct Denoise {
    enabled: bool,
    denoiser: Arc<dyn Denoiser>,
    pool: WorkerPool,
    policy: ChunkPolicy,
    sample_rate: f32,
    passes: u8,
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
            passes: 1,
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
            passes: 1,
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

    /// Set the denoise pass budget (plan §2.4). Bounded to `[1, 2]`: Restoration
    /// HQ runs a conditional second pass; it is never three.
    pub fn set_passes(&mut self, passes: u8) {
        self.passes = passes.clamp(1, 2);
    }

    /// The effective pass budget (`1` or `2`).
    pub fn passes(&self) -> u8 {
        self.passes
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
            let pass1 =
                stream::process_streamed(&input, sr, &self.policy, &self.pool, &self.denoiser);
            let out = if self.passes >= 2 {
                // Restoration HQ: a second pass, gated to the frames where pass 1
                // still leaves low-level broadband noise — loud voiced frames are
                // protected so the deepest mode never re-processes the voice.
                let pass2 =
                    stream::process_streamed(&pass1, sr, &self.policy, &self.pool, &self.denoiser);
                gate_second_pass(&pass1, &pass2, sr)
            } else {
                pass1
            };
            buffer.channel_mut(ch).copy_from_slice(&out);
        }
    }

    fn reset(&mut self) {}
}

/// Blend a conditional second denoise pass into `pass1` only where it still
/// carries low-level broadband noise (plan §2.4). The gate keys on a smoothed
/// short-term **RMS** level (a fast-attack/slow-release one-pole over `pass1²`,
/// then `10·log10`), so its dBFS thresholds mean what §3's RMS noise-floor
/// estimator means: residual noise between roughly −52 and −40 dBFS gets the
/// full second pass, the gate ramps to zero by −20 dBFS (loud voiced frames are
/// protected) and below a ~−58 dBFS floor (already quiet). The envelope is
/// continuous, so the blend is click-free.
fn gate_second_pass(pass1: &[f32], pass2: &[f32], sample_rate: f32) -> Vec<f32> {
    let atk = (-1.0 / (0.005 * sample_rate)).exp(); // 5 ms
    let rel = (-1.0 / (0.050 * sample_rate)).exp(); // 50 ms
    let mut power = 0.0f32;
    let mut out = Vec::with_capacity(pass1.len());
    for (i, &p1) in pass1.iter().enumerate() {
        let inst = p1 * p1;
        let coeff = if inst > power { atk } else { rel };
        power = coeff * power + (1.0 - coeff) * inst;
        let dbfs = 10.0 * power.max(1.0e-12).log10();
        let g = gate(dbfs);
        let p2 = pass2.get(i).copied().unwrap_or(p1);
        out.push(p1 + g * (p2 - p1));
    }
    out
}

/// Trapezoidal gate over the short-term RMS level (dBFS): 0 below −58, ramps to
/// 1 by −52, holds 1 to −40, ramps back to 0 by −20.
fn gate(dbfs: f32) -> f32 {
    const LO_A: f32 = -58.0;
    const LO_B: f32 = -52.0;
    const HI_A: f32 = -40.0;
    const HI_B: f32 = -20.0;
    if dbfs <= LO_A || dbfs >= HI_B {
        0.0
    } else if dbfs < LO_B {
        (dbfs - LO_A) / (LO_B - LO_A)
    } else if dbfs <= HI_A {
        1.0
    } else {
        (HI_B - dbfs) / (HI_B - HI_A)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::SampleEncoding;
    use std::f32::consts::PI;

    const SR: u32 = 48_000;

    #[cfg(feature = "neural")]
    #[test]
    #[ignore = "needs ORT_DYLIB_PATH — ort's dynamic-load probe blocks without the runtime"]
    fn a_garbage_model_file_falls_back_to_light_not_a_crash() {
        // A corrupt/wrong ONNX at YESVOX_MODEL_PATH must degrade to the light
        // engine with a reason, never panic (#33). Needs the onnxruntime lib
        // (ORT_DYLIB_PATH) present; the *mechanism* (neural request -> light +
        // reason) is covered without the feature by
        // `factory_builds_light_and_falls_back_without_a_model`.
        use crate::quality::{Engine, QualityMode};
        let path = std::env::temp_dir().join(format!("yesvox-garbage-{}.onnx", std::process::id()));
        std::fs::write(&path, b"this is definitely not an onnx graph").unwrap();
        let built = build_denoiser(QualityMode::Balanced.binding(), Some(&path));
        assert_eq!(
            built.actual,
            Engine::Light,
            "garbage model -> light fallback"
        );
        assert!(
            built.fallback_reason.is_some(),
            "the corrupt-model fallback carries a reason"
        );
        let _ = std::fs::remove_file(&path);
    }

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
        // Natural always binds the light engine — not a fallback, so no note.
        let nat = build_denoiser(QualityMode::Natural.binding(), None);
        assert_eq!(nat.actual, Engine::Light);
        assert!(nat.fallback_reason.is_none());
        // Balanced wants the neural engine, but with no model (and no feature in
        // the default build) it transparently falls back to light with a
        // user-presentable reason (#33).
        let bal = build_denoiser(QualityMode::Balanced.binding(), None);
        assert_eq!(bal.actual, Engine::Light);
        assert!(
            bal.fallback_reason.is_some(),
            "a neural->light fallback must carry a reason"
        );
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

    /// Deterministic low-level broadband noise (~-45 dBFS), no tonal content.
    fn noisy(n: usize, amp: f32) -> Vec<f32> {
        let mut s: u32 = 0x1234_5678;
        (0..n)
            .map(|_| {
                s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let u = (s >> 8) as f32 / (1u32 << 24) as f32;
                amp * (u * 2.0 - 1.0)
            })
            .collect()
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
    }

    fn run(mut d: Denoise, x: &[f32]) -> Vec<f32> {
        let mut b = AudioBuffer::new(SR, 16, SampleEncoding::Int, vec![x.to_vec()]);
        d.prepare(SR, 4096, b.layout());
        d.process(&mut b);
        b.channel(0).to_vec()
    }

    #[test]
    fn passes_is_bounded_to_two_and_at_least_one() {
        let mut d = Denoise::new(1);
        assert_eq!(d.passes(), 1, "single pass is the default");
        d.set_passes(5);
        assert_eq!(d.passes(), 2, "never more than two passes");
        d.set_passes(0);
        assert_eq!(d.passes(), 1, "never fewer than one");
    }

    #[test]
    fn second_pass_denoises_residual_noise_further() {
        let x = noisy(48_000, 0.01); // ~-45 dBFS broadband
        let one = run(Denoise::new(2), &x);
        let mut two_d = Denoise::new(2);
        two_d.set_passes(2);
        let two = run(two_d, &x);
        let (r0, r1, r2) = (rms(&x), rms(&one), rms(&two));
        assert!(r1 < r0, "one pass should reduce noise ({r1} !< {r0})");
        assert!(
            r2 < r1,
            "the gated second pass should reduce residual noise further ({r2} !< {r1})"
        );
    }

    #[test]
    fn second_pass_protects_a_loud_tone() {
        // A loud tone sits well above the -55 dBFS gate floor, so the second
        // pass is gated out — character is preserved (two-pass ≈ one-pass here).
        let x: Vec<f32> = (0..48_000)
            .map(|i| 0.5 * (2.0 * PI * 220.0 * i as f32 / SR as f32).sin())
            .collect();
        let one = run(Denoise::new(2), &x);
        let mut two_d = Denoise::new(2);
        two_d.set_passes(2);
        let two = run(two_d, &x);
        // Ignore the first 100 ms while the energy envelope ramps up.
        let drift = one[4_800..]
            .iter()
            .zip(&two[4_800..])
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        assert!(
            drift < 1e-3,
            "the second pass must barely touch a loud tone (drift {drift})"
        );
    }

    #[test]
    fn two_pass_bypass_is_still_identity() {
        let x = noisy(20_000, 0.02);
        let mut d = Denoise::new(2);
        d.set_passes(2);
        d.set_enabled(false);
        let y = run(d, &x);
        for (a, b) in y.iter().zip(&x) {
            assert!(
                (a - b).abs() <= f32::EPSILON,
                "bypass identity with passes=2"
            );
        }
    }
}
