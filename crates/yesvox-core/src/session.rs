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
use crate::modules::{BleedVerdict, LoudnessTarget};
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

/// The export container the OUTPUT panel chooses. The sample rate is *not* here —
/// it is locked to the input (a hard guardrail + the same-length CI gate), so the
/// OUTPUT panel shows it as a read-only chip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputFormat {
    /// RIFF/WAVE.
    Wav,
    /// FLAC (lossless compressed).
    Flac,
}

impl OutputFormat {
    /// The lowercase file extension (no dot).
    pub fn extension(self) -> &'static str {
        match self {
            OutputFormat::Wav => "wav",
            OutputFormat::Flac => "flac",
        }
    }

    /// The OUTPUT-panel display label.
    pub fn label(self) -> &'static str {
        match self {
            OutputFormat::Wav => "WAV",
            OutputFormat::Flac => "FLAC",
        }
    }
}

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

/// Independent enable overrides for the Standard finish cards that share
/// `Stage::Finish` underneath.
#[derive(Clone, Copy, Default)]
struct FinishCards {
    presence_air_enabled: Option<bool>,
    leveling_enabled: Option<bool>,
    loudness_enabled: Option<bool>,
}

impl FinishCards {
    fn set_enabled(&mut self, card: CardId, on: bool) -> bool {
        match card {
            CardId::PresenceAir => self.presence_air_enabled = Some(on),
            CardId::Leveling => self.leveling_enabled = Some(on),
            CardId::Loudness => self.loudness_enabled = Some(on),
            _ => return false,
        }
        true
    }

    fn enabled_override(&self, card: CardId) -> Option<bool> {
        match card {
            CardId::PresenceAir => self.presence_air_enabled,
            CardId::Leveling => self.leveling_enabled,
            CardId::Loudness => self.loudness_enabled,
            _ => None,
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
    finish_cards: FinishCards,
    /// Global dry→wet master over the whole rack (the "Flow Amount" macro), `[0, 1]`.
    /// 1 = full effect, 0 = the rack is the identity. Composes multiplicatively
    /// with each card's own mix.
    flow: f32,
    /// The "Polish Intensity" macro (Gentle ↔ Strong), `[0, 1]`. The default 0.5
    /// is the preset's nominal polish; lower backs the polish group off. See
    /// [`Session::set_polish_intensity`].
    polish: f32,
    /// A Session-level loudness-target override (the OUTPUT panel / Loudness card).
    /// `None` honors the active preset's target.
    loudness_override: Option<LoudnessTarget>,
    /// Export container (OUTPUT panel). Sample rate is locked to the input.
    output_format: OutputFormat,
    /// Output file name without extension (FILE panel); defaulted on `open`.
    output_name: Option<String>,
    /// Output directory (FILE panel); defaults to the input's parent on `open`.
    output_dir: Option<PathBuf>,
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
            finish_cards: FinishCards::default(),
            flow: 1.0,
            polish: 0.5,
            loudness_override: None,
            output_format: OutputFormat::Wav,
            output_name: None,
            output_dir: None,
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
        if self.finish_cards.set_enabled(card, on) {
            self.output = None;
            return;
        }
        for &stage in card.stages() {
            self.set_stage_enabled(stage, on);
        }
    }

    /// Whether a UI card is effectively on — true iff every stage it governs
    /// resolves to enabled.
    pub fn card_enabled(&self, card: CardId) -> bool {
        if let Some(on) = self.resolve_finish_card_enabled(card) {
            return on;
        }
        card.stages().iter().all(|&s| self.resolve_stage_enabled(s))
    }

    fn resolve_finish_card_enabled(&self, card: CardId) -> Option<bool> {
        let sub = self.finish_cards.enabled_override(card)?;
        Some(self.resolve_stage_enabled(Stage::Finish) && sub)
    }

    /// Enable or bypass the Presence/Air card without affecting Leveling or
    /// Loudness. This is the Session-level surface behind the Standard card.
    pub fn set_presence_air_enabled(&mut self, on: bool) {
        self.finish_cards.presence_air_enabled = Some(on);
        self.output = None;
    }

    /// Whether the Presence/Air card is effectively active.
    pub fn presence_air_enabled(&self) -> bool {
        self.resolve_finish_card_enabled(CardId::PresenceAir)
            .unwrap_or_else(|| self.resolve_stage_enabled(Stage::Finish))
    }

    /// Enable or bypass the Leveling card. Audio remains neutral until G9 lands
    /// the actual program leveler, but the user-visible state is tracked now.
    pub fn set_leveling_enabled(&mut self, on: bool) {
        self.finish_cards.leveling_enabled = Some(on);
        self.output = None;
    }

    /// Whether the Leveling card is effectively active.
    pub fn leveling_enabled(&self) -> bool {
        self.resolve_finish_card_enabled(CardId::Leveling)
            .unwrap_or_else(|| self.resolve_stage_enabled(Stage::Finish))
    }

    /// Enable or bypass loudness normalization and the true-peak limiter.
    pub fn set_loudness_enabled(&mut self, on: bool) {
        self.finish_cards.loudness_enabled = Some(on);
        self.output = None;
    }

    /// Whether loudness normalization and the true-peak limiter are active.
    pub fn loudness_enabled(&self) -> bool {
        self.resolve_finish_card_enabled(CardId::Loudness)
            .unwrap_or_else(|| self.resolve_stage_enabled(Stage::Finish))
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

    /// Set the integrated-loudness target (the OUTPUT panel + the Loudness card,
    /// which bind to this one override), overriding the active preset's target.
    /// Use [`LoudnessTarget::Custom`] for the editable LUFS box. Invalidates the
    /// last render.
    pub fn set_loudness_target(&mut self, target: LoudnessTarget) {
        self.loudness_override = Some(target);
        self.output = None;
    }

    /// The effective loudness target — the Session override if set, otherwise the
    /// active preset's target.
    pub fn loudness_target(&self) -> LoudnessTarget {
        self.loudness_override
            .unwrap_or(self.preset.loudness_target)
    }

    /// Override the export bit depth; `None` honors the source file's depth
    /// (OUTPUT panel). Export-time only — does not invalidate the render.
    pub fn set_bit_depth(&mut self, bits: Option<u16>) {
        self.settings.export.bit_depth_override = bits;
    }

    /// Toggle TPDF dither on ≤24-bit integer export (OUTPUT panel).
    pub fn set_dither(&mut self, on: bool) {
        self.settings.export.dither = on;
    }

    /// Set the export container (OUTPUT panel). The sample rate stays locked to
    /// the input.
    pub fn set_output_format(&mut self, format: OutputFormat) {
        self.output_format = format;
    }

    /// The chosen export container.
    pub fn output_format(&self) -> OutputFormat {
        self.output_format
    }

    /// Set the output file name (without extension, FILE panel).
    pub fn set_output_name(&mut self, name: &str) {
        self.output_name = Some(name.to_string());
    }

    /// The output file name without extension, if set (defaulted on `open`).
    pub fn output_name(&self) -> Option<&str> {
        self.output_name.as_deref()
    }

    /// Set the output directory (FILE panel).
    pub fn set_output_location(&mut self, dir: impl Into<PathBuf>) {
        self.output_dir = Some(dir.into());
    }

    /// The output directory, if set (defaults to the input's parent on `open`).
    pub fn output_location(&self) -> Option<&Path> {
        self.output_dir.as_deref()
    }

    /// The composed export path — `location / name . format-extension` — the
    /// "EXPORT VOCAL" button writes to. `None` until a name and location exist
    /// (i.e. after `open`).
    pub fn export_target(&self) -> Option<PathBuf> {
        let dir = self.output_dir.as_ref()?;
        let name = self.output_name.as_ref()?;
        Some(dir.join(format!("{name}.{}", self.output_format.extension())))
    }

    /// Reset every per-card and finish override to "use the preset default" — a
    /// preset change resets the rack (plan §5.3). The master macros (Flow Amount,
    /// Polish Intensity) persist across preset changes.
    fn reset_cards(&mut self) {
        self.cards = [Card::default(); STAGE_COUNT];
        self.finish_cards = FinishCards::default();
        self.loudness_override = None;
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
        // Default the export target from the opened file (FILE panel), as the mock
        // shows ("Lead Vocal" → "Lead Vocal_YesVox" beside the source's folder).
        if self.output_dir.is_none() {
            self.output_dir = path.parent().map(Path::to_path_buf);
        }
        if self.output_name.is_none() {
            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("vocal");
            self.output_name = Some(format!("{stem}_YesVox"));
        }
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

    /// The dry "A" scaled to the processed "B"'s loudness — the buffer the "Volume
    /// Match" toggle auditions so the compare is about character, not level.
    /// `None` until a render exists.
    pub fn ab_matched_input(&self) -> Option<AudioBuffer> {
        Some(crate::monitor::apply_gain(
            self.input.as_ref()?,
            self.ab_match_gain()?,
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
        // The Session-level loudness override (OUTPUT panel / Loudness card) wins
        // over the preset's finish target; with no override this re-asserts the
        // preset's own target, so it is a no-op.
        built.chain.finish.set_target(self.loudness_target());
        built
            .chain
            .finish
            .set_presence_air_enabled(self.presence_air_enabled());
        built
            .chain
            .finish
            .set_leveling_enabled(self.leveling_enabled());
        built
            .chain
            .finish
            .set_loudness_enabled(self.loudness_enabled());

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
    /// path). The container is inferred from `path`'s extension. Errors if nothing
    /// has been rendered.
    pub fn export(&self, path: &Path) -> Result<PathBuf, SessionError> {
        let buffer = self.output.as_ref().ok_or(SessionError::NotRendered)?;
        let opts = self
            .settings
            .export
            .to_options(buffer.bits_per_sample(), buffer.encoding());
        export(path, buffer, opts).map_err(SessionError::Io)
    }

    /// Export the last render to the composed [`Session::export_target`] (the
    /// "EXPORT VOCAL" button). Errors if nothing is loaded or rendered.
    pub fn export_to_target(&self) -> Result<PathBuf, SessionError> {
        let target = self.export_target().ok_or(SessionError::NotLoaded)?;
        self.export(&target)
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
    fn output_target_composes_name_location_format_and_bit_depth() {
        let in_path = temp("g12-in");
        quiet_tone_fixture(&in_path); // 24-bit WAV in temp_dir
        let mut s = Session::new();
        s.open(&in_path).unwrap();

        // Defaults are derived from the opened file (FILE panel).
        assert_eq!(s.output_location().unwrap(), in_path.parent().unwrap());
        assert!(
            s.output_name().unwrap().ends_with("_YesVox"),
            "default name suffixes the source stem, got {:?}",
            s.output_name()
        );
        assert_eq!(s.output_format(), OutputFormat::Wav);

        // Drive the OUTPUT + FILE panels.
        s.set_bit_depth(Some(16));
        s.set_dither(false);
        s.set_output_format(OutputFormat::Flac);
        let name = format!("yesvox-g12-target-{}", std::process::id());
        s.set_output_name(&name);
        s.set_output_location(std::env::temp_dir());
        s.render().unwrap();

        // export_target composes location / name . format-extension.
        let target = s.export_target().unwrap();
        assert!(
            target.ends_with(format!("{name}.flac")),
            "composed: {target:?}"
        );
        let _ = std::fs::remove_file(&target);
        let written = s.export_to_target().unwrap();
        assert_eq!(written, target);
        let back = crate::read_audio(&written).unwrap();
        assert_eq!(
            back.bits_per_sample(),
            16,
            "exported at the chosen bit depth, as FLAC"
        );

        let _ = std::fs::remove_file(&in_path);
        let _ = std::fs::remove_file(&written);
    }

    #[test]
    fn volume_match_brings_the_dry_to_the_processed_loudness() {
        let path = temp("vmatch");
        degraded_fixture(&path);
        let mut s = Session::new();
        s.open(&path).unwrap();
        s.render().unwrap();
        // The "Volume Match" checkbox auditions the dry A at the processed B's
        // loudness, so the compare is about character, not level.
        let matched = s.ab_matched_input().unwrap();
        let m_lufs = integrated_lufs(&matched).unwrap();
        let out_lufs = s.output_lufs().unwrap();
        assert!(
            (m_lufs - out_lufs).abs() <= 0.6,
            "volume-matched A must sit at B's loudness: {m_lufs} vs {out_lufs}"
        );
        // No render yet → nothing to match against.
        let mut fresh = Session::new();
        fresh.open(&path).unwrap();
        assert!(fresh.ab_matched_input().is_none());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn loudness_target_is_settable_on_the_session() {
        use crate::modules::LoudnessTarget;
        let path = temp("loudness-target");
        quiet_tone_fixture(&path);
        let mut s = Session::new();
        s.open(&path).unwrap();
        let in_lufs = s.input_lufs().unwrap();

        // Override the preset's target from the Session (OUTPUT panel / Loudness card).
        s.set_loudness_target(LoudnessTarget::Streaming);
        assert_eq!(s.loudness_target(), LoudnessTarget::Streaming);
        s.render().unwrap();
        assert!(
            (s.output_lufs().unwrap() - (-14.0)).abs() <= 0.5,
            "Streaming → −14 LUFS, got {:?}",
            s.output_lufs()
        );

        s.set_loudness_target(LoudnessTarget::Podcast);
        s.render().unwrap();
        assert!(
            (s.output_lufs().unwrap() - (-16.0)).abs() <= 0.5,
            "Podcast → −16 LUFS"
        );

        s.set_loudness_target(LoudnessTarget::MatchInput);
        s.render().unwrap();
        assert!(
            (s.output_lufs().unwrap() - in_lufs).abs() <= 1.0,
            "MatchInput keeps the input loudness"
        );

        // The editable LUFS box: an arbitrary custom target.
        s.set_loudness_target(LoudnessTarget::Custom(-20.0));
        s.render().unwrap();
        assert!(
            (s.output_lufs().unwrap() - (-20.0)).abs() <= 0.5,
            "Custom(-20) → −20 LUFS"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn finish_card_enables_are_independent_session_state() {
        let mut s = Session::new();
        assert!(s.presence_air_enabled());
        assert!(s.leveling_enabled());
        assert!(s.loudness_enabled());

        s.set_card_enabled(CardId::PresenceAir, false);
        assert!(!s.card_enabled(CardId::PresenceAir));
        assert!(s.card_enabled(CardId::Leveling));
        assert!(s.card_enabled(CardId::Loudness));
        assert_eq!(
            s.stage_enabled_override(Stage::Finish),
            None,
            "Presence/Air must not bypass the whole Finish stage"
        );

        s.set_leveling_enabled(false);
        assert!(
            !s.leveling_enabled(),
            "Leveling state is tracked independently"
        );
        assert!(s.loudness_enabled());

        s.set_loudness_enabled(false);
        assert!(!s.loudness_enabled());
        s.set_presence_air_enabled(true);
        assert!(s.presence_air_enabled());
        assert!(!s.leveling_enabled());
        assert!(!s.loudness_enabled());

        s.select_preset(Preset::NARRATION);
        assert!(
            s.presence_air_enabled(),
            "preset changes reset finish card state"
        );
        assert!(s.leveling_enabled());
        assert!(s.loudness_enabled());
    }

    #[test]
    fn finish_card_enables_reach_the_rendered_finish_stage() {
        let path = temp("finish-card-render");
        quiet_tone_fixture(&path);

        let mut no_presence = Session::new();
        no_presence.open(&path).unwrap();
        no_presence.set_card_enabled(CardId::PresenceAir, false);
        no_presence.render().unwrap();
        let no_presence_lufs = no_presence.output_lufs().unwrap();
        assert!(
            (no_presence_lufs - (-16.0)).abs() <= 0.6,
            "Presence/Air bypass must leave loudness running: {no_presence_lufs}"
        );

        let mut no_loudness = Session::new();
        no_loudness.open(&path).unwrap();
        no_loudness.set_card_enabled(CardId::Loudness, false);
        no_loudness.render().unwrap();
        let no_loudness_lufs = no_loudness.output_lufs().unwrap();
        assert!(
            (no_loudness_lufs - (-16.0)).abs() > 2.0,
            "Loudness bypass must reach render and skip normalization: {no_loudness_lufs}"
        );

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
