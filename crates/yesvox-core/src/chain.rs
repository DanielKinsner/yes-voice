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
    /// The HF reduction (dB) de-ess published on the last [`Chain::process`] and
    /// handed to breath — a report-worthy observable of the sidechain handoff.
    pub(crate) last_shared_hf_db: f32,
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
            last_shared_hf_db: 0.0,
        }
    }

    /// The HF reduction (dB) de-ess handed to breath on the last process call.
    pub fn last_shared_hf_db(&self) -> f32 {
        self.last_shared_hf_db
    }

    /// The de-bleed refusal verdict for this buffer (plan §4.5). The host reads
    /// this *before* processing so a `Block` can halt with an actionable message.
    pub fn assess_bleed(&self, buffer: &AudioBuffer) -> BleedVerdict {
        self.debleed.assess(buffer)
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
        // Canonical order (plan §649). De-bleed front-cleans an imported stem
        // (and refuses a mix via its own gate); the rest is the restoration rack.
        self.debleed.process(buffer);
        self.declip.process(buffer);
        self.declick.process(buffer);
        self.depop.process(buffer);
        self.dehum.process(buffer);
        self.denoise.process(buffer);
        self.dereverb.process(buffer);
        self.deess.process(buffer);
        // The one genuine in-chain handoff (§3.2): hand breath the HF reduction
        // de-ess just applied so the two never double-attenuate the 4–12 kHz band.
        self.last_shared_hf_db = self.deess.hf_reduction_db();
        self.breath
            .set_shared_hf_reduction_db(self.last_shared_hf_db);
        self.breath.process(buffer);
        self.finish.process(buffer);
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
