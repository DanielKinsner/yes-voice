//! The canonical-order processing chain (plan §649, product.md §70).
//!
//! Every repair/finish module in [`crate::modules`] implements the same
//! [`VocalProcessor`] contract and is the sample-accurate identity when
//! bypassed — but nothing *composed* them until this module. [`Chain`] is the
//! host-side rack: it owns the modules in the one canonical order and runs a
//! whole buffer through them, threading the one genuine in-chain handoff (the
//! de-ess → breath HF sidechain, §3.2) so the two never double-attenuate the
//! 4–12 kHz band.
//!
//! The chain is itself a [`VocalProcessor`], so the host (app/tests) treats the
//! whole rack exactly like a single module — including the all-bypass NULL
//! guarantee: a [`Chain::all_bypassed`] is bit-exact identity, which is what the
//! four guarantee gates assert against once a real chain exists.
//!
//! Canonical order (plan §649):
//! de-bleed → declip → de-click/mouth-click → de-pop → de-hum → denoise →
//! de-reverb → de-ess → breath → finish (presence/air → leveler → limiter).

use crate::audio::{AudioBuffer, ChannelLayout};
use crate::denoise::Denoise;
use crate::modules::{
    BleedVerdict, BreathControl, DeBleed, DeClick, DeClip, DeEss, DeEssMode, DeHum, DePop,
    DeReverb, FinishStage, LoudnessTarget,
};
use crate::processor::VocalProcessor;
use crate::settings::ContentMode;

/// One stage of the rack, in canonical order — the unit a UI "card" maps to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    /// Imported-stem de-bleed (off for native takes).
    DeBleed,
    /// Declip.
    DeClip,
    /// De-click + mouth-click.
    DeClick,
    /// De-pop.
    DePop,
    /// De-hum.
    DeHum,
    /// Denoise.
    Denoise,
    /// De-reverb.
    DeReverb,
    /// De-ess.
    DeEss,
    /// Breath control.
    Breath,
    /// Finish: presence/air EQ + leveler + loudness/limiter.
    Finish,
}

/// Number of rack stages.
pub const STAGE_COUNT: usize = 10;

impl Stage {
    /// Every stage in canonical order.
    pub const ALL: [Stage; STAGE_COUNT] = [
        Stage::DeBleed,
        Stage::DeClip,
        Stage::DeClick,
        Stage::DePop,
        Stage::DeHum,
        Stage::Denoise,
        Stage::DeReverb,
        Stage::DeEss,
        Stage::Breath,
        Stage::Finish,
    ];
}

/// The host-side repair/finish rack: every module in canonical order, run as one
/// [`VocalProcessor`].
pub struct Chain {
    pub(crate) debleed: DeBleed,
    pub(crate) declip: DeClip,
    pub(crate) declick: DeClick,
    pub(crate) depop: DePop,
    pub(crate) dehum: DeHum,
    pub(crate) denoise: Denoise,
    pub(crate) dereverb: DeReverb,
    pub(crate) deess: DeEss,
    pub(crate) breath: BreathControl,
    pub(crate) finish: FinishStage,
    /// Per-stage wet/dry mix in `[0, 1]` (1 = full effect, 0 = dialed out) — the
    /// uniform "amount" knob behind each UI rack card and the §5.5 deterministic
    /// reblend. Default 1.0 for every stage.
    pub(crate) mix: [f32; STAGE_COUNT],
    /// The HF reduction (dB) de-ess published on the last [`Chain::process`] and
    /// handed to breath — a report-worthy observable of the sidechain handoff.
    pub(crate) last_shared_hf_db: f32,
}

/// Blend the processed `buffer` back toward the `dry` snapshot by `1 - mix`.
fn lerp_into(buffer: &mut AudioBuffer, dry: &[Vec<f32>], mix: f32) {
    for (ch, d) in dry.iter().enumerate() {
        for (s, &dv) in buffer.channel_mut(ch).iter_mut().zip(d) {
            *s = (1.0 - mix) * dv + mix * *s;
        }
    }
}

impl Chain {
    /// A fully bypassed chain: every module disabled, so [`Chain::process`] is
    /// bit-exact identity. This is what the chain-level NULL gate drives.
    pub fn all_bypassed(workers: usize) -> Self {
        let mut chain = Self::voiceover_default(workers);
        chain.debleed.set_enabled(false);
        chain.declip.set_enabled(false);
        chain.declick.set_enabled(false);
        chain.depop.set_enabled(false);
        chain.dehum.set_enabled(false);
        chain.denoise.set_enabled(false);
        chain.dereverb.set_enabled(false);
        chain.deess.set_enabled(false);
        chain.breath.set_enabled(false);
        chain.finish.set_enabled(false);
        chain
    }

    /// A sensible all-enabled voiceover chain (the default before presets land).
    /// De-bleed is off (native take, not an imported stem).
    pub fn voiceover_default(workers: usize) -> Self {
        Chain {
            debleed: DeBleed::for_source(false),
            declip: DeClip::default(),
            declick: DeClick::new(),
            depop: DePop::new(),
            dehum: DeHum::default(),
            denoise: Denoise::new(workers),
            dereverb: DeReverb::voiceover(),
            deess: DeEss::new(DeEssMode::Split),
            breath: BreathControl::for_content(ContentMode::Voiceover),
            finish: FinishStage::new(LoudnessTarget::Podcast),
            mix: [1.0; STAGE_COUNT],
            last_shared_hf_db: 0.0,
        }
    }

    /// The HF reduction (dB) de-ess handed to breath on the last process call.
    pub fn last_shared_hf_db(&self) -> f32 {
        self.last_shared_hf_db
    }

    /// Set a stage's wet/dry mix (the UI card "amount" knob), clamped to `[0, 1]`.
    pub fn set_mix(&mut self, stage: Stage, mix: f32) {
        self.mix[stage as usize] = mix.clamp(0.0, 1.0);
    }

    /// A stage's current wet/dry mix.
    pub fn mix(&self, stage: Stage) -> f32 {
        self.mix[stage as usize]
    }

    /// Enable or bypass a stage (the UI card toggle).
    pub fn set_stage_enabled(&mut self, stage: Stage, on: bool) {
        match stage {
            Stage::DeBleed => self.debleed.set_enabled(on),
            Stage::DeClip => self.declip.set_enabled(on),
            Stage::DeClick => self.declick.set_enabled(on),
            Stage::DePop => self.depop.set_enabled(on),
            Stage::DeHum => self.dehum.set_enabled(on),
            Stage::Denoise => self.denoise.set_enabled(on),
            Stage::DeReverb => self.dereverb.set_enabled(on),
            Stage::DeEss => self.deess.set_enabled(on),
            Stage::Breath => self.breath.set_enabled(on),
            Stage::Finish => self.finish.set_enabled(on),
        }
    }

    /// Whether a stage is enabled.
    pub fn stage_enabled(&self, stage: Stage) -> bool {
        match stage {
            Stage::DeBleed => self.debleed.enabled(),
            Stage::DeClip => self.declip.enabled(),
            Stage::DeClick => self.declick.enabled(),
            Stage::DePop => self.depop.enabled(),
            Stage::DeHum => self.dehum.enabled(),
            Stage::Denoise => self.denoise.enabled(),
            Stage::DeReverb => self.dereverb.enabled(),
            Stage::DeEss => self.deess.enabled(),
            Stage::Breath => self.breath.enabled(),
            Stage::Finish => self.finish.enabled(),
        }
    }

    /// The de-bleed refusal verdict for this buffer (plan §4.5). The host reads
    /// this *before* processing so a `Block` can halt with an actionable message.
    pub fn assess_bleed(&self, buffer: &AudioBuffer) -> BleedVerdict {
        self.debleed.assess(buffer)
    }

    /// Feed the whole-file analysis into the modules that consume it, so the
    /// chain adapts to the actual material rather than to fixed defaults. Today
    /// that is the de-reverb auto-soften (an already-dry room pulls the
    /// de-reverb toward zero from the measured RT60, plan §4.1).
    pub fn tune_from_report(&mut self, report: &crate::analysis::AnalysisReport) {
        self.dereverb.set_rt60(report.reverb_time_s);
    }
}

impl VocalProcessor for Chain {
    fn prepare(&mut self, sample_rate: u32, max_block: usize, layout: ChannelLayout) {
        self.debleed.prepare(sample_rate, max_block, layout);
        self.declip.prepare(sample_rate, max_block, layout);
        self.declick.prepare(sample_rate, max_block, layout);
        self.depop.prepare(sample_rate, max_block, layout);
        self.dehum.prepare(sample_rate, max_block, layout);
        self.denoise.prepare(sample_rate, max_block, layout);
        self.dereverb.prepare(sample_rate, max_block, layout);
        self.deess.prepare(sample_rate, max_block, layout);
        self.breath.prepare(sample_rate, max_block, layout);
        self.finish.prepare(sample_rate, max_block, layout);
    }

    fn process(&mut self, buffer: &mut AudioBuffer) {
        // Run a stage and blend it back toward dry by its per-stage mix (1 = full
        // effect, 0 = dialed out → identity). Canonical order (plan §649).
        macro_rules! run {
            ($field:ident, $stage:expr) => {{
                let m = self.mix[$stage as usize];
                if m >= 1.0 {
                    self.$field.process(buffer);
                } else if m > 0.0 {
                    let dry: Vec<Vec<f32>> = buffer.channels().to_vec();
                    self.$field.process(buffer);
                    lerp_into(buffer, &dry, m);
                }
                // m <= 0.0 → fully dry, skip the stage entirely.
            }};
        }

        // De-bleed front-cleans an imported stem (and refuses a mix via its own
        // gate); the rest is the restoration rack.
        run!(debleed, Stage::DeBleed);
        run!(declip, Stage::DeClip);
        run!(declick, Stage::DeClick);
        run!(depop, Stage::DePop);
        run!(dehum, Stage::DeHum);
        run!(denoise, Stage::Denoise);
        run!(dereverb, Stage::DeReverb);
        run!(deess, Stage::DeEss);
        // The one genuine in-chain handoff (§3.2): hand breath the HF reduction
        // de-ess just applied so the two never double-attenuate the 4–12 kHz band.
        // Scale by the de-ess mix — the published reduction is for the full wet.
        let deess_mix = self.mix[Stage::DeEss as usize].clamp(0.0, 1.0);
        self.last_shared_hf_db = self.deess.hf_reduction_db() * deess_mix;
        self.breath
            .set_shared_hf_reduction_db(self.last_shared_hf_db);
        run!(breath, Stage::Breath);
        run!(finish, Stage::Finish);
    }

    fn reset(&mut self) {
        self.debleed.reset();
        self.declip.reset();
        self.declick.reset();
        self.depop.reset();
        self.dehum.reset();
        self.denoise.reset();
        self.dereverb.reset();
        self.deess.reset();
        self.breath.reset();
        self.finish.reset();
    }

    fn latency_samples(&self) -> u32 {
        self.debleed.latency_samples()
            + self.declip.latency_samples()
            + self.declick.latency_samples()
            + self.depop.latency_samples()
            + self.dehum.latency_samples()
            + self.denoise.latency_samples()
            + self.dereverb.latency_samples()
            + self.deess.latency_samples()
            + self.breath.latency_samples()
            + self.finish.latency_samples()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::SampleEncoding;
    use std::f32::consts::PI;

    const SR: u32 = 48_000;

    /// A degraded mono signal: a 200 Hz tone with a strong 8 kHz sibilant
    /// component, a couple of hard-clipped peaks, and an injected click.
    fn degraded_signal(n: usize) -> Vec<f32> {
        let mut x: Vec<f32> = (0..n)
            .map(|i| {
                let t = i as f32 / SR as f32;
                let body = 0.5 * (2.0 * PI * 200.0 * t).sin();
                let sib = 0.35 * (2.0 * PI * 8000.0 * t).sin();
                (body + sib).clamp(-0.8, 0.8)
            })
            .collect();
        // an isolated click
        if n > 1000 {
            x[1000] = 0.95;
        }
        x
    }

    fn mono(samples: Vec<f32>) -> AudioBuffer {
        AudioBuffer::new(SR, 16, SampleEncoding::Int, vec![samples])
    }

    #[test]
    fn mix_zero_dials_a_stage_fully_out() {
        let x = degraded_signal(30_000);
        let mut buf = mono(x.clone());
        let mut chain = Chain::all_bypassed(2);
        chain.set_stage_enabled(Stage::Denoise, true);
        chain.set_mix(Stage::Denoise, 0.0);
        chain.prepare(SR, 4096, buf.layout());
        chain.reset();
        chain.process(&mut buf);
        for (a, b) in buf.channel(0).iter().zip(&x) {
            assert!((a - b).abs() <= 1e-6, "mix 0 must dial the stage fully out");
        }
    }

    #[test]
    fn mid_mix_is_the_per_sample_blend_of_dry_and_wet() {
        let x = degraded_signal(30_000);
        let run = |mix: f32| {
            let mut buf = mono(x.clone());
            let mut c = Chain::all_bypassed(2);
            c.set_stage_enabled(Stage::Denoise, true);
            c.set_mix(Stage::Denoise, mix);
            c.prepare(SR, 4096, buf.layout());
            c.reset();
            c.process(&mut buf);
            buf.channel(0).to_vec()
        };
        let dry = run(0.0);
        let wet = run(1.0);
        let mid = run(0.5);
        let mut max_err = 0.0f32;
        for i in 0..x.len() {
            let expect = 0.5 * dry[i] + 0.5 * wet[i];
            max_err = max_err.max((mid[i] - expect).abs());
        }
        assert!(
            max_err < 1e-5,
            "mid mix must be the per-sample (1-m)·dry + m·wet blend, err {max_err}"
        );
    }

    #[test]
    fn tune_from_report_feeds_rt60_into_dereverb() {
        use crate::analysis::{AnalysisReport, ContentGuess, HumEstimate};
        let report = AnalysisReport {
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
            reverb_time_s: 0.42,
            content: ContentGuess {
                mode: ContentMode::Voiceover,
                confidence: 0.9,
            },
            residual_nonvocal: 0.0,
            resampled_for_batch: false,
        };
        let mut chain = Chain::voiceover_default(1);
        chain.tune_from_report(&report);
        assert_eq!(
            chain.dereverb.rt60(),
            0.42,
            "the analyzer's RT60 must reach de-reverb's auto-soften"
        );
    }

    #[test]
    fn all_bypassed_chain_is_exact_identity() {
        let x = degraded_signal(20_000);
        let mut buf = mono(x.clone());
        let mut chain = Chain::all_bypassed(2);
        chain.prepare(SR, 4096, buf.layout());
        chain.reset();
        chain.process(&mut buf);
        for (a, b) in buf.channel(0).iter().zip(&x) {
            assert!((a - b).abs() <= f32::EPSILON, "all-bypass must be identity");
        }
    }

    #[test]
    fn chain_preserves_length() {
        let n = 30_000;
        let mut buf = mono(degraded_signal(n));
        let mut chain = Chain::voiceover_default(2);
        chain.prepare(SR, 4096, buf.layout());
        chain.reset();
        chain.process(&mut buf);
        assert_eq!(buf.frames(), n, "chain must preserve length");
    }

    #[test]
    fn active_chain_modifies_a_degraded_signal() {
        let x = degraded_signal(30_000);
        let mut buf = mono(x.clone());
        let mut chain = Chain::voiceover_default(2);
        chain.prepare(SR, 4096, buf.layout());
        chain.reset();
        chain.process(&mut buf);
        let changed = buf
            .channel(0)
            .iter()
            .zip(&x)
            .any(|(a, b)| (a - b).abs() > 1e-4);
        assert!(
            changed,
            "an active chain must actually change a degraded signal"
        );
    }

    #[test]
    fn breath_receives_the_deess_hf_reduction() {
        // de-ess (Wide) + breath active, everything else bypassed: the chain must
        // hand de-ess's published HF reduction to breath before running it.
        let x = degraded_signal(30_000);
        let mut buf = mono(x);
        let mut chain = Chain::all_bypassed(2);
        chain.deess = DeEss::new(DeEssMode::Wide); // enabled
        chain.breath = BreathControl::for_content(ContentMode::Voiceover); // enabled
        chain.prepare(SR, 4096, buf.layout());
        chain.reset();
        chain.process(&mut buf);
        assert!(
            chain.last_shared_hf_db() > 0.0,
            "de-ess cut HF, so breath must have received a positive shared reduction"
        );
    }

    #[test]
    fn bypassed_deess_hands_breath_zero() {
        let x = degraded_signal(20_000);
        let mut buf = mono(x);
        let mut chain = Chain::all_bypassed(2);
        chain.breath = BreathControl::for_content(ContentMode::Voiceover); // breath on, de-ess off
        chain.prepare(SR, 4096, buf.layout());
        chain.reset();
        chain.process(&mut buf);
        assert_eq!(
            chain.last_shared_hf_db(),
            0.0,
            "a bypassed de-ess publishes 0 dB to breath"
        );
    }
}
