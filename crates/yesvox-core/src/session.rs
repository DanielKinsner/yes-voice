//! The stateful editing session — the agent-native model the GUI (and CLI, and
//! any agent) drives (plan §5; UI ref "every visible action must have an
//! agent-callable equivalent").
//!
//! [`BuiltChain`] is stateless orchestration; a [`Session`] is the stateful layer
//! over it: it holds the open vocal, its analysis, the chosen content mode /
//! intensity / preset / export settings, and the last render — and exposes every
//! user action as a plain method (open, set content mode, set intensity, select
//! preset, render, export). The egui shell renders this state and calls these
//! methods; nothing about the UI knows DSP.

use std::path::{Path, PathBuf};

use crate::analysis::{analyze, AnalysisError, AnalysisReport, ContentGuess};
use crate::audio::AudioBuffer;
use crate::build::BuiltChain;
use crate::chain::{Stage, STAGE_COUNT};
use crate::io::{export, read_audio, IoError};
use crate::loudness::integrated_lufs;
use crate::modules::BleedVerdict;
use crate::preset::Preset;
use crate::processor::VocalProcessor;
use crate::quality::QualityMode;
use crate::settings::{ContentMode, Settings};

/// Why a session action failed — each renders an actionable message.
#[derive(Debug)]
pub enum SessionError {
    /// No vocal has been opened yet.
    NotLoaded,
    /// `render` has not run, so there is nothing to export.
    NotRendered,
    /// Decode/IO failure.
    Io(IoError),
    /// The whole-file analyzer rejected the input.
    Analysis(AnalysisError),
    /// De-bleed refused the input as a mix (plan §4.5).
    BleedBlocked(String),
}

impl std::fmt::Display for SessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SessionError::NotLoaded => write!(f, "no vocal is open yet"),
            SessionError::NotRendered => write!(f, "nothing to export — render first"),
            SessionError::Io(e) => write!(f, "{e}"),
            SessionError::Analysis(e) => write!(f, "{e}"),
            SessionError::BleedBlocked(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for SessionError {}

/// A per-rack-card override layered over the preset's defaults: an optional
/// enable (None = use the preset's default) and a wet/dry mix.
#[derive(Clone, Copy)]
struct Card {
    enabled: Option<bool>,
    mix: f32,
}

impl Default for Card {
    fn default() -> Self {
        Card {
            enabled: None,
            mix: 1.0,
        }
    }
}

/// A stateful vocal-editing session.
pub struct Session {
    settings: Settings,
    preset: Preset,
    input: Option<AudioBuffer>,
    report: Option<AnalysisReport>,
    output: Option<AudioBuffer>,
    cards: [Card; STAGE_COUNT],
    workers: usize,
    neural_model: Option<PathBuf>,
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

impl Session {
    /// A fresh session: default (voiceover / Balanced) settings, the Podcast
    /// preset, workers sized to the machine.
    pub fn new() -> Self {
        let settings = Settings::default();
        let preset = Preset::default_for(settings.content_mode);
        let workers = std::thread::available_parallelism()
            .map(|n| n.get().saturating_sub(1).max(1))
            .unwrap_or(1);
        Session {
            settings,
            preset,
            input: None,
            report: None,
            output: None,
            cards: [Card::default(); STAGE_COUNT],
            workers,
            neural_model: std::env::var_os("YESVOX_MODEL_PATH").map(PathBuf::from),
        }
    }

    /// A session seeded with specific settings + preset (used by the batch queue
    /// to apply one configuration across many files).
    pub fn with_config(settings: Settings, preset: Preset) -> Self {
        let mut s = Session::new();
        s.settings = settings;
        s.preset = preset;
        s
    }

    /// Open → render → export in one shot, using the session's current
    /// configuration (the headless/batch path). Returns the written path.
    pub fn process_file(&mut self, input: &Path, output: &Path) -> Result<PathBuf, SessionError> {
        self.open(input)?;
        self.render()?;
        self.export(output)
    }

    /// Set a rack card's wet/dry mix (its "amount" knob), `[0, 1]`.
    pub fn set_stage_mix(&mut self, stage: Stage, mix: f32) {
        self.cards[stage as usize].mix = mix.clamp(0.0, 1.0);
        self.output = None;
    }

    /// A rack card's current wet/dry mix.
    pub fn stage_mix(&self, stage: Stage) -> f32 {
        self.cards[stage as usize].mix
    }

    /// Enable or bypass a rack card (overriding the preset default).
    pub fn set_stage_enabled(&mut self, stage: Stage, on: bool) {
        self.cards[stage as usize].enabled = Some(on);
        self.output = None;
    }

    /// A rack card's enable override, if the user set one (`None` = preset default).
    pub fn stage_enabled_override(&self, stage: Stage) -> Option<bool> {
        self.cards[stage as usize].enabled
    }

    /// Reset every card to "use the preset default" — a preset change resets the
    /// rack (plan §5.3).
    fn reset_cards(&mut self) {
        self.cards = [Card::default(); STAGE_COUNT];
    }

    /// Open and analyze a vocal. The content toggle stays user-authoritative —
    /// the analyzer's guess is exposed via [`Session::detected_content`] but does
    /// not flip the mode. Clears any previous render.
    pub fn open(&mut self, path: &Path) -> Result<&AnalysisReport, SessionError> {
        let buffer = read_audio(path).map_err(SessionError::Io)?;
        let report = analyze(&buffer).map_err(SessionError::Analysis)?;
        self.input = Some(buffer);
        self.report = Some(report);
        self.output = None;
        Ok(self.report.as_ref().unwrap())
    }

    /// The analyzer's voiceover-vs-singing guess (advisory; the toggle decides).
    pub fn detected_content(&self) -> Option<ContentGuess> {
        self.report.map(|r| r.content)
    }

    /// Set the content mode (the Voiceover/Singing toggle). Re-selects the default
    /// preset for the new mode and invalidates the last render.
    pub fn set_content_mode(&mut self, mode: ContentMode) {
        self.settings.content_mode = mode;
        self.preset = Preset::default_for(mode);
        self.reset_cards();
        self.output = None;
    }

    /// Set the processing intensity (the Restoration Focus macro / quality slider).
    pub fn set_quality_mode(&mut self, mode: QualityMode) {
        self.settings.quality_mode = mode;
        self.output = None;
    }

    /// Select a preset from the gallery. Adopts the preset's content mode and
    /// resets the intensity to the preset's default (plan §5.3).
    pub fn select_preset(&mut self, preset: Preset) {
        self.preset = preset;
        self.settings.content_mode = preset.content_mode;
        self.settings.quality_mode = preset.intensity_default;
        self.reset_cards();
        self.output = None;
    }

    /// The current settings (content mode, intensity, export, singing).
    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// The active preset.
    pub fn preset(&self) -> Preset {
        self.preset
    }

    /// The analysis report for the open vocal, if any.
    pub fn report(&self) -> Option<&AnalysisReport> {
        self.report.as_ref()
    }

    /// Input integrated loudness (LUFS) for the meters.
    pub fn input_lufs(&self) -> Option<f32> {
        self.input.as_ref().and_then(integrated_lufs)
    }

    /// Output integrated loudness (LUFS), once rendered.
    pub fn output_lufs(&self) -> Option<f32> {
        self.output.as_ref().and_then(integrated_lufs)
    }

    /// The processed output buffer, once rendered.
    pub fn output(&self) -> Option<&AudioBuffer> {
        self.output.as_ref()
    }

    /// The dry input — the "A" / "Original" side of the A/B compare.
    pub fn input(&self) -> Option<&AudioBuffer> {
        self.input.as_ref()
    }

    /// The difference-monitor signal — "what was removed" (`original − processed`,
    /// plan §5.5). `None` until a render exists.
    pub fn difference(&self) -> Option<AudioBuffer> {
        Some(crate::monitor::difference(
            self.input.as_ref()?,
            self.output.as_ref()?,
        ))
    }

    /// Reblend `amount` of the removed signal back into the processed output — the
    /// global post-rack wet/dry (plan §5.5). `amount` 0 = processed, 1 = original.
    pub fn reblend(&self, amount: f32) -> Option<AudioBuffer> {
        Some(crate::monitor::reblend(
            self.output.as_ref()?,
            self.input.as_ref()?,
            amount,
        ))
    }

    /// The gain to apply to the dry "A" so it loudness-matches the processed "B"
    /// for a fair A/B compare (plan §5.5).
    pub fn ab_match_gain(&self) -> Option<f32> {
        Some(crate::monitor::loudness_match_gain(
            self.input_lufs()?,
            self.output_lufs()?,
        ))
    }

    /// Run the bound chain on the open vocal, caching the processed result.
    /// Refuses a mix (de-bleed `Block`) with an actionable error.
    pub fn render(&mut self) -> Result<(), SessionError> {
        let input = self.input.as_ref().ok_or(SessionError::NotLoaded)?;
        let report = self.report.as_ref().ok_or(SessionError::NotLoaded)?;

        // No SKU table at runtime, so est_render_s = 0.0 disables the downshift
        // here (the visible Fast-mode badge is a GUI concern).
        let mut built = BuiltChain::build(
            &self.settings,
            self.preset,
            report,
            self.neural_model.as_deref(),
            self.workers,
            0.0,
        );

        // Apply the per-card overrides (the UI rack) over the preset's defaults.
        for stage in Stage::ALL {
            let card = self.cards[stage as usize];
            built.chain.set_mix(stage, card.mix);
            if let Some(on) = card.enabled {
                built.chain.set_stage_enabled(stage, on);
            }
        }

        // De-bleed refusal (plan §4.5): refuse a mix before processing.
        let verdict = built.chain.assess_bleed(input);
        if verdict == BleedVerdict::Block {
            return Err(SessionError::BleedBlocked(
                verdict
                    .message()
                    .unwrap_or("input looks like a mix — run YesStems first")
                    .to_string(),
            ));
        }

        let mut buffer = input.clone();
        built.chain.prepare(
            buffer.sample_rate(),
            buffer.frames().max(1),
            buffer.layout(),
        );
        built.chain.reset();
        built.chain.process(&mut buffer);
        self.output = Some(buffer);
        Ok(())
    }

    /// Export the last render to `path` (never-overwrites, returns the written
    /// path). Errors if nothing has been rendered.
    pub fn export(&self, path: &Path) -> Result<PathBuf, SessionError> {
        let buffer = self.output.as_ref().ok_or(SessionError::NotRendered)?;
        let opts = self
            .settings
            .export
            .to_options(buffer.bits_per_sample(), buffer.encoding());
        export(path, buffer, opts).map_err(SessionError::Io)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::SampleEncoding;
    use crate::io::write_wav;
    use std::f32::consts::PI;

    const SR: u32 = 48_000;

    fn degraded_fixture(path: &Path) -> AudioBuffer {
        let n = 48_000;
        let mut x: Vec<f32> = (0..n)
            .map(|i| {
                let t = i as f32 / SR as f32;
                (0.4 * (2.0 * PI * 200.0 * t).sin() + 0.3 * (2.0 * PI * 7000.0 * t).sin())
                    .clamp(-0.7, 0.7)
            })
            .collect();
        for p in [5_000usize, 15_000, 25_000, 35_000] {
            x[p] += 0.9;
        }
        let buf = AudioBuffer::new(SR, 24, SampleEncoding::Int, vec![x]);
        write_wav(path, &buf).expect("write fixture");
        buf
    }

    /// A clean, quiet 220 Hz tone — integrated loudness well below −16 LUFS, so a
    /// MatchInput preset and a −16 LUFS preset render to clearly different levels.
    fn quiet_tone_fixture(path: &Path) -> AudioBuffer {
        let n = SR as usize * 2;
        let x: Vec<f32> = (0..n)
            .map(|i| 0.05 * (2.0 * PI * 220.0 * i as f32 / SR as f32).sin())
            .collect();
        let buf = AudioBuffer::new(SR, 24, SampleEncoding::Int, vec![x]);
        write_wav(path, &buf).expect("write fixture");
        buf
    }

    fn temp(tag: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("yesvox-session-{tag}-{}.wav", std::process::id()));
        let _ = std::fs::remove_file(&p);
        p
    }

    #[test]
    fn render_applies_the_selected_preset_not_the_mode_default() {
        // Regression for the preset-application defect: render() must build the
        // chain from the *selected* preset, not Preset::default_for(content_mode).
        let path = temp("g0-selected-preset");
        quiet_tone_fixture(&path);
        let mut s = Session::new();
        s.open(&path).expect("open");

        // The default voiceover preset is Podcast (loudness target −16 LUFS).
        s.render().expect("render");
        let podcast_lufs = s.output_lufs().expect("podcast out lufs");
        assert!(
            (podcast_lufs - (-16.0)).abs() <= 0.6,
            "the default Podcast preset normalizes to −16 LUFS, got {podcast_lufs}"
        );

        // Select Dialogue-cleanup — a *non-default* voiceover preset whose loudness
        // target is MatchInput (keep the quiet input level), not −16. Same content
        // mode + intensity as the default, so the *only* difference is the preset
        // character: selecting it must change what renders, not just what's shown.
        s.select_preset(Preset::DIALOGUE_CLEANUP);
        s.render().expect("render");
        let dialogue_lufs = s.output_lufs().expect("dialogue out lufs");
        assert!(
            dialogue_lufs < -18.0,
            "Dialogue-cleanup (MatchInput) must keep the quiet input level, not snap \
             to the Podcast −16 default — got {dialogue_lufs} (bug: the selected \
             preset is ignored and the mode-default Podcast is rendered)"
        );

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn open_sets_report_and_default_preset() {
        let path = temp("open-in");
        degraded_fixture(&path);
        let mut s = Session::new();
        s.open(&path).expect("open");
        assert!(s.report().is_some());
        assert_eq!(s.preset().name, "Podcast", "voiceover default preset");
        assert!(s.input_lufs().is_some());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn content_toggle_and_preset_selection_drive_settings() {
        let mut s = Session::new();
        s.set_content_mode(ContentMode::Singing);
        assert_eq!(s.preset().name, "Lead Vocal");
        assert_eq!(s.settings().content_mode, ContentMode::Singing);
        // Selecting a preset adopts its mode and resets intensity to its default.
        s.select_preset(Preset::NARRATION);
        assert_eq!(s.settings().content_mode, ContentMode::Voiceover);
        assert_eq!(s.settings().quality_mode, QualityMode::RestorationHq);
    }

    #[test]
    fn render_then_export_writes_processed_output() {
        let in_path = temp("render-in");
        let out_path = temp("render-out");
        let input = degraded_fixture(&in_path);
        let before = std::fs::read(&in_path).unwrap();

        let mut s = Session::new();
        s.open(&in_path).expect("open");
        s.render().expect("render");

        let output = s.output().expect("rendered output");
        assert_eq!(output.frames(), input.frames(), "same length");
        let changed = output
            .channel(0)
            .iter()
            .zip(input.channel(0))
            .any(|(a, b)| (a - b).abs() > 1e-3);
        assert!(changed, "the chain actually processed the vocal");
        assert!(s.output_lufs().is_some());

        let written = s.export(&out_path).expect("export");
        assert!(written.exists());
        // Non-destructive: the input file is untouched.
        assert_eq!(before, std::fs::read(&in_path).unwrap());

        let _ = std::fs::remove_file(&in_path);
        let _ = std::fs::remove_file(&written);
    }

    #[test]
    fn export_before_render_errors() {
        let s = Session::new();
        assert!(matches!(
            s.export(Path::new("nope.wav")),
            Err(SessionError::NotRendered)
        ));
    }

    #[test]
    fn dialing_every_card_to_dry_renders_the_input_unchanged() {
        let path = temp("cards-dry");
        let input = degraded_fixture(&path);
        let mut s = Session::new();
        s.open(&path).expect("open");
        for stage in Stage::ALL {
            s.set_stage_mix(stage, 0.0);
        }
        s.render().expect("render");
        let out = s.output().unwrap();
        for (a, b) in out.channel(0).iter().zip(input.channel(0)) {
            assert!((a - b).abs() <= 1e-6, "all cards dry → input unchanged");
        }
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn disabling_a_card_changes_the_render() {
        let path = temp("cards-disable");
        degraded_fixture(&path);
        let render = |disable: bool| {
            let mut s = Session::new();
            s.open(&path).expect("open");
            if disable {
                s.set_stage_enabled(Stage::Denoise, false);
            }
            s.render().expect("render");
            s.output().unwrap().channel(0).to_vec()
        };
        let on = render(false);
        let off = render(true);
        let changed = on.iter().zip(&off).any(|(a, b)| (a - b).abs() > 1e-4);
        assert!(changed, "toggling a card must change the render");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn monitor_surface_appears_after_render() {
        let path = temp("monitor");
        let input = degraded_fixture(&path);
        let mut s = Session::new();
        s.open(&path).expect("open");
        // Nothing to monitor before a render.
        assert!(s.difference().is_none());
        assert!(s.reblend(0.5).is_none());

        s.render().expect("render");
        let diff = s.difference().expect("difference");
        // The chain changed something, so "what was removed" isn't all silence.
        assert!(
            diff.channel(0).iter().any(|&v| v.abs() > 1e-4),
            "difference monitor should show removed signal"
        );
        // Reblend endpoints: 0 == processed, 1 == original.
        let out = s.output().unwrap();
        let r0 = s.reblend(0.0).unwrap();
        for (a, b) in r0.channel(0).iter().zip(out.channel(0)) {
            assert!((a - b).abs() <= 1e-6, "reblend 0 == processed");
        }
        let r1 = s.reblend(1.0).unwrap();
        for (a, b) in r1.channel(0).iter().zip(input.channel(0)) {
            assert!((a - b).abs() <= 1e-6, "reblend 1 == original");
        }
        assert!(s.ab_match_gain().is_some());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn preset_change_resets_card_tweaks() {
        let mut s = Session::new();
        s.set_stage_mix(Stage::DeReverb, 0.2);
        assert_eq!(s.stage_mix(Stage::DeReverb), 0.2);
        s.select_preset(Preset::NARRATION);
        assert_eq!(
            s.stage_mix(Stage::DeReverb),
            1.0,
            "selecting a preset resets the rack (§5.3)"
        );
    }
}
