//! The v1 preset taxonomy (plan §5.7) as data.
//!
//! A [`Preset`] is the Standard surface's unit of choice: a stored patch of
//! per-module settings plus an `intensity_default` and a `loudness_target`.
//! Restoration always runs in canonical order (declip/de-click/de-pop/de-hum are
//! on in every preset); the **finish** stage and the de-reverb/de-ess/breath
//! character are what differ between voiceover and singing.
//!
//! Crucially, a preset carries an `intensity_default` ([`QualityMode`]) but
//! **cannot** name a denoise engine: intensity owns the engine binding (§2.4),
//! so [`Preset::build_chain`] takes the already-built denoiser as an argument.
//! That is the structural reason a preset can never override the binding.
//!
//! The singing presets are defined here so the data exists, but the Singing
//! toggle only surfaces them once the §6 listening gate (#16) passes.

use std::sync::Arc;

use crate::chain::Chain;
use crate::denoise::{Denoise, Denoiser};
use crate::modules::{
    BreathControl, DeBleed, DeClick, DeClip, DeEss, DeEssMode, DeHum, DePop, DeReverb, FinishStage,
    LoudnessTarget,
};
use crate::quality::QualityMode;
use crate::settings::ContentMode;

/// A v1 Standard preset (plan §5.7): a per-module patch + intensity + loudness.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Preset {
    /// Display name, shown in the Standard gallery.
    pub name: &'static str,
    /// Which toggle group the preset belongs to (Voiceover or Singing).
    pub content_mode: ContentMode,
    /// The default intensity — this, not the preset, owns the denoise engine.
    pub intensity_default: QualityMode,
    /// Loudness target for the finish stage.
    pub loudness_target: LoudnessTarget,
    /// Whether de-reverb runs (singing presets keep it off to protect tails).
    pub dereverb_enabled: bool,
    /// The de-ess implementation. Split (sibilant-band only) for both voiceover
    /// and singing so the body/sustain is never ducked; Wide stays available in
    /// Advanced.
    pub deess_mode: DeEssMode,
    /// Breath attenuation in dB; `None` turns breath control off (singing).
    pub breath_db: Option<f32>,
    /// Presence (~5 kHz) finish boost in dB.
    pub presence_db: f32,
    /// Air (~12 kHz) finish boost in dB.
    pub air_db: f32,
}

impl Preset {
    /// Podcast — full clean-up + presence, balanced neural denoise (§5.7).
    pub const PODCAST: Preset = Preset {
        name: "Podcast",
        content_mode: ContentMode::Voiceover,
        intensity_default: QualityMode::Balanced,
        loudness_target: LoudnessTarget::Podcast,
        dereverb_enabled: true,
        deess_mode: DeEssMode::Split,
        breath_db: Some(6.0),
        presence_db: 1.5,
        air_db: 0.0,
    };

    /// Narration — gentler breath, a touch of air, deepest (HQ) denoise (§5.7).
    pub const NARRATION: Preset = Preset {
        name: "Narration",
        content_mode: ContentMode::Voiceover,
        intensity_default: QualityMode::RestorationHq,
        loudness_target: LoudnessTarget::Podcast,
        dereverb_enabled: true,
        deess_mode: DeEssMode::Split,
        breath_db: Some(3.0),
        presence_db: 0.0,
        air_db: 1.0,
    };

    /// Dialogue-cleanup — aggressive restoration, no tonal finish, match-input
    /// loudness (§5.7).
    pub const DIALOGUE_CLEANUP: Preset = Preset {
        name: "Dialogue-cleanup",
        content_mode: ContentMode::Voiceover,
        intensity_default: QualityMode::Balanced,
        loudness_target: LoudnessTarget::MatchInput,
        dereverb_enabled: true,
        deess_mode: DeEssMode::Split,
        breath_db: Some(6.0),
        presence_db: 0.0,
        air_db: 0.0,
    };

    /// Lead Vocal (singing) — tail-preserving (de-reverb off), split-band de-ess
    /// (only the sibilant band, so sustains aren't ducked), breath off, a touch
    /// of air, streaming loudness (§5.7, research §5/§6).
    pub const LEAD_VOCAL: Preset = Preset {
        name: "Lead Vocal",
        content_mode: ContentMode::Singing,
        intensity_default: QualityMode::Balanced,
        loudness_target: LoudnessTarget::Streaming,
        dereverb_enabled: false,
        deess_mode: DeEssMode::Split,
        breath_db: None,
        presence_db: 0.0,
        air_db: 1.0,
    };

    /// Natural Vocal (singing) — most conservative: light (Natural) denoise,
    /// gentle split-band de-ess, no finish boosts, match-input loudness (§5.7).
    pub const NATURAL_VOCAL: Preset = Preset {
        name: "Natural Vocal",
        content_mode: ContentMode::Singing,
        intensity_default: QualityMode::Natural,
        loudness_target: LoudnessTarget::MatchInput,
        dereverb_enabled: false,
        deess_mode: DeEssMode::Split,
        breath_db: None,
        presence_db: 0.0,
        air_db: 0.0,
    };

    /// Every v1 preset, voiceover first then singing.
    pub const ALL: [Preset; 5] = [
        Preset::PODCAST,
        Preset::NARRATION,
        Preset::DIALOGUE_CLEANUP,
        Preset::LEAD_VOCAL,
        Preset::NATURAL_VOCAL,
    ];

    /// The default preset for a content mode — what the Standard view selects
    /// before the user picks from the gallery, and what the headless CLI uses.
    pub fn default_for(content_mode: ContentMode) -> Preset {
        match content_mode {
            ContentMode::Voiceover => Preset::PODCAST,
            ContentMode::Singing => Preset::LEAD_VOCAL,
        }
    }

    /// Build a [`Chain`] configured by this preset, with the already-built
    /// `denoiser` dropped into the denoise slot (the engine the caller resolved
    /// from the intensity binding — never chosen by the preset).
    pub fn build_chain(&self, denoiser: Arc<dyn Denoiser>, workers: usize) -> Chain {
        // De-reverb character follows the content mode; the preset only gates it.
        let mut dereverb = match self.content_mode {
            ContentMode::Voiceover => DeReverb::voiceover(),
            ContentMode::Singing => DeReverb::singing(),
        };
        dereverb.set_enabled(self.dereverb_enabled);

        let mut breath = BreathControl::for_content(self.content_mode);
        match self.breath_db {
            Some(db) => {
                breath.set_enabled(true);
                breath.set_strength_db(db);
            }
            None => breath.set_enabled(false),
        }

        let mut finish = FinishStage::new(self.loudness_target);
        finish.set_eq(self.presence_db, self.air_db);

        // Sung sibilants want a gentler hand: duck less, floor the gain higher,
        // so falsetto/breathy HF isn't over-attenuated (research §6).
        let mut deess = DeEss::new(self.deess_mode);
        if self.content_mode == ContentMode::Singing {
            deess.set_sensitivity(0.6, 0.25);
        }

        Chain {
            // Restoration always runs (plan §5.7): declip/de-click/de-pop/de-hum
            // are on in every preset; de-bleed stays off for a native take.
            debleed: DeBleed::for_source(false),
            declip: DeClip::default(),
            declick: DeClick::new(),
            depop: DePop::new(),
            dehum: DeHum::default(),
            denoise: Denoise::with_engine(workers, denoiser),
            dereverb,
            deess,
            breath,
            finish,
            mix: [1.0; crate::chain::STAGE_COUNT],
            last_shared_hf_db: 0.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::denoise::SpectralDenoiser;
    use crate::processor::VocalProcessor;
    use crate::quality::Engine;

    fn light() -> Arc<dyn Denoiser> {
        Arc::new(SpectralDenoiser::default())
    }

    #[test]
    fn voiceover_presets_are_voiceover_grouped() {
        for p in [Preset::PODCAST, Preset::NARRATION, Preset::DIALOGUE_CLEANUP] {
            assert_eq!(p.content_mode, ContentMode::Voiceover);
        }
    }

    #[test]
    fn singing_presets_preserve_tails_and_silence_breath() {
        for p in [Preset::LEAD_VOCAL, Preset::NATURAL_VOCAL] {
            assert_eq!(p.content_mode, ContentMode::Singing);
            assert!(!p.dereverb_enabled, "singing keeps de-reverb off for tails");
            assert_eq!(p.breath_db, None, "singing leaves breaths intact");
            assert_eq!(
                p.deess_mode,
                DeEssMode::Split,
                "singing uses split-band de-ess so sustains aren't ducked (research §5)"
            );
        }
    }

    #[test]
    fn intensity_owns_the_engine_not_the_preset() {
        // The preset only carries an intensity; the engine falls out of the
        // intensity's binding. Balanced/HQ → neural, Natural → light.
        assert_eq!(
            Preset::PODCAST.intensity_default.binding().engine,
            Engine::Neural
        );
        assert_eq!(
            Preset::NARRATION.intensity_default.binding().engine,
            Engine::Neural
        );
        assert_eq!(
            Preset::NATURAL_VOCAL.intensity_default.binding().engine,
            Engine::Light
        );
    }

    #[test]
    fn default_for_content_mode_picks_the_lead_preset() {
        assert_eq!(Preset::default_for(ContentMode::Voiceover).name, "Podcast");
        assert_eq!(Preset::default_for(ContentMode::Singing).name, "Lead Vocal");
    }

    #[test]
    fn podcast_builds_a_voiceover_finish_chain() {
        let chain = Preset::PODCAST.build_chain(light(), 2);
        assert!(chain.breath.enabled(), "Podcast runs breath control");
        assert!(chain.dereverb.enabled(), "Podcast runs de-reverb");
        assert!(chain.deess.enabled());
        assert_eq!(chain.deess.mode(), DeEssMode::Split);
        assert!(chain.finish.enabled());
        assert!(chain.denoise.enabled());
    }

    #[test]
    fn lead_vocal_builds_a_tail_preserving_chain() {
        let chain = Preset::LEAD_VOCAL.build_chain(light(), 2);
        assert!(!chain.dereverb.enabled(), "Lead Vocal keeps de-reverb off");
        assert!(!chain.breath.enabled(), "Lead Vocal leaves breaths intact");
        assert_eq!(chain.deess.mode(), DeEssMode::Split);
    }

    #[test]
    fn built_chain_preserves_length() {
        use crate::audio::{AudioBuffer, ChannelLayout, SampleEncoding};
        use std::f32::consts::PI;
        let n = 20_000;
        let ch: Vec<f32> = (0..n)
            .map(|i| 0.3 * (2.0 * PI * 220.0 * i as f32 / 48_000.0).sin())
            .collect();
        let mut buf = AudioBuffer::new(48_000, 16, SampleEncoding::Int, vec![ch]);
        let mut chain = Preset::PODCAST.build_chain(light(), 2);
        chain.prepare(48_000, 4096, ChannelLayout::Mono);
        chain.reset();
        chain.process(&mut buf);
        assert_eq!(buf.frames(), n);
    }
}
