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
use crate::playback::PlaybackSource;
use crate::preset::Preset;
use crate::processor::VocalProcessor;
use crate::quality::QualityMode;
use crate::settings::{ContentMode, ExportOptions, Settings};

/// Why a session action failed — each renders an actionable message.
#[derive(Debug)]
pub enum SessionError {
    /// No vocal has been opened yet.
    NotLoaded,
    /// `render` has not run, so there is nothing to export.
    NotRendered,
    /// The render was canceled cooperatively between DSP stages.
    Canceled,
    /// Decode/IO failure.
    Io(IoError),
    /// The whole-file analyzer rejected the input.
    Analysis(AnalysisError),
    /// De-bleed refused the input as a mix (plan §4.5).
    BleedBlocked(String),
    /// The opened file has zero samples (#32).
    EmptyInput,
    /// The sample rate exceeds the ingest cap (#32).
    SampleRateTooHigh(u32),
    /// The vocal is longer than the ingest duration cap (#32).
    TooLong { seconds: f32 },
    /// The render's estimated peak memory exceeds the available budget and the
    /// over-budget override is off (#32).
    MemoryBudgetExceeded { estimated_mb: u64, budget_mb: u64 },
}

impl std::fmt::Display for SessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SessionError::NotLoaded => write!(f, "no vocal is open yet"),
            SessionError::NotRendered => write!(f, "nothing to export — render first"),
            SessionError::Canceled => write!(f, "render canceled"),
            SessionError::Io(e) => write!(f, "{e}"),
            SessionError::Analysis(e) => write!(f, "{e}"),
            SessionError::BleedBlocked(m) => write!(f, "{m}"),
            SessionError::EmptyInput => write!(f, "this file has no audio to process"),
            SessionError::SampleRateTooHigh(sr) => write!(
                f,
                "{sr} Hz is above the {} kHz limit — resample the vocal first",
                MAX_SAMPLE_RATE / 1000
            ),
            SessionError::TooLong { seconds } => write!(
                f,
                "this vocal is {:.0} min, over the {:.0} h limit — split it into shorter takes",
                seconds / 60.0,
                MAX_DURATION_SECS / 3600.0
            ),
            SessionError::MemoryBudgetExceeded {
                estimated_mb,
                budget_mb,
            } => write!(
                f,
                "this vocal needs ~{estimated_mb} MB to process, over the ~{budget_mb} MB budget — close other apps, split the file, or allow over-budget in settings"
            ),
        }
    }
}

/// Ingest caps (#32): reject files that would OOM or that are degenerate, with
/// an actionable message, before the render's whole-buffer copies explode.
const MAX_SAMPLE_RATE: u32 = 192_000;
const MAX_DURATION_SECS: f32 = 2.0 * 3600.0; // 2 hours — generous for one vocal.

/// Validate a decoded buffer's shape against the ingest caps (pure so it is
/// tested without a giant fixture).
fn check_ingest(sample_rate: u32, frames: usize) -> Result<(), SessionError> {
    if frames == 0 {
        return Err(SessionError::EmptyInput);
    }
    if sample_rate > MAX_SAMPLE_RATE {
        return Err(SessionError::SampleRateTooHigh(sample_rate));
    }
    let seconds = frames as f32 / sample_rate.max(1) as f32;
    if seconds > MAX_DURATION_SECS {
        return Err(SessionError::TooLong { seconds });
    }
    Ok(())
}

/// Best-effort available physical memory in bytes (#32). Windows uses
/// `GlobalMemoryStatusEx`; other platforms have no portable query, so
/// enforcement there relies on an explicit override. `None` disables the
/// preflight rather than guessing.
#[cfg(windows)]
fn available_memory_bytes() -> Option<u64> {
    #[allow(dead_code)] // fields exist for the exact FFI layout, not to be read
    #[repr(C)]
    struct MemoryStatusEx {
        length: u32,
        memory_load: u32,
        total_phys: u64,
        avail_phys: u64,
        total_page_file: u64,
        avail_page_file: u64,
        total_virtual: u64,
        avail_virtual: u64,
        avail_extended_virtual: u64,
    }
    extern "system" {
        fn GlobalMemoryStatusEx(buffer: *mut MemoryStatusEx) -> i32;
    }
    let mut status = MemoryStatusEx {
        length: std::mem::size_of::<MemoryStatusEx>() as u32,
        memory_load: 0,
        total_phys: 0,
        avail_phys: 0,
        total_page_file: 0,
        avail_page_file: 0,
        total_virtual: 0,
        avail_virtual: 0,
        avail_extended_virtual: 0,
    };
    // SAFETY: `GlobalMemoryStatusEx` fills the correctly-sized, correctly-aligned
    // struct we hand it and reports success via a nonzero return.
    if unsafe { GlobalMemoryStatusEx(&mut status) } != 0 {
        Some(status.avail_phys)
    } else {
        None
    }
}

#[cfg(not(windows))]
fn available_memory_bytes() -> Option<u64> {
    None
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

/// The deterministic Smart Assist recommendation for one Standard rack card.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SmartAssistCard {
    /// The card Smart Assist configures.
    pub card: CardId,
    /// Whether Smart Assist recommends the card should be active.
    pub enabled: bool,
    /// The recommended card amount, `[0, 1]`.
    pub amount: f32,
}

/// Smart Assist's complete analysis-to-Standard-view recommendation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SmartAssistPlan {
    /// Card recommendations in [`CardId::ALL`] display order.
    pub cards: [SmartAssistCard; CardId::ALL.len()],
    /// The Restoration Focus radial position, `[0, 1]`.
    pub restoration_focus: f32,
    /// The Polish Intensity radial position, `[0, 1]`.
    pub polish_intensity: f32,
}

/// A compact Standard-view verdict for one residual isolation dimension.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IsolationVerdict {
    /// Residual is low enough to show a pass state.
    Clean,
    /// Residual is noticeable; show a caution state.
    Warn,
    /// Residual is high enough to invite another pass or a different source.
    Attention,
}

impl IsolationVerdict {
    /// User-facing short label for the bottom-strip verdict.
    pub fn label(self) -> &'static str {
        match self {
            IsolationVerdict::Clean => "Clean",
            IsolationVerdict::Warn => "Check",
            IsolationVerdict::Attention => "Needs attention",
        }
    }

    fn rank(self) -> u8 {
        match self {
            IsolationVerdict::Clean => 0,
            IsolationVerdict::Warn => 1,
            IsolationVerdict::Attention => 2,
        }
    }

    fn max(self, other: Self) -> Self {
        if other.rank() > self.rank() {
            other
        } else {
            self
        }
    }
}

/// The Vocal Isolation Check result: the rendered output is re-analyzed and the
/// residual noise, room tail, and non-vocal bleed dimensions are turned into a
/// deterministic verdict for the Standard bottom strip.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VocalIsolationCheck {
    /// The analyzer report from the rendered output, not the source input.
    pub report: AnalysisReport,
    /// Residual noise-floor verdict.
    pub noise: IsolationVerdict,
    /// Residual de-reverb / room-tail verdict.
    pub reverb: IsolationVerdict,
    /// Residual bleed / mix-likeness verdict.
    pub bleed: BleedVerdict,
}

impl VocalIsolationCheck {
    /// The worst residual dimension, as a single status chip for the UI.
    pub fn overall(self) -> IsolationVerdict {
        let bleed = match self.bleed {
            BleedVerdict::Clean => IsolationVerdict::Clean,
            BleedVerdict::Warn => IsolationVerdict::Warn,
            BleedVerdict::Block => IsolationVerdict::Attention,
        };
        self.noise.max(self.reverb).max(bleed)
    }

    /// A concise Standard-view summary string.
    pub fn summary(self) -> String {
        format!(
            "{} - noise {:.0} dBFS, RT60 {:.2}s, bleed {}",
            self.overall().label(),
            self.report.noise_floor_db,
            self.report.reverb_time_s,
            self.bleed_label()
        )
    }

    /// The bleed verdict's UI label.
    pub fn bleed_label(self) -> &'static str {
        match self.bleed {
            BleedVerdict::Clean => "clean",
            BleedVerdict::Warn => "check",
            BleedVerdict::Block => "too high",
        }
    }
}

fn scale01(value: f32, low: f32, high: f32) -> f32 {
    ((value - low) / (high - low)).clamp(0.0, 1.0)
}

fn amount_from_score(score: f32, min: f32, max: f32) -> f32 {
    if score <= 0.0 {
        0.0
    } else {
        min + (max - min) * score.clamp(0.0, 1.0)
    }
}

fn residual_noise_verdict(noise_floor_db: f32) -> IsolationVerdict {
    if noise_floor_db <= -55.0 {
        IsolationVerdict::Clean
    } else if noise_floor_db <= -42.0 {
        IsolationVerdict::Warn
    } else {
        IsolationVerdict::Attention
    }
}

fn residual_reverb_verdict(reverb_time_s: f32, residual_nonvocal: f32) -> IsolationVerdict {
    if reverb_time_s <= 0.25 || residual_nonvocal <= 0.03 {
        IsolationVerdict::Clean
    } else if reverb_time_s <= 0.65 || residual_nonvocal <= 0.12 {
        IsolationVerdict::Warn
    } else {
        IsolationVerdict::Attention
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
#[derive(Clone, Copy)]
struct FinishCards {
    presence_air_enabled: Option<bool>,
    leveling_enabled: Option<bool>,
    loudness_enabled: Option<bool>,
    presence_air_amount: f32,
    leveling_amount: f32,
}

impl Default for FinishCards {
    fn default() -> Self {
        FinishCards {
            presence_air_enabled: None,
            leveling_enabled: None,
            loudness_enabled: None,
            presence_air_amount: 1.0,
            leveling_amount: 1.0,
        }
    }
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

    fn amount(&self, card: CardId) -> Option<f32> {
        match card {
            CardId::PresenceAir => Some(self.presence_air_amount),
            CardId::Leveling => Some(self.leveling_amount),
            _ => None,
        }
    }

    fn set_amount(&mut self, card: CardId, amount: f32) -> bool {
        let amount = amount.clamp(0.0, 1.0);
        match card {
            CardId::PresenceAir => self.presence_air_amount = amount,
            CardId::Leveling => self.leveling_amount = amount,
            _ => return false,
        }
        true
    }
}

/// Independent Standard-card state for the two transient paths that share
/// `Stage::DeClick` internally.
#[derive(Clone, Copy)]
struct TransientCards {
    click_enabled: Option<bool>,
    mouth_click_enabled: Option<bool>,
    click_amount: f32,
    mouth_click_amount: f32,
}

impl Default for TransientCards {
    fn default() -> Self {
        TransientCards {
            click_enabled: None,
            mouth_click_enabled: None,
            click_amount: 1.0,
            mouth_click_amount: 1.0,
        }
    }
}

impl TransientCards {
    fn set_enabled(&mut self, card: CardId, on: bool) -> bool {
        match card {
            CardId::DeClickDePop => self.click_enabled = Some(on),
            CardId::MouthClicks => self.mouth_click_enabled = Some(on),
            _ => return false,
        }
        true
    }

    fn amount(&self, card: CardId) -> Option<f32> {
        match card {
            CardId::DeClickDePop => Some(self.click_amount),
            CardId::MouthClicks => Some(self.mouth_click_amount),
            _ => None,
        }
    }

    fn set_amount(&mut self, card: CardId, amount: f32) -> bool {
        let amount = amount.clamp(0.0, 1.0);
        match card {
            CardId::DeClickDePop => self.click_amount = amount,
            CardId::MouthClicks => self.mouth_click_amount = amount,
            _ => return false,
        }
        true
    }
}

/// Advanced-only De-bleed controls. De-bleed has no Standard card because native
/// takes leave it off; Advanced exposes source mode and mix-refusal thresholds.
#[derive(Clone, Copy)]
struct DeBleedAdvanced {
    source_imported: bool,
    warn_threshold: f32,
    block_threshold: f32,
}

impl Default for DeBleedAdvanced {
    fn default() -> Self {
        DeBleedAdvanced {
            source_imported: false,
            warn_threshold: 0.02,
            block_threshold: 0.15,
        }
    }
}

impl DeBleedAdvanced {
    fn set_thresholds(&mut self, warn: f32, block: f32) {
        self.warn_threshold = warn.clamp(0.0, 1.0);
        self.block_threshold = block.clamp(0.0, 1.0).max(self.warn_threshold);
    }
}

/// A stateful vocal-editing session.
pub struct Session {
    settings: Settings,
    preset: Preset,
    input: Option<AudioBuffer>,
    report: Option<AnalysisReport>,
    output: Option<AudioBuffer>,
    compare_snapshot: Option<AudioBuffer>,
    cards: [Card; STAGE_COUNT],
    finish_cards: FinishCards,
    transient_cards: TransientCards,
    debleed: DeBleedAdvanced,
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
    /// Override for the available-memory query used by the render RAM preflight
    /// (#32); `None` uses the real system query.
    available_memory_override: Option<u64>,
    /// When true, render proceeds even if the preflight estimate is over budget
    /// (the explicit user override).
    allow_over_budget: bool,
    /// The neural→light fallback reason from the last render, if any (#33).
    engine_note: Option<String>,
}

/// An opaque snapshot of a [`Session`]'s **settings state** — the fields undo/redo
/// covers (#25): preset, cards + macros, content mode, loudness target, and
/// output options. Excludes the loaded audio, the render, and file
/// name/location. Produced by [`Session::snapshot`] and applied by
/// [`Session::restore_snapshot`].
#[derive(Clone)]
pub struct SessionSnapshot {
    settings: Settings,
    preset: Preset,
    cards: [Card; STAGE_COUNT],
    finish_cards: FinishCards,
    transient_cards: TransientCards,
    debleed: DeBleedAdvanced,
    flow: f32,
    polish: f32,
    loudness_override: Option<LoudnessTarget>,
    output_format: OutputFormat,
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
            compare_snapshot: None,
            cards: [Card::default(); STAGE_COUNT],
            finish_cards: FinishCards::default(),
            transient_cards: TransientCards::default(),
            debleed: DeBleedAdvanced::default(),
            flow: 1.0,
            polish: 0.5,
            loudness_override: None,
            output_format: OutputFormat::Wav,
            output_name: None,
            output_dir: None,
            workers,
            neural_model: std::env::var_os("YESVOX_MODEL_PATH").map(PathBuf::from),
            available_memory_override: None,
            allow_over_budget: false,
            engine_note: None,
        }
    }

    /// Override the available-memory figure the render RAM preflight uses (#32),
    /// mainly for tests/config; `None` restores the real system query.
    pub fn set_available_memory_override(&mut self, bytes: Option<u64>) {
        self.available_memory_override = bytes;
    }

    /// Allow a render whose estimated peak exceeds the memory budget (the
    /// explicit "I know, proceed anyway" override).
    pub fn set_allow_over_budget(&mut self, allow: bool) {
        self.allow_over_budget = allow;
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

    /// Whether the Advanced De-bleed source mode is "imported stem". Imported
    /// stems enable De-bleed by default; native takes leave it off.
    pub fn debleed_source_imported(&self) -> bool {
        self.debleed.source_imported
    }

    /// Set Advanced De-bleed source mode. This controls the default enabled
    /// state, while an explicit stage override still wins.
    pub fn set_debleed_source_imported(&mut self, imported: bool) {
        self.debleed.source_imported = imported;
        self.output = None;
    }

    /// Advanced De-bleed warn/block thresholds for the mix-likeness guard.
    pub fn debleed_thresholds(&self) -> (f32, f32) {
        (self.debleed.warn_threshold, self.debleed.block_threshold)
    }

    /// Set Advanced De-bleed warn/block thresholds. Values clamp to `[0, 1]`,
    /// and the block threshold is never below the warn threshold.
    pub fn set_debleed_thresholds(&mut self, warn: f32, block: f32) {
        self.debleed.set_thresholds(warn, block);
        self.output = None;
    }

    /// Set a visible Standard card's amount. Finish sub-controls that share
    /// `Stage::Finish` get independent amounts here instead of mutating the whole
    /// Finish stage mix.
    pub fn set_card_amount(&mut self, card: CardId, amount: f32) {
        if self.finish_cards.set_amount(card, amount) {
            self.output = None;
            return;
        }
        if self.transient_cards.set_amount(card, amount) {
            if card == CardId::DeClickDePop {
                self.set_stage_mix(Stage::DePop, amount);
            }
            self.output = None;
            return;
        }
        if card == CardId::Loudness {
            return;
        }
        for &stage in card.stages() {
            self.set_stage_mix(stage, amount);
        }
    }

    /// A visible Standard card's current amount.
    pub fn card_amount(&self, card: CardId) -> f32 {
        if let Some(amount) = self.finish_cards.amount(card) {
            return amount;
        }
        if let Some(amount) = self.transient_cards.amount(card) {
            return amount;
        }
        if card == CardId::Loudness {
            return 1.0;
        }
        self.stage_mix(card.stages()[0])
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
        if stage == Stage::DeBleed {
            return self.cards[stage as usize]
                .enabled
                .unwrap_or(self.debleed.source_imported);
        }
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
        if self.transient_cards.set_enabled(card, on) {
            if card == CardId::DeClickDePop {
                self.set_stage_enabled(Stage::DePop, on);
            } else {
                self.output = None;
            }
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
        if let Some(on) = self.resolve_transient_card_enabled(card) {
            return on;
        }
        card.stages().iter().all(|&s| self.resolve_stage_enabled(s))
    }

    fn resolve_finish_card_enabled(&self, card: CardId) -> Option<bool> {
        let sub = self.finish_cards.enabled_override(card)?;
        Some(self.resolve_stage_enabled(Stage::Finish) && sub)
    }

    fn resolve_transient_card_enabled(&self, card: CardId) -> Option<bool> {
        match card {
            CardId::DeClickDePop => {
                Some(self.click_path_enabled() && self.resolve_stage_enabled(Stage::DePop))
            }
            CardId::MouthClicks => Some(self.mouth_click_path_enabled()),
            _ => None,
        }
    }

    fn click_path_enabled(&self) -> bool {
        self.resolve_stage_enabled(Stage::DeClick)
            && self.transient_cards.click_enabled.unwrap_or(true)
    }

    fn mouth_click_path_enabled(&self) -> bool {
        self.resolve_stage_enabled(Stage::DeClick)
            && self.transient_cards.mouth_click_enabled.unwrap_or(true)
    }

    /// Enable or bypass the short Click/crackle path without touching Mouth Clicks.
    pub fn set_click_repair_enabled(&mut self, on: bool) {
        self.transient_cards.click_enabled = Some(on);
        self.output = None;
    }

    /// Whether short Click/crackle repair is effectively active.
    pub fn click_repair_enabled(&self) -> bool {
        self.click_path_enabled()
    }

    /// Enable or bypass Mouth Clicks without touching short Click/crackle repair.
    pub fn set_mouth_click_enabled(&mut self, on: bool) {
        self.transient_cards.mouth_click_enabled = Some(on);
        self.output = None;
    }

    /// Whether Mouth Click repair is effectively active.
    pub fn mouth_click_enabled(&self) -> bool {
        self.mouth_click_path_enabled()
    }

    /// Set the short Click/crackle amount.
    pub fn set_click_repair_amount(&mut self, amount: f32) {
        self.transient_cards.click_amount = amount.clamp(0.0, 1.0);
        self.output = None;
    }

    /// Current short Click/crackle amount.
    pub fn click_repair_amount(&self) -> f32 {
        self.transient_cards.click_amount
    }

    /// Set the Mouth Click amount.
    pub fn set_mouth_click_amount(&mut self, amount: f32) {
        self.transient_cards.mouth_click_amount = amount.clamp(0.0, 1.0);
        self.output = None;
    }

    /// Current Mouth Click amount.
    pub fn mouth_click_amount(&self) -> f32 {
        self.transient_cards.mouth_click_amount
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

    /// Set the Presence/Air amount, scaling the active preset's EQ patch without
    /// touching Leveling or Loudness.
    pub fn set_presence_air_amount(&mut self, amount: f32) {
        self.finish_cards.presence_air_amount = amount.clamp(0.0, 1.0);
        self.output = None;
    }

    /// The current Presence/Air amount.
    pub fn presence_air_amount(&self) -> f32 {
        self.finish_cards.presence_air_amount
    }

    /// Enable or bypass the Leveling card's program-level gain rider.
    pub fn set_leveling_enabled(&mut self, on: bool) {
        self.finish_cards.leveling_enabled = Some(on);
        self.output = None;
    }

    /// Whether the Leveling card is effectively active.
    pub fn leveling_enabled(&self) -> bool {
        self.resolve_finish_card_enabled(CardId::Leveling)
            .unwrap_or_else(|| self.resolve_stage_enabled(Stage::Finish))
    }

    /// Set the Leveling amount that feeds the program-level gain rider.
    pub fn set_leveling_amount(&mut self, amount: f32) {
        self.finish_cards.leveling_amount = amount.clamp(0.0, 1.0);
        self.output = None;
    }

    /// The current Leveling amount.
    pub fn leveling_amount(&self) -> f32 {
        self.finish_cards.leveling_amount
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
        self.transient_cards = TransientCards::default();
        self.loudness_override = None;
    }

    /// Open and analyze a vocal. The content toggle stays user-authoritative —
    /// the analyzer's guess is exposed via [`Session::detected_content`] but does
    /// not flip the mode. Clears any previous render.
    pub fn open(&mut self, path: &Path) -> Result<&AnalysisReport, SessionError> {
        let buffer = read_audio(path).map_err(SessionError::Io)?;
        // Reject degenerate / oversized input before analysis or the render's
        // whole-buffer copies (#32).
        check_ingest(buffer.sample_rate(), buffer.frames())?;
        let report = analyze(&buffer).map_err(SessionError::Analysis)?;
        self.input = Some(buffer);
        self.report = Some(report);
        self.output = None;
        self.compare_snapshot = None;
        // Default the export target from the opened file (FILE panel), as the mock
        // shows ("Lead Vocal" → "Lead Vocal_YesVox" beside the source's folder).
        if self.output_dir.is_none() {
            self.output_dir = path.parent().map(Path::to_path_buf);
        }
        if self.output_name.is_none() {
            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("vocal");
            self.output_name = Some(format!("{stem}_YesVox"));
        }
        // The report was just set above; `ok_or` keeps this panic-free without an
        // unwrap (#29).
        self.report.as_ref().ok_or(SessionError::NotLoaded)
    }

    /// The analyzer's voiceover-vs-singing guess (advisory; the toggle decides).
    pub fn detected_content(&self) -> Option<ContentGuess> {
        self.report.map(|r| r.content)
    }

    /// Build the deterministic Smart Assist recommendation for an analysis
    /// report under the current user-authoritative content mode. This is a pure
    /// mapping: no cloud, no model call, and no hidden mode flip.
    pub fn smart_assist_plan(report: &AnalysisReport, mode: ContentMode) -> SmartAssistPlan {
        let noise = scale01(report.noise_floor_db, -70.0, -35.0);
        let reverb_raw = scale01(report.reverb_time_s, 0.12, 1.10);
        let click = scale01(report.click_rate_hz, 0.20, 8.0);
        let mouth_click = scale01(report.click_rate_hz, 2.0, 14.0);
        let hum = if report.hum.frequency > 0.0 {
            scale01(report.hum.strength_db, -38.0, -8.0)
        } else {
            0.0
        };
        let bleed = scale01(report.residual_nonvocal, 0.04, 0.35);
        let clipped = scale01(report.clip_fraction, 0.0005, 0.05);
        let defect = noise
            .max(reverb_raw)
            .max(click)
            .max(hum)
            .max(bleed)
            .max(clipped);

        let singing = mode == ContentMode::Singing;
        let reverb = if singing {
            (reverb_raw * 0.55).min(0.45)
        } else {
            reverb_raw
        };
        let deess = if singing {
            0.28 + 0.22 * click.max(noise * 0.35)
        } else {
            0.35 + 0.30 * click.max(noise * 0.40)
        }
        .clamp(0.0, 0.75);
        let breath = if singing {
            0.0
        } else {
            0.25 + 0.35 * noise.max(click * 0.5)
        }
        .clamp(0.0, 0.75);
        let presence = if singing {
            0.30 + 0.20 * (1.0 - noise).max(0.0)
        } else {
            0.35 + 0.25 * (1.0 - noise).max(0.0)
        }
        .clamp(0.0, 0.65);
        let leveling = scale01((-18.0 - report.integrated_lufs).abs(), 1.0, 12.0);

        let restoration_focus = if defect < 0.22 {
            1.0 / 6.0
        } else if defect < 0.68 {
            0.5
        } else {
            5.0 / 6.0
        };
        let polish_intensity = if singing {
            0.38 + 0.17 * defect.min(1.0)
        } else {
            0.45 + 0.25 * defect.min(1.0)
        }
        .clamp(0.0, 1.0);

        SmartAssistPlan {
            cards: [
                SmartAssistCard {
                    card: CardId::Denoise,
                    enabled: noise > 0.08,
                    amount: amount_from_score(noise, 0.20, 0.90),
                },
                SmartAssistCard {
                    card: CardId::DeReverb,
                    enabled: if singing {
                        report.reverb_time_s >= 0.55
                    } else {
                        reverb_raw > 0.08
                    },
                    amount: amount_from_score(reverb, 0.18, if singing { 0.45 } else { 0.85 }),
                },
                SmartAssistCard {
                    card: CardId::DeClickDePop,
                    enabled: click > 0.0 || clipped > 0.0,
                    amount: amount_from_score(click.max(clipped * 0.60), 0.25, 0.90),
                },
                SmartAssistCard {
                    card: CardId::MouthClicks,
                    enabled: mouth_click > 0.0,
                    amount: amount_from_score(mouth_click, 0.20, 0.75),
                },
                SmartAssistCard {
                    card: CardId::DeHum,
                    enabled: hum > 0.0,
                    amount: amount_from_score(hum, 0.25, 0.85),
                },
                SmartAssistCard {
                    card: CardId::DeEss,
                    enabled: true,
                    amount: deess,
                },
                SmartAssistCard {
                    card: CardId::Breath,
                    enabled: !singing,
                    amount: breath,
                },
                SmartAssistCard {
                    card: CardId::PresenceAir,
                    enabled: true,
                    amount: presence,
                },
                SmartAssistCard {
                    card: CardId::Leveling,
                    enabled: true,
                    amount: amount_from_score(leveling, 0.30, 0.80),
                },
                SmartAssistCard {
                    card: CardId::Loudness,
                    enabled: true,
                    amount: 1.0,
                },
            ],
            restoration_focus,
            polish_intensity,
        }
    }

    /// Analyze → configure the Standard-view cards from the current report. The
    /// Voiceover/Singing toggle remains authoritative; Smart Assist uses it for
    /// conservative singing settings but never changes it behind the user's back.
    pub fn apply_smart_assist(&mut self) -> Result<SmartAssistPlan, SessionError> {
        let report = *self.report.as_ref().ok_or(SessionError::NotLoaded)?;
        let plan = Self::smart_assist_plan(&report, self.settings.content_mode);
        self.set_restoration_focus(plan.restoration_focus);
        self.set_polish_intensity(plan.polish_intensity);
        for rec in plan.cards {
            self.set_card_enabled(rec.card, rec.enabled);
            if rec.card != CardId::Loudness {
                self.set_card_amount(rec.card, rec.amount);
            }
        }
        self.output = None;
        Ok(plan)
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

    /// Whether the active Standard intensity calls for generative reconstruction
    /// and should be labelled with the plan's `GEN` badge.
    pub fn generative_reconstruction_badge(&self) -> bool {
        self.settings.quality_mode.is_generative()
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

    /// Pin the current processed output as Advanced compare slot C. A later render
    /// becomes B while C stays fixed until replaced or a new file is opened.
    pub fn pin_compare_snapshot(&mut self) -> bool {
        let Some(output) = self.output.as_ref() else {
            return false;
        };
        self.compare_snapshot = Some(output.clone());
        true
    }

    /// Clear Advanced compare slot C.
    pub fn clear_compare_snapshot(&mut self) {
        self.compare_snapshot = None;
    }

    /// The pinned Advanced compare slot C, if one exists.
    pub fn compare_snapshot(&self) -> Option<&AudioBuffer> {
        self.compare_snapshot.as_ref()
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

    /// The pinned "C" snapshot scaled to the current processed "B"'s loudness.
    /// `None` until both a snapshot and the current render exist.
    pub fn ab_matched_compare_snapshot(&self) -> Option<AudioBuffer> {
        let snapshot = self.compare_snapshot()?;
        let gain =
            crate::monitor::loudness_match_gain(integrated_lufs(snapshot)?, self.output_lufs()?);
        Some(crate::monitor::apply_gain(snapshot, gain))
    }

    /// The buffer to audition for a given [`PlaybackSource`] (#23) — the single
    /// mapping the transport/GUI use so A/B, reblend and difference all pull from
    /// the same monitor buffers. `None` until the relevant buffer exists.
    pub fn playback_buffer(
        &self,
        source: PlaybackSource,
        volume_match: bool,
        reblend_amount: f32,
    ) -> Option<AudioBuffer> {
        match source {
            PlaybackSource::Original if volume_match => self.ab_matched_input(),
            PlaybackSource::Original => self.input().cloned(),
            PlaybackSource::Processed => self.output().cloned(),
            PlaybackSource::CompareSnapshot if volume_match => self.ab_matched_compare_snapshot(),
            PlaybackSource::CompareSnapshot => self.compare_snapshot().cloned(),
            PlaybackSource::Reblend => self.reblend(reblend_amount),
            PlaybackSource::Difference => self.difference(),
        }
    }

    /// Capture the current settings state for the undo/redo stack (#25). Cheap:
    /// clones only the settings-affecting fields, not the audio.
    pub fn snapshot(&self) -> SessionSnapshot {
        SessionSnapshot {
            settings: self.settings.clone(),
            preset: self.preset,
            cards: self.cards,
            finish_cards: self.finish_cards,
            transient_cards: self.transient_cards,
            debleed: self.debleed,
            flow: self.flow,
            polish: self.polish,
            loudness_override: self.loudness_override,
            output_format: self.output_format,
        }
    }

    /// Restore a settings [`SessionSnapshot`] (undo/redo). Reverts every settings
    /// field and marks the render stale (`output = None`) — exactly what a direct
    /// settings edit does — while leaving the loaded audio and file target alone.
    pub fn restore_snapshot(&mut self, snapshot: &SessionSnapshot) {
        self.settings = snapshot.settings.clone();
        self.preset = snapshot.preset;
        self.cards = snapshot.cards;
        self.finish_cards = snapshot.finish_cards;
        self.transient_cards = snapshot.transient_cards;
        self.debleed = snapshot.debleed;
        self.flow = snapshot.flow;
        self.polish = snapshot.polish;
        self.loudness_override = snapshot.loudness_override;
        self.output_format = snapshot.output_format;
        self.output = None;
    }

    /// Re-analyze the rendered output and return the bottom-strip Vocal Isolation
    /// Check verdict. This intentionally depends on the current processed buffer:
    /// changing cards/macros invalidates the output, and the next check reflects
    /// the newly rendered audio rather than stale source analysis.
    pub fn vocal_isolation_check(&self) -> Result<VocalIsolationCheck, SessionError> {
        let output = self.output.as_ref().ok_or(SessionError::NotRendered)?;
        let report = analyze(output).map_err(SessionError::Analysis)?;
        let bleed = crate::modules::DeBleed::default().assess(output);
        Ok(VocalIsolationCheck {
            report,
            noise: residual_noise_verdict(report.noise_floor_db),
            reverb: residual_reverb_verdict(report.reverb_time_s, report.residual_nonvocal),
            bleed,
        })
    }

    /// Run the bound chain on the open vocal, caching the processed result.
    /// Refuses a mix (de-bleed `Block`) with an actionable error.
    pub fn render(&mut self) -> Result<(), SessionError> {
        self.render_with_cancel(|| false)
    }

    /// Run the bound chain, polling `should_cancel` between DSP stages.
    /// A canceled render leaves the prior output state unchanged.
    pub fn render_with_cancel<F>(&mut self, mut should_cancel: F) -> Result<(), SessionError>
    where
        F: FnMut() -> bool,
    {
        let input = self.input.as_ref().ok_or(SessionError::NotLoaded)?;
        let report = self.report.as_ref().ok_or(SessionError::NotLoaded)?;

        // Enforcing RAM preflight (#32): the whole-buffer render makes several
        // full-length copies, so estimate peak memory and refuse before it OOMs
        // the process — unless the user opted out. `None` available memory (no
        // portable query, no override) skips enforcement.
        if let Some(available) = self
            .available_memory_override
            .or_else(available_memory_bytes)
        {
            let preflight = crate::denoise::ChunkPolicy::default().preflight(
                input.frames(),
                input.channel_count(),
                input.sample_rate() as f32,
                available,
            );
            if preflight.exceeds && !self.allow_over_budget {
                return Err(SessionError::MemoryBudgetExceeded {
                    estimated_mb: preflight.estimated_bytes / 1_000_000,
                    budget_mb: preflight.budget_bytes / 1_000_000,
                });
            }
        }

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
        // Carry the neural→light fallback reason (if any) for the GUI/CLI (#33).
        let engine_note = built.engine_note.take();

        built.chain.debleed = crate::modules::DeBleed::for_source(self.debleed_source_imported());
        let (debleed_warn, debleed_block) = self.debleed_thresholds();
        built
            .chain
            .debleed
            .set_thresholds(debleed_warn, debleed_block);

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
        built
            .chain
            .finish
            .set_presence_air_amount(self.presence_air_amount());
        built
            .chain
            .finish
            .set_leveling_amount(self.leveling_amount());
        built.chain.set_click_repair(
            self.click_repair_enabled(),
            self.click_repair_amount() * self.flow,
        );
        built.chain.set_mouth_click_repair(
            self.mouth_click_enabled(),
            self.mouth_click_amount() * self.flow,
        );

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
        built
            .chain
            .process_with_cancel(&mut buffer, &mut should_cancel)
            .map_err(|_| SessionError::Canceled)?;
        self.output = Some(buffer);
        self.engine_note = engine_note;
        Ok(())
    }

    /// A user-presentable note when the last render fell back from the neural
    /// engine to the light engine (#33), else `None`.
    pub fn engine_note(&self) -> Option<&str> {
        self.engine_note.as_deref()
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

    /// Build a detached, `Send` [`PreparedExport`] for the last render + the
    /// composed target, so the disk write can run off the UI thread (#22).
    /// Snapshots the output buffer + options now; equivalent to
    /// [`Session::export_to_target`] but the returned unit borrows nothing.
    /// Errors if nothing is loaded (no target) or rendered (no output).
    pub fn prepare_export(&self) -> Result<PreparedExport, SessionError> {
        let target = self.export_target().ok_or(SessionError::NotLoaded)?;
        let buffer = self
            .output
            .as_ref()
            .ok_or(SessionError::NotRendered)?
            .clone();
        let opts = self
            .settings
            .export
            .to_options(buffer.bits_per_sample(), buffer.encoding());
        Ok(PreparedExport {
            target,
            buffer,
            opts,
        })
    }
}

/// A self-contained, `Send` export unit: everything needed to write the last
/// render to disk, detached from the [`Session`] so it can run on a worker
/// thread (#22). Produced by [`Session::prepare_export`].
pub struct PreparedExport {
    target: PathBuf,
    buffer: AudioBuffer,
    opts: ExportOptions,
}

impl PreparedExport {
    /// The destination path (before the writer's never-overwrite suffixing).
    pub fn target(&self) -> &Path {
        &self.target
    }

    /// Write the snapshotted buffer to disk, returning the path actually written
    /// (auto-suffixed on collision). Same never-overwrite semantics as
    /// [`Session::export`].
    pub fn write(self) -> Result<PathBuf, IoError> {
        export(&self.target, &self.buffer, self.opts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::{ContainerMetadata, SampleEncoding, WavMetadataChunk};
    use crate::io::{read_wav, write_wav, write_wav_with};
    use crate::settings::{ExportOptions, DEFAULT_FLAC_COMPRESSION_LEVEL};
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

    fn transient_fixture(path: &Path) -> AudioBuffer {
        let n = 48_000;
        let mut x: Vec<f32> = (0..n)
            .map(|i| 0.25 * (2.0 * PI * 200.0 * i as f32 / SR as f32).sin())
            .collect();
        x[6000] += 0.9;
        let start = 18_000;
        for i in 0..120 {
            let t = i as f32 / SR as f32;
            x[start + i] += 0.75 * (2.0 * PI * 4800.0 * t).sin();
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

    fn stepped_level_fixture(path: &Path) -> AudioBuffer {
        let n = SR as usize * 4;
        let mut x = Vec::with_capacity(n);
        for i in 0..n {
            let amp = if i < n / 4 {
                0.04
            } else if i < n / 2 {
                0.22
            } else if i < 3 * n / 4 {
                0.07
            } else {
                0.35
            };
            x.push(amp * (2.0 * PI * 220.0 * i as f32 / SR as f32).sin());
        }
        let buf = AudioBuffer::new(SR, 24, SampleEncoding::Int, vec![x]);
        write_wav(path, &buf).expect("write fixture");
        buf
    }

    fn block_rms_spread_db(samples: &[f32], block: usize) -> f32 {
        let mut levels: Vec<f32> = samples
            .chunks(block)
            .filter(|chunk| !chunk.is_empty())
            .map(|chunk| {
                let rms = (chunk.iter().map(|s| s * s).sum::<f32>() / chunk.len() as f32).sqrt();
                20.0 * rms.max(1.0e-6).log10()
            })
            .collect();
        levels.sort_by(|a, b| a.partial_cmp(b).unwrap());
        levels[levels.len() - 1] - levels[0]
    }

    fn temp(tag: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("yesvox-session-{tag}-{}.wav", std::process::id()));
        let _ = std::fs::remove_file(&p);
        p
    }

    fn report(
        noise_floor_db: f32,
        reverb_time_s: f32,
        click_rate_hz: f32,
        hum_strength_db: f32,
        residual_nonvocal: f32,
        mode: ContentMode,
        confidence: f32,
    ) -> AnalysisReport {
        AnalysisReport {
            channel_mode: crate::audio::ChannelLayout::Mono,
            integrated_lufs: -20.0,
            true_peak_dbtp: -3.0,
            noise_floor_db,
            clip_sample_count: 0,
            clip_fraction: 0.0,
            click_rate_hz,
            hum: crate::analysis::HumEstimate {
                frequency: if hum_strength_db > -120.0 { 60.0 } else { 0.0 },
                harmonic_count: if hum_strength_db > -120.0 { 2 } else { 0 },
                strength_db: hum_strength_db,
            },
            reverb_time_s,
            content: ContentGuess { mode, confidence },
            residual_nonvocal,
            resampled_for_batch: false,
            non_finite_samples: 0,
        }
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
    fn smart_assist_maps_more_defects_to_more_restoration() {
        let clean = report(-78.0, 0.03, 0.0, -120.0, 0.01, ContentMode::Voiceover, 0.9);
        let damaged = report(-38.0, 0.90, 9.0, -10.0, 0.30, ContentMode::Voiceover, 0.9);

        let clean_plan = Session::smart_assist_plan(&clean, ContentMode::Voiceover);
        let damaged_plan = Session::smart_assist_plan(&damaged, ContentMode::Voiceover);

        let amount = |plan: &SmartAssistPlan, card| {
            plan.cards
                .iter()
                .find(|rec| rec.card == card)
                .expect("card")
                .amount
        };
        let enabled = |plan: &SmartAssistPlan, card| {
            plan.cards
                .iter()
                .find(|rec| rec.card == card)
                .expect("card")
                .enabled
        };

        assert!(!enabled(&clean_plan, CardId::Denoise));
        assert!(enabled(&damaged_plan, CardId::Denoise));
        assert!(amount(&damaged_plan, CardId::Denoise) > amount(&clean_plan, CardId::Denoise));
        assert!(amount(&damaged_plan, CardId::DeReverb) > amount(&clean_plan, CardId::DeReverb));
        assert!(
            amount(&damaged_plan, CardId::DeClickDePop) > amount(&clean_plan, CardId::DeClickDePop)
        );
        assert!(amount(&damaged_plan, CardId::DeHum) > amount(&clean_plan, CardId::DeHum));
        assert_eq!(
            damaged_plan.restoration_focus,
            5.0 / 6.0,
            "severe defects choose the Restoration HQ focus band"
        );
    }

    #[test]
    fn apply_smart_assist_configures_cards_but_not_the_content_toggle() {
        let mut s = Session::new();
        s.report = Some(report(
            -40.0,
            0.80,
            10.0,
            -12.0,
            0.20,
            ContentMode::Singing,
            0.98,
        ));
        assert_eq!(s.settings().content_mode, ContentMode::Voiceover);

        let plan = s.apply_smart_assist().expect("smart assist");

        assert_eq!(
            s.settings().content_mode,
            ContentMode::Voiceover,
            "the analyzer guess is advisory; the explicit toggle stays authoritative"
        );
        for rec in plan.cards {
            assert_eq!(
                s.card_enabled(rec.card),
                rec.enabled,
                "{:?} enabled",
                rec.card
            );
            if rec.card != CardId::Loudness {
                assert_eq!(s.card_amount(rec.card), rec.amount, "{:?} amount", rec.card);
            }
        }
        assert_eq!(s.restoration_focus(), plan.restoration_focus);
        assert_eq!(s.polish_intensity(), plan.polish_intensity);
    }

    #[test]
    fn smart_assist_keeps_singing_dereverb_and_breath_conservative() {
        let roomy = report(-42.0, 0.95, 4.0, -120.0, 0.05, ContentMode::Singing, 0.9);

        let voiceover = Session::smart_assist_plan(&roomy, ContentMode::Voiceover);
        let singing = Session::smart_assist_plan(&roomy, ContentMode::Singing);

        let amount = |plan: &SmartAssistPlan, card| {
            plan.cards
                .iter()
                .find(|rec| rec.card == card)
                .expect("card")
                .amount
        };
        let enabled = |plan: &SmartAssistPlan, card| {
            plan.cards
                .iter()
                .find(|rec| rec.card == card)
                .expect("card")
                .enabled
        };

        assert!(
            amount(&singing, CardId::DeReverb) < amount(&voiceover, CardId::DeReverb),
            "singing de-reverb must stay gentler to preserve tails"
        );
        assert!(enabled(&voiceover, CardId::Breath));
        assert!(!enabled(&singing, CardId::Breath));
        assert_eq!(amount(&singing, CardId::Breath), 0.0);
    }

    #[test]
    fn smart_assist_requires_an_analysis_report() {
        let mut s = Session::new();
        assert!(matches!(
            s.apply_smart_assist(),
            Err(SessionError::NotLoaded)
        ));
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
    fn render_with_cancel_returns_canceled_without_caching_output() {
        let path = temp("render-cancel");
        quiet_tone_fixture(&path);
        let mut s = Session::new();
        s.open(&path).expect("open");

        let err = s
            .render_with_cancel(|| true)
            .expect_err("render should cancel");

        assert!(matches!(err, SessionError::Canceled));
        assert!(
            s.output().is_none(),
            "a canceled render must not publish output"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn snapshot_restore_reverts_settings_and_marks_output_stale() {
        // #25: restoring a settings snapshot reverts the value AND invalidates
        // the render, exactly like the direct edit did.
        let in_path = temp("snap-in");
        degraded_fixture(&in_path);
        let mut s = Session::new();
        s.open(&in_path).expect("open");
        s.render().expect("render");

        let original = s.card_amount(CardId::PresenceAir);
        let snap = s.snapshot();

        // A direct settings edit marks the render stale...
        let target = if original >= 0.5 { 0.2 } else { 0.8 };
        s.set_card_amount(CardId::PresenceAir, target);
        assert!(s.output().is_none(), "direct edit marks output stale");
        s.render().expect("re-render");
        assert!(s.output().is_some());
        assert_ne!(s.card_amount(CardId::PresenceAir), original);

        // ...and so does restoring the snapshot (undo).
        s.restore_snapshot(&snap);
        assert_eq!(
            s.card_amount(CardId::PresenceAir),
            original,
            "snapshot restore reverts the setting"
        );
        assert!(
            s.output().is_none(),
            "restore marks the render stale like a direct edit"
        );

        let _ = std::fs::remove_file(&in_path);
    }

    #[test]
    fn snapshot_restore_roundtrips_preset_and_loudness() {
        let mut s = Session::new();
        s.select_preset(Preset::NARRATION);
        s.set_loudness_target(LoudnessTarget::Podcast);
        let snap = s.snapshot();
        s.select_preset(Preset::LEAD_VOCAL);
        s.set_loudness_target(LoudnessTarget::Custom(-9.0));
        s.restore_snapshot(&snap);
        assert_eq!(s.preset(), Preset::NARRATION);
        assert_eq!(s.loudness_target(), LoudnessTarget::Podcast);
    }

    #[test]
    fn playback_buffer_maps_each_source_to_its_monitor_buffer() {
        // #23 source selection: every A/B source resolves to the right monitor
        // buffer, all same length, so a shared playhead is valid.
        let in_path = temp("playback-in");
        degraded_fixture(&in_path);
        let mut s = Session::new();
        s.open(&in_path).expect("open");
        s.render().expect("render");

        let frames = s.input().unwrap().frames();

        let original = s
            .playback_buffer(PlaybackSource::Original, false, 0.0)
            .unwrap();
        assert_eq!(original.channel(0), s.input().unwrap().channel(0));

        let matched = s
            .playback_buffer(PlaybackSource::Original, true, 0.0)
            .unwrap();
        assert_eq!(matched.channel(0), s.ab_matched_input().unwrap().channel(0));

        let processed = s
            .playback_buffer(PlaybackSource::Processed, false, 0.0)
            .unwrap();
        assert_eq!(processed.channel(0), s.output().unwrap().channel(0));

        let reblend = s
            .playback_buffer(PlaybackSource::Reblend, false, 0.4)
            .unwrap();
        assert_eq!(reblend.channel(0), s.reblend(0.4).unwrap().channel(0));

        let difference = s
            .playback_buffer(PlaybackSource::Difference, false, 0.0)
            .unwrap();
        assert_eq!(difference.channel(0), s.difference().unwrap().channel(0));

        // All sources share one length → one playhead is valid.
        for b in [&original, &matched, &processed, &reblend, &difference] {
            assert_eq!(b.frames(), frames, "source length diverged");
        }

        let _ = std::fs::remove_file(&in_path);
    }

    #[test]
    fn pinned_compare_snapshot_is_independent_playback_source_c() {
        let in_path = temp("compare-c-in");
        degraded_fixture(&in_path);
        let mut s = Session::new();
        s.open(&in_path).expect("open");
        s.render().expect("first render");

        let first_render = s.output().expect("first output").channel(0).to_vec();
        assert!(
            s.pin_compare_snapshot(),
            "rendered output can be pinned as C"
        );

        s.set_flow_amount(0.0);
        s.render().expect("second render");
        let second_render = s.output().expect("second output").channel(0).to_vec();
        assert!(
            first_render
                .iter()
                .zip(&second_render)
                .any(|(a, b)| (a - b).abs() > 1e-4),
            "second render should differ from the pinned C snapshot"
        );

        let processed = s
            .playback_buffer(PlaybackSource::Processed, false, 0.0)
            .expect("processed playback");
        let snapshot = s
            .playback_buffer(PlaybackSource::CompareSnapshot, false, 0.0)
            .expect("snapshot playback");
        assert_eq!(processed.channel(0), &second_render);
        assert_eq!(snapshot.channel(0), &first_render);

        let _ = std::fs::remove_file(&in_path);
    }

    #[test]
    fn volume_match_brings_compare_snapshot_to_current_processed_loudness() {
        let in_path = temp("compare-c-vmatch");
        degraded_fixture(&in_path);
        let mut s = Session::new();
        s.open(&in_path).expect("open");
        s.set_loudness_target(LoudnessTarget::Custom(-28.0));
        s.render().expect("first render");
        assert!(s.pin_compare_snapshot());

        let raw_snapshot = s
            .playback_buffer(PlaybackSource::CompareSnapshot, false, 0.0)
            .expect("raw snapshot");
        let raw_lufs = integrated_lufs(&raw_snapshot).expect("raw lufs");

        s.set_loudness_target(LoudnessTarget::Custom(-14.0));
        s.render().expect("second render");
        let current_lufs = s.output_lufs().expect("current lufs");
        assert!(
            (raw_lufs - current_lufs).abs() > 6.0,
            "test setup needs a meaningful C-vs-B loudness offset: {raw_lufs} vs {current_lufs}"
        );

        let matched_snapshot = s
            .playback_buffer(PlaybackSource::CompareSnapshot, true, 0.0)
            .expect("matched snapshot");
        let matched_lufs = integrated_lufs(&matched_snapshot).expect("matched lufs");
        assert!(
            (matched_lufs - current_lufs).abs() <= 0.6,
            "volume-matched C must sit at current B loudness: {matched_lufs} vs {current_lufs}"
        );

        let _ = std::fs::remove_file(&in_path);
    }

    #[test]
    fn playback_buffer_is_none_before_render() {
        let s = Session::new();
        assert!(s
            .playback_buffer(PlaybackSource::Processed, false, 0.0)
            .is_none());
        assert!(s
            .playback_buffer(PlaybackSource::Difference, false, 0.0)
            .is_none());
    }

    #[test]
    fn ingest_caps_reject_empty_high_rate_and_too_long() {
        // Pure caps — tested without a giant fixture (#32).
        assert!(matches!(
            check_ingest(48_000, 0),
            Err(SessionError::EmptyInput)
        ));
        assert!(matches!(
            check_ingest(200_000, 48_000),
            Err(SessionError::SampleRateTooHigh(200_000))
        ));
        // 3 hours at 48 kHz exceeds the 2 h cap.
        let long_frames = (3.0 * 3600.0 * 48_000.0) as usize;
        assert!(matches!(
            check_ingest(48_000, long_frames),
            Err(SessionError::TooLong { .. })
        ));
        // A normal 1 s vocal is fine, and every message is non-empty.
        assert!(check_ingest(48_000, 48_000).is_ok());
        for err in [
            SessionError::EmptyInput,
            SessionError::SampleRateTooHigh(200_000),
            SessionError::TooLong { seconds: 9000.0 },
            SessionError::MemoryBudgetExceeded {
                estimated_mb: 900,
                budget_mb: 100,
            },
        ] {
            assert!(!format!("{err}").is_empty());
        }
    }

    #[test]
    fn render_surfaces_the_neural_light_fallback_note() {
        // In the default build (no `neural` feature) a Balanced/HQ render requests
        // the neural engine and falls back to light — the reason must surface on
        // the session for the GUI/CLI (#33).
        let path = temp("engine-note");
        degraded_fixture(&path);
        let mut s = Session::new();
        s.set_quality_mode(QualityMode::Balanced);
        s.open(&path).expect("open");
        assert!(s.engine_note().is_none(), "no note before a render");
        s.render().expect("render");
        let note = s.engine_note().expect("a fallback note after render");
        assert!(!note.is_empty(), "the note is user-presentable");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn open_rejects_a_too_high_sample_rate_file() {
        let path = temp("hi-rate");
        // A tiny file, but with a sample rate above the cap.
        let buf = AudioBuffer::new(200_000, 16, SampleEncoding::Int, vec![vec![0.1; 128]]);
        write_wav(&path, &buf).expect("write hi-rate wav");
        let mut s = Session::new();
        assert!(matches!(
            s.open(&path),
            Err(SessionError::SampleRateTooHigh(200_000))
        ));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn render_refuses_over_budget_unless_overridden() {
        let path = temp("budget");
        degraded_fixture(&path);
        let mut s = Session::new();
        s.open(&path).expect("open");

        // A tiny available-memory figure makes the preflight exceed the budget.
        s.set_available_memory_override(Some(1_000));
        assert!(
            matches!(s.render(), Err(SessionError::MemoryBudgetExceeded { .. })),
            "an over-budget render must refuse"
        );

        // The explicit override lets it through.
        s.set_allow_over_budget(true);
        assert!(s.render().is_ok(), "over-budget override renders");

        // With enforcement off (no override figure), a normal render is fine.
        s.set_allow_over_budget(false);
        s.set_available_memory_override(None);
        assert!(s.render().is_ok(), "unenforced render is fine");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn prepare_export_matches_export_to_target_byte_for_byte() {
        // The off-thread export path (#22): a detached PreparedExport must write
        // exactly what the synchronous export_to_target would.
        let in_path = temp("prep-in");
        degraded_fixture(&in_path);
        let dir = std::env::temp_dir();

        let mut s = Session::new();
        s.open(&in_path).expect("open");
        s.render().expect("render");
        s.set_output_location(&dir);
        s.set_output_name(&format!("yesvox-prep-{}", std::process::id()));

        let sync_path = s.export_to_target().expect("export_to_target");
        // Same composed target → the never-overwrite writer auto-suffixes.
        let prepared = s.prepare_export().expect("prepare_export");
        assert_eq!(
            prepared.target(),
            sync_path.as_path(),
            "planned target should match"
        );
        let prep_path = prepared.write().expect("prepared write");

        assert_ne!(sync_path, prep_path, "second write must be suffixed");
        assert_eq!(
            std::fs::read(&sync_path).unwrap(),
            std::fs::read(&prep_path).unwrap(),
            "PreparedExport bytes differ from export_to_target"
        );

        for p in [in_path, sync_path, prep_path] {
            let _ = std::fs::remove_file(p);
        }
    }

    #[test]
    fn render_then_export_preserves_source_wav_metadata() {
        let in_path = temp("metadata-render-in");
        let out_path = temp("metadata-render-out");
        let mut input = quiet_tone_fixture(&in_path);
        let metadata = ContainerMetadata::with_wav_chunks(vec![
            WavMetadataChunk::new(*b"bext", b"session broadcast metadata".to_vec()),
            WavMetadataChunk::new(
                *b"iXML",
                b"<BWFXML><PROJECT>Session</PROJECT></BWFXML>".to_vec(),
            ),
            WavMetadataChunk::new(*b"LIST", b"INFOICMT\x0c\x00\x00\x00session path".to_vec()),
        ]);
        input.set_metadata(metadata.clone());
        write_wav_with(
            &in_path,
            &input,
            ExportOptions {
                bit_depth: 24,
                encoding: SampleEncoding::Int,
                dither: false,
                flac_compression_level: DEFAULT_FLAC_COMPRESSION_LEVEL,
            },
        )
        .expect("write metadata fixture");

        let mut s = Session::new();
        s.open(&in_path).expect("open");
        s.render().expect("render");
        let written = s.export(&out_path).expect("export");

        let back = read_wav(&written).expect("read exported metadata");
        assert_eq!(
            back.metadata().wav_chunks(),
            metadata.wav_chunks(),
            "session open -> render -> export must preserve source WAV metadata"
        );

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

    fn noise(amp: f32, n: usize, seed0: u32) -> Vec<f32> {
        let mut s = seed0;
        (0..n)
            .map(|_| {
                s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (((s >> 9) as f32 / (1u32 << 23) as f32) - 0.5) * 2.0 * amp
            })
            .collect()
    }

    fn direct_output(samples: Vec<f32>) -> Session {
        let mut s = Session::new();
        s.output = Some(AudioBuffer::new(SR, 24, SampleEncoding::Int, vec![samples]));
        s
    }

    #[test]
    fn vocal_isolation_check_requires_a_render() {
        let mut s = Session::new();
        assert!(matches!(
            s.vocal_isolation_check(),
            Err(SessionError::NotRendered)
        ));

        let path = temp("g8-not-rendered");
        quiet_tone_fixture(&path);
        s.open(&path).unwrap();
        assert!(matches!(
            s.vocal_isolation_check(),
            Err(SessionError::NotRendered)
        ));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn vocal_isolation_check_reanalyzes_the_rendered_output() {
        let mut samples = vec![0.0; SR as usize * 2];
        for start in [SR as usize / 4, SR as usize + SR as usize / 4] {
            for i in 0..SR as usize / 3 {
                let t = i as f32 / SR as f32;
                samples[start + i] = 0.25 * (2.0 * PI * 220.0 * t).sin();
            }
        }
        let s = direct_output(samples);

        let check = s.vocal_isolation_check().expect("check");

        assert_eq!(check.noise, IsolationVerdict::Clean);
        assert_eq!(check.reverb, IsolationVerdict::Clean);
        assert_eq!(check.bleed, BleedVerdict::Clean);
        assert_eq!(check.overall(), IsolationVerdict::Clean);
        assert_eq!(check.report.channel_mode, crate::audio::ChannelLayout::Mono);
    }

    #[test]
    fn vocal_isolation_check_flags_residual_noise_and_bleed() {
        let samples = noise(0.35, SR as usize * 2, 12);
        let s = direct_output(samples);

        let check = s.vocal_isolation_check().expect("check");

        assert_eq!(check.noise, IsolationVerdict::Attention);
        assert_eq!(check.bleed, BleedVerdict::Block);
        assert_eq!(check.overall(), IsolationVerdict::Attention);
        assert!(check.summary().contains("Needs attention"));
    }

    #[test]
    fn vocal_isolation_check_has_a_middle_warning_band() {
        let report = AnalysisReport {
            channel_mode: crate::audio::ChannelLayout::Mono,
            integrated_lufs: -20.0,
            true_peak_dbtp: -3.0,
            noise_floor_db: -48.0,
            clip_sample_count: 0,
            clip_fraction: 0.0,
            click_rate_hz: 0.0,
            hum: crate::analysis::HumEstimate {
                frequency: 0.0,
                harmonic_count: 0,
                strength_db: -120.0,
            },
            reverb_time_s: 0.40,
            content: ContentGuess {
                mode: ContentMode::Voiceover,
                confidence: 0.8,
            },
            residual_nonvocal: 0.08,
            resampled_for_batch: false,
            non_finite_samples: 0,
        };
        let check = VocalIsolationCheck {
            report,
            noise: residual_noise_verdict(report.noise_floor_db),
            reverb: residual_reverb_verdict(report.reverb_time_s, report.residual_nonvocal),
            bleed: BleedVerdict::Clean,
        };

        assert_eq!(check.noise, IsolationVerdict::Warn);
        assert_eq!(check.reverb, IsolationVerdict::Warn);
        assert_eq!(check.overall(), IsolationVerdict::Warn);
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
    fn transient_card_enables_are_independent_session_state() {
        let mut s = Session::new();
        assert!(s.click_repair_enabled());
        assert!(s.mouth_click_enabled());

        s.set_card_enabled(CardId::DeClickDePop, false);
        assert!(!s.click_repair_enabled());
        assert!(s.mouth_click_enabled());
        assert_eq!(
            s.stage_enabled_override(Stage::DeClick),
            None,
            "De-click / De-pop must not bypass the whole shared DeClick stage"
        );
        assert_eq!(
            s.stage_enabled_override(Stage::DePop),
            Some(false),
            "De-click / De-pop still drives the DePop stage"
        );
        assert!(!s.card_enabled(CardId::DeClickDePop));
        assert!(s.card_enabled(CardId::MouthClicks));

        s.set_mouth_click_enabled(false);
        assert!(!s.mouth_click_enabled());
        s.set_click_repair_enabled(true);
        assert!(s.click_repair_enabled());
        assert!(!s.mouth_click_enabled());

        s.select_preset(Preset::NARRATION);
        assert!(s.click_repair_enabled());
        assert!(s.mouth_click_enabled());
    }

    #[test]
    fn transient_card_amounts_do_not_mutate_each_other() {
        let mut s = Session::new();
        s.set_card_amount(CardId::DeClickDePop, 0.25);
        assert_eq!(s.click_repair_amount(), 0.25);
        assert_eq!(s.card_amount(CardId::DeClickDePop), 0.25);
        assert_eq!(
            s.mouth_click_amount(),
            1.0,
            "De-click amount must not change Mouth Clicks"
        );
        assert_eq!(
            s.stage_mix(Stage::DeClick),
            1.0,
            "Click amount must not mutate the whole shared DeClick stage mix"
        );
        assert_eq!(
            s.stage_mix(Stage::DePop),
            0.25,
            "De-click / De-pop amount still drives the DePop amount"
        );

        s.set_card_amount(CardId::MouthClicks, 0.4);
        assert_eq!(s.mouth_click_amount(), 0.4);
        assert_eq!(s.click_repair_amount(), 0.25);
        assert_eq!(s.stage_mix(Stage::DePop), 0.25);

        s.select_preset(Preset::NARRATION);
        assert_eq!(s.click_repair_amount(), 1.0);
        assert_eq!(s.mouth_click_amount(), 1.0);
        assert_eq!(s.stage_mix(Stage::DePop), 1.0);
    }

    #[test]
    fn transient_card_enables_reach_the_rendered_declick_paths() {
        let path = temp("g11-transient-render");
        transient_fixture(&path);

        let render = |click_on: bool, mouth_on: bool| {
            let mut s = Session::new();
            s.open(&path).unwrap();
            for stage in Stage::ALL {
                s.set_stage_enabled(stage, false);
            }
            s.set_stage_enabled(Stage::DeClick, true);
            s.set_card_enabled(CardId::DeClickDePop, click_on);
            s.set_card_enabled(CardId::MouthClicks, mouth_on);
            s.render().unwrap();
            (
                s.input().unwrap().channel(0).to_vec(),
                s.output().unwrap().channel(0).to_vec(),
            )
        };

        let (input, no_click) = render(false, true);
        assert!(
            (no_click[6000] - input[6000]).abs() <= f32::EPSILON,
            "click-card bypass must leave the short click untouched"
        );
        assert!(
            no_click[18_000..18_120]
                .iter()
                .zip(&input[18_000..18_120])
                .any(|(a, b)| (a - b).abs() > 1e-4),
            "mouth-click repair must still run when the click card is bypassed"
        );

        let (input, no_mouth) = render(true, false);
        assert!(
            (no_mouth[6000] - input[6000]).abs() > 1e-4,
            "short click repair must still run when Mouth Clicks is bypassed"
        );
        assert!(
            no_mouth[18_000..18_120]
                .iter()
                .zip(&input[18_000..18_120])
                .all(|(a, b)| (a - b).abs() <= f32::EPSILON),
            "Mouth Clicks bypass must leave the mouth-click region untouched"
        );

        let _ = std::fs::remove_file(&path);
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
    fn finish_card_amounts_do_not_mutate_the_shared_finish_stage_mix() {
        let mut s = Session::new();
        s.set_card_amount(CardId::PresenceAir, 0.25);
        assert_eq!(s.presence_air_amount(), 0.25);
        assert_eq!(s.card_amount(CardId::PresenceAir), 0.25);
        assert_eq!(
            s.stage_mix(Stage::Finish),
            1.0,
            "Presence/Air amount must not touch the whole Finish stage mix"
        );

        s.set_card_amount(CardId::Leveling, 0.4);
        assert_eq!(s.leveling_amount(), 0.4);
        assert_eq!(s.card_amount(CardId::Leveling), 0.4);
        assert_eq!(
            s.stage_mix(Stage::Finish),
            1.0,
            "Leveling amount must not touch the whole Finish stage mix"
        );

        s.select_preset(Preset::NARRATION);
        assert_eq!(s.presence_air_amount(), 1.0);
        assert_eq!(s.leveling_amount(), 1.0);
    }

    #[test]
    fn presence_air_amount_reaches_render_without_bypassing_loudness() {
        let path = temp("presence-air-amount");
        degraded_fixture(&path);

        let render = |amount: f32, enabled: bool| {
            let mut s = Session::new();
            s.open(&path).unwrap();
            s.set_presence_air_amount(amount);
            s.set_presence_air_enabled(enabled);
            s.render().unwrap();
            (
                s.output().unwrap().channel(0).to_vec(),
                s.output_lufs().unwrap(),
            )
        };

        let (dry_amount, dry_amount_lufs) = render(0.0, true);
        let (disabled, disabled_lufs) = render(1.0, false);
        assert!(
            (dry_amount_lufs - (-16.0)).abs() <= 0.6,
            "Presence/Air amount 0 must leave loudness running: {dry_amount_lufs}"
        );
        assert!(
            (disabled_lufs - (-16.0)).abs() <= 0.6,
            "Presence/Air disabled must leave loudness running: {disabled_lufs}"
        );
        for (a, b) in dry_amount.iter().zip(&disabled) {
            assert!(
                (a - b).abs() <= 1e-6,
                "Presence/Air amount 0 must render like Presence/Air disabled"
            );
        }

        let (wet_amount, _) = render(1.0, true);
        assert!(
            wet_amount
                .iter()
                .zip(&dry_amount)
                .any(|(a, b)| (a - b).abs() > 1e-5),
            "Presence/Air amount 1 must reach the rendered EQ"
        );

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn leveling_amount_reaches_rendered_program_leveler() {
        let path = temp("leveling-amount");
        stepped_level_fixture(&path);

        let render = |amount: f32, enabled: bool| {
            let mut s = Session::new();
            s.open(&path).unwrap();
            s.set_card_enabled(CardId::Denoise, false);
            s.set_card_enabled(CardId::DeReverb, false);
            s.set_card_enabled(CardId::DeClickDePop, false);
            s.set_card_enabled(CardId::MouthClicks, false);
            s.set_card_enabled(CardId::DeHum, false);
            s.set_card_enabled(CardId::DeEss, false);
            s.set_card_enabled(CardId::Breath, false);
            s.set_card_enabled(CardId::PresenceAir, false);
            s.set_card_enabled(CardId::Loudness, false);
            s.set_leveling_enabled(enabled);
            s.set_leveling_amount(amount);
            s.render().unwrap();
            s.output().unwrap().channel(0).to_vec()
        };
        let disabled = render(1.0, false);
        let amount_zero = render(0.0, true);
        for (a, b) in disabled.iter().zip(&amount_zero) {
            assert!(
                (a - b).abs() <= 1e-6,
                "Leveling amount 0 must render like Leveling disabled"
            );
        }

        let leveled = render(1.0, true);
        assert_eq!(
            leveled.len(),
            disabled.len(),
            "Leveling must preserve sample length"
        );
        let dry_spread = block_rms_spread_db(&disabled, SR as usize / 2);
        let leveled_spread = block_rms_spread_db(&leveled, SR as usize / 2);
        assert!(
            leveled_spread < dry_spread - 1.5,
            "Leveling card should reduce rendered dynamics: {dry_spread} -> {leveled_spread} dB"
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
    fn generative_badge_tracks_the_active_intensity() {
        let mut s = Session::new();

        s.set_quality_mode(QualityMode::Natural);
        assert!(!s.generative_reconstruction_badge());

        s.set_quality_mode(QualityMode::Balanced);
        assert!(s.generative_reconstruction_badge());

        s.set_quality_mode(QualityMode::RestorationHq);
        assert!(s.generative_reconstruction_badge());
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

        // One UI card fans out to DePop plus the Click sub-path under DeClick.
        s.set_card_enabled(CardId::DeClickDePop, false);
        assert_eq!(s.stage_enabled_override(Stage::DeClick), None);
        assert_eq!(s.stage_enabled_override(Stage::DePop), Some(false));
        assert!(!s.card_enabled(CardId::DeClickDePop));
        assert!(s.card_enabled(CardId::MouthClicks));
        // A 1:1 card resolves through the same path.
        assert!(s.card_enabled(CardId::Denoise));
    }

    #[test]
    fn advanced_debleed_source_mode_sets_the_debleed_default() {
        let mut s = Session::new();
        assert!(!s.debleed_source_imported());
        assert!(
            !s.resolve_stage_enabled(Stage::DeBleed),
            "native takes leave de-bleed off by default"
        );

        s.set_debleed_source_imported(true);
        assert!(s.debleed_source_imported());
        assert!(
            s.resolve_stage_enabled(Stage::DeBleed),
            "imported stems enable de-bleed by default"
        );

        s.set_stage_enabled(Stage::DeBleed, false);
        assert!(
            !s.resolve_stage_enabled(Stage::DeBleed),
            "an explicit stage override still wins over source mode"
        );
    }

    #[test]
    fn advanced_debleed_thresholds_reach_render_refusal() {
        let path = temp("advanced-debleed-thresholds");
        degraded_fixture(&path);
        let mut s = Session::new();
        s.open(&path).expect("open");

        assert_eq!(s.debleed_thresholds(), (0.02, 0.15));
        s.set_debleed_thresholds(0.8, 0.2);
        assert_eq!(
            s.debleed_thresholds(),
            (0.8, 0.8),
            "block threshold clamps to at least warn"
        );

        s.set_debleed_thresholds(0.0, 0.0);
        assert!(
            matches!(s.render(), Err(SessionError::BleedBlocked(_))),
            "zero block threshold must refuse before processing"
        );

        s.set_debleed_thresholds(0.95, 0.99);
        s.render().expect("relaxed thresholds render");

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
