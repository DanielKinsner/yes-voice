//! Render-session assembly (plan §1.3, §2.4): turn [`Settings`] + the whole-file
//! [`AnalysisReport`] into a configured, ready-to-run [`Chain`] plus the three
//! decisions the host has to surface — the effective intensity, the slow-hardware
//! **Fast-mode** downshift, and which denoise engine was *actually* built (the
//! neural engine transparently falls back to light when its feature/model is
//! absent).
//!
//! This is the single place that wires #11's logic core (the
//! [`crate::quality::resolve`] downshift + [`crate::denoise::build_denoiser`]
//! factory) into a runnable chain. The app render path is a thin shell over it.

use std::path::Path;

use crate::analysis::AnalysisReport;
use crate::chain::Chain;
use crate::denoise::build_denoiser;
use crate::preset::Preset;
use crate::quality::{resolve, Engine, Resolved};
use crate::settings::Settings;

/// A chain assembled for one render, with the decisions the host must report.
pub struct BuiltChain {
    /// The configured, ready-to-run rack.
    pub chain: Chain,
    /// The preset that configured it (its module character).
    pub preset: Preset,
    /// The effective intensity decision (requested mode, possibly downshifted,
    /// plus the `fast_mode` badge state).
    pub resolved: Resolved,
    /// The engine actually built — may be [`Engine::Light`] even when the binding
    /// asked for neural, if the `neural` feature/model is unavailable.
    pub engine: Engine,
    /// A user-presentable reason when a neural request fell back to light (#33):
    /// feature off, no model, or a load/integrity failure. `None` otherwise.
    pub engine_note: Option<String>,
}

impl BuiltChain {
    /// Assemble the chain for a file. The caller passes the *selected* `preset`
    /// (the gallery choice held by the [`crate::Session`]); the CLI passes
    /// [`Preset::default_for`] since it has no gallery. `est_render_s` is the
    /// preflight render estimate for the requested mode on the detected machine
    /// (from the SKU table, plan §8.1); pass `0.0` to disable the downshift when
    /// no estimate is available (e.g. the headless CLI).
    pub fn build(
        settings: &Settings,
        preset: Preset,
        report: &AnalysisReport,
        neural_model: Option<&Path>,
        workers: usize,
        est_render_s: f32,
    ) -> Self {
        // The selected preset (user-authoritative gallery choice) supplies the
        // module character; the intensity slider (also user-authoritative) drives
        // the engine binding.
        let resolved = resolve(settings.quality_mode, est_render_s);
        // Intensity owns the engine (§2.4); neural falls back to light when its
        // feature/model is absent (reported via `engine`).
        let built = build_denoiser(resolved.binding, neural_model);
        let mut chain = preset.build_chain(built.denoiser, workers);
        // The binding's pass budget rides with the engine (Restoration HQ = 2).
        chain.denoise.set_passes(resolved.binding.max_passes);
        // Adapt the chain to the actual material (de-reverb auto-soften, …).
        chain.tune_from_report(report);
        BuiltChain {
            chain,
            preset,
            resolved,
            engine: built.actual,
            engine_note: built.fallback_reason,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::{AnalysisReport, ContentGuess, HumEstimate};
    use crate::audio::{AudioBuffer, ChannelLayout, SampleEncoding};
    use crate::processor::VocalProcessor;
    use crate::quality::QualityMode;
    use crate::settings::{ContentMode, Settings};
    use std::f32::consts::PI;

    fn report() -> AnalysisReport {
        AnalysisReport {
            channel_mode: ChannelLayout::Mono,
            integrated_lufs: -20.0,
            true_peak_dbtp: -3.0,
            noise_floor_db: -60.0,
            clip_sample_count: 0,
            clip_fraction: 0.0,
            click_rate_hz: 0.0,
            hum: HumEstimate {
                frequency: 0.0,
                harmonic_count: 0,
                strength_db: 0.0,
            },
            reverb_time_s: 0.30,
            content: ContentGuess {
                mode: ContentMode::Voiceover,
                confidence: 0.9,
            },
            residual_nonvocal: 0.0,
            resampled_for_batch: false,
            non_finite_samples: 0,
        }
    }

    #[test]
    fn balanced_default_keeps_the_mode_and_falls_back_to_light() {
        // Default settings = Voiceover + Balanced. The neural feature is off in
        // the default build, so the engine falls back to light, but the mode is
        // preserved and there is no Fast-mode downshift.
        let settings = Settings::default();
        let preset = Preset::default_for(settings.content_mode);
        let built = BuiltChain::build(&settings, preset, &report(), None, 2, 0.0);
        assert_eq!(built.resolved.mode, QualityMode::Balanced);
        assert!(!built.resolved.fast_mode);
        assert_eq!(built.engine, Engine::Light);
        assert_eq!(built.preset.name, "Podcast");
    }

    #[test]
    fn slow_hardware_downshifts_to_natural_light_with_fast_mode() {
        let settings = Settings {
            quality_mode: QualityMode::RestorationHq,
            ..Settings::default()
        };
        // A render estimate well over 1.5 × 45 s trips the downshift.
        let preset = Preset::default_for(settings.content_mode);
        let built = BuiltChain::build(&settings, preset, &report(), None, 2, 1000.0);
        assert_eq!(built.resolved.mode, QualityMode::Natural);
        assert_eq!(built.engine, Engine::Light);
        assert!(
            built.resolved.fast_mode,
            "downshift must raise the Fast badge"
        );
    }

    #[test]
    fn singing_settings_pick_the_lead_preset() {
        let settings = Settings {
            content_mode: ContentMode::Singing,
            quality_mode: QualityMode::Natural,
            ..Settings::default()
        };
        let preset = Preset::default_for(settings.content_mode);
        let built = BuiltChain::build(&settings, preset, &report(), None, 2, 0.0);
        assert_eq!(built.preset.name, "Lead Vocal");
        assert_eq!(built.resolved.mode, QualityMode::Natural);
        assert_eq!(built.engine, Engine::Light);
    }

    #[test]
    fn restoration_hq_binding_sets_two_passes_balanced_one() {
        let hq = Settings {
            quality_mode: QualityMode::RestorationHq,
            ..Settings::default()
        };
        let preset = Preset::default_for(hq.content_mode);
        let built = BuiltChain::build(&hq, preset, &report(), None, 2, 0.0);
        assert_eq!(
            built.chain.denoise.passes(),
            2,
            "Restoration HQ runs the conditional second pass"
        );

        let balanced = Settings::default();
        let preset = Preset::default_for(balanced.content_mode);
        let built = BuiltChain::build(&balanced, preset, &report(), None, 2, 0.0);
        assert_eq!(built.chain.denoise.passes(), 1, "Balanced is single pass");
    }

    #[test]
    fn built_chain_preserves_length() {
        let n = 24_000;
        let ch: Vec<f32> = (0..n)
            .map(|i| 0.3 * (2.0 * PI * 220.0 * i as f32 / 48_000.0).sin())
            .collect();
        let mut buf = AudioBuffer::new(48_000, 16, SampleEncoding::Int, vec![ch]);
        let settings = Settings::default();
        let preset = Preset::default_for(settings.content_mode);
        let mut built = BuiltChain::build(&settings, preset, &report(), None, 2, 0.0);
        built.chain.prepare(48_000, 4096, ChannelLayout::Mono);
        built.chain.reset();
        built.chain.process(&mut buf);
        assert_eq!(buf.frames(), n);
    }
}
