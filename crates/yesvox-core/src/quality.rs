//! Quality mode → engine binding + slow-hardware downshift (plan §2.4, #11).
//!
//! Intensity is the **authoritative axis** for engine choice: the [`QualityMode`]
//! decides which denoiser runs and how many passes. Presets pick a *default*
//! mode (their `intensity_default`) but can never override the binding — the
//! engine is always [`QualityMode::binding`], a pure function of the mode, with
//! no separate engine field to override. This preserves the difference-monitor
//! mode-switch semantics (§5.5) and keeps the slow-hardware fallback honest.
//!
//! The bindings (plan §2.4):
//! - **Natural** — light deterministic engine ([`crate::denoise::SpectralDenoiser`]),
//!   single pass. Also the slow-hardware fallback target.
//! - **Balanced** (default) — neural engine (MossFormer2), single pass.
//! - **Restoration HQ** — neural engine, two-pass (pass 2 runs only on frames
//!   the noise-floor estimator still flags; bounded at 2, never 3).

use serde::{Deserialize, Serialize};

/// The processing-intensity axis that owns engine selection (plan §2.4).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QualityMode {
    /// Light deterministic DSP, single pass. The slow-hardware fallback target.
    Natural,
    /// Neural engine, single pass. The v1 default.
    #[default]
    Balanced,
    /// Neural engine, conditional two-pass — the deepest restoration.
    RestorationHq,
}

/// Which denoiser an intensity binds to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Engine {
    /// The always-available pure-Rust `SpectralDenoiser` (deterministic).
    Light,
    /// The MossFormer2 neural engine (generative; behind the `neural` feature).
    Neural,
}

impl Engine {
    /// Generative engines synthesize rather than subtract, which changes how the
    /// difference monitor is computed (see [`QualityMode::diff_monitor`]).
    pub fn is_generative(self) -> bool {
        matches!(self, Engine::Neural)
    }
}

/// The fixed engine + pass budget an intensity binds to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EngineBinding {
    /// The denoiser this mode runs.
    pub engine: Engine,
    /// Maximum denoise passes (Restoration HQ's 2nd pass is conditional).
    pub max_passes: u8,
}

/// How the difference monitor presents "what was removed" (plan §5.5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffMonitor {
    /// Deterministic engine: the literal linear residual `original − processed`.
    LinearResidual,
    /// Generative engine: a wet/dry blend (the residual isn't physically
    /// meaningful when the net synthesizes), so the monitor blends instead.
    WetDryBlend,
}

/// Render-time budget per the <60 s drop-to-export target (plan §8.4: ≤45 s
/// render). Slow-hardware downshift trips above 1.5× this (plan §2.4).
pub const RENDER_BUDGET_S: f32 = 45.0;
/// Preflight trip multiplier for the slow-hardware downshift (plan §2.4).
pub const DOWNSHIFT_FACTOR: f32 = 1.5;

impl QualityMode {
    /// The fixed engine binding for this mode. Pure function of the mode — this
    /// is *the* reason presets can't override the engine.
    pub fn binding(self) -> EngineBinding {
        match self {
            QualityMode::Natural => EngineBinding {
                engine: Engine::Light,
                max_passes: 1,
            },
            QualityMode::Balanced => EngineBinding {
                engine: Engine::Neural,
                max_passes: 1,
            },
            QualityMode::RestorationHq => EngineBinding {
                engine: Engine::Neural,
                max_passes: 2,
            },
        }
    }

    /// Whether this mode runs a generative engine.
    pub fn is_generative(self) -> bool {
        self.binding().engine.is_generative()
    }

    /// The difference-monitor mode this intensity implies (plan §5.5).
    pub fn diff_monitor(self) -> DiffMonitor {
        if self.is_generative() {
            DiffMonitor::WetDryBlend
        } else {
            DiffMonitor::LinearResidual
        }
    }
}

/// The resolved processing decision after the slow-hardware preflight.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Resolved {
    /// The mode actually used (possibly downshifted to Natural).
    pub mode: QualityMode,
    /// The engine binding for [`Resolved::mode`].
    pub binding: EngineBinding,
    /// Whether a downshift happened — the UI shows a persistent **Fast mode**
    /// badge when this is set (plan §2.4).
    pub fast_mode: bool,
}

/// Resolve the effective mode given a preflight render estimate (seconds) for
/// the *requested* mode on the detected machine.
///
/// `est_render_s` comes from the SKU benchmark table (plan §8.1), **not** a
/// runtime guess. Balanced/Restoration HQ downshift to Natural when the estimate
/// exceeds `1.5 × 45 s`; Natural never downshifts (it is already the floor).
pub fn resolve(requested: QualityMode, est_render_s: f32) -> Resolved {
    let over_budget = est_render_s > RENDER_BUDGET_S * DOWNSHIFT_FACTOR;
    if over_budget && requested != QualityMode::Natural {
        Resolved {
            mode: QualityMode::Natural,
            binding: QualityMode::Natural.binding(),
            fast_mode: true,
        }
    } else {
        Resolved {
            mode: requested,
            binding: requested.binding(),
            fast_mode: false,
        }
    }
}

/// The linear residual `original − processed` — "what was removed" for a
/// deterministic engine. Lengths must match; trailing extra samples are ignored.
pub fn linear_residual(original: &[f32], processed: &[f32]) -> Vec<f32> {
    original.iter().zip(processed).map(|(o, p)| o - p).collect()
}

/// Wet/dry blend `(1 − t)·original + t·processed` — the difference-monitor
/// signal for a generative engine. `t = 0` is the untouched original, `t = 1` is
/// the fully processed result; `t` is clamped to `[0, 1]`.
pub fn wet_dry_blend(original: &[f32], processed: &[f32], t: f32) -> Vec<f32> {
    let t = t.clamp(0.0, 1.0);
    original
        .iter()
        .zip(processed)
        .map(|(o, p)| (1.0 - t) * o + t * p)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bindings_match_the_plan() {
        assert_eq!(
            QualityMode::Natural.binding(),
            EngineBinding {
                engine: Engine::Light,
                max_passes: 1
            }
        );
        assert_eq!(
            QualityMode::Balanced.binding(),
            EngineBinding {
                engine: Engine::Neural,
                max_passes: 1
            }
        );
        assert_eq!(
            QualityMode::RestorationHq.binding(),
            EngineBinding {
                engine: Engine::Neural,
                max_passes: 2
            }
        );
        // Bounded at two passes, never three.
        assert!(QualityMode::RestorationHq.binding().max_passes <= 2);
        // Balanced is the v1 default.
        assert_eq!(QualityMode::default(), QualityMode::Balanced);
    }

    #[test]
    fn engine_is_a_pure_function_of_mode() {
        // There is no API to set an engine independently of the mode, so a preset
        // (which only carries a QualityMode) can never override the binding.
        for m in [
            QualityMode::Natural,
            QualityMode::Balanced,
            QualityMode::RestorationHq,
        ] {
            assert_eq!(m.binding(), m.binding());
        }
        assert!(!QualityMode::Natural.is_generative());
        assert!(QualityMode::Balanced.is_generative());
        assert!(QualityMode::RestorationHq.is_generative());
    }

    #[test]
    fn diff_monitor_tracks_engine_kind() {
        assert_eq!(
            QualityMode::Natural.diff_monitor(),
            DiffMonitor::LinearResidual
        );
        assert_eq!(
            QualityMode::Balanced.diff_monitor(),
            DiffMonitor::WetDryBlend
        );
        assert_eq!(
            QualityMode::RestorationHq.diff_monitor(),
            DiffMonitor::WetDryBlend
        );
    }

    #[test]
    fn downshift_trips_only_over_budget_and_never_for_natural() {
        let budget = RENDER_BUDGET_S * DOWNSHIFT_FACTOR; // 67.5 s
                                                         // Within budget: requested mode is kept, no badge.
        let r = resolve(QualityMode::RestorationHq, budget - 1.0);
        assert_eq!(r.mode, QualityMode::RestorationHq);
        assert!(!r.fast_mode);
        // Over budget: Balanced/HQ downshift to Natural with the Fast-mode badge.
        let r = resolve(QualityMode::RestorationHq, budget + 1.0);
        assert_eq!(r.mode, QualityMode::Natural);
        assert_eq!(r.binding.engine, Engine::Light);
        assert!(r.fast_mode);
        let r = resolve(QualityMode::Balanced, budget + 50.0);
        assert_eq!(r.mode, QualityMode::Natural);
        assert!(r.fast_mode);
        // Natural is already the floor — it never downshifts or badges.
        let r = resolve(QualityMode::Natural, budget * 10.0);
        assert_eq!(r.mode, QualityMode::Natural);
        assert!(!r.fast_mode);
    }

    #[test]
    fn difference_monitors_compute_as_specified() {
        let original = [1.0, 0.5, -0.5, 0.2];
        let processed = [0.8, 0.4, -0.3, 0.1];
        // Deterministic: literal residual.
        let res = linear_residual(&original, &processed);
        assert_eq!(res, vec![1.0 - 0.8, 0.5 - 0.4, -0.5 - (-0.3), 0.2 - 0.1]);
        // Generative blend endpoints and midpoint.
        assert_eq!(wet_dry_blend(&original, &processed, 0.0), original.to_vec());
        assert_eq!(
            wet_dry_blend(&original, &processed, 1.0),
            processed.to_vec()
        );
        let mid = wet_dry_blend(&original, &processed, 0.5);
        assert!((mid[0] - 0.9).abs() < 1e-6);
        // t is clamped.
        assert_eq!(
            wet_dry_blend(&original, &processed, 2.0),
            processed.to_vec()
        );
    }
}
