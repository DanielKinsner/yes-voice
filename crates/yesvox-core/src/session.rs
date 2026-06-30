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
use crate::card::CardId;
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
    /// Global dry→wet master over the whole rack (the "Flow Amount" macro), `[0, 1]`.
    /// 1 = full effect, 0 = the rack is the identity. Composes multiplicatively
    /// with each card's own mix.
    flow: f32,
    /// The "Polish Intensity" macro (Gentle ↔ Strong), `[0, 1]`. The default 0.5
    /// is the preset's nominal polish; lower backs the polish group off. See
    /// [`Session::set_polish_intensity`].
    polish: f32,
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
            flow: 1.0,
            polish: 0.5,
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

    /// The *effective* enabled state of a stage — the user's override if set,
    /// otherwise the active preset's default. This is the on/off position the
    /// Standard-view card toggle shows, resolvable without a render.
    pub fn resolve_stage_enabled(&self, stage: Stage) -> bool {
        self.cards[stage as usize]
            .enabled
            .unwrap_or_else(|| self.preset.stage_default_enabled(stage))
    }

    /// Enable or bypass a UI **card** (UI ref card model). A card fans out to one
    /// or more engine stages — e.g. "De-click / De-pop" toggles both `DeClick` and
    /// `DePop` — so the agent and the egui toggle drive the same single action.
    pub fn set_card_enabled(&mut self, card: CardId, on: bool) {
        for &stage in card.stages() {
            self.set_stage_enabled(stage, on);
        }
    }

    /// Whether a UI card is effectively on — true iff every stage it governs
    /// resolves to enabled.
    pub fn card_enabled(&self, card: CardId) -> bool {
        card.stages().iter().all(|&s| self.resolve_stage_enabled(s))
    }

    /// Set the global dry→wet master (the "Flow Amount" macro), clamped `[0, 1]`.
    /// 0 makes the whole rack the identity; 1 is full effect. Invalidates the
    /// last render.
    pub fn set_flow_amount(&mut self, amount: f32) {
        self.flow = amount.clamp(0.0, 1.0);
        self.output = None;
    }

    /// The current Flow Amount master.
    pub fn flow_amount(&self) -> f32 {
        self.flow
    }

    /// The polish-rack stages the "Polish Intensity" macro scales. v1 covers the
    /// 1:1 polish cards (de-ess, breath); presence/air + leveling join once their
    /// finish sub-controls are independently addressable, and loudness stays out
    /// so the target can't be perturbed.
    const POLISH_STAGES: [Stage; 2] = [Stage::DeEss, Stage::Breath];

    /// Set the **Polish Intensity** radial (Gentle ↔ Strong), `[0, 1]`. The default
    /// 0.5 maps to the preset's nominal polish (scale 1.0), so default renders are
    /// unchanged; the lower half backs the polish group off toward Gentle. The
    /// upper half is reserved for future over-drive (which needs the polish
    /// sub-params, not a wet/dry that already tops out at full). Invalidates the
    /// last render.
    pub fn set_polish_intensity(&mut self, t: f32) {
        self.polish = t.clamp(0.0, 1.0);
        self.output = None;
    }

    /// The current Polish Intensity macro position.
    pub fn polish_intensity(&self) -> f32 {
        self.polish
    }

    /// The polish-group scale the macro currently applies (0.5 → 1.0 nominal,
    /// Gentle reduces, Strong tops out at full).
    fn polish_scale(&self) -> f32 {
        (2.0 * self.polish).clamp(0.0, 1.0)
    }

    /// The effective wet/dry mix a stage renders at: the card's own mix scaled by
    /// the rack-wide macros (Flow Amount globally, Polish Intensity over the polish
    /// group). The single place macros compose, so the null guarantees stay clean
    /// as more macros land.
    fn effective_mix(&self, stage: Stage) -> f32 {
        let polish = if Self::POLISH_STAGES.contains(&stage) {
            self.polish_scale()
        } else {
            1.0
        };
        self.cards[stage as usize].mix * self.flow * polish
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

    /// Set the processing intensity (the quality slider) directly.
    pub fn set_quality_mode(&mut self, mode: QualityMode) {
        self.settings.quality_mode = mode;
        self.output = None;
    }

    /// Set the **Restoration Focus** radial (More Transparent ↔ More Aggressive),
    /// `[0, 1]`. The radial is visually continuous but [`QualityMode`] is the
    /// authoritative 3-value engine axis, so this quantizes onto it in equal
    /// thirds (Natural / Balanced / Restoration HQ) rather than inventing a fourth
    /// engine state. [`Session::set_quality_mode`] stays the exact escape hatch.
    pub fn set_restoration_focus(&mut self, t: f32) {
        let t = t.clamp(0.0, 1.0);
        let mode = if t < 1.0 / 3.0 {
            QualityMode::Natural
        } else if t < 2.0 / 3.0 {
            QualityMode::Balanced
        } else {
            QualityMode::RestorationHq
        };
        self.set_quality_mode(mode);
    }

    /// The Restoration Focus radial position — the canonical center of the current
    /// intensity's band, so the radial re-renders stably (and round-trips through
    /// [`Session::set_restoration_focus`]).
    pub fn restoration_focus(&self) -> f32 {
        match self.settings.quality_mode {
            QualityMode::Natural => 1.0 / 6.0,
            QualityMode::Balanced => 0.5,
            QualityMode::RestorationHq => 5.0 / 6.0,
        }
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

        // Apply the per-card overrides (the UI rack) over the preset's defaults,
        // each scaled by the rack-wide macros (Flow Amount, …) via effective_mix.
        for stage in Stage::ALL {
            built.chain.set_mix(stage, self.effective_mix(stage));
            if let Some(on) = self.cards[stage as usize].enabled {
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
    fn polish_intensity_scales_only_the_polish_group() {
        let path = temp("polish");
        degraded_fixture(&path);

        let mut nominal_s = Session::new();
        nominal_s.open(&path).unwrap();
        nominal_s.render().unwrap();
        let nominal = nominal_s.output().unwrap().channel(0).to_vec();

        let mut gentle_s = Session::new();
        gentle_s.open(&path).unwrap();
        gentle_s.set_polish_intensity(0.0);
        gentle_s.render().unwrap();
        let gentle = gentle_s.output().unwrap().channel(0).to_vec();

        // Polish 0 backs the polish group off → the render changes.
        assert!(
            nominal
                .iter()
                .zip(&gentle)
                .any(|(a, b)| (a - b).abs() > 1e-4),
            "polish 0 must back the polish group off"
        );

        // It is *exactly* equivalent to dialing the de-ess + breath cards to dry,
        // proving Polish Intensity scales only that group (restore + the loudness
        // target are untouched).
        let mut cards_s = Session::new();
        cards_s.open(&path).unwrap();
        cards_s.set_stage_mix(Stage::DeEss, 0.0);
        cards_s.set_stage_mix(Stage::Breath, 0.0);
        cards_s.render().unwrap();
        let cards_dry = cards_s.output().unwrap().channel(0).to_vec();
        for (a, b) in gentle.iter().zip(&cards_dry) {
            assert!(
                (a - b).abs() <= 1e-6,
                "polish 0 ≡ de-ess + breath dialed dry"
            );
        }

        // The default (0.5) is the preset's nominal polish: the default render is
        // unchanged.
        assert_eq!(Session::new().polish_intensity(), 0.5);
        let mut base = Session::new();
        base.open(&path).unwrap();
        base.render().unwrap();
        for (a, b) in nominal.iter().zip(base.output().unwrap().channel(0)) {
            assert!(
                (a - b).abs() <= 1e-6,
                "default polish leaves the render nominal"
            );
        }
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn restoration_focus_quantizes_to_quality_mode() {
        let mut s = Session::new();
        s.set_restoration_focus(0.0);
        assert_eq!(s.settings().quality_mode, QualityMode::Natural);
        s.set_restoration_focus(0.5);
        assert_eq!(s.settings().quality_mode, QualityMode::Balanced);
        s.set_restoration_focus(1.0);
        assert_eq!(s.settings().quality_mode, QualityMode::RestorationHq);
        // Band boundaries: 1/3 enters Balanced, 2/3 enters Restoration HQ.
        s.set_restoration_focus(0.33);
        assert_eq!(s.settings().quality_mode, QualityMode::Natural);
        s.set_restoration_focus(0.34);
        assert_eq!(s.settings().quality_mode, QualityMode::Balanced);
        s.set_restoration_focus(0.66);
        assert_eq!(s.settings().quality_mode, QualityMode::Balanced);
        s.set_restoration_focus(0.67);
        assert_eq!(s.settings().quality_mode, QualityMode::RestorationHq);
        // The radial readout round-trips back into the same band.
        for mode in [
            QualityMode::Natural,
            QualityMode::Balanced,
            QualityMode::RestorationHq,
        ] {
            s.set_quality_mode(mode);
            let t = s.restoration_focus();
            s.set_restoration_focus(t);
            assert_eq!(
                s.settings().quality_mode,
                mode,
                "the focus readout must round-trip into the same mode"
            );
        }
    }

    #[test]
    fn flow_amount_master_nulls_the_whole_rack_at_zero() {
        let path = temp("flow-null");
        let input = degraded_fixture(&path);
        let mut s = Session::new();
        s.open(&path).expect("open");

        // Flow 0 dials every card fully dry → the whole rack is the identity.
        s.set_flow_amount(0.0);
        assert_eq!(s.flow_amount(), 0.0);
        s.render().expect("render");
        let dry = s.output().unwrap();
        for (a, b) in dry.channel(0).iter().zip(input.channel(0)) {
            assert!((a - b).abs() <= 1e-6, "flow 0 must null the whole rack");
        }

        // Flow 1 renders identically to the default render (which never set flow).
        s.set_flow_amount(1.0);
        s.render().expect("render");
        let wet = s.output().unwrap().channel(0).to_vec();
        let mut base = Session::new();
        base.open(&path).expect("open");
        base.render().expect("render");
        let base = base.output().unwrap().channel(0).to_vec();
        for (a, b) in wet.iter().zip(&base) {
            assert!((a - b).abs() <= 1e-6, "flow 1 == the default render");
        }
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn card_model_resolves_preset_defaults_and_fans_out_to_stages() {
        use crate::card::CardId;
        // resolve_stage_enabled folds the preset default under the None override,
        // so the UI toggle shows the *true* on/off position without a render.
        let mut s = Session::new(); // voiceover / Podcast
        assert_eq!(s.stage_enabled_override(Stage::Breath), None);
        assert!(
            s.resolve_stage_enabled(Stage::Breath),
            "Podcast runs breath control by default"
        );
        assert!(
            !s.resolve_stage_enabled(Stage::DeBleed),
            "de-bleed is off for a native take"
        );

        // The singing default (Lead Vocal) turns breath + de-reverb off by default.
        s.set_content_mode(ContentMode::Singing);
        assert!(
            !s.resolve_stage_enabled(Stage::Breath),
            "Lead Vocal leaves breaths intact"
        );
        assert!(
            !s.resolve_stage_enabled(Stage::DeReverb),
            "Lead Vocal keeps de-reverb off to protect tails"
        );

        // One UI card fans out to two engine stages (De-click/De-pop → DeClick+DePop).
        s.set_card_enabled(CardId::DeClickDePop, false);
        assert_eq!(s.stage_enabled_override(Stage::DeClick), Some(false));
        assert_eq!(s.stage_enabled_override(Stage::DePop), Some(false));
        assert!(!s.card_enabled(CardId::DeClickDePop));
        // A 1:1 card resolves through the same path.
        assert!(s.card_enabled(CardId::Denoise));
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
