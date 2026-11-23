//! YesVox — the egui **Standard-view** shell over the agent-native
//! [`yesvox_core::Session`] (UI ref `docs/ui/yesvox-v1-ui-reference.md`, #17).
//!
//! This crate keeps DSP out of the GUI: visible controls call `Session` methods,
//! while the visual layer is custom-painted to track the 1920x1080 reference
//! mock closely enough for product review.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::{
    fmt::Debug,
    hash::Hash,
    path::{Path, PathBuf},
};

mod audio;
mod standard;

use audio::Player;
use eframe::egui;
use yesvox_core::worker::{spawn, JobEvent, JobHandle};
use yesvox_core::{
    AudioBuffer, BatchControl, BatchPresetMode, BatchSummary, CardId, ContentMode, DeEssMode,
    History, JobStatus, LoudnessTarget, OutputFormat, PlaybackSource, Preset, QualityMode,
    RenderQueue, Session, SessionError, SessionSnapshot, Stage, VocalIsolationCheck,
};

/// Undo/redo depth cap (#25).
const HISTORY_CAP: usize = 100;

/// Status line shown when a settings edit is refused mid-job (#36).
const SETTINGS_LOCKED_STATUS: &str = "Working - settings unlock when the job finishes";

const DEFAULT_VIEWPORT_SIZE: [f32; 2] = [1920.0, 1080.0];
const MIN_VIEWPORT_SIZE: [f32; 2] = [1280.0, 720.0];

// eframe storage keys for persistence-lite (#24).
const STORAGE_LAST_DIR: &str = "yesvox_last_output_dir";
const STORAGE_LAST_FORMAT: &str = "yesvox_last_output_format";
const CANVAS: egui::Vec2 = egui::vec2(1920.0, 1080.0);

// Shared YES-suite graphite material system. YesVox keeps its own cyan/violet
// signal colors, but the surface depth, text hierarchy, and restrained chrome
// intentionally track YES Master's shipped design language.
const BG_TOP: egui::Color32 = egui::Color32::from_rgb(5, 7, 11);
const BG_BOTTOM: egui::Color32 = egui::Color32::from_rgb(8, 10, 15);
const PANEL: egui::Color32 = egui::Color32::from_rgb(13, 16, 23);
const PANEL_2: egui::Color32 = egui::Color32::from_rgb(19, 23, 32);
const CARD: egui::Color32 = egui::Color32::from_rgb(23, 28, 39);
const FIELD: egui::Color32 = egui::Color32::from_rgb(7, 9, 14);
const BORDER: egui::Color32 = egui::Color32::from_rgb(45, 54, 74);
const BORDER_DIM: egui::Color32 = egui::Color32::from_rgb(27, 33, 46);
const TEXT: egui::Color32 = egui::Color32::from_rgb(238, 241, 248);
const TEXT_MUTED: egui::Color32 = egui::Color32::from_rgb(174, 182, 204);
const TEXT_DIM: egui::Color32 = egui::Color32::from_rgb(109, 117, 146);
const BLUE: egui::Color32 = egui::Color32::from_rgb(70, 130, 246);
const CYAN: egui::Color32 = egui::Color32::from_rgb(73, 202, 238);
const PURPLE: egui::Color32 = egui::Color32::from_rgb(128, 82, 238);
const VIOLET: egui::Color32 = egui::Color32::from_rgb(88, 75, 214);
const AMBER: egui::Color32 = egui::Color32::from_rgb(241, 211, 91);

const KNOB_SWEEP_START_DEG: f32 = -225.0;
const KNOB_SWEEP_DEGREES: f32 = 270.0;
const KNOB_TICK_COUNT: usize = 28;
const KNOB_MAJOR_EVERY: usize = 7;

fn main() -> eframe::Result {
    // Installed layout (#20): point ort at the bundled onnxruntime before any
    // neural load — the load-dynamic probe hangs without a path (#33).
    yesvox_core::neural::configure_bundled_ort_runtime();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size(DEFAULT_VIEWPORT_SIZE)
            .with_min_inner_size(MIN_VIEWPORT_SIZE)
            .with_drag_and_drop(true)
            .with_title("YesVox"),
        ..Default::default()
    };
    eframe::run_native(
        "YesVox",
        options,
        Box::new(|cc| Ok(Box::new(YesVoxApp::new(cc)))),
    )
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Restore,
    Polish,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum ViewMode {
    #[default]
    Standard,
    Advanced,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AbView {
    Original,
    Processed,
    CompareSnapshot,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GuiBatchPresetMode {
    ForceSelected,
    DetectPerFile,
}

impl GuiBatchPresetMode {
    fn label(self) -> &'static str {
        match self {
            GuiBatchPresetMode::ForceSelected => "Force preset",
            GuiBatchPresetMode::DetectPerFile => "Detect each",
        }
    }

    fn to_core(self, fallback: Preset) -> BatchPresetMode {
        match self {
            GuiBatchPresetMode::ForceSelected => BatchPresetMode::Force(fallback),
            GuiBatchPresetMode::DetectPerFile => BatchPresetMode::DetectPerFile { fallback },
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OutputFieldChrome {
    Selector,
    ReadOnly,
}

fn output_field_chrome(label: &str) -> OutputFieldChrome {
    match label {
        "Sample Rate" => OutputFieldChrome::ReadOnly,
        _ => OutputFieldChrome::Selector,
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct AdvancedDeBleedPanel {
    source_imported: bool,
    warn_threshold: f32,
    block_threshold: f32,
    amount: f32,
}

impl AdvancedDeBleedPanel {
    fn from_session(session: &Session) -> Self {
        let (warn_threshold, block_threshold) = session.debleed_thresholds();
        AdvancedDeBleedPanel {
            source_imported: session.debleed_source_imported(),
            warn_threshold,
            block_threshold,
            amount: session.stage_mix(Stage::DeBleed),
        }
    }

    fn apply_to_session(self, session: &mut Session) {
        session.set_debleed_source_imported(self.source_imported);
        session.set_debleed_thresholds(self.warn_threshold, self.block_threshold);
        session.set_stage_mix(Stage::DeBleed, self.amount);
    }

    fn source_label(self) -> &'static str {
        if self.source_imported {
            "Imported stem"
        } else {
            "Native take"
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct AdvancedDeBleedControlRects {
    native_source: egui::Rect,
    imported_source: egui::Rect,
    warn_slider: egui::Rect,
    block_slider: egui::Rect,
    amount_knob: egui::Rect,
}

fn advanced_debleed_control_rects(card: egui::Rect) -> AdvancedDeBleedControlRects {
    AdvancedDeBleedControlRects {
        native_source: rect(card.left() + 12.0, card.top() + 42.0, 74.0, 22.0),
        imported_source: rect(card.left() + 90.0, card.top() + 42.0, 78.0, 22.0),
        warn_slider: rect(card.left() + 76.0, card.top() + 76.0, 98.0, 5.0),
        block_slider: rect(card.left() + 76.0, card.top() + 99.0, 98.0, 5.0),
        amount_knob: rect(card.right() - 72.0, card.top() + 50.0, 60.0, 60.0),
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct AdvancedDeClipPanel {
    threshold: f32,
    strength: f32,
    amount: f32,
}

impl AdvancedDeClipPanel {
    fn from_session(session: &Session) -> Self {
        let (threshold, strength) = session.declip_settings();
        AdvancedDeClipPanel {
            threshold,
            strength,
            amount: session.stage_mix(Stage::DeClip),
        }
    }

    fn apply_to_session(self, session: &mut Session) {
        session.set_declip_settings(self.threshold, self.strength);
        session.set_stage_mix(Stage::DeClip, self.amount);
    }
}

#[derive(Clone, Copy, Debug)]
struct AdvancedDeClipControlRects {
    threshold_slider: egui::Rect,
    strength_slider: egui::Rect,
    amount_knob: egui::Rect,
}

fn advanced_declip_control_rects(card: egui::Rect) -> AdvancedDeClipControlRects {
    AdvancedDeClipControlRects {
        threshold_slider: rect(card.left() + 86.0, card.top() + 57.0, 90.0, 5.0),
        strength_slider: rect(card.left() + 86.0, card.top() + 88.0, 90.0, 5.0),
        amount_knob: rect(card.right() - 72.0, card.top() + 50.0, 60.0, 60.0),
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct AdvancedDeClickPanel {
    click_enabled: bool,
    mouth_enabled: bool,
    click_amount: f32,
    mouth_amount: f32,
}

impl AdvancedDeClickPanel {
    fn from_session(session: &Session) -> Self {
        AdvancedDeClickPanel {
            click_enabled: session.click_repair_enabled(),
            mouth_enabled: session.mouth_click_enabled(),
            click_amount: session.click_repair_amount(),
            mouth_amount: session.mouth_click_amount(),
        }
    }

    fn apply_to_session(self, session: &mut Session) {
        session.set_click_repair_enabled(self.click_enabled);
        session.set_mouth_click_enabled(self.mouth_enabled);
        session.set_click_repair_amount(self.click_amount);
        session.set_mouth_click_amount(self.mouth_amount);
    }
}

#[derive(Clone, Copy, Debug)]
struct AdvancedDeClickControlRects {
    click_toggle: egui::Rect,
    mouth_toggle: egui::Rect,
    click_knob: egui::Rect,
    mouth_knob: egui::Rect,
}

fn advanced_declick_control_rects(card: egui::Rect) -> AdvancedDeClickControlRects {
    AdvancedDeClickControlRects {
        click_toggle: rect(card.left() + 12.0, card.top() + 54.0, 34.0, 20.0),
        mouth_toggle: rect(card.left() + 12.0, card.top() + 88.0, 34.0, 20.0),
        click_knob: rect(card.left() + 118.0, card.top() + 48.0, 60.0, 60.0),
        mouth_knob: rect(card.left() + 180.0, card.top() + 48.0, 60.0, 60.0),
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct AdvancedDePopPanel {
    cutoff_hz: f32,
    strength: f32,
    amount: f32,
}

impl AdvancedDePopPanel {
    fn from_session(session: &Session) -> Self {
        let (cutoff_hz, strength) = session.depop_settings();
        AdvancedDePopPanel {
            cutoff_hz,
            strength,
            amount: session.stage_mix(Stage::DePop),
        }
    }

    fn apply_to_session(self, session: &mut Session) {
        session.set_depop_settings(self.cutoff_hz, self.strength);
        session.set_stage_mix(Stage::DePop, self.amount);
    }
}

#[derive(Clone, Copy, Debug)]
struct AdvancedDePopControlRects {
    cutoff_slider: egui::Rect,
    strength_slider: egui::Rect,
    amount_knob: egui::Rect,
}

fn advanced_depop_control_rects(card: egui::Rect) -> AdvancedDePopControlRects {
    AdvancedDePopControlRects {
        cutoff_slider: rect(card.left() + 86.0, card.top() + 57.0, 90.0, 5.0),
        strength_slider: rect(card.left() + 86.0, card.top() + 88.0, 90.0, 5.0),
        amount_knob: rect(card.right() - 72.0, card.top() + 50.0, 60.0, 60.0),
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct AdvancedDeHumPanel {
    nominal: Option<f32>,
    harmonics: u32,
    q: f32,
    track: bool,
    amount: f32,
}

impl AdvancedDeHumPanel {
    fn from_session(session: &Session) -> Self {
        let (nominal, harmonics, q, track) = session.dehum_settings();
        AdvancedDeHumPanel {
            nominal,
            harmonics,
            q,
            track,
            amount: session.stage_mix(Stage::DeHum),
        }
    }

    fn apply_to_session(self, session: &mut Session) {
        session.set_dehum_settings(self.nominal, self.harmonics, self.q, self.track);
        session.set_stage_mix(Stage::DeHum, self.amount);
    }

    fn nominal_label(self) -> &'static str {
        match self.nominal {
            None => "Auto",
            Some(f) if f < 55.0 => "50 Hz",
            Some(_) => "60 Hz",
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct AdvancedDeHumControlRects {
    auto_button: egui::Rect,
    hz50_button: egui::Rect,
    hz60_button: egui::Rect,
    track_toggle: egui::Rect,
    harmonics_slider: egui::Rect,
    q_slider: egui::Rect,
    amount_knob: egui::Rect,
}

fn advanced_dehum_control_rects(card: egui::Rect) -> AdvancedDeHumControlRects {
    AdvancedDeHumControlRects {
        auto_button: rect(card.left() + 12.0, card.top() + 42.0, 50.0, 20.0),
        hz50_button: rect(card.left() + 66.0, card.top() + 42.0, 34.0, 20.0),
        hz60_button: rect(card.left() + 104.0, card.top() + 42.0, 34.0, 20.0),
        track_toggle: rect(card.left() + 144.0, card.top() + 42.0, 34.0, 20.0),
        harmonics_slider: rect(card.left() + 82.0, card.top() + 76.0, 72.0, 5.0),
        q_slider: rect(card.left() + 82.0, card.top() + 99.0, 72.0, 5.0),
        amount_knob: rect(card.right() - 68.0, card.top() + 52.0, 56.0, 56.0),
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct AdvancedDenoisePanel {
    quality: QualityMode,
    workers: usize,
    amount: f32,
}

impl AdvancedDenoisePanel {
    fn from_session(session: &Session) -> Self {
        AdvancedDenoisePanel {
            quality: session.settings().quality_mode,
            workers: session.worker_count(),
            amount: session.stage_mix(Stage::Denoise),
        }
    }

    fn apply_to_session(self, session: &mut Session) {
        session.set_quality_mode(self.quality);
        session.set_worker_count(self.workers);
        session.set_stage_mix(Stage::Denoise, self.amount);
    }

    fn quality_label(self) -> &'static str {
        match self.quality {
            QualityMode::Natural => "Natural",
            QualityMode::Balanced => "Balanced",
            QualityMode::RestorationHq => "HQ",
        }
    }

    fn engine_label(self) -> &'static str {
        if self.quality.is_generative() {
            "Neural"
        } else {
            "Light"
        }
    }

    fn pass_label(self) -> &'static str {
        match self.quality.binding().max_passes {
            1 => "1 pass",
            _ => "2 pass",
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct AdvancedDenoiseControlRects {
    natural_button: egui::Rect,
    balanced_button: egui::Rect,
    hq_button: egui::Rect,
    workers_slider: egui::Rect,
    amount_knob: egui::Rect,
}

fn advanced_denoise_control_rects(card: egui::Rect) -> AdvancedDenoiseControlRects {
    AdvancedDenoiseControlRects {
        natural_button: rect(card.left() + 12.0, card.top() + 42.0, 42.0, 20.0),
        balanced_button: rect(card.left() + 58.0, card.top() + 42.0, 42.0, 20.0),
        hq_button: rect(card.left() + 104.0, card.top() + 42.0, 34.0, 20.0),
        workers_slider: rect(card.left() + 82.0, card.top() + 84.0, 62.0, 5.0),
        amount_knob: rect(card.right() - 68.0, card.top() + 52.0, 56.0, 56.0),
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct AdvancedDeReverbPanel {
    strength: f32,
    tail_preservation: f32,
    rt60_s: f32,
    amount: f32,
}

impl AdvancedDeReverbPanel {
    fn from_session(session: &Session) -> Self {
        let (strength, tail_preservation) = session.dereverb_settings();
        AdvancedDeReverbPanel {
            strength,
            tail_preservation,
            rt60_s: session.dereverb_rt60(),
            amount: session.stage_mix(Stage::DeReverb),
        }
    }

    fn apply_to_session(self, session: &mut Session) {
        session.set_dereverb_settings(self.strength, self.tail_preservation);
        session.set_stage_mix(Stage::DeReverb, self.amount);
    }

    fn rt60_label(self) -> String {
        if self.rt60_s > 0.0 {
            format!("RT60 {:.2}s", self.rt60_s)
        } else {
            "RT60 --".to_owned()
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct AdvancedDeReverbControlRects {
    strength_slider: egui::Rect,
    tail_slider: egui::Rect,
    amount_knob: egui::Rect,
}

fn advanced_dereverb_control_rects(card: egui::Rect) -> AdvancedDeReverbControlRects {
    AdvancedDeReverbControlRects {
        strength_slider: rect(card.left() + 86.0, card.top() + 57.0, 74.0, 5.0),
        tail_slider: rect(card.left() + 86.0, card.top() + 88.0, 74.0, 5.0),
        amount_knob: rect(card.right() - 68.0, card.top() + 52.0, 56.0, 56.0),
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct AdvancedDeEssPanel {
    mode: DeEssMode,
    ess_ratio: f32,
    min_gain: f32,
    hf_reduction_db: f32,
    amount: f32,
}

impl AdvancedDeEssPanel {
    fn from_session(session: &Session) -> Self {
        let (mode, ess_ratio, min_gain) = session.deess_settings();
        AdvancedDeEssPanel {
            mode,
            ess_ratio,
            min_gain,
            hf_reduction_db: session.deess_hf_reduction_db(),
            amount: session.stage_mix(Stage::DeEss),
        }
    }

    fn apply_to_session(self, session: &mut Session) {
        session.set_deess_settings(self.mode, self.ess_ratio, self.min_gain);
        session.set_stage_mix(Stage::DeEss, self.amount);
    }

    fn mode_label(self) -> &'static str {
        match self.mode {
            DeEssMode::Split => "Split",
            DeEssMode::Wide => "Wide",
        }
    }

    fn hf_label(self) -> String {
        if self.hf_reduction_db > 0.0 {
            format!("HF {:.1} dB", self.hf_reduction_db)
        } else {
            "HF --".to_owned()
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct AdvancedDeEssControlRects {
    split_button: egui::Rect,
    wide_button: egui::Rect,
    ess_ratio_slider: egui::Rect,
    min_gain_slider: egui::Rect,
    amount_knob: egui::Rect,
}

fn advanced_deess_control_rects(card: egui::Rect) -> AdvancedDeEssControlRects {
    AdvancedDeEssControlRects {
        split_button: rect(card.left() + 12.0, card.top() + 42.0, 52.0, 20.0),
        wide_button: rect(card.left() + 68.0, card.top() + 42.0, 52.0, 20.0),
        ess_ratio_slider: rect(card.left() + 86.0, card.top() + 78.0, 90.0, 5.0),
        min_gain_slider: rect(card.left() + 86.0, card.top() + 101.0, 90.0, 5.0),
        amount_knob: rect(card.right() - 68.0, card.top() + 52.0, 56.0, 56.0),
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct AdvancedBreathPanel {
    strength_db: f32,
    shared_hf_reduction_db: f32,
    amount: f32,
}

impl AdvancedBreathPanel {
    fn from_session(session: &Session) -> Self {
        AdvancedBreathPanel {
            strength_db: session.breath_strength_db(),
            shared_hf_reduction_db: session.breath_shared_hf_reduction_db(),
            amount: session.stage_mix(Stage::Breath),
        }
    }

    fn apply_to_session(self, session: &mut Session) {
        session.set_breath_strength_db(self.strength_db);
        session.set_stage_mix(Stage::Breath, self.amount);
    }

    fn sidechain_label(self) -> String {
        if self.shared_hf_reduction_db > 0.0 {
            format!("HF {:.1} dB", self.shared_hf_reduction_db)
        } else {
            "HF --".to_owned()
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct AdvancedBreathControlRects {
    strength_slider: egui::Rect,
    amount_knob: egui::Rect,
}

fn advanced_breath_control_rects(card: egui::Rect) -> AdvancedBreathControlRects {
    AdvancedBreathControlRects {
        strength_slider: rect(card.left() + 86.0, card.top() + 78.0, 90.0, 5.0),
        amount_knob: rect(card.right() - 68.0, card.top() + 52.0, 56.0, 56.0),
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct AdvancedFinishPanel {
    presence_air_enabled: bool,
    leveling_enabled: bool,
    loudness_enabled: bool,
    presence_db: f32,
    air_db: f32,
    presence_air_amount: f32,
    leveling_amount: f32,
    loudness_target: LoudnessTarget,
}

impl AdvancedFinishPanel {
    fn from_session(session: &Session) -> Self {
        let (presence_db, air_db) = session.presence_air_eq();
        AdvancedFinishPanel {
            presence_air_enabled: session.presence_air_enabled(),
            leveling_enabled: session.leveling_enabled(),
            loudness_enabled: session.loudness_enabled(),
            presence_db,
            air_db,
            presence_air_amount: session.presence_air_amount(),
            leveling_amount: session.leveling_amount(),
            loudness_target: session.loudness_target(),
        }
    }

    fn apply_to_session(self, session: &mut Session) {
        session.set_presence_air_enabled(self.presence_air_enabled);
        session.set_leveling_enabled(self.leveling_enabled);
        session.set_loudness_enabled(self.loudness_enabled);
        session.set_presence_air_eq(self.presence_db, self.air_db);
        session.set_presence_air_amount(self.presence_air_amount);
        session.set_leveling_amount(self.leveling_amount);
        session.set_loudness_target(self.loudness_target);
    }

    fn target_short_label(self) -> &'static str {
        match self.loudness_target {
            LoudnessTarget::Podcast => "-16",
            LoudnessTarget::Streaming => "-14",
            LoudnessTarget::MatchInput => "Match",
            LoudnessTarget::PeakNormalize => "Peak",
            LoudnessTarget::Custom(_) => "Custom",
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct AdvancedFinishControlRects {
    presence_toggle: egui::Rect,
    presence_slider: egui::Rect,
    air_slider: egui::Rect,
    presence_amount_knob: egui::Rect,
    leveling_toggle: egui::Rect,
    leveling_slider: egui::Rect,
    loudness_toggle: egui::Rect,
    target_button: egui::Rect,
    loudness_knob: egui::Rect,
}

fn advanced_finish_control_rects(card: egui::Rect) -> AdvancedFinishControlRects {
    AdvancedFinishControlRects {
        presence_toggle: rect(card.left() + 12.0, card.top() + 49.0, 18.0, 18.0),
        presence_slider: rect(card.left() + 84.0, card.top() + 59.0, 66.0, 5.0),
        air_slider: rect(card.left() + 84.0, card.top() + 89.0, 66.0, 5.0),
        presence_amount_knob: rect(card.left() + 28.0, card.top() + 126.0, 54.0, 54.0),
        leveling_toggle: rect(card.left() + 96.0, card.top() + 109.0, 18.0, 18.0),
        leveling_slider: rect(card.left() + 124.0, card.top() + 130.0, 56.0, 5.0),
        loudness_toggle: rect(card.left() + 166.0, card.top() + 49.0, 18.0, 18.0),
        target_button: rect(card.left() + 188.0, card.top() + 83.0, 68.0, 24.0),
        loudness_knob: rect(card.left() + 198.0, card.top() + 126.0, 54.0, 54.0),
    }
}

#[derive(Clone, Copy, Debug)]
struct AdvancedFinishLabelRects {
    presence: egui::Rect,
    presence_value: egui::Rect,
    air: egui::Rect,
    air_value: egui::Rect,
    presence_amount: egui::Rect,
    leveling: egui::Rect,
    leveling_value: egui::Rect,
    loudness: egui::Rect,
    target: egui::Rect,
    limiter: egui::Rect,
}

fn advanced_finish_label_rects(card: egui::Rect) -> AdvancedFinishLabelRects {
    AdvancedFinishLabelRects {
        presence: rect(card.left() + 34.0, card.top() + 49.0, 44.0, 12.0),
        presence_value: rect(card.left() + 84.0, card.top() + 43.0, 44.0, 12.0),
        air: rect(card.left() + 34.0, card.top() + 79.0, 22.0, 12.0),
        air_value: rect(card.left() + 84.0, card.top() + 73.0, 44.0, 12.0),
        presence_amount: rect(card.left() + 26.0, card.top() + 108.0, 60.0, 12.0),
        leveling: rect(card.left() + 118.0, card.top() + 108.0, 48.0, 12.0),
        leveling_value: rect(card.left() + 124.0, card.top() + 140.0, 32.0, 12.0),
        loudness: rect(card.left() + 188.0, card.top() + 49.0, 52.0, 12.0),
        target: rect(card.left() + 188.0, card.top() + 66.0, 36.0, 12.0),
        limiter: rect(card.left() + 194.0, card.top() + 184.0, 56.0, 12.0),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct BatchQueueRow {
    input: PathBuf,
    output: PathBuf,
    status: JobStatus,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct BatchQueueCounts {
    pending: usize,
    done: usize,
    failed: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct BatchQueueUi {
    rows: Vec<BatchQueueRow>,
    mode: GuiBatchPresetMode,
    paused: bool,
}

impl Default for BatchQueueUi {
    fn default() -> Self {
        BatchQueueUi {
            rows: Vec::new(),
            mode: GuiBatchPresetMode::ForceSelected,
            paused: false,
        }
    }
}

impl BatchQueueUi {
    fn enqueue(&mut self, input: PathBuf, output: PathBuf) -> bool {
        if self
            .rows
            .iter()
            .any(|row| row.input == input && matches!(row.status, JobStatus::Pending))
        {
            return false;
        }
        self.rows.push(BatchQueueRow {
            input,
            output,
            status: JobStatus::Pending,
        });
        true
    }

    fn counts(&self) -> BatchQueueCounts {
        let mut counts = BatchQueueCounts::default();
        for row in &self.rows {
            match row.status {
                JobStatus::Pending => counts.pending += 1,
                JobStatus::Done(_) => counts.done += 1,
                JobStatus::Failed(_) => counts.failed += 1,
            }
        }
        counts
    }

    fn summary_label(&self) -> String {
        let counts = self.counts();
        if self.rows.is_empty() {
            "No queued vocals".to_owned()
        } else {
            format!(
                "{} queued - {} done - {} failed",
                counts.pending, counts.done, counts.failed
            )
        }
    }

    fn pending_count(&self) -> usize {
        self.counts().pending
    }

    fn preset_mode(&self, fallback: Preset) -> BatchPresetMode {
        self.mode.to_core(fallback)
    }
}

#[derive(Clone, Copy)]
struct CanvasMap {
    rect: egui::Rect,
    scale: f32,
}

impl CanvasMap {
    fn new(available: egui::Rect) -> Self {
        let scale = (available.width() / CANVAS.x)
            .min(available.height() / CANVAS.y)
            .max(0.01);
        let size = CANVAS * scale;
        let rect = egui::Rect::from_center_size(available.center(), size);
        CanvasMap { rect, scale }
    }

    fn x(self, x: f32) -> f32 {
        self.rect.left() + x * self.scale
    }

    fn y(self, y: f32) -> f32 {
        self.rect.top() + y * self.scale
    }

    fn pos(self, x: f32, y: f32) -> egui::Pos2 {
        egui::pos2(self.x(x), self.y(y))
    }

    fn v(self, value: f32) -> f32 {
        value * self.scale
    }

    fn rect(self, r: egui::Rect) -> egui::Rect {
        egui::Rect::from_min_max(self.pos(r.left(), r.top()), self.pos(r.right(), r.bottom()))
    }

    fn font(self, size: f32) -> egui::FontId {
        egui::FontId::proportional(self.v(size))
    }
}

#[derive(Clone, Copy)]
struct ReferenceLayout {
    top: egui::Rect,
    left: egui::Rect,
    center: egui::Rect,
    right: egui::Rect,
    status: egui::Rect,
    hero: egui::Rect,
    restore: egui::Rect,
    polish: egui::Rect,
    monitor: egui::Rect,
}

impl ReferenceLayout {
    fn new() -> Self {
        ReferenceLayout {
            top: rect(0.0, 0.0, 1920.0, 70.0),
            left: rect(4.0, 70.0, 282.0, 970.0),
            center: rect(302.0, 70.0, 1254.0, 970.0),
            right: rect(1574.0, 70.0, 332.0, 970.0),
            status: rect(0.0, 1040.0, 1920.0, 40.0),
            hero: rect(302.0, 70.0, 1254.0, 350.0),
            restore: rect(302.0, 428.0, 1254.0, 217.0),
            polish: rect(302.0, 653.0, 1254.0, 217.0),
            monitor: rect(302.0, 878.0, 1254.0, 150.0),
        }
    }
}

#[derive(Clone, Copy)]
struct AdvancedLayout {
    top: egui::Rect,
    work: egui::Rect,
    status: egui::Rect,
    spectrogram: egui::Rect,
    group_panels: [egui::Rect; 5],
    module_cards: [egui::Rect; 10],
    monitor: egui::Rect,
    export: egui::Rect,
}

impl AdvancedLayout {
    fn new() -> Self {
        AdvancedLayout {
            top: rect(0.0, 0.0, 1920.0, 70.0),
            work: rect(88.0, 70.0, 1744.0, 970.0),
            status: rect(0.0, 1040.0, 1920.0, 40.0),
            spectrogram: rect(88.0, 96.0, 1744.0, 270.0),
            group_panels: [
                rect(88.0, 390.0, 1112.0, 318.0),  // Restore
                rect(1212.0, 390.0, 300.0, 318.0), // Polish
                rect(1524.0, 390.0, 308.0, 318.0), // Finish
                rect(88.0, 724.0, 870.0, 292.0),   // Monitor
                rect(970.0, 724.0, 862.0, 292.0),  // Export / safety
            ],
            module_cards: [
                rect(108.0, 442.0, 250.0, 118.0),
                rect(382.0, 442.0, 250.0, 118.0),
                rect(656.0, 442.0, 250.0, 118.0),
                rect(930.0, 442.0, 250.0, 118.0),
                rect(108.0, 574.0, 250.0, 118.0),
                rect(382.0, 574.0, 250.0, 118.0),
                rect(656.0, 574.0, 250.0, 118.0),
                rect(1232.0, 442.0, 260.0, 118.0),
                rect(1232.0, 574.0, 260.0, 118.0),
                rect(1544.0, 474.0, 268.0, 214.0),
            ],
            monitor: rect(88.0, 724.0, 870.0, 292.0),
            export: rect(970.0, 724.0, 862.0, 292.0),
        }
    }
}

fn rect(x: f32, y: f32, w: f32, h: f32) -> egui::Rect {
    egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(w, h))
}

/// Result of an off-thread render: the moved-out session (now rendered) comes
/// back so the UI can adopt it, plus a status line.
struct RenderDone {
    session: Session,
    ok: bool,
    status: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AudibleCacheKey {
    ab: AbView,
    volume_match: bool,
    reblend_bits: u32,
    auditioning_reblend: bool,
    generation: u64,
}

impl AudibleCacheKey {
    fn playback_source(self) -> PlaybackSource {
        if self.auditioning_reblend {
            PlaybackSource::Reblend
        } else {
            match self.ab {
                AbView::Original => PlaybackSource::Original,
                AbView::Processed => PlaybackSource::Processed,
                AbView::CompareSnapshot => PlaybackSource::CompareSnapshot,
            }
        }
    }
}

/// Badge text when the audition stream runs at a different rate than the
/// source (#36). The preview resamples to the device rate when the device
/// won't open at the file's rate (#35) — export is unaffected, and staying
/// silent about it reads as "my file changed sample rate".
fn preview_rate_badge(stream_hz: u32, source_hz: u32) -> Option<String> {
    (stream_hz != source_hz).then(|| {
        format!(
            "Preview {:.1} kHz - source {:.1} kHz",
            stream_hz as f32 / 1000.0,
            source_hz as f32 / 1000.0
        )
    })
}

/// Status line for a canceled render (#36). A canceled render leaves the prior
/// output untouched, so playback keeps auditioning the previous processed take
/// when one exists — say so instead of being silently correct.
fn canceled_render_status(has_previous_output: bool) -> &'static str {
    if has_previous_output {
        "Render canceled - showing previous processed"
    } else {
        "Render canceled"
    }
}

fn audible_cache_key(
    ab: AbView,
    volume_match: bool,
    reblend_amount: f32,
    auditioning_reblend: bool,
    generation: u64,
) -> AudibleCacheKey {
    AudibleCacheKey {
        ab,
        volume_match,
        // Only the Reblend source renders the blend — folding the slider into
        // every key made A/B drags rebuild the buffer + LUFS per frame (#36).
        reblend_bits: if auditioning_reblend {
            reblend_amount.clamp(0.0, 1.0).to_bits()
        } else {
            0
        },
        auditioning_reblend,
        generation,
    }
}

fn should_refresh_player_source(current: Option<AudibleCacheKey>, next: AudibleCacheKey) -> bool {
    current != Some(next)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SpectrogramCacheKey {
    audible: AudibleCacheKey,
    columns: usize,
    rows: usize,
}

fn advanced_spectrogram_dimensions(r: egui::Rect) -> (usize, usize) {
    let columns = ((r.width() / 7.0).round() as usize).clamp(64, 240);
    let rows = ((r.height() / 4.0).round() as usize).clamp(32, 96);
    (columns, rows)
}

fn spectrogram_cache_key(
    audible: AudibleCacheKey,
    columns: usize,
    rows: usize,
) -> SpectrogramCacheKey {
    SpectrogramCacheKey {
        audible,
        columns,
        rows,
    }
}

#[derive(Clone, Copy)]
struct AudibleMetrics {
    peak_db: f32,
    lufs: Option<f32>,
}

struct AudibleCache {
    key: AudibleCacheKey,
    buffer: AudioBuffer,
    metrics: AudibleMetrics,
}

impl AudibleCache {
    fn new(key: AudibleCacheKey, buffer: AudioBuffer) -> Self {
        let metrics = AudibleMetrics {
            peak_db: yesvox_core::monitor::channel_peak_db(&buffer, 0),
            lufs: yesvox_core::loudness::integrated_lufs(&buffer),
        };
        Self {
            key,
            buffer,
            metrics,
        }
    }
}

struct SpectrogramCache {
    key: SpectrogramCacheKey,
    spectrogram: yesvox_core::monitor::Spectrogram,
}

/// Result of an off-thread batch run: the per-row status updates to apply and
/// the run summary.
struct BatchDone {
    updates: Vec<(usize, JobStatus)>,
    summary: BatchSummary,
}

struct YesVoxApp {
    session: Session,
    view_mode: ViewMode,
    ab: AbView,
    show_standard_controls: bool,
    source_path: Option<PathBuf>,
    source_name: String,
    name_input: String,
    status: String,
    reblend_amount: f32,
    volume_match: bool,
    batch_queue: BatchQueueUi,
    isolation_check: Option<VocalIsolationCheck>,
    /// In-flight background jobs (#22). Processing runs off the UI thread; the
    /// UI polls these each frame. Only one runs at a time (see `busy`).
    render_job: Option<JobHandle<RenderDone>>,
    export_job: Option<JobHandle<PathBuf>>,
    batch_job: Option<JobHandle<BatchDone>>,
    /// Last real progress fraction reported by the current foreground job — what
    /// drives the progress bars (replaces the old 0/0.55/1.0 heuristic).
    job_progress: f32,
    /// The cpal playback stream (#23), created on first Play. `None` = stopped /
    /// no device. Holds the shared transport; switching A/B source preserves the
    /// playhead.
    player: Option<Player>,
    /// What the transport audits: `false` = the A/B compare (Original vs the
    /// pure processed output — never a blend, so the compare is honest);
    /// `true` = the reblend strip's mix. Cleared by any A/B interaction.
    auditioning_reblend: bool,
    /// A reblend-slider change happened mid-drag; the playback source refreshes
    /// on pointer release (per-frame full-buffer swaps thrash the audio path).
    reblend_dirty: bool,
    /// Bumps whenever imported/rendered/invalidated audio changes, so cached
    /// monitor buffers cannot outlive the session audio they summarize.
    audio_generation: u64,
    audible_cache: Option<AudibleCache>,
    spectrogram_cache: Option<SpectrogramCache>,
    player_source_key: Option<AudibleCacheKey>,
    /// Settings snapshot taken before each render (the session is *moved* onto
    /// the worker; if the worker panics, the session is lost with it and gets
    /// rebuilt from the source path + this snapshot).
    render_recovery: Option<SessionSnapshot>,
    /// Whether the top-bar preset dropdown is showing (#24).
    preset_menu_open: bool,
    /// Bounded undo/redo over settings snapshots (#25).
    history: History<SessionSnapshot>,
}

impl YesVoxApp {
    fn top_bar_view_rects() -> [egui::Rect; 2] {
        [
            rect(835.0, 13.0, 124.0, 44.0),
            rect(961.0, 13.0, 124.0, 44.0),
        ]
    }

    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        install_theme(&cc.egui_ctx);
        let mut session = Self::default_session();

        // Persistence-lite (#24): restore the last output dir + format from
        // eframe storage. Degrades to defaults when there is no stored value.
        if let Some(storage) = cc.storage {
            if let Some(dir) = storage.get_string(STORAGE_LAST_DIR) {
                if !dir.is_empty() {
                    session.set_output_location(PathBuf::from(dir));
                }
            }
            if storage.get_string(STORAGE_LAST_FORMAT).as_deref() == Some("FLAC") {
                session.set_output_format(OutputFormat::Flac);
            }
        }

        Self::from_session(session)
    }

    /// The product-default session `new` starts from (before storage restore).
    fn default_session() -> Session {
        let mut session = Session::new();
        session.select_preset(Preset::LEAD_VOCAL);
        session.set_flow_amount(1.0);
        session.set_restoration_focus(0.5);
        session.set_polish_intensity(0.5);
        session.set_loudness_target(LoudnessTarget::Streaming);
        session.set_output_format(OutputFormat::Wav);
        session.set_bit_depth(Some(24));
        session.set_dither(true);
        for (card, amount) in [
            (CardId::Denoise, 0.48),
            (CardId::DeReverb, 0.42),
            (CardId::DeClickDePop, 0.55),
            (CardId::MouthClicks, 0.60),
            (CardId::DeHum, 0.35),
            (CardId::DeEss, 0.40),
            (CardId::Breath, 0.30),
            (CardId::PresenceAir, 0.35),
            (CardId::Leveling, 0.45),
        ] {
            session.set_card_enabled(card, true);
            session.set_card_amount(card, amount);
        }
        session.set_card_enabled(CardId::Loudness, true);
        session
    }

    /// Fresh app state over `session` (theme/storage are `new`'s concern —
    /// this is also the test constructor).
    fn from_session(session: Session) -> Self {
        let history = History::new(session.snapshot(), HISTORY_CAP);
        YesVoxApp {
            session,
            view_mode: ViewMode::Standard,
            ab: AbView::Processed,
            show_standard_controls: false,
            source_path: None,
            source_name: String::new(),
            name_input: String::new(),
            status: "Ready - import a vocal".to_owned(),
            reblend_amount: 0.25,
            volume_match: false,
            batch_queue: BatchQueueUi::default(),
            isolation_check: None,
            render_job: None,
            export_job: None,
            batch_job: None,
            job_progress: 0.0,
            player: None,
            auditioning_reblend: false,
            reblend_dirty: false,
            audio_generation: 0,
            audible_cache: None,
            spectrogram_cache: None,
            player_source_key: None,
            render_recovery: None,
            preset_menu_open: false,
            history,
        }
    }

    /// Record the current settings state as an undo step (#25). `key` is the
    /// control's identity so rapid same-control drags coalesce into one step.
    fn commit_edit(&mut self, key: &str) {
        let snapshot = self.session.snapshot();
        self.history.record(snapshot, Some(key));
        self.bump_audio_generation();
    }

    /// Whether settings edits are locked (#36). The render path moves the
    /// session onto the worker and leaves a placeholder behind — an edit made
    /// now would write to the placeholder and silently vanish when the
    /// rendered session is adopted back, with a bogus history entry left over.
    /// Export/batch keep the session in place but share the lock so the
    /// settings that produced an in-flight job can't drift under it.
    fn edits_locked(&self) -> bool {
        self.busy()
    }

    /// The single gate for settings edits (#36): apply `edit` to the session
    /// and record the undo step, or refuse with a status line while a job is
    /// in flight. Returns whether the edit landed.
    fn apply_edit(&mut self, key: &str, edit: impl FnOnce(&mut Session)) -> bool {
        if self.edits_locked() {
            self.status = SETTINGS_LOCKED_STATUS.to_owned();
            return false;
        }
        edit(&mut self.session);
        self.commit_edit(key);
        true
    }

    fn bump_audio_generation(&mut self) {
        self.audio_generation = self.audio_generation.wrapping_add(1);
        self.audible_cache = None;
        self.spectrogram_cache = None;
        self.player_source_key = None;
    }

    /// Undo the last settings edit (if any), restoring the session.
    fn undo(&mut self) {
        if self.edits_locked() {
            // A restore is a settings edit — it would land on the render
            // placeholder and vanish on adopt-back, desyncing history (#36).
            self.status = SETTINGS_LOCKED_STATUS.to_owned();
            return;
        }
        if let Some(snapshot) = self.history.undo().cloned() {
            self.session.restore_snapshot(&snapshot);
            self.stop_playback();
            self.bump_audio_generation();
            if self.view_mode == ViewMode::Standard && !self.show_standard_controls {
                self.leave_special_audition();
            }
            // The verdict described the pre-undo render; don't show it against
            // reverted settings.
            self.isolation_check = None;
            self.status = "Undo".to_owned();
        }
    }

    /// Redo the last undone edit (if any).
    fn redo(&mut self) {
        if self.edits_locked() {
            self.status = SETTINGS_LOCKED_STATUS.to_owned();
            return;
        }
        if let Some(snapshot) = self.history.redo().cloned() {
            self.session.restore_snapshot(&snapshot);
            self.stop_playback();
            self.bump_audio_generation();
            if self.view_mode == ViewMode::Standard && !self.show_standard_controls {
                self.leave_special_audition();
            }
            self.isolation_check = None;
            self.status = "Redo".to_owned();
        }
    }

    /// Draw the preset dropdown (when open) over everything else, and apply a
    /// selection. Rendered last in `ui()` so it sits above the panels (#24).
    fn draw_preset_menu(&mut self, ui: &mut egui::Ui, painter: &egui::Painter, c: CanvasMap) {
        if self.preset_menu_open && self.edits_locked() {
            // A job started with the menu open — presets are settings (#36).
            self.preset_menu_open = false;
        }
        if !self.preset_menu_open {
            return;
        }
        let presets = Preset::ALL;
        let row_h = 34.0;
        let menu = rect(1700.0, 58.0, 205.0, 12.0 + row_h * presets.len() as f32);
        paint_panel(painter, c, menu, 8, alpha(PANEL_2, 250), BORDER);
        let current = self.session.preset();
        let mut pick = None;
        for (i, preset) in presets.iter().enumerate() {
            let row = rect(
                menu.left() + 8.0,
                menu.top() + 6.0 + row_h * i as f32,
                menu.width() - 16.0,
                row_h - 4.0,
            );
            if self.segment_button(
                ui,
                painter,
                c,
                row,
                preset.name,
                *preset == current,
                ("preset_pick", preset.name),
            ) {
                pick = Some(*preset);
            }
        }
        if let Some(preset) = pick {
            if self.apply_edit("preset", |s| s.select_preset(preset)) {
                self.stop_playback(); // preset changes the render
                self.status = format!("Preset - {}", preset.name);
            }
            self.preset_menu_open = false;
        }
    }

    fn audible_key(&self) -> AudibleCacheKey {
        audible_cache_key(
            self.ab,
            self.volume_match,
            self.reblend_amount,
            self.auditioning_reblend,
            self.audio_generation,
        )
    }

    fn audible_buffer(&mut self) -> Option<&AudioBuffer> {
        let key = self.audible_key();
        if self.audible_cache.as_ref().is_some_and(|c| c.key == key) {
            return self.audible_cache.as_ref().map(|c| &c.buffer);
        }

        let buffer = self
            .session
            .playback_buffer(
                key.playback_source(),
                self.volume_match,
                self.reblend_amount,
            )
            .or_else(|| self.session.input().cloned())?;
        self.audible_cache = Some(AudibleCache::new(key, buffer));
        self.audible_cache.as_ref().map(|c| &c.buffer)
    }

    fn audible_metrics(&mut self) -> Option<AudibleMetrics> {
        let key = self.audible_key();
        let _ = self.audible_buffer()?;
        self.audible_cache
            .as_ref()
            .filter(|cache| cache.key == key)
            .map(|cache| cache.metrics)
    }

    fn spectrogram_for(
        &mut self,
        columns: usize,
        rows: usize,
    ) -> Option<&yesvox_core::monitor::Spectrogram> {
        let key = spectrogram_cache_key(self.audible_key(), columns, rows);
        if self
            .spectrogram_cache
            .as_ref()
            .is_some_and(|cache| cache.key == key)
        {
            return self
                .spectrogram_cache
                .as_ref()
                .map(|cache| &cache.spectrogram);
        }

        let buffer = self.audible_buffer()?.clone();
        let spectrogram = yesvox_core::monitor::spectrogram(&buffer, 0, columns, rows);
        self.spectrogram_cache = Some(SpectrogramCache { key, spectrogram });
        self.spectrogram_cache
            .as_ref()
            .map(|cache| &cache.spectrogram)
    }

    fn set_ab_view(&mut self, ab: AbView) {
        self.ab = ab;
        self.auditioning_reblend = false;
        self.refresh_playback_source();
    }

    fn toggle_ab_view(&mut self) {
        let next = match self.ab {
            AbView::Original => AbView::Processed,
            AbView::Processed | AbView::CompareSnapshot => AbView::Original,
        };
        self.set_ab_view(next);
    }

    fn pin_compare_snapshot(&mut self) {
        if self.session.pin_compare_snapshot() {
            self.bump_audio_generation();
            self.set_ab_view(AbView::CompareSnapshot);
            self.status = "Pinned C snapshot".to_owned();
        } else {
            self.status = "Render before pinning C".to_owned();
        }
    }

    fn clear_compare_snapshot(&mut self) {
        if self.session.compare_snapshot().is_none() {
            return;
        }
        self.session.clear_compare_snapshot();
        self.bump_audio_generation();
        if self.ab == AbView::CompareSnapshot {
            self.set_ab_view(AbView::Processed);
        } else {
            self.refresh_playback_source();
        }
        self.status = "Cleared C snapshot".to_owned();
    }

    /// Play/pause the current audible source, opening the cpal device on first use. A device
    /// failure becomes a status line, never a panic (#23). Switching the source
    /// on an existing player preserves the playhead (instant A/B).
    fn play_pause_audible(&mut self) {
        let key = self.audible_key();
        if self.player.is_some() {
            if should_refresh_player_source(self.player_source_key, key) {
                let Some(buffer) = self.audible_buffer().cloned() else {
                    self.status = "Nothing to play - import a vocal".to_owned();
                    return;
                };
                if let Some(p) = self.player.as_ref() {
                    p.set_source(&buffer);
                }
                self.player_source_key = Some(key);
            }
            if let Some(p) = self.player.as_ref() {
                p.toggle();
            }
            return;
        }

        let Some(buffer) = self.audible_buffer().cloned() else {
            self.status = "Nothing to play - import a vocal".to_owned();
            return;
        };
        match Player::new(&buffer) {
            Ok(p) => {
                p.play();
                self.player = Some(p);
                self.player_source_key = Some(key);
            }
            Err(e) => self.status = format!("Playback unavailable: {e}"),
        }
    }

    /// Swap the audible source into the live player without moving the playhead —
    /// what makes an A/B / Volume Match / reblend change instant. No-op if not
    /// playing anything.
    fn refresh_playback_source(&mut self) {
        if self.player.is_none() {
            return;
        }
        let key = self.audible_key();
        if !should_refresh_player_source(self.player_source_key, key) {
            return;
        }
        let Some(buffer) = self.audible_buffer().cloned() else {
            return;
        };
        if let Some(p) = self.player.as_ref() {
            p.set_source(&buffer);
            self.player_source_key = Some(key);
        }
    }

    /// Stop playback and release the device (new audio invalidates the stream).
    fn stop_playback(&mut self) {
        self.player = None;
        self.player_source_key = None;
    }

    /// Whether a background job is running. Only one foreground job at a time —
    /// new triggers are ignored while busy (the render path even moves the
    /// session onto the worker, so it must not be touched concurrently).
    fn busy(&self) -> bool {
        self.render_job.is_some() || self.export_job.is_some() || self.batch_job.is_some()
    }

    /// Drain background-job events once per frame and adopt their results. Must
    /// run at the top of `ui()`; requests a repaint while any job is live so the
    /// progress bar keeps moving without user input.
    fn pump_jobs(&mut self, ctx: &egui::Context) {
        // A dead audio stream (device unplugged mid-playback) must not leave a
        // frozen play icon: surface the error and release the player.
        if let Some(err) = self.player.as_ref().and_then(Player::take_error) {
            self.status = format!("Playback stopped: {err}");
            self.player = None;
        }

        // Render: the session was moved onto the worker; adopt it back on Done.
        if let Some((events, finished)) = self
            .render_job
            .as_mut()
            .map(|j| (j.poll(), j.is_finished()))
        {
            for ev in events {
                match ev {
                    JobEvent::Progress(p) => self.job_progress = p,
                    JobEvent::Done(done) => {
                        self.session = done.session;
                        self.bump_audio_generation();
                        self.status = done.status;
                        self.render_recovery = None;
                        if done.ok {
                            self.ab = AbView::Processed;
                            self.isolation_check = None;
                            // The processed audio changed — drop the stale stream.
                            self.player = None;
                        }
                    }
                    JobEvent::Failed(msg) => {
                        // Only a panic reaches here — and the session, which was
                        // moved onto the worker, was lost with it. Rebuild it so
                        // the user's file and tuning survive the crash.
                        self.status = format!("Render crashed: {msg}");
                        self.recover_session_after_crash();
                    }
                }
            }
            if finished {
                self.render_job = None;
            }
        }

        // Export: read-only; the session stayed put.
        if let Some((events, finished)) = self
            .export_job
            .as_mut()
            .map(|j| (j.poll(), j.is_finished()))
        {
            for ev in events {
                match ev {
                    JobEvent::Progress(p) => self.job_progress = p,
                    JobEvent::Done(path) => self.status = format!("Exported - {}", path.display()),
                    JobEvent::Failed(msg) => self.status = format!("Export failed: {msg}"),
                }
            }
            if finished {
                self.export_job = None;
            }
        }

        // Batch: apply per-row status updates on Done.
        if let Some((events, finished)) =
            self.batch_job.as_mut().map(|j| (j.poll(), j.is_finished()))
        {
            for ev in events {
                match ev {
                    JobEvent::Progress(p) => self.job_progress = p,
                    JobEvent::Done(done) => {
                        for (row, status) in done.updates {
                            if let Some(r) = self.batch_queue.rows.get_mut(row) {
                                r.status = status;
                            }
                        }
                        self.status = format_batch_status(&done.summary);
                    }
                    JobEvent::Failed(msg) => self.status = format!("Batch failed: {msg}"),
                }
            }
            if finished {
                self.batch_job = None;
            }
        }

        // Keep repainting while a job runs or audio is playing so progress bars,
        // the playhead and the position readout advance without user input.
        if self.busy() || self.player.as_ref().is_some_and(|p| p.is_playing()) {
            ctx.request_repaint();
        }
    }

    fn import(&mut self) {
        let path = rfd::FileDialog::new()
            .add_filter("Audio", &["wav", "wave", "flac"])
            .set_title("Import Vocal")
            .pick_file();
        let Some(path) = path else {
            self.status = "Import cancelled".to_owned();
            return;
        };
        self.open_path(path);
    }

    fn open_path(&mut self, path: PathBuf) {
        if self.busy() {
            // The render path moves the session onto a worker; loading a file
            // into the placeholder now would be discarded when the render's
            // session is adopted back. Refuse until the job finishes (#22).
            self.status = "Working - please wait".to_owned();
            return;
        }
        match self.session.open(&path) {
            Ok(_) => {
                self.stop_playback(); // new audio invalidates the current stream
                self.view_mode = ViewMode::Standard;
                self.show_standard_controls = false;
                self.preset_menu_open = false;
                self.source_path = Some(path.clone());
                self.auditioning_reblend = false;
                self.source_name = path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("Vocal.wav")
                    .to_owned();
                let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("vocal");
                self.name_input = format!("{stem}_YesVox");
                self.session.set_output_name(&self.name_input);
                if let Some(parent) = path.parent() {
                    self.session.set_output_location(parent);
                }
                self.ab = AbView::Original;
                self.bump_audio_generation();
                self.render_now();
            }
            Err(e) => self.status = format!("Import failed: {e}"),
        }
    }

    fn render_now(&mut self) {
        if self.busy() {
            self.status = "Working - please wait".to_owned();
            return;
        }
        // Stop playback: the processed audio is about to change under it (#23).
        self.stop_playback();
        self.bump_audio_generation();
        // Move the session onto a worker thread; the UI stays responsive and
        // shows a processing state until it comes back rendered (#22). Keep a
        // settings snapshot: if the worker panics, the session is lost with it
        // and is rebuilt from the source path + this snapshot.
        self.render_recovery = Some(self.session.snapshot());
        let session = std::mem::replace(&mut self.session, Session::new());
        self.job_progress = 0.0;
        self.status = "Analyzing...".to_owned();
        self.render_job = Some(spawn(move |ctx| {
            ctx.progress(0.05);
            let mut session = session;
            let (ok, status) = match session.render_with_cancel(|| ctx.is_canceled()) {
                Ok(()) => {
                    let lufs = session.output_lufs().unwrap_or(f32::NAN);
                    (true, format!("Rendered - OUT {lufs:.1} LUFS"))
                }
                Err(SessionError::Canceled) => (
                    false,
                    canceled_render_status(session.output().is_some()).to_owned(),
                ),
                Err(e) => (false, format!("Render failed: {e}")),
            };
            ctx.progress(1.0);
            Ok(RenderDone {
                session,
                ok,
                status,
            })
        }));
    }

    fn cancel_render(&mut self) {
        if let Some(job) = self.render_job.as_ref() {
            job.cancel();
            self.status = "Render cancel requested".to_owned();
        } else {
            self.status = "No render running".to_owned();
        }
    }

    /// Apply the global keyboard shortcuts: undo/redo (#25) and Esc = cancel
    /// the live render (#36). Esc is a no-op when no render is running so the
    /// key doesn't spray status noise.
    fn apply_shortcuts(&mut self, do_undo: bool, do_redo: bool, do_cancel: bool) {
        if do_undo {
            self.undo();
        }
        if do_redo {
            self.redo();
        }
        if do_cancel && self.render_job.is_some() {
            self.cancel_render();
        }
    }

    /// Rebuild the session after a render-worker panic (the session was moved
    /// onto the worker and died with it): re-open the source file and restore
    /// the pre-render settings snapshot, so the user loses the crashed render —
    /// not their file, tuning, or undo history.
    fn recover_session_after_crash(&mut self) {
        let mut session = Session::new();
        if let Some(path) = self.source_path.clone() {
            match session.open(&path) {
                Ok(_) => {
                    if let Some(parent) = path.parent() {
                        session.set_output_location(parent);
                    }
                }
                Err(e) => self.status = format!("{} (reimport failed: {e})", self.status),
            }
        }
        if let Some(snapshot) = self.render_recovery.take() {
            session.restore_snapshot(&snapshot);
        }
        session.set_output_name(&self.name_input);
        self.session = session;
        self.player = None;
        self.bump_audio_generation();
        self.isolation_check = None;
    }

    fn queue_current_vocal(&mut self) {
        let Some(input) = self.source_path.clone() else {
            self.status = "Queue needs an imported vocal".to_owned();
            return;
        };
        let output = self
            .session
            .export_target()
            .unwrap_or_else(|| batch_output_path(&input, self.session.output_format()));
        if self.batch_queue.enqueue(input, output) {
            self.status = format!("Queued - {}", self.batch_queue.summary_label());
        } else {
            self.status = "Already queued".to_owned();
        }
    }

    fn run_batch_queue(&mut self) {
        if self.busy() {
            self.status = "Working - please wait".to_owned();
            return;
        }
        if self.batch_queue.paused {
            self.status = "Batch queue is paused".to_owned();
            return;
        }
        if self.batch_queue.pending_count() == 0 {
            self.status = "No pending batch items".to_owned();
            return;
        }

        let preset_mode = self.batch_queue.preset_mode(self.session.preset());
        let mut queue = match preset_mode {
            BatchPresetMode::Force(preset) => {
                RenderQueue::new(self.session.settings().clone(), preset)
            }
            BatchPresetMode::DetectPerFile { fallback } => {
                RenderQueue::detect_per_file(self.session.settings().clone(), fallback)
            }
        };
        let mut pending_indices = Vec::new();
        for (index, row) in self.batch_queue.rows.iter().enumerate() {
            if matches!(row.status, JobStatus::Pending) {
                queue.enqueue(row.input.clone(), row.output.clone());
                pending_indices.push(index);
            }
        }

        // Run the batch off the UI thread; real per-file progress + cooperative
        // cancel flow through the worker (#22).
        self.job_progress = 0.0;
        self.status = "Batch running...".to_owned();
        self.batch_job = Some(spawn(move |ctx| {
            let mut queue = queue;
            let summary = queue.run_with_progress(|progress| {
                ctx.progress(progress.completed as f32 / progress.total.max(1) as f32);
                if ctx.is_canceled() {
                    BatchControl::Cancel
                } else {
                    BatchControl::Continue
                }
            });
            let updates = queue
                .items()
                .iter()
                .zip(pending_indices)
                .map(|(item, row_index)| (row_index, item.status.clone()))
                .collect();
            Ok(BatchDone { updates, summary })
        }));
    }

    fn check_isolation_now(&mut self) {
        match self.session.vocal_isolation_check() {
            Ok(check) => {
                self.status = format!("Isolation check: {}", check.summary());
                self.isolation_check = Some(check);
            }
            Err(e) => self.status = format!("Isolation check failed: {e}"),
        }
    }

    fn input_duration(&self) -> Option<f32> {
        let input = self.session.input()?;
        Some(input.frames() as f32 / input.sample_rate().max(1) as f32)
    }

    fn duration_label(&self) -> String {
        self.input_duration()
            .map(format_duration)
            .unwrap_or_else(|| "--:--".to_owned())
    }

    fn source_format_label(&self) -> String {
        self.source_path
            .as_ref()
            .and_then(|p| p.extension())
            .and_then(|s| s.to_str())
            .map(|s| s.to_ascii_uppercase())
            .unwrap_or_else(|| "WAV".to_owned())
    }

    fn sample_rate_label(&self) -> String {
        self.session
            .input()
            .map(|b| format!("{:.1} kHz", b.sample_rate() as f32 / 1000.0))
            .unwrap_or_else(|| "-- kHz".to_owned())
    }

    fn bit_depth_label(&self) -> String {
        // Show the export override when set, else the source's depth.
        self.session
            .settings()
            .export
            .bit_depth_override
            .or_else(|| self.session.input().map(|b| b.bits_per_sample()))
            .map(|b| format!("{b}-bit"))
            .unwrap_or_else(|| "--bit".to_owned())
    }

    fn channel_label(&self) -> &'static str {
        match self.session.input().map(AudioBuffer::channel_count) {
            Some(1) => "Mono",
            Some(2) => "Stereo",
            _ => "--",
        }
    }

    fn processing_label(&self) -> &'static str {
        if self.render_job.is_some() {
            "Analyzing"
        } else if self.export_job.is_some() {
            "Exporting"
        } else if self.batch_job.is_some() {
            "Batch"
        } else if self.session.output().is_some() {
            "Rendered"
        } else if self.session.input().is_some() {
            "Loaded"
        } else {
            "Ready"
        }
    }

    /// Real progress fraction for the processing bars (replaces the old
    /// 0/0.55/1.0 heuristic): the live job's reported progress while busy,
    /// otherwise 1.0 once rendered and 0.0 when idle.
    fn processing_progress(&self) -> f32 {
        if self.busy() {
            self.job_progress
        } else if self.session.output().is_some() {
            1.0
        } else {
            0.0
        }
    }
}

impl eframe::App for YesVoxApp {
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        // Persistence-lite (#24): remember the last output dir + format.
        let dir = self
            .session
            .output_location()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        storage.set_string(STORAGE_LAST_DIR, dir);
        storage.set_string(
            STORAGE_LAST_FORMAT,
            self.session.output_format().label().to_string(),
        );
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.draw_screen(ui);
    }
}

impl YesVoxApp {
    fn draw_screen(&mut self, ui: &mut egui::Ui) {
        // Drain background-job events and repaint while work is in flight (#22).
        self.pump_jobs(ui.ctx());
        self.import_dropped_vocal(ui);
        // Undo/redo shortcuts (#25): Ctrl/Cmd+Z, Ctrl/Cmd+Shift+Z, Ctrl/Cmd+Y.
        // Esc cancels a live render from any view (#36).
        let (do_undo, do_redo, do_cancel) = ui.input(|i| {
            let cmd = i.modifiers.command;
            let undo = cmd && !i.modifiers.shift && i.key_pressed(egui::Key::Z);
            let redo = cmd
                && ((i.modifiers.shift && i.key_pressed(egui::Key::Z))
                    || (!i.modifiers.shift && i.key_pressed(egui::Key::Y)));
            (undo, redo, i.key_pressed(egui::Key::Escape))
        });
        self.apply_shortcuts(do_undo, do_redo, do_cancel);
        let available = ui.max_rect();
        let canvas = CanvasMap::new(available);
        let painter = ui.painter().clone();

        painter.rect_filled(available, egui::CornerRadius::ZERO, BG_BOTTOM);
        paint_v_gradient(&painter, canvas.rect, BG_TOP, BG_BOTTOM, 72);
        if !self.has_vocal_workspace() {
            self.draw_import_screen(ui, &painter, canvas);
            self.draw_build_stamp(&painter, canvas);
            return;
        }

        let layout = ReferenceLayout::new();
        self.draw_top_bar(ui, &painter, canvas, layout);
        match self.view_mode {
            ViewMode::Standard if self.show_standard_controls => {
                self.draw_left_sidebar(ui, &painter, canvas, layout);
                self.draw_center(ui, &painter, canvas, layout);
                self.draw_right_sidebar(ui, &painter, canvas, layout);
                self.draw_status_bar(&painter, canvas, layout);
            }
            ViewMode::Standard => {
                self.draw_standard_workspace(ui, &painter, canvas, layout);
            }
            ViewMode::Advanced => {
                let advanced = AdvancedLayout::new();
                self.draw_advanced(ui, &painter, canvas, advanced);
                self.draw_status_bar(&painter, canvas, ReferenceLayout::new());
            }
        }
        // Always-visible build stamp (#20/W1), painted over both views.
        self.draw_build_stamp(&painter, canvas);
        self.draw_preset_menu(ui, &painter, canvas); // overlay, drawn last
    }
}

impl YesVoxApp {
    /// Dedicated drag target for the borderless shell. It occupies the quiet
    /// center-right title-bar gap and never overlays an application control.
    fn top_bar_drag_rect() -> egui::Rect {
        rect(1122.0, 4.0, 248.0, 60.0)
    }

    fn top_bar_presets_rect() -> egui::Rect {
        rect(1570.0, 14.0, 130.0, 40.0)
    }

    /// Minimize, maximize/restore, and close targets for the undecorated
    /// viewport. Keeping these as geometry helpers makes reachability and
    /// collision checks mechanical.
    fn top_bar_window_control_rects() -> [egui::Rect; 3] {
        [
            rect(1780.0, 14.0, 38.0, 40.0),
            rect(1824.0, 14.0, 38.0, 40.0),
            rect(1868.0, 14.0, 38.0, 40.0),
        ]
    }

    /// Top-bar render progress readout (#36). Lives in the bar drawn by every
    /// view, so a render is visibly in flight from Advanced too.
    fn top_bar_render_progress_rect() -> egui::Rect {
        rect(1566.0, 14.0, 152.0, 40.0)
    }

    /// Top-bar render cancel button (#36) — the view-independent way to stop
    /// a render (the Standard sidebar keeps its own affordance).
    fn top_bar_render_cancel_rect() -> egui::Rect {
        rect(1726.0, 14.0, 38.0, 40.0)
    }

    fn draw_top_bar(
        &mut self,
        ui: &mut egui::Ui,
        painter: &egui::Painter,
        c: CanvasMap,
        l: ReferenceLayout,
    ) {
        paint_panel(painter, c, l.top, 0, alpha(PANEL, 248), BORDER_DIM);
        self.draw_wordmark(painter, c, egui::pos2(18.0, 35.0));

        let drag_response = ui.interact(
            c.rect(Self::top_bar_drag_rect()),
            ui.make_persistent_id("window_drag_region"),
            egui::Sense::click_and_drag(),
        );
        if drag_response.drag_started() {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
        if drag_response.double_clicked() {
            let maximized = ui.input(|input| input.viewport().maximized.unwrap_or(false));
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
        }

        let [standard_rect, advanced_rect] = Self::top_bar_view_rects();
        paint_panel(
            painter,
            c,
            standard_rect.union(advanced_rect),
            8,
            FIELD,
            BORDER_DIM,
        );
        if self.segment_button(
            ui,
            painter,
            c,
            standard_rect,
            if self.show_standard_controls && self.view_mode == ViewMode::Standard {
                "Back to Standard"
            } else {
                "Standard"
            },
            self.view_mode == ViewMode::Standard && !self.show_standard_controls,
            "view_standard",
        ) {
            self.view_mode = ViewMode::Standard;
            self.show_standard_controls = false;
            self.preset_menu_open = false;
            self.leave_special_audition();
        }
        if self.segment_button(
            ui,
            painter,
            c,
            advanced_rect,
            "Advanced",
            self.view_mode == ViewMode::Advanced,
            "view_advanced",
        ) {
            self.view_mode = ViewMode::Advanced;
            self.show_standard_controls = false;
        }

        // Undo/redo (#25): enabled state reflects real stack depth; clicks drive
        // the tested History.
        if self.icon_button(
            ui,
            painter,
            c,
            rect(1386.0, 14.0, 38.0, 40.0),
            "Undo",
            self.history.can_undo() && !self.edits_locked(),
        ) {
            self.undo();
        }
        if self.icon_button(
            ui,
            painter,
            c,
            rect(1434.0, 14.0, 38.0, 40.0),
            "Redo",
            self.history.can_redo() && !self.edits_locked(),
        ) {
            self.redo();
        }
        if (self.view_mode == ViewMode::Advanced || self.show_standard_controls)
            && self.outline_button(
                ui,
                painter,
                c,
                rect(1485.0, 14.0, 68.0, 40.0),
                "A / B",
                "top_ab",
            )
        {
            self.toggle_ab_view(); // instant A/B at the shared playhead
        }
        // View-independent render progress + cancel (#36): the Standard
        // sidebar's cancel is unreachable from Advanced; this one is not.
        if self.render_job.is_some() {
            let progress_rect = Self::top_bar_render_progress_rect();
            paint_panel(painter, c, progress_rect, 6, FIELD, BORDER_DIM);
            painter.text(
                c.rect(progress_rect).center(),
                egui::Align2::CENTER_CENTER,
                format!("Rendering {:.0}%", self.processing_progress() * 100.0),
                c.font(13.0),
                CYAN,
            );
            if self.outline_button(
                ui,
                painter,
                c,
                Self::top_bar_render_cancel_rect(),
                "x",
                "top_render_cancel",
            ) {
                self.cancel_render();
            }
        }
        // Preset picker (#24): hidden while a render owns this lane because
        // settings are locked and the progress/cancel controls take priority.
        // The compact Standard workspace has its own preset picker.
        if self.render_job.is_none()
            && (self.view_mode == ViewMode::Advanced || self.show_standard_controls)
            && self.outline_button(
                ui,
                painter,
                c,
                Self::top_bar_presets_rect(),
                "Presets v",
                "top_presets",
            )
        {
            self.preset_menu_open = !self.preset_menu_open;
        }

        let [minimize, maximize, close] = Self::top_bar_window_control_rects();
        if self.outline_button(ui, painter, c, minimize, "-", "window_minimize") {
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::Minimized(true));
        }
        if self.outline_button(ui, painter, c, maximize, "[]", "window_maximize") {
            let maximized = ui.input(|input| input.viewport().maximized.unwrap_or(false));
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
        }
        if self.outline_button(ui, painter, c, close, "x", "window_close") {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    fn draw_left_sidebar(
        &mut self,
        ui: &mut egui::Ui,
        painter: &egui::Painter,
        c: CanvasMap,
        l: ReferenceLayout,
    ) {
        paint_panel(painter, c, l.left, 0, alpha(PANEL, 245), BORDER_DIM);
        self.section_label(painter, c, 28.0, 94.0, "VOCAL ORDER");
        let duration = self.duration_label();
        self.small_text(
            painter,
            c,
            28.0,
            118.0,
            &format!("1 vocal - {duration}"),
            TEXT_MUTED,
        );
        paint_panel(
            painter,
            c,
            rect(18.0, 142.0, 256.0, 64.0),
            6,
            alpha(BLUE, 28),
            alpha(BLUE, 145),
        );
        painter.rect_filled(
            c.rect(rect(18.0, 142.0, 3.0, 64.0)),
            egui::CornerRadius::same(2),
            CYAN,
        );
        self.body_text(painter, c, 38.0, 171.0, "01", BLUE);
        self.body_text(painter, c, 72.0, 161.0, &self.source_name, TEXT);
        self.small_text(painter, c, 72.0, 184.0, &duration, CYAN);
        self.draw_mini_wave(painter, c, rect(224.0, 162.0, 28.0, 22.0));
        if self.outline_button(
            ui,
            painter,
            c,
            rect(18.0, 220.0, 256.0, 48.0),
            "+   Add Vocal",
            "add_vocal",
        ) {
            self.import();
        }

        painter.line_segment(
            [c.pos(28.0, 350.0), c.pos(264.0, 350.0)],
            egui::Stroke::new(c.v(1.0), BORDER_DIM),
        );
        self.section_label(painter, c, 28.0, 382.0, "VOICE TYPE");
        let mode = self.session.settings().content_mode;
        if self.mode_button(
            ui,
            painter,
            c,
            rect(24.0, 402.0, 112.0, 88.0),
            "Mic",
            "Voiceover",
            mode == ContentMode::Voiceover,
            "mode_voice",
        ) {
            // Undoable — it resets the rack (#25).
            if self.apply_edit("content_mode", |s| {
                s.set_content_mode(ContentMode::Voiceover)
            }) {
                self.stop_playback();
            }
        }
        if self.mode_button(
            ui,
            painter,
            c,
            rect(150.0, 402.0, 112.0, 88.0),
            "Note",
            "Singing",
            mode == ContentMode::Singing,
            "mode_sing",
        ) {
            // Undoable — it resets the rack (#25).
            if self.apply_edit("content_mode", |s| s.set_content_mode(ContentMode::Singing)) {
                self.stop_playback();
            }
        }

        paint_panel(
            painter,
            c,
            rect(18.0, 515.0, 256.0, 256.0),
            8,
            alpha(PANEL_2, 230),
            BORDER_DIM,
        );
        self.body_text(painter, c, 78.0, 566.0, "SMART ASSIST", TEXT);
        self.badge(painter, c, rect(226.0, 546.0, 30.0, 28.0), "AI", false);
        self.star(painter, c, egui::pos2(56.0, 556.0), 13.0, CYAN);
        self.body_text(
            painter,
            c,
            44.0,
            622.0,
            "Analyzes your vocal and",
            TEXT_MUTED,
        );
        self.body_text(
            painter,
            c,
            44.0,
            650.0,
            "builds the best settings",
            TEXT_MUTED,
        );
        self.body_text(
            painter,
            c,
            44.0,
            678.0,
            "for a clean, natural result.",
            TEXT_MUTED,
        );
        if self.gradient_button(
            ui,
            painter,
            c,
            rect(44.0, 704.0, 212.0, 54.0),
            "Analyze Vocal",
            "analyze_vocal",
        ) {
            self.render_now();
        }

        paint_panel(
            painter,
            c,
            rect(18.0, 890.0, 256.0, 66.0),
            6,
            alpha(PANEL_2, 220),
            BORDER_DIM,
        );
        self.small_text(painter, c, 34.0, 918.0, self.processing_label(), CYAN);
        let progress = self.processing_progress();
        self.small_text(
            painter,
            c,
            if self.render_job.is_some() {
                190.0
            } else {
                218.0
            },
            918.0,
            &format!("{:.0}%", progress * 100.0),
            CYAN,
        );
        if self.render_job.is_some()
            && self.outline_button(
                ui,
                painter,
                c,
                rect(230.0, 902.0, 22.0, 22.0),
                "x",
                "render_cancel",
            )
        {
            self.cancel_render();
        }
        self.progress_bar(painter, c, rect(34.0, 934.0, 218.0, 6.0), progress);
        if self.gradient_button(
            ui,
            painter,
            c,
            rect(18.0, 972.0, 256.0, 58.0),
            "Import Vocal",
            "import_vocal",
        ) {
            self.import();
        }
    }

    fn draw_center(
        &mut self,
        ui: &mut egui::Ui,
        painter: &egui::Painter,
        c: CanvasMap,
        l: ReferenceLayout,
    ) {
        let _center_bounds = l.center;
        paint_panel(painter, c, l.hero, 10, alpha(PANEL, 232), BORDER_DIM);
        self.title_text(painter, c, 322.0, 104.0, &self.source_name);
        self.small_text(
            painter,
            c,
            322.0,
            132.0,
            &self.source_format_label(),
            TEXT_MUTED,
        );
        self.small_text(painter, c, 370.0, 132.0, "|", TEXT_DIM);
        self.small_text(
            painter,
            c,
            392.0,
            132.0,
            &self.sample_rate_label(),
            TEXT_MUTED,
        );
        self.small_text(painter, c, 456.0, 132.0, "|", TEXT_DIM);
        self.small_text(
            painter,
            c,
            478.0,
            132.0,
            &self.bit_depth_label(),
            TEXT_MUTED,
        );
        self.small_text(painter, c, 538.0, 132.0, "|", TEXT_DIM);
        self.small_text(painter, c, 560.0, 132.0, self.channel_label(), TEXT_MUTED);
        self.small_text(painter, c, 610.0, 132.0, "|", TEXT_DIM);
        self.small_text(painter, c, 632.0, 132.0, &self.duration_label(), TEXT_MUTED);

        self.small_text(painter, c, 782.0, 116.0, "FLOW AMOUNT", TEXT_MUTED);
        let mut flow = self.session.flow_amount();
        flow = self.slider(
            ui,
            painter,
            c,
            rect(902.0, 111.0, 370.0, 10.0),
            flow,
            "flow_amount",
            BLUE,
            PURPLE,
        );
        if (flow - self.session.flow_amount()).abs() > f32::EPSILON {
            self.apply_edit("flow", |s| s.set_flow_amount(flow));
        }
        self.body_text(painter, c, 1288.0, 120.0, &format!("x{flow:.2}"), TEXT);

        let ab_rect = rect(1368.0, 86.0, 188.0, 34.0);
        paint_panel(painter, c, ab_rect, 6, FIELD, BORDER_DIM);
        if self.segment_button(
            ui,
            painter,
            c,
            rect(1368.0, 86.0, 94.0, 34.0),
            "Original",
            self.ab == AbView::Original,
            "ab_original",
        ) {
            self.set_ab_view(AbView::Original);
        }
        if self.segment_button(
            ui,
            painter,
            c,
            rect(1462.0, 86.0, 94.0, 34.0),
            "Processed",
            self.ab == AbView::Processed,
            "ab_processed",
        ) {
            self.set_ab_view(AbView::Processed);
        }

        // One unified listening deck, matching the suite's studio-hardware
        // hierarchy: title/insight above, waveform + meters + transport within.
        paint_panel(
            painter,
            c,
            rect(314.0, 140.0, 1228.0, 272.0),
            9,
            alpha(FIELD, 245),
            BORDER_DIM,
        );
        let wave_rect = rect(322.0, 148.0, 1032.0, 160.0);
        paint_panel(painter, c, wave_rect, 5, FIELD, BORDER);
        self.draw_waveform(painter, c, wave_rect);
        // Click/drag the waveform to seek (#23).
        if self.player.is_some() {
            let resp = ui.interact(
                c.rect(wave_rect),
                ui.make_persistent_id("waveform_seek"),
                egui::Sense::click_and_drag(),
            );
            if resp.clicked() || resp.dragged() {
                if let Some(pos) = resp.interact_pointer_pos() {
                    let sr = c.rect(wave_rect);
                    let f = ((pos.x - sr.left()) / sr.width()).clamp(0.0, 1.0);
                    if let Some(p) = &self.player {
                        p.seek_fraction(f);
                    }
                }
            }
        }
        self.draw_time_labels(painter, c, wave_rect);
        self.draw_meter_panel(painter, c, rect(1370.0, 148.0, 166.0, 160.0));
        self.draw_transport(ui, painter, c);

        self.draw_rack_panel(ui, painter, c, l.restore, Tab::Restore);
        self.draw_rack_panel(ui, painter, c, l.polish, Tab::Polish);
        self.draw_monitor_strip(ui, painter, c, l.monitor);
    }

    fn draw_advanced(
        &mut self,
        ui: &mut egui::Ui,
        painter: &egui::Painter,
        c: CanvasMap,
        l: AdvancedLayout,
    ) {
        let _chrome = (l.top, l.status);
        paint_panel(painter, c, l.work, 8, alpha(PANEL, 186), BORDER_DIM);

        paint_panel(painter, c, l.spectrogram, 8, alpha(PANEL_2, 230), BORDER);
        self.section_label(
            painter,
            c,
            l.spectrogram.left() + 20.0,
            l.spectrogram.top() + 28.0,
            "SPECTROGRAM",
        );
        self.small_text(
            painter,
            c,
            l.spectrogram.left() + 140.0,
            l.spectrogram.top() + 28.0,
            "full vocal view - brush heal is post-v1",
            TEXT_MUTED,
        );
        let spec = rect(
            l.spectrogram.left() + 20.0,
            l.spectrogram.top() + 52.0,
            l.spectrogram.width() - 40.0,
            l.spectrogram.height() - 76.0,
        );
        let (columns, rows) = advanced_spectrogram_dimensions(spec);
        {
            let spectrogram = self.spectrogram_for(columns, rows);
            draw_advanced_spectrogram(painter, c, spec, spectrogram);
        }
        self.draw_time_labels(painter, c, spec);

        let group_titles = [
            ("RESTORE", "repair damage and room"),
            ("POLISH", "sibilance and breath"),
            ("FINISH", "presence, rider, loudness"),
            ("MONITOR", "A/B/C, difference, reblend"),
            ("EXPORT + SAFETY", "format, target, memory"),
        ];
        for (panel, (title, subtitle)) in l.group_panels.iter().zip(group_titles) {
            paint_panel(painter, c, *panel, 8, alpha(PANEL, 218), BORDER_DIM);
            self.section_label(painter, c, panel.left() + 18.0, panel.top() + 28.0, title);
            self.micro_text(
                painter,
                c,
                panel.left() + 18.0,
                panel.top() + 48.0,
                subtitle,
                TEXT_MUTED,
            );
        }

        let modules = [
            ("1", "De-bleed", "Mix-likeness", "Warn 0.02 / Block 0.15"),
            ("2", "Declip", "Threshold 0.98", "Strength 1.00"),
            ("3", "De-click", "Click repair", "Mouth clicks split"),
            ("4", "De-pop", "Low-cut 120 Hz", "Strength 1.00"),
            ("5", "De-hum", "Auto 50/60 Hz", "Track on"),
            ("6", "Denoise", "Balanced", "Neural -> light fallback"),
            ("7", "De-reverb", "RT60 readout", "Tail preserve"),
            ("8", "De-ess", "Split / Wide", "HF reduction readout"),
            ("9", "Breath", "Attenuation dB", "De-ess sidechain"),
            ("10", "Finish", "Presence + leveling", "Loudness limiter"),
        ];
        for (i, (card, _module)) in l.module_cards.iter().zip(modules).enumerate() {
            if i == 0 {
                self.advanced_debleed_card(ui, painter, c, *card);
            } else if i == 1 {
                self.advanced_declip_card(ui, painter, c, *card);
            } else if i == 2 {
                self.advanced_declick_card(ui, painter, c, *card);
            } else if i == 3 {
                self.advanced_depop_card(ui, painter, c, *card);
            } else if i == 4 {
                self.advanced_dehum_card(ui, painter, c, *card);
            } else if i == 5 {
                self.advanced_denoise_card(ui, painter, c, *card);
            } else if i == 6 {
                self.advanced_dereverb_card(ui, painter, c, *card);
            } else if i == 7 {
                self.advanced_deess_card(ui, painter, c, *card);
            } else if i == 8 {
                self.advanced_breath_card(ui, painter, c, *card);
            } else {
                self.advanced_finish_card(ui, painter, c, *card);
            }
        }

        let compare_pinned = self.session.compare_snapshot().is_some();
        let monitor_y = l.monitor.top() + 78.0;
        if self.segment_button(
            ui,
            painter,
            c,
            rect(l.monitor.left() + 24.0, monitor_y, 108.0, 32.0),
            "A Original",
            self.ab == AbView::Original,
            "advanced_ab_original",
        ) {
            self.set_ab_view(AbView::Original);
        }
        if self.segment_button(
            ui,
            painter,
            c,
            rect(l.monitor.left() + 132.0, monitor_y, 116.0, 32.0),
            "B Processed",
            self.ab == AbView::Processed,
            "advanced_ab_processed",
        ) {
            self.set_ab_view(AbView::Processed);
        }
        let compare_rect = rect(l.monitor.left() + 248.0, monitor_y, 116.0, 32.0);
        if compare_pinned {
            if self.segment_button(
                ui,
                painter,
                c,
                compare_rect,
                "C Snapshot",
                self.ab == AbView::CompareSnapshot,
                "advanced_ab_compare",
            ) {
                self.set_ab_view(AbView::CompareSnapshot);
            }
        } else {
            paint_panel(painter, c, compare_rect, 5, FIELD, BORDER_DIM);
            self.small_text(
                painter,
                c,
                compare_rect.left() + 18.0,
                compare_rect.top() + 21.0,
                "C Empty",
                TEXT_DIM,
            );
        }
        if self.outline_button(
            ui,
            painter,
            c,
            rect(l.monitor.left() + 378.0, monitor_y, 72.0, 32.0),
            "Pin C",
            "advanced_pin_c",
        ) {
            self.pin_compare_snapshot();
        }
        if compare_pinned {
            if self.outline_button(
                ui,
                painter,
                c,
                rect(l.monitor.left() + 462.0, monitor_y, 68.0, 32.0),
                "Clear",
                "advanced_clear_c",
            ) {
                self.clear_compare_snapshot();
            }
        } else {
            paint_panel(
                painter,
                c,
                rect(l.monitor.left() + 462.0, monitor_y, 68.0, 32.0),
                5,
                alpha(FIELD, 100),
                BORDER_DIM,
            );
            self.micro_text(
                painter,
                c,
                l.monitor.left() + 481.0,
                monitor_y + 20.0,
                "Clear",
                TEXT_DIM,
            );
        }
        self.small_text(
            painter,
            c,
            l.monitor.left() + 24.0,
            l.monitor.top() + 132.0,
            "Volume Match",
            if self.volume_match { CYAN } else { TEXT_MUTED },
        );
        self.small_text(
            painter,
            c,
            l.monitor.left() + 24.0,
            l.monitor.top() + 164.0,
            "Reblend",
            TEXT_MUTED,
        );
        let reblend_bar = rect(
            l.monitor.left() + 108.0,
            l.monitor.top() + 158.0,
            260.0,
            8.0,
        );
        paint_panel(
            painter,
            c,
            reblend_bar,
            4,
            FIELD,
            egui::Color32::TRANSPARENT,
        );
        paint_h_gradient(
            painter,
            c,
            rect(
                reblend_bar.left(),
                reblend_bar.top(),
                reblend_bar.width() * self.reblend_amount.clamp(0.0, 1.0),
                reblend_bar.height(),
            ),
            BLUE,
            CYAN,
            16,
            4,
        );
        self.draw_spectrum(
            painter,
            c,
            rect(
                l.monitor.left() + 430.0,
                l.monitor.top() + 82.0,
                402.0,
                158.0,
            ),
            self.session.difference().as_ref(),
        );

        let export_x = l.export.left() + 24.0;
        self.body_text(
            painter,
            c,
            export_x,
            l.export.top() + 88.0,
            &format!("Format  {}", self.session.output_format().label()),
            TEXT,
        );
        self.body_text(
            painter,
            c,
            export_x,
            l.export.top() + 122.0,
            &format!("Bit depth  {}", self.bit_depth_label()),
            TEXT,
        );
        self.body_text(
            painter,
            c,
            export_x,
            l.export.top() + 156.0,
            &format!("Target  {}", loudness_label(self.session.loudness_target())),
            TEXT,
        );
        self.small_text(
            painter,
            c,
            export_x,
            l.export.top() + 202.0,
            "Memory preflight enforced before render",
            TEXT_MUTED,
        );
    }

    fn advanced_debleed_card(
        &mut self,
        ui: &mut egui::Ui,
        painter: &egui::Painter,
        c: CanvasMap,
        r: egui::Rect,
    ) {
        self.advanced_card_header(painter, c, r, "1", "De-bleed");
        let controls = advanced_debleed_control_rects(r);
        let original = AdvancedDeBleedPanel::from_session(&self.session);
        let mut panel = original;
        self.micro_text(
            painter,
            c,
            r.left() + 144.0,
            r.top() + 25.0,
            panel.source_label(),
            if panel.source_imported {
                CYAN
            } else {
                TEXT_MUTED
            },
        );

        if self.segment_button(
            ui,
            painter,
            c,
            controls.native_source,
            "Native",
            !panel.source_imported,
            "advanced_debleed_native",
        ) {
            panel.source_imported = false;
        }
        if self.segment_button(
            ui,
            painter,
            c,
            controls.imported_source,
            "Imported",
            panel.source_imported,
            "advanced_debleed_imported",
        ) {
            panel.source_imported = true;
        }

        self.micro_text(
            painter,
            c,
            r.left() + 12.0,
            r.top() + 79.0,
            &format!("Warn {:.2}", panel.warn_threshold),
            TEXT_MUTED,
        );
        panel.warn_threshold = self.slider(
            ui,
            painter,
            c,
            controls.warn_slider,
            panel.warn_threshold,
            "advanced_debleed_warn",
            AMBER,
            CYAN,
        );
        self.micro_text(
            painter,
            c,
            r.left() + 12.0,
            r.top() + 102.0,
            &format!("Block {:.2}", panel.block_threshold),
            TEXT_MUTED,
        );
        panel.block_threshold = self.slider(
            ui,
            painter,
            c,
            controls.block_slider,
            panel.block_threshold,
            "advanced_debleed_block",
            BLUE,
            CYAN,
        );
        if panel.block_threshold < panel.warn_threshold {
            panel.block_threshold = panel.warn_threshold;
        }

        self.micro_text(
            painter,
            c,
            controls.amount_knob.left() + 7.0,
            controls.amount_knob.top() - 5.0,
            "Amount",
            TEXT_MUTED,
        );
        panel.amount = self.knob_control(
            ui,
            painter,
            c,
            controls.amount_knob.center(),
            panel.amount,
            "advanced_debleed_amount",
        );

        if panel != original {
            self.apply_edit("advanced_debleed", |s| panel.apply_to_session(s));
        }
    }

    fn advanced_declip_card(
        &mut self,
        ui: &mut egui::Ui,
        painter: &egui::Painter,
        c: CanvasMap,
        r: egui::Rect,
    ) {
        self.advanced_card_header(painter, c, r, "2", "Declip");
        let controls = advanced_declip_control_rects(r);
        let original = AdvancedDeClipPanel::from_session(&self.session);
        let mut panel = original;

        self.micro_text(
            painter,
            c,
            r.left() + 12.0,
            r.top() + 60.0,
            &format!("Thresh {:.2}", panel.threshold),
            TEXT_MUTED,
        );
        panel.threshold = self.slider(
            ui,
            painter,
            c,
            controls.threshold_slider,
            panel.threshold,
            "advanced_declip_threshold",
            BLUE,
            CYAN,
        );

        self.micro_text(
            painter,
            c,
            r.left() + 12.0,
            r.top() + 91.0,
            &format!("Strength {:.2}", panel.strength),
            TEXT_MUTED,
        );
        panel.strength = self.slider(
            ui,
            painter,
            c,
            controls.strength_slider,
            panel.strength,
            "advanced_declip_strength",
            BLUE,
            CYAN,
        );

        self.micro_text(
            painter,
            c,
            controls.amount_knob.left() + 7.0,
            controls.amount_knob.top() - 5.0,
            "Amount",
            TEXT_MUTED,
        );
        panel.amount = self.knob_control(
            ui,
            painter,
            c,
            controls.amount_knob.center(),
            panel.amount,
            "advanced_declip_amount",
        );

        if panel != original {
            self.apply_edit("advanced_declip", |s| panel.apply_to_session(s));
        }
    }

    fn advanced_declick_card(
        &mut self,
        ui: &mut egui::Ui,
        painter: &egui::Painter,
        c: CanvasMap,
        r: egui::Rect,
    ) {
        self.advanced_card_header(painter, c, r, "3", "De-click");
        let controls = advanced_declick_control_rects(r);
        let original = AdvancedDeClickPanel::from_session(&self.session);
        let mut panel = original;

        self.micro_text(
            painter,
            c,
            r.left() + 190.0,
            r.top() + 25.0,
            if panel.click_enabled || panel.mouth_enabled {
                "Split"
            } else {
                "Bypass"
            },
            if panel.click_enabled || panel.mouth_enabled {
                CYAN
            } else {
                TEXT_MUTED
            },
        );

        self.micro_text(
            painter,
            c,
            r.left() + 54.0,
            r.top() + 66.0,
            "Click repair",
            TEXT_MUTED,
        );
        if self.toggle(
            ui,
            painter,
            c,
            controls.click_toggle,
            panel.click_enabled,
            "advanced_declick_click_toggle",
        ) {
            panel.click_enabled = !panel.click_enabled;
        }
        self.micro_text(
            painter,
            c,
            r.left() + 54.0,
            r.top() + 100.0,
            "Mouth click",
            TEXT_MUTED,
        );
        if self.toggle(
            ui,
            painter,
            c,
            controls.mouth_toggle,
            panel.mouth_enabled,
            "advanced_declick_mouth_toggle",
        ) {
            panel.mouth_enabled = !panel.mouth_enabled;
        }

        self.micro_text(
            painter,
            c,
            controls.click_knob.left() + 14.0,
            controls.click_knob.top() - 5.0,
            "Click",
            TEXT_MUTED,
        );
        panel.click_amount = self.knob_control(
            ui,
            painter,
            c,
            controls.click_knob.center(),
            panel.click_amount,
            "advanced_declick_click_amount",
        );
        self.micro_text(
            painter,
            c,
            controls.mouth_knob.left() + 10.0,
            controls.mouth_knob.top() - 5.0,
            "Mouth",
            TEXT_MUTED,
        );
        panel.mouth_amount = self.knob_control(
            ui,
            painter,
            c,
            controls.mouth_knob.center(),
            panel.mouth_amount,
            "advanced_declick_mouth_amount",
        );

        if panel != original {
            self.apply_edit("advanced_declick", |s| panel.apply_to_session(s));
        }
    }

    fn advanced_depop_card(
        &mut self,
        ui: &mut egui::Ui,
        painter: &egui::Painter,
        c: CanvasMap,
        r: egui::Rect,
    ) {
        self.advanced_card_header(painter, c, r, "4", "De-pop");
        let controls = advanced_depop_control_rects(r);
        let original = AdvancedDePopPanel::from_session(&self.session);
        let mut panel = original;

        self.micro_text(
            painter,
            c,
            r.left() + 12.0,
            r.top() + 60.0,
            &format!("Low-cut {:.0} Hz", panel.cutoff_hz),
            TEXT_MUTED,
        );
        let cutoff_t = ((panel.cutoff_hz - 80.0) / 40.0).clamp(0.0, 1.0);
        let next_cutoff_t = self.slider(
            ui,
            painter,
            c,
            controls.cutoff_slider,
            cutoff_t,
            "advanced_depop_cutoff",
            BLUE,
            CYAN,
        );
        panel.cutoff_hz = 80.0 + next_cutoff_t * 40.0;

        self.micro_text(
            painter,
            c,
            r.left() + 12.0,
            r.top() + 91.0,
            &format!("Strength {:.2}", panel.strength),
            TEXT_MUTED,
        );
        panel.strength = self.slider(
            ui,
            painter,
            c,
            controls.strength_slider,
            panel.strength,
            "advanced_depop_strength",
            BLUE,
            CYAN,
        );

        self.micro_text(
            painter,
            c,
            controls.amount_knob.left() + 7.0,
            controls.amount_knob.top() - 5.0,
            "Amount",
            TEXT_MUTED,
        );
        panel.amount = self.knob_control(
            ui,
            painter,
            c,
            controls.amount_knob.center(),
            panel.amount,
            "advanced_depop_amount",
        );

        if panel != original {
            self.apply_edit("advanced_depop", |s| panel.apply_to_session(s));
        }
    }

    fn advanced_dehum_card(
        &mut self,
        ui: &mut egui::Ui,
        painter: &egui::Painter,
        c: CanvasMap,
        r: egui::Rect,
    ) {
        self.advanced_card_header(painter, c, r, "5", "De-hum");
        let controls = advanced_dehum_control_rects(r);
        let original = AdvancedDeHumPanel::from_session(&self.session);
        let mut panel = original;
        let tracked_hz = self.session.dehum_tracked_frequency();
        let tracked_label = if tracked_hz > 0.0 {
            format!("{tracked_hz:.1} Hz")
        } else {
            panel.nominal_label().to_string()
        };

        self.micro_text(
            painter,
            c,
            r.left() + 156.0,
            r.top() + 25.0,
            &tracked_label,
            if tracked_hz > 0.0 || panel.nominal.is_some() {
                CYAN
            } else {
                TEXT_MUTED
            },
        );

        if self.segment_button(
            ui,
            painter,
            c,
            controls.auto_button,
            "Auto",
            panel.nominal.is_none(),
            "advanced_dehum_auto",
        ) {
            panel.nominal = None;
        }
        if self.segment_button(
            ui,
            painter,
            c,
            controls.hz50_button,
            "50",
            matches!(panel.nominal, Some(f) if f < 55.0),
            "advanced_dehum_50",
        ) {
            panel.nominal = Some(50.0);
        }
        if self.segment_button(
            ui,
            painter,
            c,
            controls.hz60_button,
            "60",
            matches!(panel.nominal, Some(f) if f >= 55.0),
            "advanced_dehum_60",
        ) {
            panel.nominal = Some(60.0);
        }
        if self.segment_button(
            ui,
            painter,
            c,
            controls.track_toggle,
            "Trk",
            panel.track,
            "advanced_dehum_track",
        ) {
            panel.track = !panel.track;
        }

        self.micro_text(
            painter,
            c,
            r.left() + 12.0,
            r.top() + 79.0,
            &format!("Harm {}", panel.harmonics),
            TEXT_MUTED,
        );
        let harmonics_t = ((panel.harmonics.saturating_sub(1)) as f32 / 11.0).clamp(0.0, 1.0);
        let next_harmonics_t = self.slider(
            ui,
            painter,
            c,
            controls.harmonics_slider,
            harmonics_t,
            "advanced_dehum_harmonics",
            BLUE,
            CYAN,
        );
        panel.harmonics = (1.0 + next_harmonics_t * 11.0).round().clamp(1.0, 12.0) as u32;

        self.micro_text(
            painter,
            c,
            r.left() + 12.0,
            r.top() + 102.0,
            &format!("Q {:.0}", panel.q),
            TEXT_MUTED,
        );
        let q_t = ((panel.q - 5.0) / 75.0).clamp(0.0, 1.0);
        let next_q_t = self.slider(
            ui,
            painter,
            c,
            controls.q_slider,
            q_t,
            "advanced_dehum_q",
            BLUE,
            CYAN,
        );
        panel.q = 5.0 + next_q_t * 75.0;

        self.micro_text(
            painter,
            c,
            controls.amount_knob.left() + 5.0,
            controls.amount_knob.top() - 5.0,
            "Amount",
            TEXT_MUTED,
        );
        panel.amount = self.knob_control(
            ui,
            painter,
            c,
            controls.amount_knob.center(),
            panel.amount,
            "advanced_dehum_amount",
        );

        if panel != original {
            self.apply_edit("advanced_dehum", |s| panel.apply_to_session(s));
        }
    }

    fn advanced_denoise_card(
        &mut self,
        ui: &mut egui::Ui,
        painter: &egui::Painter,
        c: CanvasMap,
        r: egui::Rect,
    ) {
        self.advanced_card_header(painter, c, r, "6", "Denoise");
        let controls = advanced_denoise_control_rects(r);
        let original = AdvancedDenoisePanel::from_session(&self.session);
        let mut panel = original;

        self.micro_text(
            painter,
            c,
            r.left() + 164.0,
            r.top() + 25.0,
            panel.quality_label(),
            if panel.quality.is_generative() {
                CYAN
            } else {
                TEXT_MUTED
            },
        );

        if self.segment_button(
            ui,
            painter,
            c,
            controls.natural_button,
            "Nat",
            panel.quality == QualityMode::Natural,
            "advanced_denoise_natural",
        ) {
            panel.quality = QualityMode::Natural;
        }
        if self.segment_button(
            ui,
            painter,
            c,
            controls.balanced_button,
            "Bal",
            panel.quality == QualityMode::Balanced,
            "advanced_denoise_balanced",
        ) {
            panel.quality = QualityMode::Balanced;
        }
        if self.segment_button(
            ui,
            painter,
            c,
            controls.hq_button,
            "HQ",
            panel.quality == QualityMode::RestorationHq,
            "advanced_denoise_hq",
        ) {
            panel.quality = QualityMode::RestorationHq;
        }

        let max_workers = Session::max_worker_count().max(1);
        panel.workers = panel.workers.clamp(1, max_workers);
        self.micro_text(
            painter,
            c,
            r.left() + 12.0,
            r.top() + 87.0,
            &format!("Workers {}", panel.workers),
            TEXT_MUTED,
        );
        let worker_t = if max_workers <= 1 {
            0.0
        } else {
            (panel.workers.saturating_sub(1)) as f32 / (max_workers - 1) as f32
        };
        let next_worker_t = self.slider(
            ui,
            painter,
            c,
            controls.workers_slider,
            worker_t,
            "advanced_denoise_workers",
            BLUE,
            CYAN,
        );
        panel.workers = if max_workers <= 1 {
            1
        } else {
            (1.0 + next_worker_t * (max_workers - 1) as f32)
                .round()
                .clamp(1.0, max_workers as f32) as usize
        };

        self.micro_text(
            painter,
            c,
            r.left() + 12.0,
            r.top() + 108.0,
            panel.engine_label(),
            TEXT_MUTED,
        );
        self.micro_text(
            painter,
            c,
            r.left() + 62.0,
            r.top() + 108.0,
            panel.pass_label(),
            TEXT_MUTED,
        );

        self.micro_text(
            painter,
            c,
            controls.amount_knob.left() + 5.0,
            controls.amount_knob.top() - 5.0,
            "Amount",
            TEXT_MUTED,
        );
        panel.amount = self.knob_control(
            ui,
            painter,
            c,
            controls.amount_knob.center(),
            panel.amount,
            "advanced_denoise_amount",
        );

        if panel != original {
            self.apply_edit("advanced_denoise", |s| panel.apply_to_session(s));
        }
    }

    fn advanced_dereverb_card(
        &mut self,
        ui: &mut egui::Ui,
        painter: &egui::Painter,
        c: CanvasMap,
        r: egui::Rect,
    ) {
        self.advanced_card_header(painter, c, r, "7", "De-reverb");
        let controls = advanced_dereverb_control_rects(r);
        let original = AdvancedDeReverbPanel::from_session(&self.session);
        let mut panel = original;

        self.micro_text(
            painter,
            c,
            r.left() + 148.0,
            r.top() + 25.0,
            &panel.rt60_label(),
            if panel.rt60_s > 0.0 { CYAN } else { TEXT_MUTED },
        );

        self.micro_text(
            painter,
            c,
            r.left() + 12.0,
            r.top() + 60.0,
            &format!("Strength {:.2}", panel.strength),
            TEXT_MUTED,
        );
        let strength_t = (panel.strength / 2.0).clamp(0.0, 1.0);
        let next_strength_t = self.slider(
            ui,
            painter,
            c,
            controls.strength_slider,
            strength_t,
            "advanced_dereverb_strength",
            BLUE,
            CYAN,
        );
        panel.strength = next_strength_t * 2.0;

        self.micro_text(
            painter,
            c,
            r.left() + 12.0,
            r.top() + 91.0,
            &format!("Tail {:.2}", panel.tail_preservation),
            TEXT_MUTED,
        );
        panel.tail_preservation = self.slider(
            ui,
            painter,
            c,
            controls.tail_slider,
            panel.tail_preservation,
            "advanced_dereverb_tail",
            BLUE,
            CYAN,
        );

        self.micro_text(
            painter,
            c,
            controls.amount_knob.left() + 5.0,
            controls.amount_knob.top() - 5.0,
            "Amount",
            TEXT_MUTED,
        );
        panel.amount = self.knob_control(
            ui,
            painter,
            c,
            controls.amount_knob.center(),
            panel.amount,
            "advanced_dereverb_amount",
        );

        if panel != original {
            self.apply_edit("advanced_dereverb", |s| panel.apply_to_session(s));
        }
    }

    fn advanced_deess_card(
        &mut self,
        ui: &mut egui::Ui,
        painter: &egui::Painter,
        c: CanvasMap,
        r: egui::Rect,
    ) {
        self.advanced_card_header(painter, c, r, "8", "De-ess");
        let controls = advanced_deess_control_rects(r);
        let original = AdvancedDeEssPanel::from_session(&self.session);
        let mut panel = original;
        let mode_label = panel.mode_label();
        let hf_label = panel.hf_label();

        self.micro_text(
            painter,
            c,
            r.left() + 128.0,
            r.top() + 25.0,
            mode_label,
            TEXT_MUTED,
        );
        self.micro_text(
            painter,
            c,
            r.left() + 178.0,
            r.top() + 25.0,
            &hf_label,
            if panel.hf_reduction_db > 0.0 {
                CYAN
            } else {
                TEXT_MUTED
            },
        );

        if self.segment_button(
            ui,
            painter,
            c,
            controls.split_button,
            "Split",
            panel.mode == DeEssMode::Split,
            "advanced_deess_split",
        ) {
            panel.mode = DeEssMode::Split;
        }
        if self.segment_button(
            ui,
            painter,
            c,
            controls.wide_button,
            "Wide",
            panel.mode == DeEssMode::Wide,
            "advanced_deess_wide",
        ) {
            panel.mode = DeEssMode::Wide;
        }

        self.micro_text(
            painter,
            c,
            r.left() + 12.0,
            r.top() + 81.0,
            &format!("Ratio {:.2}", panel.ess_ratio),
            TEXT_MUTED,
        );
        panel.ess_ratio = self.slider(
            ui,
            painter,
            c,
            controls.ess_ratio_slider,
            panel.ess_ratio,
            "advanced_deess_ratio",
            BLUE,
            CYAN,
        );

        self.micro_text(
            painter,
            c,
            r.left() + 12.0,
            r.top() + 104.0,
            &format!("Min {:.2}", panel.min_gain),
            TEXT_MUTED,
        );
        panel.min_gain = self.slider(
            ui,
            painter,
            c,
            controls.min_gain_slider,
            panel.min_gain,
            "advanced_deess_min_gain",
            BLUE,
            CYAN,
        );

        self.micro_text(
            painter,
            c,
            controls.amount_knob.left() + 5.0,
            controls.amount_knob.top() - 5.0,
            "Amount",
            TEXT_MUTED,
        );
        panel.amount = self.knob_control(
            ui,
            painter,
            c,
            controls.amount_knob.center(),
            panel.amount,
            "advanced_deess_amount",
        );

        if panel != original {
            self.apply_edit("advanced_deess", |s| panel.apply_to_session(s));
        }
    }

    fn advanced_breath_card(
        &mut self,
        ui: &mut egui::Ui,
        painter: &egui::Painter,
        c: CanvasMap,
        r: egui::Rect,
    ) {
        self.advanced_card_header(painter, c, r, "9", "Breath");
        let controls = advanced_breath_control_rects(r);
        let original = AdvancedBreathPanel::from_session(&self.session);
        let mut panel = original;
        let sidechain_label = panel.sidechain_label();

        self.micro_text(
            painter,
            c,
            r.left() + 178.0,
            r.top() + 25.0,
            &sidechain_label,
            if panel.shared_hf_reduction_db > 0.0 {
                CYAN
            } else {
                TEXT_MUTED
            },
        );

        self.micro_text(
            painter,
            c,
            r.left() + 12.0,
            r.top() + 81.0,
            &format!("Atten {:.1} dB", panel.strength_db),
            TEXT_MUTED,
        );
        let strength_t = (panel.strength_db / 12.0).clamp(0.0, 1.0);
        let next_strength_t = self.slider(
            ui,
            painter,
            c,
            controls.strength_slider,
            strength_t,
            "advanced_breath_strength",
            BLUE,
            CYAN,
        );
        panel.strength_db = next_strength_t * 12.0;

        self.micro_text(
            painter,
            c,
            r.left() + 12.0,
            r.top() + 104.0,
            "De-ess guard",
            TEXT_MUTED,
        );

        self.micro_text(
            painter,
            c,
            controls.amount_knob.left() + 5.0,
            controls.amount_knob.top() - 5.0,
            "Amount",
            TEXT_MUTED,
        );
        panel.amount = self.knob_control(
            ui,
            painter,
            c,
            controls.amount_knob.center(),
            panel.amount,
            "advanced_breath_amount",
        );

        if panel != original {
            self.apply_edit("advanced_breath", |s| panel.apply_to_session(s));
        }
    }

    fn advanced_finish_card(
        &mut self,
        ui: &mut egui::Ui,
        painter: &egui::Painter,
        c: CanvasMap,
        r: egui::Rect,
    ) {
        self.advanced_card_header(painter, c, r, "10", "Finish");
        let controls = advanced_finish_control_rects(r);
        let labels = advanced_finish_label_rects(r);
        let original = AdvancedFinishPanel::from_session(&self.session);
        let mut panel = original;

        self.micro_text(
            painter,
            c,
            labels.presence.left(),
            labels.presence.center().y,
            "Presence",
            if panel.presence_air_enabled {
                CYAN
            } else {
                TEXT_MUTED
            },
        );
        if self.checkbox(
            ui,
            painter,
            c,
            controls.presence_toggle,
            panel.presence_air_enabled,
            "advanced_finish_presence_enabled",
        ) {
            panel.presence_air_enabled = !panel.presence_air_enabled;
        }
        self.micro_text(
            painter,
            c,
            labels.presence_value.left(),
            labels.presence_value.center().y,
            &format!("{:.1} dB", panel.presence_db),
            TEXT_MUTED,
        );
        panel.presence_db = self.slider(
            ui,
            painter,
            c,
            controls.presence_slider,
            panel.presence_db / 3.0,
            "advanced_finish_presence_db",
            BLUE,
            CYAN,
        ) * 3.0;

        self.micro_text(
            painter,
            c,
            labels.air.left(),
            labels.air.center().y,
            "Air",
            TEXT_MUTED,
        );
        self.micro_text(
            painter,
            c,
            labels.air_value.left(),
            labels.air_value.center().y,
            &format!("{:.1} dB", panel.air_db),
            TEXT_MUTED,
        );
        panel.air_db = self.slider(
            ui,
            painter,
            c,
            controls.air_slider,
            panel.air_db / 3.0,
            "advanced_finish_air_db",
            VIOLET,
            CYAN,
        ) * 3.0;

        self.micro_text(
            painter,
            c,
            labels.presence_amount.left(),
            labels.presence_amount.center().y,
            "P/A Amount",
            TEXT_MUTED,
        );
        panel.presence_air_amount = self.knob_control(
            ui,
            painter,
            c,
            controls.presence_amount_knob.center(),
            panel.presence_air_amount,
            "advanced_finish_presence_amount",
        );

        self.micro_text(
            painter,
            c,
            labels.leveling.left(),
            labels.leveling.center().y,
            "Leveling",
            if panel.leveling_enabled {
                CYAN
            } else {
                TEXT_MUTED
            },
        );
        if self.checkbox(
            ui,
            painter,
            c,
            controls.leveling_toggle,
            panel.leveling_enabled,
            "advanced_finish_leveling_enabled",
        ) {
            panel.leveling_enabled = !panel.leveling_enabled;
        }
        self.micro_text(
            painter,
            c,
            labels.leveling_value.left(),
            labels.leveling_value.center().y,
            &format!("{:.0}%", panel.leveling_amount * 100.0),
            TEXT_MUTED,
        );
        panel.leveling_amount = self.slider(
            ui,
            painter,
            c,
            controls.leveling_slider,
            panel.leveling_amount,
            "advanced_finish_leveling_amount",
            BLUE,
            PURPLE,
        );

        self.micro_text(
            painter,
            c,
            labels.target.left(),
            labels.target.center().y,
            "Target",
            TEXT_MUTED,
        );
        if self.checkbox(
            ui,
            painter,
            c,
            controls.loudness_toggle,
            panel.loudness_enabled,
            "advanced_finish_loudness_enabled",
        ) {
            panel.loudness_enabled = !panel.loudness_enabled;
        }
        self.micro_text(
            painter,
            c,
            labels.loudness.left(),
            labels.loudness.center().y,
            "Loudness",
            if panel.loudness_enabled {
                CYAN
            } else {
                TEXT_MUTED
            },
        );
        if self.outline_button(
            ui,
            painter,
            c,
            controls.target_button,
            panel.target_short_label(),
            "advanced_finish_loudness_target",
        ) {
            panel.loudness_target = cycle_loudness(panel.loudness_target);
        }
        self.loudness_target_knob(
            painter,
            c,
            controls.loudness_knob.center(),
            panel.loudness_target,
        );
        self.micro_text(
            painter,
            c,
            labels.limiter.left(),
            labels.limiter.center().y,
            if panel.loudness_enabled {
                "Limiter on"
            } else {
                "Limiter off"
            },
            if panel.loudness_enabled {
                CYAN
            } else {
                TEXT_MUTED
            },
        );

        if panel != original {
            self.apply_edit("advanced_finish", |s| panel.apply_to_session(s));
        }
    }

    fn advanced_card_header(
        &self,
        painter: &egui::Painter,
        c: CanvasMap,
        r: egui::Rect,
        index: &str,
        title: &str,
    ) {
        paint_panel(painter, c, r, 7, alpha(CARD, 235), BORDER_DIM);
        paint_panel(
            painter,
            c,
            rect(r.left() + 12.0, r.top() + 12.0, 30.0, 24.0),
            4,
            FIELD,
            BORDER_DIM,
        );
        painter.text(
            c.pos(r.left() + 27.0, r.top() + 24.0),
            egui::Align2::CENTER_CENTER,
            index,
            c.font(13.0),
            TEXT,
        );
        self.body_text(painter, c, r.left() + 54.0, r.top() + 25.0, title, TEXT);
    }

    fn draw_rack_panel(
        &mut self,
        ui: &mut egui::Ui,
        painter: &egui::Painter,
        c: CanvasMap,
        panel: egui::Rect,
        tab: Tab,
    ) {
        paint_panel(painter, c, panel, 8, alpha(PANEL, 218), BORDER_DIM);
        let title = match tab {
            Tab::Restore => "RESTORE",
            Tab::Polish => "POLISH",
        };
        self.section_label(painter, c, panel.left() + 20.0, panel.top() + 28.0, title);

        let cards = match tab {
            Tab::Restore => CardId::RESTORE,
            Tab::Polish => CardId::POLISH,
        };
        let mut x = panel.left() + 20.0;
        for card in cards {
            self.rack_card(
                ui,
                painter,
                c,
                rect(x, panel.top() + 48.0, 210.0, 160.0),
                card,
            );
            x += 222.0;
        }
        let macro_rect = rect(panel.right() - 150.0, panel.top() + 42.0, 130.0, 155.0);
        match tab {
            Tab::Restore => {
                self.small_text(
                    painter,
                    c,
                    macro_rect.left() + 10.0,
                    macro_rect.top() + 12.0,
                    "Restoration Focus",
                    TEXT,
                );
                let mut value = self.session.restoration_focus();
                value = self.radial_control(
                    ui,
                    painter,
                    c,
                    macro_rect.translate(egui::vec2(0.0, 10.0)),
                    value,
                    "restore_focus",
                );
                if (value - self.session.restoration_focus()).abs() > f32::EPSILON {
                    self.apply_edit("restoration_focus", |s| s.set_restoration_focus(value));
                }
                self.micro_text(
                    painter,
                    c,
                    macro_rect.left() - 2.0,
                    macro_rect.bottom() - 22.0,
                    "More",
                    TEXT_MUTED,
                );
                self.micro_text(
                    painter,
                    c,
                    macro_rect.left() - 2.0,
                    macro_rect.bottom() - 8.0,
                    "Transparent",
                    TEXT_MUTED,
                );
                self.micro_text(
                    painter,
                    c,
                    macro_rect.right() - 46.0,
                    macro_rect.bottom() - 22.0,
                    "More",
                    TEXT_MUTED,
                );
                self.micro_text(
                    painter,
                    c,
                    macro_rect.right() - 56.0,
                    macro_rect.bottom() - 8.0,
                    "Aggressive",
                    TEXT_MUTED,
                );
            }
            Tab::Polish => {
                self.small_text(
                    painter,
                    c,
                    macro_rect.left() + 10.0,
                    macro_rect.top() + 12.0,
                    "Polish Intensity",
                    TEXT,
                );
                let mut value = self.session.polish_intensity();
                value = self.radial_control(
                    ui,
                    painter,
                    c,
                    macro_rect.translate(egui::vec2(0.0, 10.0)),
                    value,
                    "polish_intensity",
                );
                if (value - self.session.polish_intensity()).abs() > f32::EPSILON {
                    self.apply_edit("polish", |s| s.set_polish_intensity(value));
                }
                self.micro_text(
                    painter,
                    c,
                    macro_rect.left() + 18.0,
                    macro_rect.bottom() - 22.0,
                    "Gentle",
                    TEXT_MUTED,
                );
                self.micro_text(
                    painter,
                    c,
                    macro_rect.right() - 54.0,
                    macro_rect.bottom() - 22.0,
                    "Strong",
                    TEXT_MUTED,
                );
                // "Auto Polish" removed (#24): it overlapped Smart Assist and
                // was never wired.
            }
        }
    }

    fn rack_card(
        &mut self,
        ui: &mut egui::Ui,
        painter: &egui::Painter,
        c: CanvasMap,
        r: egui::Rect,
        card: CardId,
    ) {
        paint_panel(painter, c, r, 7, alpha(CARD, 235), BORDER_DIM);
        self.card_icon(
            painter,
            c,
            egui::pos2(r.left() + 28.0, r.top() + 26.0),
            card,
        );
        self.body_text(
            painter,
            c,
            r.left() + 54.0,
            r.top() + 33.0,
            card.label(),
            TEXT,
        );
        let on = self.session.card_enabled(card);
        if self.toggle(
            ui,
            painter,
            c,
            rect(r.right() - 43.0, r.top() + 18.0, 34.0, 20.0),
            on,
            ("card_toggle", card.label()),
        ) {
            let next = !on;
            self.apply_edit(&format!("card_toggle:{}", card.label()), |s| {
                s.set_card_enabled(card, next)
            });
        }
        let (a, b) = card_blurb(card);
        self.micro_text(painter, c, r.left() + 12.0, r.top() + 68.0, a, TEXT_MUTED);
        self.micro_text(painter, c, r.left() + 12.0, r.top() + 95.0, b, TEXT_MUTED);
        if card == CardId::Loudness {
            // The loudness *target* is selected in the OUTPUT panel; the card's
            // duplicate "Target" button was removed (#24). The knob shows the
            // current target.
            self.loudness_knob(painter, c, egui::pos2(r.right() - 38.0, r.bottom() - 50.0));
        } else {
            let current = self.session.card_amount(card);
            let next = self.knob_control(
                ui,
                painter,
                c,
                egui::pos2(r.right() - 38.0, r.bottom() - 58.0),
                current,
                ("card_amount", card.label()),
            );
            if (next - current).abs() > 1e-6 {
                self.apply_edit(&format!("card_amount:{}", card.label()), |s| {
                    s.set_card_amount(card, next)
                });
            }
        }
        // Per-card "Advanced" buttons removed (#24): the Advanced screen is a
        // separate, deferred surface (#26 → #18); no dead links to it in v1.
    }

    fn draw_right_sidebar(
        &mut self,
        ui: &mut egui::Ui,
        painter: &egui::Painter,
        c: CanvasMap,
        l: ReferenceLayout,
    ) {
        paint_panel(painter, c, l.right, 0, alpha(PANEL, 245), BORDER_DIM);
        let preset = rect(1588.0, 70.0, 304.0, 292.0);
        paint_sidebar_section(painter, c, preset);
        self.section_label(painter, c, 1608.0, 100.0, "PRESET");
        paint_panel(
            painter,
            c,
            rect(1608.0, 132.0, 264.0, 44.0),
            5,
            FIELD,
            BORDER_DIM,
        );
        self.body_text(painter, c, 1624.0, 160.0, self.session.preset().name, TEXT);
        if preset_gen_badge_visible(&self.session) {
            self.badge(painter, c, rect(1810.0, 142.0, 34.0, 22.0), "GEN", true);
        }
        self.draw_export_action(ui, c, rect(1608.0, 204.0, 264.0, 58.0));
        let summary = truncate_middle(&self.status, 30);
        self.small_text(painter, c, 1608.0, 290.0, &summary, TEXT_MUTED);
        let out = rect(1588.0, 372.0, 304.0, 294.0);
        paint_sidebar_section(painter, c, out);
        self.section_label(painter, c, 1608.0, 404.0, "OUTPUT");
        self.field_row(
            ui,
            painter,
            c,
            1608.0,
            450.0,
            "Format",
            self.session.output_format().label(),
            "format_row",
        );
        self.field_row(
            ui,
            painter,
            c,
            1608.0,
            498.0,
            "Sample Rate",
            &self.sample_rate_label(),
            "sample_rate_row",
        );
        self.field_row(
            ui,
            painter,
            c,
            1608.0,
            546.0,
            "Bit Depth",
            &self.bit_depth_label(),
            "bit_depth_row",
        );
        self.field_row(
            ui,
            painter,
            c,
            1608.0,
            594.0,
            "Loudness Target",
            &loudness_label(self.session.loudness_target()),
            "loudness_row",
        );
        self.small_text(painter, c, 1608.0, 640.0, "Dither", TEXT_MUTED);
        if self.toggle(
            ui,
            painter,
            c,
            rect(1850.0, 624.0, 34.0, 20.0),
            self.session.settings().export.dither,
            "dither_toggle",
        ) {
            let next = !self.session.settings().export.dither;
            self.apply_edit("dither", |s| s.set_dither(next));
        }

        let file = rect(1588.0, 680.0, 304.0, 216.0);
        paint_sidebar_section(painter, c, file);
        self.section_label(painter, c, 1608.0, 714.0, "FILE");
        self.small_text(painter, c, 1608.0, 754.0, "Output File Name", TEXT_MUTED);
        paint_panel(
            painter,
            c,
            rect(1608.0, 772.0, 264.0, 40.0),
            5,
            FIELD,
            BORDER_DIM,
        );
        // Editable name (#24): a transparent TextEdit over the painted field.
        // While a job runs the session may be a worker-side placeholder, so the
        // name paints read-only instead of accepting doomed edits (#36).
        if self.edits_locked() {
            self.body_text(painter, c, 1620.0, 792.0, &self.name_input, TEXT_MUTED);
        } else {
            let name_resp = ui.put(
                c.rect(rect(1620.0, 780.0, 244.0, 24.0)),
                egui::TextEdit::singleline(&mut self.name_input).text_color(TEXT),
            );
            if name_resp.changed() {
                self.session.set_output_name(&self.name_input);
            }
        }
        self.small_text(painter, c, 1608.0, 822.0, "Location", TEXT_MUTED);
        paint_panel(
            painter,
            c,
            rect(1608.0, 840.0, 264.0, 34.0),
            5,
            FIELD,
            BORDER_DIM,
        );
        let location = self
            .session
            .output_location()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "Source folder".to_owned());
        let location_short = truncate_middle(&location, 30);
        self.body_text(painter, c, 1622.0, 858.0, &location_short, TEXT);
        self.folder_icon(painter, c, egui::pos2(1850.0, 856.0));
        // Click the Location field to pick an output folder (#24, rfd).
        if self
            .response(ui, c, rect(1608.0, 840.0, 264.0, 34.0), "location_pick")
            .clicked()
        {
            if self.edits_locked() {
                self.status = SETTINGS_LOCKED_STATUS.to_owned();
            } else if let Some(dir) = rfd::FileDialog::new()
                .set_title("Output Location")
                .pick_folder()
            {
                self.session.set_output_location(&dir);
            }
        }
        // "Add to Yes Suite Library" removed (#24): the suite handoff is a
        // designed-for seam, not a built-in v1 feature.

        paint_sidebar_section(painter, c, rect(1588.0, 906.0, 304.0, 122.0));
        self.section_label(painter, c, 1608.0, 930.0, "BATCH QUEUE");
        self.small_text(
            painter,
            c,
            1710.0,
            930.0,
            &self.batch_queue.summary_label(),
            TEXT_MUTED,
        );
        let force_active = self.batch_queue.mode == GuiBatchPresetMode::ForceSelected;
        if self.segment_button(
            ui,
            painter,
            c,
            rect(1608.0, 948.0, 104.0, 30.0),
            GuiBatchPresetMode::ForceSelected.label(),
            force_active,
            "batch_force_preset",
        ) {
            self.batch_queue.mode = GuiBatchPresetMode::ForceSelected;
        }
        if self.segment_button(
            ui,
            painter,
            c,
            rect(1718.0, 948.0, 104.0, 30.0),
            GuiBatchPresetMode::DetectPerFile.label(),
            !force_active,
            "batch_detect_each",
        ) {
            self.batch_queue.mode = GuiBatchPresetMode::DetectPerFile;
        }
        if self.outline_button(
            ui,
            painter,
            c,
            rect(1828.0, 948.0, 24.0, 30.0),
            if self.batch_queue.paused { ">" } else { "||" },
            "batch_pause",
        ) {
            self.batch_queue.paused = !self.batch_queue.paused;
        }
        if self.outline_button(
            ui,
            painter,
            c,
            rect(1858.0, 948.0, 22.0, 30.0),
            "x",
            "batch_cancel",
        ) {
            if let Some(job) = self.batch_job.as_ref() {
                job.cancel();
                self.status = "Batch cancel requested".to_owned();
            } else {
                self.status = "No batch running".to_owned();
            }
        }
        if self.outline_button(
            ui,
            painter,
            c,
            rect(1608.0, 982.0, 128.0, 30.0),
            "+ Queue Current",
            "batch_queue_current",
        ) {
            self.queue_current_vocal();
        }
        if self.outline_button(
            ui,
            painter,
            c,
            rect(1744.0, 982.0, 136.0, 30.0),
            "Export Batch",
            "batch_export",
        ) {
            self.run_batch_queue();
        }
    }

    fn draw_status_bar(&mut self, painter: &egui::Painter, c: CanvasMap, l: ReferenceLayout) {
        paint_panel(painter, c, l.status, 0, alpha(PANEL, 245), BORDER_DIM);
        painter.circle_filled(c.pos(18.0, 1062.0), c.v(5.0), BLUE);
        self.small_text(
            painter,
            c,
            34.0,
            1066.0,
            self.processing_label(),
            TEXT_MUTED,
        );
        painter.circle_filled(c.pos(116.0, 1062.0), c.v(5.0), TEXT_DIM);
        self.small_text(
            painter,
            c,
            132.0,
            1066.0,
            &format!("Quality {:?}", self.session.settings().quality_mode),
            TEXT_MUTED,
        );
        let metrics = self.audible_metrics();
        let peak = metrics.map(|m| m.peak_db);
        // LIVE LUFS reports what is audible right now: the reblend mix only
        // while the strip audition is active, else the honest A/B side.
        let lufs = metrics.and_then(|m| m.lufs);
        self.small_text(
            painter,
            c,
            832.0,
            1066.0,
            &format!("LIVE PEAK  {}", db_label(peak)),
            TEXT_MUTED,
        );
        self.small_text(
            painter,
            c,
            950.0,
            1066.0,
            &format!("LIVE LUFS  {}", lufs_label(lufs)),
            TEXT_MUTED,
        );
        // Preview-rate badge (#36): visible in both views whenever the open
        // stream runs at a device rate that differs from the file's.
        if let (Some(player), Some(input)) = (self.player.as_ref(), self.session.input()) {
            if let Some(badge) = preview_rate_badge(player.sample_rate(), input.sample_rate()) {
                self.small_text(painter, c, 1150.0, 1066.0, &badge, TEXT_MUTED);
            }
        }
        self.small_text(painter, c, 1662.0, 1066.0, &self.status, TEXT_DIM);
        self.badge(
            painter,
            c,
            rect(1760.0, 1049.0, 142.0, 24.0),
            &self.processing_label().to_ascii_uppercase(),
            true,
        );
    }

    /// The always-visible build-stamp text (#20/W1): `v<version> · <hash> ·
    /// <engine>`, where the engine is the one the last render actually used (a
    /// neutral `—` before any render), plus a `· fallback` marker when the
    /// neural engine degraded to light (`engine_note` set). Its whole job is to
    /// kill stale-build ghost bugs and make a neural→light fallback visible in
    /// the app, not only in the CLI. The painted chip is visual; this composes
    /// the string so it can be tested mechanically.
    fn build_stamp_text(&self) -> String {
        let engine = self.session.last_engine_label().unwrap_or("—");
        let mut stamp = yesvox_core::version::gui_stamp(engine);
        if self.session.engine_note().is_some() {
            stamp.push_str(" · fallback");
        }
        stamp
    }

    /// Paint the build stamp, dim, in the free slot of the always-visible status
    /// strip (drawn in both Standard and Advanced views).
    fn draw_build_stamp(&self, painter: &egui::Painter, c: CanvasMap) {
        self.small_text(
            painter,
            c,
            360.0,
            1066.0,
            &self.build_stamp_text(),
            TEXT_DIM,
        );
    }
}

impl YesVoxApp {
    fn draw_wordmark(&self, painter: &egui::Painter, c: CanvasMap, anchor: egui::Pos2) {
        let bars = [14.0, 24.0, 32.0, 22.0, 12.0];
        for (i, h) in bars.iter().enumerate() {
            let x = anchor.x + i as f32 * 7.0;
            let r = rect(x, anchor.y - h / 2.0, 4.0, *h);
            paint_h_gradient(painter, c, r, BLUE, CYAN, 4, 3);
        }
        painter.text(
            c.pos(anchor.x + 50.0, anchor.y + 4.0),
            egui::Align2::LEFT_CENTER,
            "YES",
            c.font(23.0),
            TEXT,
        );
        painter.text(
            c.pos(anchor.x + 101.0, anchor.y + 4.0),
            egui::Align2::LEFT_CENTER,
            "Vox",
            c.font(23.0),
            CYAN,
        );
    }

    fn history_icon(
        &self,
        painter: &egui::Painter,
        c: CanvasMap,
        center: egui::Pos2,
        redo: bool,
        color: egui::Color32,
    ) {
        let sc = c.pos(center.x, center.y);
        let r = c.v(10.0);
        if redo {
            arc(
                painter,
                sc,
                r,
                -115.0,
                150.0,
                egui::Stroke::new(c.v(1.8), color),
            );
            painter.line_segment(
                [
                    sc + egui::vec2(c.v(8.0), -c.v(10.0)),
                    sc + egui::vec2(c.v(13.0), -c.v(7.0)),
                ],
                egui::Stroke::new(c.v(1.8), color),
            );
            painter.line_segment(
                [
                    sc + egui::vec2(c.v(8.0), -c.v(10.0)),
                    sc + egui::vec2(c.v(7.0), -c.v(4.0)),
                ],
                egui::Stroke::new(c.v(1.8), color),
            );
        } else {
            arc(
                painter,
                sc,
                r,
                30.0,
                295.0,
                egui::Stroke::new(c.v(1.8), color),
            );
            painter.line_segment(
                [
                    sc + egui::vec2(-c.v(8.0), -c.v(10.0)),
                    sc + egui::vec2(-c.v(13.0), -c.v(7.0)),
                ],
                egui::Stroke::new(c.v(1.8), color),
            );
            painter.line_segment(
                [
                    sc + egui::vec2(-c.v(8.0), -c.v(10.0)),
                    sc + egui::vec2(-c.v(7.0), -c.v(4.0)),
                ],
                egui::Stroke::new(c.v(1.8), color),
            );
        }
    }

    fn upload_icon(
        &self,
        painter: &egui::Painter,
        c: CanvasMap,
        center: egui::Pos2,
        color: egui::Color32,
    ) {
        let sc = c.pos(center.x, center.y);
        painter.line_segment(
            [
                sc + egui::vec2(0.0, c.v(8.0)),
                sc + egui::vec2(0.0, -c.v(8.0)),
            ],
            egui::Stroke::new(c.v(2.0), color),
        );
        painter.line_segment(
            [
                sc + egui::vec2(-c.v(6.0), -c.v(2.0)),
                sc + egui::vec2(0.0, -c.v(8.0)),
            ],
            egui::Stroke::new(c.v(2.0), color),
        );
        painter.line_segment(
            [
                sc + egui::vec2(c.v(6.0), -c.v(2.0)),
                sc + egui::vec2(0.0, -c.v(8.0)),
            ],
            egui::Stroke::new(c.v(2.0), color),
        );
        painter.line_segment(
            [
                sc + egui::vec2(-c.v(9.0), c.v(10.0)),
                sc + egui::vec2(c.v(9.0), c.v(10.0)),
            ],
            egui::Stroke::new(c.v(2.0), color),
        );
    }

    fn preview_icon(
        &self,
        painter: &egui::Painter,
        c: CanvasMap,
        center: egui::Pos2,
        color: egui::Color32,
    ) {
        let tri = vec![
            c.pos(center.x - 4.0, center.y - 7.0),
            c.pos(center.x - 4.0, center.y + 7.0),
            c.pos(center.x + 8.0, center.y),
        ];
        painter.add(egui::Shape::convex_polygon(tri, color, egui::Stroke::NONE));
    }

    fn mic_icon(
        &self,
        painter: &egui::Painter,
        c: CanvasMap,
        center: egui::Pos2,
        color: egui::Color32,
    ) {
        let body = rect(center.x - 6.0, center.y - 17.0, 12.0, 24.0);
        painter.rect_stroke(
            c.rect(body),
            egui::CornerRadius::same(6),
            egui::Stroke::new(c.v(1.8), color),
            egui::StrokeKind::Inside,
        );
        arc(
            painter,
            c.pos(center.x, center.y + 2.0),
            c.v(14.0),
            20.0,
            160.0,
            egui::Stroke::new(c.v(1.8), color),
        );
        painter.line_segment(
            [
                c.pos(center.x, center.y + 17.0),
                c.pos(center.x, center.y + 25.0),
            ],
            egui::Stroke::new(c.v(1.8), color),
        );
        painter.line_segment(
            [
                c.pos(center.x - 8.0, center.y + 25.0),
                c.pos(center.x + 8.0, center.y + 25.0),
            ],
            egui::Stroke::new(c.v(1.8), color),
        );
    }

    fn note_icon(
        &self,
        painter: &egui::Painter,
        c: CanvasMap,
        center: egui::Pos2,
        color: egui::Color32,
    ) {
        painter.line_segment(
            [
                c.pos(center.x + 5.0, center.y - 18.0),
                c.pos(center.x + 5.0, center.y + 11.0),
            ],
            egui::Stroke::new(c.v(2.0), color),
        );
        painter.line_segment(
            [
                c.pos(center.x + 5.0, center.y - 18.0),
                c.pos(center.x + 18.0, center.y - 13.0),
            ],
            egui::Stroke::new(c.v(2.0), color),
        );
        painter.circle_stroke(
            c.pos(center.x - 3.0, center.y + 13.0),
            c.v(8.0),
            egui::Stroke::new(c.v(2.0), color),
        );
    }

    fn draw_transport(&mut self, ui: &mut egui::Ui, painter: &egui::Painter, c: CanvasMap) {
        let play_rect = rect(322.0, 358.0, 52.0, 52.0);
        let play_clicked = self.response(ui, c, play_rect, "transport_play").clicked();
        let play = c.rect(play_rect);
        painter.circle_filled(play.center(), c.v(26.0), VIOLET);
        let inner = play.center() - egui::vec2(c.v(5.0), c.v(5.0));
        painter.circle_filled(inner, c.v(24.0), BLUE);
        if self.player.as_ref().is_some_and(|p| p.is_playing()) {
            // Pause icon (two bars).
            for dx in [-6.0, 6.0] {
                painter.rect_filled(
                    egui::Rect::from_center_size(
                        inner + egui::vec2(c.v(dx), 0.0),
                        egui::vec2(c.v(5.0), c.v(20.0)),
                    ),
                    egui::CornerRadius::ZERO,
                    TEXT,
                );
            }
        } else {
            let tri = vec![
                c.pos(342.0, 374.0),
                c.pos(342.0, 394.0),
                c.pos(360.0, 384.0),
            ];
            painter.add(egui::Shape::convex_polygon(tri, TEXT, egui::Stroke::NONE));
        }
        if play_clicked {
            // The main transport audits the A/B compare; the reblend strip's
            // own play button is the only way into the mix audition.
            self.auditioning_reblend = false;
            self.play_pause_audible();
        }
        let pos_s = self
            .player
            .as_ref()
            .map(|p| p.position() as f32 / p.sample_rate().max(1) as f32)
            .unwrap_or(0.0);
        self.body_text(
            painter,
            c,
            396.0,
            390.0,
            &format!("{} / {}", format_duration(pos_s), self.duration_label()),
            TEXT,
        );
        // "Loop" button removed (#24): looping is an explicitly deferred #23
        // feature — no dead control for it in v1.
        self.body_text(painter, c, 756.0, 385.0, self.processing_label(), TEXT);
        let bar = rect(712.0, 398.0, 350.0, 6.0);
        if self.player.is_some() {
            // The bar shows the playback playhead and is a seek control.
            let resp = ui.interact(
                c.rect(bar.expand(10.0)),
                ui.make_persistent_id("transport_seek"),
                egui::Sense::click_and_drag(),
            );
            if resp.clicked() || resp.dragged() {
                if let Some(pos) = resp.interact_pointer_pos() {
                    let sr = c.rect(bar);
                    let f = ((pos.x - sr.left()) / sr.width()).clamp(0.0, 1.0);
                    if let Some(p) = &self.player {
                        p.seek_fraction(f);
                    }
                }
            }
            let frac = self.player.as_ref().map(|p| p.fraction()).unwrap_or(0.0);
            self.body_text(
                painter,
                c,
                884.0,
                385.0,
                &format!("{:.0}%", frac * 100.0),
                TEXT,
            );
            self.progress_bar(painter, c, bar, frac);
        } else {
            // Idle/rendering: the bar shows processing progress (non-interactive).
            let progress = self.processing_progress();
            self.body_text(
                painter,
                c,
                884.0,
                385.0,
                &format!("{:.0}%", progress * 100.0),
                TEXT,
            );
            self.progress_bar(painter, c, bar, progress);
        }
        // Volume Match is playback-only state — it stays live mid-job (#36).
        if self.live_checkbox(
            ui,
            painter,
            c,
            rect(1136.0, 374.0, 18.0, 18.0),
            self.volume_match,
            "volume_match",
            true,
        ) {
            self.volume_match = !self.volume_match;
            self.refresh_playback_source();
        }
        self.body_text(painter, c, 1165.0, 390.0, "Volume Match", TEXT_MUTED);
        // "Preview LUFS" checkbox removed (#24): it was never wired.
    }

    fn draw_waveform(&mut self, painter: &egui::Painter, c: CanvasMap, r: egui::Rect) {
        let sr = c.rect(r);
        let mid = sr.center().y;
        let height = sr.height() * 0.42;
        let buckets = sr.width().round().max(1.0) as usize;
        let peaks = self
            .audible_buffer()
            .map(|b| yesvox_core::monitor::waveform_peaks(b, 0, buckets));
        let Some(peaks) = peaks else {
            return;
        };
        let wave_color = if self.ab == AbView::Original && !self.auditioning_reblend {
            TEXT_MUTED
        } else {
            CYAN
        };
        for (i, (lo, hi)) in peaks.into_iter().enumerate() {
            let x = sr.left() + i as f32;
            painter.line_segment(
                [
                    egui::pos2(x, mid - hi.abs() * height),
                    egui::pos2(x, mid + lo.abs() * height),
                ],
                egui::Stroke::new(c.v(1.0), wave_color),
            );
        }
        if let Some(player) = &self.player {
            let x = sr.left() + player.fraction() * sr.width();
            painter.line_segment(
                [egui::pos2(x, sr.top()), egui::pos2(x, sr.bottom())],
                egui::Stroke::new(c.v(1.5), TEXT),
            );
        }
    }

    fn draw_time_labels(&self, painter: &egui::Painter, c: CanvasMap, r: egui::Rect) {
        let Some(duration) = self.input_duration() else {
            return;
        };
        self.small_text(painter, c, r.left(), r.bottom() + 26.0, "0:00", TEXT_MUTED);
        self.small_text(
            painter,
            c,
            r.left() + r.width() * 0.32,
            r.bottom() + 26.0,
            &format_duration(duration * 0.32),
            TEXT_MUTED,
        );
        self.small_text(
            painter,
            c,
            r.left() + r.width() * 0.61,
            r.bottom() + 26.0,
            &format_duration(duration * 0.61),
            TEXT_MUTED,
        );
        self.small_text(
            painter,
            c,
            r.right() - 36.0,
            r.bottom() + 26.0,
            &format_duration(duration),
            TEXT_MUTED,
        );
    }

    fn draw_meter_panel(&self, painter: &egui::Painter, c: CanvasMap, r: egui::Rect) {
        painter.line_segment(
            [c.pos(r.left(), r.top()), c.pos(r.left(), r.bottom())],
            egui::Stroke::new(c.v(1.0), BORDER_DIM),
        );
        self.small_text(painter, c, r.left() + 24.0, r.top() + 32.0, "IN", TEXT);
        self.small_text(painter, c, r.left() + 100.0, r.top() + 32.0, "OUT", TEXT);
        self.body_text(
            painter,
            c,
            r.left() + 18.0,
            r.top() + 58.0,
            &lufs_label(self.session.input_lufs()),
            TEXT,
        );
        self.body_text(
            painter,
            c,
            r.left() + 94.0,
            r.top() + 58.0,
            &lufs_label(self.session.output_lufs()),
            TEXT,
        );
        let input = self.session.input();
        let output = self.session.output();
        self.vertical_meter(
            painter,
            c,
            rect(r.left() + 28.0, r.top() + 72.0, 11.0, 88.0),
            meter_value(input, 0),
        );
        self.vertical_meter(
            painter,
            c,
            rect(r.left() + 46.0, r.top() + 72.0, 11.0, 88.0),
            meter_value(input, 1),
        );
        self.vertical_meter(
            painter,
            c,
            rect(r.left() + 104.0, r.top() + 72.0, 11.0, 88.0),
            meter_value(output, 0),
        );
        self.vertical_meter(
            painter,
            c,
            rect(r.left() + 122.0, r.top() + 72.0, 11.0, 88.0),
            meter_value(output, 1),
        );
        for (i, label) in ["0", "-6", "-12", "-18", "-24", "-30", "-36", "-48"]
            .iter()
            .enumerate()
        {
            self.micro_text(
                painter,
                c,
                r.left() + 73.0,
                r.top() + 82.0 + i as f32 * 12.0,
                label,
                TEXT_DIM,
            );
            self.micro_text(
                painter,
                c,
                r.left() + 145.0,
                r.top() + 82.0 + i as f32 * 12.0,
                label,
                TEXT_DIM,
            );
        }
        self.micro_text(
            painter,
            c,
            r.left() + 29.0,
            r.bottom() - 10.0,
            "L",
            TEXT_MUTED,
        );
        self.micro_text(
            painter,
            c,
            r.left() + 49.0,
            r.bottom() - 10.0,
            "R",
            TEXT_MUTED,
        );
        self.micro_text(
            painter,
            c,
            r.left() + 105.0,
            r.bottom() - 10.0,
            "L",
            TEXT_MUTED,
        );
        self.micro_text(
            painter,
            c,
            r.left() + 125.0,
            r.bottom() - 10.0,
            "R",
            TEXT_MUTED,
        );
    }

    fn draw_monitor_strip(
        &mut self,
        ui: &mut egui::Ui,
        painter: &egui::Painter,
        c: CanvasMap,
        r: egui::Rect,
    ) {
        paint_panel(painter, c, r, 8, alpha(PANEL, 220), BORDER_DIM);
        self.section_label(
            painter,
            c,
            r.left() + 20.0,
            r.top() + 30.0,
            "SPECTRAL MONITOR",
        );
        self.small_text(
            painter,
            c,
            r.left() + 234.0,
            r.top() + 28.0,
            "Difference view (What was removed)",
            TEXT_MUTED,
        );
        let spec = rect(r.left() + 20.0, r.top() + 50.0, 560.0, 72.0);
        self.draw_spectrum(painter, c, spec, self.session.difference().as_ref());
        for (x, label) in [
            (0.0, "20"),
            (64.0, "50"),
            (128.0, "100"),
            (244.0, "500"),
            (330.0, "1K"),
            (394.0, "2K"),
            (506.0, "10K"),
            (548.0, "20K"),
        ] {
            self.micro_text(
                painter,
                c,
                spec.left() + x,
                spec.bottom() + 20.0,
                label,
                TEXT_MUTED,
            );
        }

        painter.line_segment(
            [
                c.pos(r.left() + 600.0, r.top()),
                c.pos(r.left() + 600.0, r.bottom()),
            ],
            egui::Stroke::new(c.v(1.0), BORDER_DIM),
        );
        self.section_label(painter, c, r.left() + 620.0, r.top() + 30.0, "REBLEND");
        self.small_text(
            painter,
            c,
            r.left() + 738.0,
            r.top() + 28.0,
            "Blend removed signal back in",
            TEXT_MUTED,
        );
        // Reblend is a playback audition blend, not a setting — live mid-job (#36).
        let mut reblend = self.reblend_amount;
        reblend = self.live_slider(
            ui,
            painter,
            c,
            rect(r.left() + 628.0, r.top() + 74.0, 185.0, 8.0),
            reblend,
            "reblend",
            BLUE,
            BLUE,
            true,
        );
        let reblend_changed = (reblend - self.reblend_amount).abs() > 1e-6;
        self.reblend_amount = reblend;
        if reblend_changed {
            // Defer the source swap to pointer release: rebuilding a full-length
            // mix buffer every drag frame thrashes the audio path (#23 review).
            self.reblend_dirty = true;
        }
        if self.reblend_dirty && !ui.input(|i| i.pointer.any_down()) {
            self.reblend_dirty = false;
            if self.auditioning_reblend {
                self.refresh_playback_source(); // the mix being auditioned changed
            }
        }
        self.body_text(
            painter,
            c,
            r.left() + 870.0,
            r.top() + 84.0,
            &format!("{:.0}%", self.reblend_amount * 100.0),
            TEXT,
        );
        if self.outline_button(
            ui,
            painter,
            c,
            rect(r.left() + 930.0, r.top() + 50.0, 48.0, 48.0),
            ">",
            "reblend_preview",
        ) {
            // Audition the reblend mix — the one path that plays a blend. The
            // main A/B transport stays pure-output (honest compare).
            self.auditioning_reblend = true;
            self.play_pause_audible();
        }

        painter.line_segment(
            [
                c.pos(r.left() + 994.0, r.top()),
                c.pos(r.left() + 994.0, r.bottom()),
            ],
            egui::Stroke::new(c.v(1.0), BORDER_DIM),
        );
        self.radar(painter, c, egui::pos2(r.left() + 1044.0, r.top() + 76.0));
        self.section_label(
            painter,
            c,
            r.left() + 1090.0,
            r.top() + 30.0,
            "Vocal Isolation Check",
        );
        let (line1, line2) = self
            .isolation_check
            .map(|check| {
                (
                    format!(
                        "{} - noise {:.0} dBFS",
                        check.overall().label(),
                        check.report.noise_floor_db
                    ),
                    format!(
                        "RT60 {:.2}s - bleed {}",
                        check.report.reverb_time_s,
                        check.bleed_label()
                    ),
                )
            })
            .unwrap_or_else(|| {
                (
                    "Checks for residual noise,".to_owned(),
                    "reverb, or bleed.".to_owned(),
                )
            });
        self.small_text(
            painter,
            c,
            r.left() + 1090.0,
            r.top() + 62.0,
            &line1,
            TEXT_MUTED,
        );
        self.small_text(
            painter,
            c,
            r.left() + 1090.0,
            r.top() + 88.0,
            &line2,
            TEXT_MUTED,
        );
        if self.outline_button(
            ui,
            painter,
            c,
            rect(r.left() + 1090.0, r.top() + 106.0, 154.0, 34.0),
            "◇  Check Now",
            "check_now",
        ) {
            self.check_isolation_now();
        }
    }
}

impl YesVoxApp {
    fn section_label(&self, painter: &egui::Painter, c: CanvasMap, x: f32, y: f32, text: &str) {
        painter.rect_filled(
            c.rect(rect(x, y - 8.0, 3.0, 16.0)),
            egui::CornerRadius::same(2),
            BLUE,
        );
        painter.text(
            c.pos(x + 12.0, y),
            egui::Align2::LEFT_CENTER,
            text,
            c.font(13.0),
            TEXT,
        );
    }

    fn title_text(&self, painter: &egui::Painter, c: CanvasMap, x: f32, y: f32, text: &str) {
        painter.text(
            c.pos(x, y),
            egui::Align2::LEFT_CENTER,
            text,
            c.font(28.0),
            TEXT,
        );
    }

    fn body_text(
        &self,
        painter: &egui::Painter,
        c: CanvasMap,
        x: f32,
        y: f32,
        text: &str,
        color: egui::Color32,
    ) {
        painter.text(
            c.pos(x, y),
            egui::Align2::LEFT_CENTER,
            text,
            c.font(14.5),
            color,
        );
    }

    fn small_text(
        &self,
        painter: &egui::Painter,
        c: CanvasMap,
        x: f32,
        y: f32,
        text: &str,
        color: egui::Color32,
    ) {
        painter.text(
            c.pos(x, y),
            egui::Align2::LEFT_CENTER,
            text,
            c.font(13.0),
            color,
        );
    }

    fn micro_text(
        &self,
        painter: &egui::Painter,
        c: CanvasMap,
        x: f32,
        y: f32,
        text: &str,
        color: egui::Color32,
    ) {
        painter.text(
            c.pos(x, y),
            egui::Align2::LEFT_CENTER,
            text,
            c.font(11.0),
            color,
        );
    }

    fn response(
        &self,
        ui: &mut egui::Ui,
        c: CanvasMap,
        r: egui::Rect,
        id: impl Hash + Debug,
    ) -> egui::Response {
        ui.interact(c.rect(r), ui.make_persistent_id(id), egui::Sense::click())
    }

    #[allow(clippy::too_many_arguments)]
    fn segment_button(
        &self,
        ui: &mut egui::Ui,
        painter: &egui::Painter,
        c: CanvasMap,
        r: egui::Rect,
        label: &str,
        selected: bool,
        id: impl Hash + Debug,
    ) -> bool {
        let resp = self.response(ui, c, r, id);
        if selected {
            paint_accent_button(painter, c, r, resp.hovered(), 7);
        } else {
            paint_panel(
                painter,
                c,
                r,
                7,
                if resp.hovered() { PANEL_2 } else { FIELD },
                BORDER_DIM,
            );
        }
        painter.text(
            c.rect(r).center(),
            egui::Align2::CENTER_CENTER,
            label,
            c.font(14.0),
            if selected { TEXT } else { TEXT_MUTED },
        );
        resp.clicked()
    }

    fn outline_button(
        &self,
        ui: &mut egui::Ui,
        painter: &egui::Painter,
        c: CanvasMap,
        r: egui::Rect,
        label: &str,
        id: impl Hash + Debug,
    ) -> bool {
        let resp = self.response(ui, c, r, id);
        let fill = if resp.hovered() {
            alpha(PANEL_2, 255)
        } else {
            FIELD
        };
        paint_panel(painter, c, r, 5, fill, BORDER_DIM);
        match label {
            ">" => self.preview_icon(painter, c, r.center(), TEXT),
            _ => {
                painter.text(
                    c.rect(r).center(),
                    egui::Align2::CENTER_CENTER,
                    label,
                    c.font(13.0),
                    TEXT,
                );
            }
        }
        resp.clicked()
    }

    fn gradient_button(
        &self,
        ui: &mut egui::Ui,
        painter: &egui::Painter,
        c: CanvasMap,
        r: egui::Rect,
        label: &str,
        id: impl Hash + Debug,
    ) -> bool {
        let resp = self.response(ui, c, r, id);
        paint_accent_button(painter, c, r, resp.hovered(), 6);
        let center = r.center();
        if label.contains("Import") || label.contains("EXPORT") {
            self.upload_icon(painter, c, egui::pos2(center.x - 58.0, center.y), TEXT);
            painter.text(
                c.pos(center.x + 8.0, center.y),
                egui::Align2::CENTER_CENTER,
                label,
                c.font(15.0),
                TEXT,
            );
        } else {
            painter.text(
                c.rect(r).center(),
                egui::Align2::CENTER_CENTER,
                label,
                c.font(15.0),
                TEXT,
            );
        }
        resp.clicked()
    }

    fn icon_button(
        &self,
        ui: &mut egui::Ui,
        painter: &egui::Painter,
        c: CanvasMap,
        r: egui::Rect,
        label: &str,
        enabled: bool,
    ) -> bool {
        let resp = self.response(ui, c, r, label);
        paint_panel(painter, c, r, 6, FIELD, BORDER_DIM);
        let color = if enabled { TEXT } else { TEXT_DIM };
        match label {
            "Undo" => self.history_icon(painter, c, r.center(), false, color),
            "Redo" => self.history_icon(painter, c, r.center(), true, color),
            _ => {
                painter.text(
                    c.rect(r).center(),
                    egui::Align2::CENTER_CENTER,
                    label,
                    c.font(13.0),
                    color,
                );
            }
        };
        resp.clicked()
    }

    #[allow(clippy::too_many_arguments)]
    fn mode_button(
        &self,
        ui: &mut egui::Ui,
        painter: &egui::Painter,
        c: CanvasMap,
        r: egui::Rect,
        icon: &str,
        label: &str,
        selected: bool,
        id: impl Hash + Debug,
    ) -> bool {
        let enabled = !self.edits_locked(); // content mode is a setting (#36)
        let resp = self.response(ui, c, r, id);
        if selected {
            paint_accent_button(painter, c, r, resp.hovered(), 6);
        } else {
            paint_panel(
                painter,
                c,
                r,
                6,
                if resp.hovered() { CARD } else { PANEL_2 },
                BORDER_DIM,
            );
        }
        let icon_color = if selected { TEXT } else { TEXT_MUTED };
        match icon {
            "Mic" => self.mic_icon(
                painter,
                c,
                egui::pos2(r.center().x, r.top() + 35.0),
                icon_color,
            ),
            "Note" => self.note_icon(
                painter,
                c,
                egui::pos2(r.center().x, r.top() + 34.0),
                icon_color,
            ),
            _ => {
                painter.text(
                    c.pos(r.center().x, r.top() + 32.0),
                    egui::Align2::CENTER_CENTER,
                    icon,
                    c.font(24.0),
                    icon_color,
                );
            }
        }
        painter.text(
            c.pos(r.center().x, r.bottom() - 22.0),
            egui::Align2::CENTER_CENTER,
            label,
            c.font(14.0),
            TEXT,
        );
        if !enabled {
            self.paint_locked_scrim(painter, c, r);
        }
        enabled && resp.clicked()
    }

    #[allow(clippy::too_many_arguments)]
    fn field_row(
        &mut self,
        ui: &mut egui::Ui,
        painter: &egui::Painter,
        c: CanvasMap,
        x: f32,
        y: f32,
        label: &str,
        value: &str,
        id: impl Hash + Debug,
    ) {
        self.small_text(painter, c, x, y + 20.0, label, TEXT_MUTED);
        let field = rect(x + 124.0, y, 172.0, 40.0);
        match output_field_chrome(label) {
            OutputFieldChrome::Selector => {
                if self.outline_button(ui, painter, c, field, value, id) {
                    match label {
                        "Format" => {
                            let next = if self.session.output_format() == OutputFormat::Wav {
                                OutputFormat::Flac
                            } else {
                                OutputFormat::Wav
                            };
                            self.apply_edit("format", |s| s.set_output_format(next));
                        }
                        "Bit Depth" => {
                            let current = self
                                .session
                                .settings()
                                .export
                                .bit_depth_override
                                .or_else(|| self.session.input().map(|b| b.bits_per_sample()))
                                .unwrap_or(24);
                            let next = Some(if current >= 24 { 16 } else { 24 });
                            self.apply_edit("bit_depth", |s| s.set_bit_depth(next));
                        }
                        "Loudness Target" => {
                            let next = cycle_loudness(self.session.loudness_target());
                            if self.apply_edit("loudness", |s| s.set_loudness_target(next)) {
                                self.stop_playback(); // target change invalidates the render
                            }
                        }
                        _ => {}
                    }
                }
                painter.text(
                    c.pos(field.right() - 22.0, field.center().y),
                    egui::Align2::CENTER_CENTER,
                    "v",
                    c.font(14.0),
                    TEXT_MUTED,
                );
            }
            OutputFieldChrome::ReadOnly => {
                paint_panel(painter, c, field, 5, FIELD, BORDER_DIM);
                self.body_text(
                    painter,
                    c,
                    field.left() + 14.0,
                    field.center().y,
                    value,
                    TEXT,
                );
            }
        }
    }
}

impl YesVoxApp {
    /// Dim a control whose edits are locked while a job runs (#36).
    fn paint_locked_scrim(&self, painter: &egui::Painter, c: CanvasMap, r: egui::Rect) {
        painter.rect_filled(c.rect(r), 4, egui::Color32::from_black_alpha(110));
    }

    /// A settings slider: locked (pointer ignored, painted dimmed) while a job
    /// is in flight (#36). Playback-only sliders use [`live_slider`](Self::live_slider).
    #[allow(clippy::too_many_arguments)]
    fn slider(
        &self,
        ui: &mut egui::Ui,
        painter: &egui::Painter,
        c: CanvasMap,
        r: egui::Rect,
        value: f32,
        id: impl Hash + Debug,
        start: egui::Color32,
        end: egui::Color32,
    ) -> f32 {
        self.live_slider(
            ui,
            painter,
            c,
            r,
            value,
            id,
            start,
            end,
            !self.edits_locked(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn live_slider(
        &self,
        ui: &mut egui::Ui,
        painter: &egui::Painter,
        c: CanvasMap,
        r: egui::Rect,
        value: f32,
        id: impl Hash + Debug,
        start: egui::Color32,
        end: egui::Color32,
        enabled: bool,
    ) -> f32 {
        let hit = r.expand(10.0);
        let resp = ui.interact(
            c.rect(hit),
            ui.make_persistent_id(id),
            egui::Sense::click_and_drag(),
        );
        let mut next = value.clamp(0.0, 1.0);
        if enabled && (resp.clicked() || resp.dragged()) && resp.interact_pointer_pos().is_some() {
            let pos = resp.interact_pointer_pos().unwrap();
            let sr = c.rect(r);
            next = ((pos.x - sr.left()) / sr.width()).clamp(0.0, 1.0);
        }
        paint_panel(painter, c, r, 3, FIELD, BORDER_DIM);
        let filled = rect(r.left(), r.top(), r.width() * next, r.height());
        paint_h_gradient(painter, c, filled, start, end, 20, 3);
        let knob = c.pos(r.left() + r.width() * next, r.center().y);
        painter.circle_filled(
            knob + egui::vec2(0.0, c.v(1.0)),
            c.v(if resp.hovered() { 7.5 } else { 6.5 }),
            egui::Color32::from_black_alpha(150),
        );
        painter.circle_filled(
            knob,
            c.v(if resp.hovered() { 7.0 } else { 6.0 }),
            egui::Color32::from_rgb(206, 214, 232),
        );
        painter.circle_filled(
            knob - egui::vec2(c.v(1.2), c.v(1.6)),
            c.v(if resp.hovered() { 4.0 } else { 3.4 }),
            egui::Color32::from_rgb(245, 247, 252),
        );
        painter.circle_stroke(knob, c.v(6.0), egui::Stroke::new(c.v(1.0), BORDER));
        if !enabled {
            self.paint_locked_scrim(painter, c, r.expand(6.0));
        }
        next
    }

    fn radial_control(
        &self,
        ui: &mut egui::Ui,
        painter: &egui::Painter,
        c: CanvasMap,
        r: egui::Rect,
        value: f32,
        id: impl Hash + Debug,
    ) -> f32 {
        let enabled = !self.edits_locked(); // settings dial — locked mid-job (#36)
        let resp = ui.interact(
            c.rect(r),
            ui.make_persistent_id(id),
            egui::Sense::click_and_drag(),
        );
        let mut next = value.clamp(0.0, 1.0);
        if enabled && (resp.clicked() || resp.dragged()) && resp.interact_pointer_pos().is_some() {
            let p = resp.interact_pointer_pos().unwrap();
            let center = c.pos(r.center().x, r.top() + 80.0);
            let angle = (p.y - center.y).atan2(p.x - center.x);
            let start = KNOB_SWEEP_START_DEG.to_radians();
            let end = (KNOB_SWEEP_START_DEG + KNOB_SWEEP_DEGREES).to_radians();
            let mut t = (angle - start) / (end - start);
            if t < -0.2 {
                t += 1.0;
            }
            next = t.clamp(0.0, 1.0);
        }
        let center = c.pos(r.center().x, r.top() + 78.0);
        let radius = c.v(48.0);
        paint_hardware_knob(
            painter,
            c,
            center,
            radius,
            next,
            &format!("{:.0}%", next * 100.0),
            None,
        );
        if !enabled {
            self.paint_locked_scrim(painter, c, r);
        }
        next
    }

    fn toggle(
        &self,
        ui: &mut egui::Ui,
        painter: &egui::Painter,
        c: CanvasMap,
        r: egui::Rect,
        on: bool,
        id: impl Hash + Debug,
    ) -> bool {
        let enabled = !self.edits_locked(); // settings toggle — locked mid-job (#36)
        let resp = self.response(ui, c, r, id);
        let sr = c.rect(r);
        paint_panel(
            painter,
            c,
            r,
            10,
            if on {
                BLUE
            } else {
                egui::Color32::from_rgb(34, 47, 69)
            },
            egui::Color32::TRANSPARENT,
        );
        let x = if on {
            sr.right() - sr.height() / 2.0
        } else {
            sr.left() + sr.height() / 2.0
        };
        painter.circle_filled(egui::pos2(x, sr.center().y), sr.height() * 0.40, TEXT);
        if !enabled {
            self.paint_locked_scrim(painter, c, r);
        }
        enabled && resp.clicked()
    }

    /// A settings checkbox: locked while a job is in flight (#36). The
    /// playback-only Volume Match box uses [`live_checkbox`](Self::live_checkbox).
    fn checkbox(
        &self,
        ui: &mut egui::Ui,
        painter: &egui::Painter,
        c: CanvasMap,
        r: egui::Rect,
        checked: bool,
        id: impl Hash + Debug,
    ) -> bool {
        self.live_checkbox(ui, painter, c, r, checked, id, !self.edits_locked())
    }

    #[allow(clippy::too_many_arguments)]
    fn live_checkbox(
        &self,
        ui: &mut egui::Ui,
        painter: &egui::Painter,
        c: CanvasMap,
        r: egui::Rect,
        checked: bool,
        id: impl Hash + Debug,
        enabled: bool,
    ) -> bool {
        let resp = self.response(ui, c, r, id);
        paint_panel(painter, c, r, 3, if checked { BLUE } else { FIELD }, BORDER);
        if checked {
            painter.line_segment(
                [
                    c.pos(r.left() + 5.0, r.center().y),
                    c.pos(r.left() + 9.0, r.bottom() - 6.0),
                ],
                egui::Stroke::new(c.v(2.0), TEXT),
            );
            painter.line_segment(
                [
                    c.pos(r.left() + 9.0, r.bottom() - 6.0),
                    c.pos(r.right() - 5.0, r.top() + 6.0),
                ],
                egui::Stroke::new(c.v(2.0), TEXT),
            );
        }
        if !enabled {
            self.paint_locked_scrim(painter, c, r);
        }
        enabled && resp.clicked()
    }
}

impl YesVoxApp {
    fn progress_bar(&self, painter: &egui::Painter, c: CanvasMap, r: egui::Rect, value: f32) {
        paint_panel(
            painter,
            c,
            r,
            3,
            egui::Color32::from_rgb(22, 44, 70),
            egui::Color32::TRANSPARENT,
        );
        paint_h_gradient(
            painter,
            c,
            rect(
                r.left(),
                r.top(),
                r.width() * value.clamp(0.0, 1.0),
                r.height(),
            ),
            BLUE,
            PURPLE,
            18,
            3,
        );
    }

    fn percent_knob(&self, painter: &egui::Painter, c: CanvasMap, center: egui::Pos2, value: f32) {
        let sc = c.pos(center.x, center.y);
        let r = c.v(27.0);
        paint_hardware_knob(
            painter,
            c,
            sc,
            r,
            value,
            &format!("{:.0}%", value * 100.0),
            None,
        );
    }

    /// An interactive percent knob: vertical drag (up = increase) adjusts the
    /// value, which is returned clamped to `[0, 1]`. Paints the [`percent_knob`]
    /// visual. Used to wire the card amount dials to `set_card_amount` (#24).
    fn knob_control(
        &self,
        ui: &mut egui::Ui,
        painter: &egui::Painter,
        c: CanvasMap,
        center: egui::Pos2,
        value: f32,
        id: impl Hash + Debug,
    ) -> f32 {
        let enabled = !self.edits_locked(); // settings knob — locked mid-job (#36)
        let hit = rect(center.x - 30.0, center.y - 30.0, 60.0, 60.0);
        let resp = ui.interact(
            c.rect(hit),
            ui.make_persistent_id(id),
            egui::Sense::click_and_drag(),
        );
        let mut next = value.clamp(0.0, 1.0);
        if enabled && resp.dragged() {
            // Screen-space vertical drag; ~250 px sweeps the full range.
            next = (next - resp.drag_delta().y * 0.004).clamp(0.0, 1.0);
        }
        self.percent_knob(painter, c, center, next);
        if !enabled {
            self.paint_locked_scrim(painter, c, hit.shrink(2.0));
        }
        next
    }

    fn loudness_knob(&self, painter: &egui::Painter, c: CanvasMap, center: egui::Pos2) {
        let sc = c.pos(center.x, center.y);
        let r = c.v(27.0);
        paint_hardware_knob(painter, c, sc, r, 0.68, "-14.0", Some("LUFS"));
    }

    fn loudness_target_knob(
        &self,
        painter: &egui::Painter,
        c: CanvasMap,
        center: egui::Pos2,
        target: LoudnessTarget,
    ) {
        let sc = c.pos(center.x, center.y);
        let r = c.v(27.0);
        let (main, sub) = match target {
            LoudnessTarget::Podcast => ("-16", "LUFS"),
            LoudnessTarget::Streaming => ("-14", "LUFS"),
            LoudnessTarget::MatchInput => ("Match", "LUFS"),
            LoudnessTarget::PeakNormalize => ("Peak", "dBTP"),
            LoudnessTarget::Custom(_) => ("Custom", "LUFS"),
        };
        paint_hardware_knob(painter, c, sc, r, 0.68, main, Some(sub));
    }

    fn vertical_meter(&self, painter: &egui::Painter, c: CanvasMap, r: egui::Rect, value: f32) {
        paint_panel(
            painter,
            c,
            r,
            2,
            egui::Color32::from_rgb(19, 34, 54),
            egui::Color32::TRANSPARENT,
        );
        let fill_h = r.height() * value.clamp(0.0, 1.0);
        let fill = rect(r.left(), r.bottom() - fill_h, r.width(), fill_h);
        paint_v_gradient(painter, c.rect(fill), AMBER, BLUE, 20);
    }

    fn draw_spectrum(
        &self,
        painter: &egui::Painter,
        c: CanvasMap,
        r: egui::Rect,
        difference: Option<&AudioBuffer>,
    ) {
        paint_panel(painter, c, r, 4, FIELD, BORDER_DIM);
        let cols = r.width().round() as usize;
        let peaks = difference.map(|b| yesvox_core::monitor::waveform_peaks(b, 0, cols.max(1)));
        for i in (0..cols).step_by(2) {
            let t = i as f32 / cols.max(1) as f32;
            let h = peaks
                .as_ref()
                .and_then(|p| p.get(i).copied())
                .map(|(lo, hi)| (lo.abs().max(hi.abs()).clamp(0.02, 1.0)) * r.height())
                .unwrap_or(0.0);
            let color = if t < 0.5 {
                lerp_color(AMBER, PURPLE, t * 2.0)
            } else {
                lerp_color(PURPLE, BLUE, (t - 0.5) * 2.0)
            };
            painter.line_segment(
                [
                    c.pos(r.left() + i as f32, r.bottom()),
                    c.pos(r.left() + i as f32, r.bottom() - h),
                ],
                egui::Stroke::new(c.v(1.5), alpha(color, 190)),
            );
        }
    }

    fn draw_mini_wave(&self, painter: &egui::Painter, c: CanvasMap, r: egui::Rect) {
        let n = 9;
        for i in 0..n {
            let t = i as f32 / (n - 1) as f32;
            let h = r.height() * (0.25 + (t * 10.0).sin().abs() * 0.65);
            let x = r.left() + t * r.width();
            painter.line_segment(
                [
                    c.pos(x, r.center().y - h / 2.0),
                    c.pos(x, r.center().y + h / 2.0),
                ],
                egui::Stroke::new(c.v(2.0), CYAN),
            );
        }
    }

    fn card_icon(&self, painter: &egui::Painter, c: CanvasMap, center: egui::Pos2, card: CardId) {
        let sc = c.pos(center.x, center.y);
        painter.circle_stroke(sc, c.v(14.0), egui::Stroke::new(c.v(1.0), BORDER));
        let glyph = match card {
            CardId::Denoise
            | CardId::DeReverb
            | CardId::DeClickDePop
            | CardId::DeHum
            | CardId::Leveling => "|||",
            CardId::MouthClicks => "<>",
            CardId::DeEss => "S",
            CardId::Breath => "~",
            CardId::PresenceAir => "+",
            CardId::Loudness => "L",
        };
        painter.text(sc, egui::Align2::CENTER_CENTER, glyph, c.font(11.0), TEXT);
    }

    fn badge(
        &self,
        painter: &egui::Painter,
        c: CanvasMap,
        r: egui::Rect,
        label: &str,
        filled: bool,
    ) {
        paint_panel(
            painter,
            c,
            r,
            10,
            if filled { alpha(VIOLET, 100) } else { FIELD },
            if filled { VIOLET } else { BORDER_DIM },
        );
        painter.text(
            c.rect(r).center(),
            egui::Align2::CENTER_CENTER,
            label,
            c.font(11.0),
            if filled { CYAN } else { TEXT_MUTED },
        );
    }

    fn star(
        &self,
        painter: &egui::Painter,
        c: CanvasMap,
        center: egui::Pos2,
        radius: f32,
        color: egui::Color32,
    ) {
        let sc = c.pos(center.x, center.y);
        painter.line_segment(
            [
                sc + egui::vec2(0.0, -c.v(radius)),
                sc + egui::vec2(0.0, c.v(radius)),
            ],
            egui::Stroke::new(c.v(2.0), color),
        );
        painter.line_segment(
            [
                sc + egui::vec2(-c.v(radius), 0.0),
                sc + egui::vec2(c.v(radius), 0.0),
            ],
            egui::Stroke::new(c.v(2.0), color),
        );
        painter.circle_filled(sc, c.v(4.0), color);
    }

    fn folder_icon(&self, painter: &egui::Painter, c: CanvasMap, pos: egui::Pos2) {
        let r = rect(pos.x, pos.y, 18.0, 13.0);
        painter.rect_stroke(
            c.rect(r),
            egui::CornerRadius::same(2),
            egui::Stroke::new(c.v(1.2), TEXT_MUTED),
            egui::StrokeKind::Inside,
        );
        painter.line_segment(
            [
                c.pos(r.left() + 2.0, r.top()),
                c.pos(r.left() + 7.0, r.top() - 4.0),
            ],
            egui::Stroke::new(c.v(1.2), TEXT_MUTED),
        );
    }

    fn radar(&self, painter: &egui::Painter, c: CanvasMap, center: egui::Pos2) {
        let sc = c.pos(center.x, center.y);
        for r in [12.0, 24.0, 34.0] {
            painter.circle_stroke(sc, c.v(r), egui::Stroke::new(c.v(1.0), alpha(CYAN, 120)));
        }
        painter.line_segment(
            [sc, sc + egui::vec2(c.v(24.0), -c.v(18.0))],
            egui::Stroke::new(c.v(2.0), BLUE),
        );
        painter.circle_filled(sc, c.v(5.0), CYAN);
    }
}

fn card_blurb(card: CardId) -> (&'static str, &'static str) {
    match card {
        CardId::Denoise => ("Reduces background noise", "while preserving detail."),
        CardId::DeReverb => ("Removes reverb while", "preserving natural space."),
        CardId::DeClickDePop => ("Removes clicks, pops", "and crackle."),
        CardId::MouthClicks => ("Reduces lip smacks", "and mouth noises."),
        CardId::DeHum => ("Removes electrical hum", "and low frequency noise."),
        CardId::DeEss => ("Controls harsh sibilance", "without dulling."),
        CardId::Breath => ("Attenuates harsh breaths", "(voiceover-leaning)."),
        CardId::PresenceAir => ("Adds clarity and air", "in an adaptive way."),
        CardId::Leveling => ("Smooths vocal level", "consistently."),
        CardId::Loudness => ("Sets loudness to", "target level."),
    }
}

fn draw_advanced_spectrogram(
    painter: &egui::Painter,
    c: CanvasMap,
    r: egui::Rect,
    spectrogram: Option<&yesvox_core::monitor::Spectrogram>,
) {
    paint_panel(painter, c, r, 4, FIELD, BORDER_DIM);
    let Some(spectrogram) = spectrogram else {
        return;
    };
    if spectrogram.columns == 0 || spectrogram.rows == 0 {
        return;
    }

    let cell_w = r.width() / spectrogram.columns as f32;
    let cell_h = r.height() / spectrogram.rows as f32;
    for x in 0..spectrogram.columns {
        for y in 0..spectrogram.rows {
            let value = spectrogram.cell(x, y);
            if value <= 0.001 {
                continue;
            }
            let draw_y = spectrogram.rows - 1 - y;
            let cell = rect(
                r.left() + x as f32 * cell_w,
                r.top() + draw_y as f32 * cell_h,
                cell_w + 0.2,
                cell_h + 0.2,
            );
            painter.rect_filled(
                c.rect(cell),
                egui::CornerRadius::ZERO,
                spectrogram_color(value),
            );
        }
    }

    for t in [0.25, 0.5, 0.75] {
        let x = r.left() + r.width() * t;
        painter.line_segment(
            [c.pos(x, r.top()), c.pos(x, r.bottom())],
            egui::Stroke::new(c.v(0.75), alpha(BORDER, 78)),
        );
    }
    for t in [0.33, 0.66] {
        let y = r.top() + r.height() * t;
        painter.line_segment(
            [c.pos(r.left(), y), c.pos(r.right(), y)],
            egui::Stroke::new(c.v(0.75), alpha(BORDER, 62)),
        );
    }
}

fn spectrogram_color(value: f32) -> egui::Color32 {
    let value = value.clamp(0.0, 1.0).powf(0.72);
    if value < 0.28 {
        lerp_color(alpha(BLUE, 46), BLUE, value / 0.28)
    } else if value < 0.58 {
        lerp_color(BLUE, CYAN, (value - 0.28) / 0.30)
    } else if value < 0.82 {
        lerp_color(CYAN, AMBER, (value - 0.58) / 0.24)
    } else {
        lerp_color(AMBER, TEXT, (value - 0.82) / 0.18)
    }
}

fn paint_panel(
    painter: &egui::Painter,
    c: CanvasMap,
    r: egui::Rect,
    radius: u8,
    fill: egui::Color32,
    stroke: egui::Color32,
) {
    let sr = c.rect(r);
    let elevated = radius >= 6 && r.width() >= 100.0 && r.height() >= 38.0;
    if elevated {
        painter.rect_filled(
            sr.translate(egui::vec2(0.0, c.v(3.0))),
            egui::CornerRadius::same(radius),
            egui::Color32::from_black_alpha(88),
        );
    }
    painter.rect_filled(sr, egui::CornerRadius::same(radius), fill);
    if elevated {
        let sheen = egui::Rect::from_min_max(
            sr.min + egui::vec2(c.v(1.0), c.v(1.0)),
            egui::pos2(sr.right() - c.v(1.0), sr.top() + c.v(2.0)),
        );
        painter.rect_filled(
            sheen,
            egui::CornerRadius::same(radius.saturating_sub(1)),
            egui::Color32::from_white_alpha(18),
        );
    }
    if stroke != egui::Color32::TRANSPARENT {
        painter.rect_stroke(
            sr,
            egui::CornerRadius::same(radius),
            egui::Stroke::new(c.v(1.0), stroke),
            egui::StrokeKind::Inside,
        );
    }
}

fn paint_sidebar_section(painter: &egui::Painter, c: CanvasMap, r: egui::Rect) {
    painter.line_segment(
        [c.pos(r.left(), r.top()), c.pos(r.right(), r.top())],
        egui::Stroke::new(c.v(1.0), BORDER_DIM),
    );
}

fn paint_accent_button(
    painter: &egui::Painter,
    c: CanvasMap,
    r: egui::Rect,
    hovered: bool,
    radius: u8,
) {
    let sr = c.rect(r);
    painter.rect_filled(
        sr.translate(egui::vec2(0.0, c.v(3.0))),
        egui::CornerRadius::same(radius),
        egui::Color32::from_rgba_premultiplied(55, 70, 180, 92),
    );
    painter.rect_filled(
        sr,
        egui::CornerRadius::same(radius),
        if hovered { BLUE } else { VIOLET },
    );
    let steps = 20;
    for i in 0..steps {
        let t0 = i as f32 / steps as f32;
        let t1 = (i + 1) as f32 / steps as f32;
        let top = sr.top() + sr.height() * t0;
        let bottom = sr.top() + sr.height() * t1 + c.v(0.5);
        let stripe = egui::Rect::from_min_max(
            egui::pos2(sr.left() + c.v(2.0), top),
            egui::pos2(sr.right() - c.v(2.0), bottom),
        );
        let bright = if hovered { CYAN } else { BLUE };
        painter.rect_filled(
            stripe,
            egui::CornerRadius::ZERO,
            lerp_color(bright, PURPLE, t0),
        );
    }
    painter.line_segment(
        [
            sr.left_top() + egui::vec2(c.v(5.0), c.v(1.0)),
            sr.right_top() + egui::vec2(-c.v(5.0), c.v(1.0)),
        ],
        egui::Stroke::new(c.v(1.0), egui::Color32::from_white_alpha(52)),
    );
    painter.rect_stroke(
        sr,
        egui::CornerRadius::same(radius),
        egui::Stroke::new(c.v(1.0), alpha(CYAN, if hovered { 170 } else { 90 })),
        egui::StrokeKind::Inside,
    );
}

#[allow(clippy::too_many_arguments)]
fn paint_hardware_knob(
    painter: &egui::Painter,
    c: CanvasMap,
    center: egui::Pos2,
    radius: f32,
    value: f32,
    main: &str,
    sub: Option<&str>,
) {
    let value = value.clamp(0.0, 1.0);
    let tick_inner = radius * 0.92;
    let tick_outer = radius * 1.08;

    painter.circle_filled(
        center + egui::vec2(0.0, c.v(2.0)),
        radius * 0.92,
        egui::Color32::from_black_alpha(180),
    );
    painter.circle_filled(center, radius * 0.88, FIELD);
    painter.circle_stroke(
        center,
        radius * 0.88,
        egui::Stroke::new(c.v(1.0), BORDER_DIM),
    );

    for i in 0..=KNOB_TICK_COUNT {
        let t = i as f32 / KNOB_TICK_COUNT as f32;
        let angle = (KNOB_SWEEP_START_DEG + KNOB_SWEEP_DEGREES * t).to_radians();
        let major = i % KNOB_MAJOR_EVERY == 0;
        let inner = if major { tick_inner * 0.96 } else { tick_inner };
        let p0 = center + egui::vec2(angle.cos() * inner, angle.sin() * inner);
        let p1 = center + egui::vec2(angle.cos() * tick_outer, angle.sin() * tick_outer);
        painter.line_segment(
            [p0, p1],
            egui::Stroke::new(
                c.v(if major { 1.6 } else { 0.8 }),
                if t <= value {
                    alpha(CYAN, 210)
                } else {
                    TEXT_DIM
                },
            ),
        );
    }

    arc(
        painter,
        center,
        radius * 0.79,
        KNOB_SWEEP_START_DEG,
        KNOB_SWEEP_START_DEG + KNOB_SWEEP_DEGREES,
        egui::Stroke::new(c.v((radius / c.scale * 0.075).max(2.0)), BORDER),
    );
    arc(
        painter,
        center,
        radius * 0.79,
        KNOB_SWEEP_START_DEG,
        KNOB_SWEEP_START_DEG + KNOB_SWEEP_DEGREES * value,
        egui::Stroke::new(c.v((radius / c.scale * 0.075).max(2.0)), BLUE),
    );

    let cap_r = radius * 0.63;
    painter.circle_filled(center, cap_r + c.v(1.0), egui::Color32::from_rgb(5, 7, 12));
    painter.circle_filled(center, cap_r, egui::Color32::from_rgb(48, 56, 73));
    painter.circle_filled(
        center - egui::vec2(0.0, cap_r * 0.16),
        cap_r * 0.82,
        egui::Color32::from_rgb(92, 103, 126),
    );
    painter.circle_filled(
        center + egui::vec2(0.0, cap_r * 0.12),
        cap_r * 0.72,
        egui::Color32::from_rgb(39, 45, 59),
    );
    painter.circle_stroke(center, cap_r, egui::Stroke::new(c.v(1.0), BORDER));

    let angle = (KNOB_SWEEP_START_DEG + KNOB_SWEEP_DEGREES * value).to_radians();
    let indicator_start =
        center + egui::vec2(angle.cos() * cap_r * 0.48, angle.sin() * cap_r * 0.48);
    let indicator_end = center + egui::vec2(angle.cos() * cap_r * 0.82, angle.sin() * cap_r * 0.82);
    painter.line_segment(
        [indicator_start, indicator_end],
        egui::Stroke::new(c.v(1.8), TEXT),
    );

    let text_y = if sub.is_some() { -c.v(2.5) } else { c.v(2.0) };
    painter.text(
        center + egui::vec2(0.0, text_y),
        egui::Align2::CENTER_CENTER,
        main,
        c.font(if radius / c.scale > 35.0 { 16.0 } else { 11.5 }),
        TEXT,
    );
    if let Some(sub) = sub {
        painter.text(
            center + egui::vec2(0.0, c.v(9.0)),
            egui::Align2::CENTER_CENTER,
            sub,
            c.font(7.5),
            TEXT_MUTED,
        );
    }
}

fn paint_h_gradient(
    painter: &egui::Painter,
    c: CanvasMap,
    r: egui::Rect,
    start: egui::Color32,
    end: egui::Color32,
    steps: usize,
    radius: u8,
) {
    let sr = c.rect(r);
    let steps = steps.max(1);
    for i in 0..steps {
        let t0 = i as f32 / steps as f32;
        let t1 = (i + 1) as f32 / steps as f32;
        let x0 = sr.left() + sr.width() * t0;
        let x1 = sr.left() + sr.width() * t1 + 0.5;
        let rr = egui::Rect::from_min_max(egui::pos2(x0, sr.top()), egui::pos2(x1, sr.bottom()));
        painter.rect_filled(
            rr,
            egui::CornerRadius::same(if i == 0 || i + 1 == steps { radius } else { 0 }),
            lerp_color(start, end, t0),
        );
    }
}

fn paint_v_gradient(
    painter: &egui::Painter,
    r: egui::Rect,
    top: egui::Color32,
    bottom: egui::Color32,
    steps: usize,
) {
    let steps = steps.max(1);
    for i in 0..steps {
        let t0 = i as f32 / steps as f32;
        let t1 = (i + 1) as f32 / steps as f32;
        let y0 = r.top() + r.height() * t0;
        let y1 = r.top() + r.height() * t1 + 0.5;
        let rr = egui::Rect::from_min_max(egui::pos2(r.left(), y0), egui::pos2(r.right(), y1));
        painter.rect_filled(rr, egui::CornerRadius::ZERO, lerp_color(top, bottom, t0));
    }
}

fn arc(
    painter: &egui::Painter,
    center: egui::Pos2,
    radius: f32,
    start_deg: f32,
    end_deg: f32,
    stroke: egui::Stroke,
) {
    let steps = ((end_deg - start_deg).abs() / 6.0).ceil().max(2.0) as usize;
    let mut last = None;
    for i in 0..=steps {
        let t = i as f32 / steps as f32;
        let a = (start_deg + (end_deg - start_deg) * t).to_radians();
        let p = center + egui::vec2(a.cos() * radius, a.sin() * radius);
        if let Some(prev) = last {
            painter.line_segment([prev, p], stroke);
        }
        last = Some(p);
    }
}

fn lerp_color(a: egui::Color32, b: egui::Color32, t: f32) -> egui::Color32 {
    let t = t.clamp(0.0, 1.0);
    let [ar, ag, ab, aa] = a.to_array();
    let [br, bg, bb, ba] = b.to_array();
    egui::Color32::from_rgba_premultiplied(
        lerp_u8(ar, br, t),
        lerp_u8(ag, bg, t),
        lerp_u8(ab, bb, t),
        lerp_u8(aa, ba, t),
    )
}

#[cfg(test)]
fn relative_luminance(color: egui::Color32) -> f32 {
    let [r, g, b, _] = color.to_array();
    0.2126 * f32::from(r) + 0.7152 * f32::from(g) + 0.0722 * f32::from(b)
}

fn lerp_u8(a: u8, b: u8, t: f32) -> u8 {
    (a as f32 + (b as f32 - a as f32) * t).round() as u8
}

fn alpha(c: egui::Color32, a: u8) -> egui::Color32 {
    let [r, g, b, _] = c.to_array();
    egui::Color32::from_rgba_unmultiplied(r, g, b, a)
}

fn batch_output_path(input: &Path, format: OutputFormat) -> PathBuf {
    let stem = input
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("vocal");
    let name = format!("{stem}_YesVox.{}", format.extension());
    input
        .parent()
        .map(|parent| parent.join(&name))
        .unwrap_or_else(|| PathBuf::from(name))
}

fn format_batch_status(summary: &BatchSummary) -> String {
    if summary.canceled {
        format!(
            "Batch canceled - {} exported - {} failed",
            summary.done, summary.failed
        )
    } else {
        format!(
            "Batch exported {} - {} failed",
            summary.done, summary.failed
        )
    }
}

fn truncate_middle(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_owned();
    }
    let keep = max_chars.saturating_sub(3);
    let front = keep / 2;
    let back = keep - front;
    let prefix: String = value.chars().take(front).collect();
    let suffix: String = value
        .chars()
        .rev()
        .take(back)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("{prefix}...{suffix}")
}

fn loudness_label(t: LoudnessTarget) -> String {
    match t {
        LoudnessTarget::Podcast => "-16 LUFS".to_owned(),
        LoudnessTarget::Streaming => "-14.0 LUFS".to_owned(),
        LoudnessTarget::MatchInput => "Match input".to_owned(),
        LoudnessTarget::PeakNormalize => "Peak norm".to_owned(),
        LoudnessTarget::Custom(v) => format!("{v:.1} LUFS"),
    }
}

/// Cycle the loudness target through the product.md §7 list, including one
/// `Custom` value (#24). A dedicated numeric editor for Custom is a later polish.
fn cycle_loudness(t: LoudnessTarget) -> LoudnessTarget {
    match t {
        LoudnessTarget::Podcast => LoudnessTarget::Streaming,
        LoudnessTarget::Streaming => LoudnessTarget::MatchInput,
        LoudnessTarget::MatchInput => LoudnessTarget::PeakNormalize,
        LoudnessTarget::PeakNormalize => LoudnessTarget::Custom(-18.0),
        LoudnessTarget::Custom(_) => LoudnessTarget::Podcast,
    }
}

fn preset_gen_badge_visible(session: &Session) -> bool {
    session.generative_reconstruction_badge()
}

fn format_duration(seconds: f32) -> String {
    let total = seconds.max(0.0).round() as u32;
    format!("{}:{:02}", total / 60, total % 60)
}

fn lufs_label(value: Option<f32>) -> String {
    value
        .map(|v| format!("{v:.1} LUFS"))
        .unwrap_or_else(|| "-- LUFS".to_owned())
}

fn db_label(value: Option<f32>) -> String {
    value
        .map(|v| format!("{v:.1} dB"))
        .unwrap_or_else(|| "-- dB".to_owned())
}

fn meter_value(buffer: Option<&AudioBuffer>, ch: usize) -> f32 {
    let Some(buffer) = buffer else {
        return 0.0;
    };
    if ch >= buffer.channel_count() {
        return 0.0;
    }
    let peak = yesvox_core::monitor::channel_peak_db(buffer, ch);
    ((peak + 48.0) / 48.0).clamp(0.0, 1.0)
}

fn install_theme(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = PANEL;
    visuals.window_fill = BG_TOP;
    visuals.extreme_bg_color = BG_TOP;
    visuals.selection.bg_fill = BLUE;
    visuals.selection.stroke = egui::Stroke::new(1.0, CYAN);
    visuals.widgets.inactive.bg_fill = PANEL_2;
    visuals.widgets.inactive.weak_bg_fill = PANEL_2;
    visuals.widgets.inactive.bg_stroke = egui::Stroke::new(1.0, BORDER);
    visuals.widgets.hovered.bg_fill = BORDER;
    visuals.widgets.hovered.weak_bg_fill = BORDER;
    visuals.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, CYAN);
    visuals.widgets.active.bg_fill = BORDER;
    visuals.widgets.active.weak_bg_fill = BORDER;
    visuals.hyperlink_color = CYAN;
    ctx.set_visuals(visuals);
    ctx.all_styles_mut(|style| {
        style.spacing.item_spacing = egui::vec2(0.0, 0.0);
        style.spacing.button_padding = egui::vec2(0.0, 0.0);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use yesvox_core::QualityMode;

    #[test]
    fn default_viewport_matches_reference_canvas() {
        assert_eq!(DEFAULT_VIEWPORT_SIZE, [1920.0, 1080.0]);
        assert_eq!(MIN_VIEWPORT_SIZE, [1280.0, 720.0]);
        assert_eq!(
            DEFAULT_VIEWPORT_SIZE[0] / DEFAULT_VIEWPORT_SIZE[1],
            16.0 / 9.0
        );
        assert_eq!(MIN_VIEWPORT_SIZE[0] / MIN_VIEWPORT_SIZE[1], 16.0 / 9.0);
    }

    #[test]
    fn suite_brand_chrome_keeps_the_primary_view_switch_centered() {
        let top = ReferenceLayout::new().top;
        let [standard, advanced] = YesVoxApp::top_bar_view_rects();
        let pair = standard.union(advanced);

        assert!((pair.center().x - top.center().x).abs() < 1.0);
        assert!(!standard.intersects(advanced));
        assert!(!pair.intersects(YesVoxApp::top_bar_drag_rect()));
        assert!(top.contains(pair.min) && top.contains(pair.max));
    }

    #[test]
    fn suite_brand_palette_is_graphite_with_ordered_surface_depth() {
        assert!(relative_luminance(BG_TOP) < relative_luminance(PANEL));
        assert!(relative_luminance(PANEL) < relative_luminance(PANEL_2));
        assert!(relative_luminance(FIELD) < relative_luminance(PANEL));
        assert!(relative_luminance(TEXT_DIM) < relative_luminance(TEXT_MUTED));
        assert!(relative_luminance(TEXT_MUTED) < relative_luminance(TEXT));
    }

    #[test]
    fn suite_brand_knobs_share_the_yes_master_hardware_geometry() {
        assert_eq!(KNOB_SWEEP_START_DEG, -225.0);
        assert_eq!(KNOB_SWEEP_DEGREES, 270.0);
        assert_eq!(KNOB_TICK_COUNT, 28);
        assert_eq!(KNOB_MAJOR_EVERY, 7);
    }

    #[test]
    fn layout_keeps_reference_dashboard_regions() {
        let l = ReferenceLayout::new();
        assert_eq!(l.top.height(), 70.0);
        assert_eq!(l.status.top(), 1040.0);
        assert!(l.left.width() > 270.0);
        assert!(l.right.width() > 320.0);
        assert!(l.center.left() > l.left.right());
        assert!(l.right.left() > l.center.right());
        assert_eq!(l.hero.width(), l.center.width());
        assert_eq!(l.restore.width(), l.center.width());
        assert_eq!(l.polish.width(), l.center.width());
        assert_eq!(l.monitor.width(), l.center.width());
    }

    #[test]
    fn advanced_layout_keeps_dense_modules_scannable() {
        let l = AdvancedLayout::new();
        assert_eq!(l.top.height(), 70.0);
        assert_eq!(l.status.top(), 1040.0);
        assert!(
            l.spectrogram.width() >= 1180.0,
            "Advanced needs a full-width spectrogram, not the compact Standard waveform"
        );
        assert!(
            l.spectrogram.height() >= 250.0,
            "spectrogram must be tall enough to inspect vocal artifacts"
        );
        assert_eq!(l.module_cards.len(), 10);
        assert_eq!(l.group_panels.len(), 5);

        for card in l.module_cards {
            assert!(card.width() >= 170.0, "module cards need readable width");
            assert!(
                card.height() >= 118.0,
                "module cards need room for labels, controls, and readouts"
            );
            assert!(l.work.contains(card.min));
            assert!(l.work.contains(card.max));
        }

        let mut panels = Vec::new();
        panels.extend(l.group_panels);
        panels.push(l.spectrogram);
        for (i, a) in panels.iter().enumerate() {
            for b in panels.iter().skip(i + 1) {
                assert!(
                    !a.intersects(*b),
                    "Advanced panels should not overlap or crowd"
                );
            }
        }
    }

    #[test]
    fn advanced_debleed_panel_round_trips_session_controls() {
        let mut session = Session::new();
        let panel = AdvancedDeBleedPanel::from_session(&session);

        assert_eq!(panel.source_label(), "Native take");
        assert!(!panel.source_imported);
        assert_eq!(panel.warn_threshold, 0.02);
        assert_eq!(panel.block_threshold, 0.15);
        assert_eq!(panel.amount, session.stage_mix(yesvox_core::Stage::DeBleed));

        let edited = AdvancedDeBleedPanel {
            source_imported: true,
            warn_threshold: 0.11,
            block_threshold: 0.42,
            amount: 0.63,
        };
        edited.apply_to_session(&mut session);

        assert!(session.debleed_source_imported());
        assert_eq!(session.debleed_thresholds(), (0.11, 0.42));
        assert_eq!(session.stage_mix(yesvox_core::Stage::DeBleed), 0.63);
        assert_eq!(
            AdvancedDeBleedPanel::from_session(&session).source_label(),
            "Imported stem"
        );
        assert_eq!(AdvancedDeBleedPanel::from_session(&session), edited);
    }

    #[test]
    fn advanced_debleed_controls_fit_without_overlapping() {
        let card = AdvancedLayout::new().module_cards[0];
        let controls = advanced_debleed_control_rects(card);
        let padded = card.shrink(8.0);
        let rects = [
            controls.native_source,
            controls.imported_source,
            controls.warn_slider,
            controls.block_slider,
            controls.amount_knob,
        ];

        for r in rects {
            assert!(padded.contains(r.min), "control left/top must stay padded");
            assert!(
                padded.contains(r.max),
                "control right/bottom must stay padded"
            );
        }

        for (i, a) in rects.iter().enumerate() {
            for b in rects.iter().skip(i + 1) {
                assert!(!a.intersects(*b), "De-bleed controls must not crowd");
            }
        }
    }

    #[test]
    fn advanced_declip_panel_round_trips_session_controls() {
        let mut session = Session::new();
        let panel = AdvancedDeClipPanel::from_session(&session);

        assert_eq!(panel.threshold, 0.98);
        assert_eq!(panel.strength, 1.0);
        assert_eq!(panel.amount, session.stage_mix(Stage::DeClip));

        let edited = AdvancedDeClipPanel {
            threshold: 0.79,
            strength: 0.55,
            amount: 0.42,
        };
        edited.apply_to_session(&mut session);

        assert_eq!(session.declip_settings(), (0.79, 0.55));
        assert_eq!(session.stage_mix(Stage::DeClip), 0.42);
        assert_eq!(AdvancedDeClipPanel::from_session(&session), edited);
    }

    #[test]
    fn advanced_declip_controls_fit_without_overlapping() {
        let card = AdvancedLayout::new().module_cards[1];
        let controls = advanced_declip_control_rects(card);
        let padded = card.shrink(8.0);
        let rects = [
            controls.threshold_slider,
            controls.strength_slider,
            controls.amount_knob,
        ];

        for r in rects {
            assert!(padded.contains(r.min), "control left/top must stay padded");
            assert!(
                padded.contains(r.max),
                "control right/bottom must stay padded"
            );
        }

        for (i, a) in rects.iter().enumerate() {
            for b in rects.iter().skip(i + 1) {
                assert!(!a.intersects(*b), "Declip controls must not crowd");
            }
        }
    }

    #[test]
    fn advanced_declick_panel_round_trips_session_controls() {
        let mut session = Session::new();
        let panel = AdvancedDeClickPanel::from_session(&session);

        assert!(panel.click_enabled);
        assert!(panel.mouth_enabled);
        assert_eq!(panel.click_amount, session.click_repair_amount());
        assert_eq!(panel.mouth_amount, session.mouth_click_amount());

        let edited = AdvancedDeClickPanel {
            click_enabled: false,
            mouth_enabled: true,
            click_amount: 0.36,
            mouth_amount: 0.58,
        };
        edited.apply_to_session(&mut session);

        assert!(!session.click_repair_enabled());
        assert!(session.mouth_click_enabled());
        assert_eq!(session.click_repair_amount(), 0.36);
        assert_eq!(session.mouth_click_amount(), 0.58);
        assert_eq!(AdvancedDeClickPanel::from_session(&session), edited);
    }

    #[test]
    fn advanced_declick_controls_fit_without_overlapping() {
        let card = AdvancedLayout::new().module_cards[2];
        let controls = advanced_declick_control_rects(card);
        let padded = card.shrink(8.0);
        let rects = [
            controls.click_toggle,
            controls.mouth_toggle,
            controls.click_knob,
            controls.mouth_knob,
        ];

        for r in rects {
            assert!(padded.contains(r.min), "control left/top must stay padded");
            assert!(
                padded.contains(r.max),
                "control right/bottom must stay padded"
            );
        }

        for (i, a) in rects.iter().enumerate() {
            for b in rects.iter().skip(i + 1) {
                assert!(!a.intersects(*b), "De-click controls must not crowd");
            }
        }
    }

    #[test]
    fn advanced_depop_panel_round_trips_session_controls() {
        let mut session = Session::new();
        let panel = AdvancedDePopPanel::from_session(&session);

        assert_eq!(panel.cutoff_hz, 120.0);
        assert_eq!(panel.strength, 1.0);
        assert_eq!(panel.amount, session.stage_mix(Stage::DePop));

        let edited = AdvancedDePopPanel {
            cutoff_hz: 90.0,
            strength: 0.65,
            amount: 0.44,
        };
        edited.apply_to_session(&mut session);

        assert_eq!(session.depop_settings(), (90.0, 0.65));
        assert_eq!(session.stage_mix(Stage::DePop), 0.44);
        assert_eq!(AdvancedDePopPanel::from_session(&session), edited);
    }

    #[test]
    fn advanced_depop_controls_fit_without_overlapping() {
        let card = AdvancedLayout::new().module_cards[3];
        let controls = advanced_depop_control_rects(card);
        let padded = card.shrink(8.0);
        let rects = [
            controls.cutoff_slider,
            controls.strength_slider,
            controls.amount_knob,
        ];

        for r in rects {
            assert!(padded.contains(r.min), "control left/top must stay padded");
            assert!(
                padded.contains(r.max),
                "control right/bottom must stay padded"
            );
        }

        for (i, a) in rects.iter().enumerate() {
            for b in rects.iter().skip(i + 1) {
                assert!(!a.intersects(*b), "De-pop controls must not crowd");
            }
        }
    }

    #[test]
    fn advanced_dehum_panel_round_trips_session_controls() {
        let mut session = Session::new();
        let panel = AdvancedDeHumPanel::from_session(&session);

        assert_eq!(panel.nominal_label(), "Auto");
        assert_eq!(panel.nominal, None);
        assert_eq!(panel.harmonics, 6);
        assert_eq!(panel.q, 30.0);
        assert!(panel.track);
        assert_eq!(panel.amount, session.stage_mix(Stage::DeHum));

        let edited = AdvancedDeHumPanel {
            nominal: Some(60.0),
            harmonics: 8,
            q: 42.0,
            track: false,
            amount: 0.52,
        };
        edited.apply_to_session(&mut session);

        assert_eq!(session.dehum_settings(), (Some(60.0), 8, 42.0, false));
        assert_eq!(session.stage_mix(Stage::DeHum), 0.52);
        assert_eq!(
            AdvancedDeHumPanel::from_session(&session).nominal_label(),
            "60 Hz"
        );
        assert_eq!(AdvancedDeHumPanel::from_session(&session), edited);
    }

    #[test]
    fn advanced_dehum_controls_fit_without_overlapping() {
        let card = AdvancedLayout::new().module_cards[4];
        let controls = advanced_dehum_control_rects(card);
        let padded = card.shrink(8.0);
        let rects = [
            controls.auto_button,
            controls.hz50_button,
            controls.hz60_button,
            controls.track_toggle,
            controls.harmonics_slider,
            controls.q_slider,
            controls.amount_knob,
        ];

        for r in rects {
            assert!(padded.contains(r.min), "control left/top must stay padded");
            assert!(
                padded.contains(r.max),
                "control right/bottom must stay padded"
            );
        }

        for (i, a) in rects.iter().enumerate() {
            for b in rects.iter().skip(i + 1) {
                assert!(!a.intersects(*b), "De-hum controls must not crowd");
            }
        }
    }

    #[test]
    fn advanced_denoise_panel_round_trips_session_controls() {
        let mut session = Session::new();
        let panel = AdvancedDenoisePanel::from_session(&session);

        assert_eq!(panel.quality, QualityMode::Balanced);
        assert_eq!(panel.quality_label(), "Balanced");
        assert_eq!(panel.engine_label(), "Neural");
        assert_eq!(panel.pass_label(), "1 pass");
        assert_eq!(panel.workers, session.worker_count());
        assert_eq!(panel.amount, session.stage_mix(Stage::Denoise));

        let edited = AdvancedDenoisePanel {
            quality: QualityMode::RestorationHq,
            workers: Session::max_worker_count().clamp(1, 3),
            amount: 0.57,
        };
        edited.apply_to_session(&mut session);

        assert_eq!(session.settings().quality_mode, QualityMode::RestorationHq);
        assert_eq!(session.worker_count(), edited.workers);
        assert_eq!(session.stage_mix(Stage::Denoise), 0.57);
        assert_eq!(
            AdvancedDenoisePanel::from_session(&session).pass_label(),
            "2 pass"
        );
        assert_eq!(AdvancedDenoisePanel::from_session(&session), edited);
    }

    #[test]
    fn advanced_denoise_controls_fit_without_overlapping() {
        let card = AdvancedLayout::new().module_cards[5];
        let controls = advanced_denoise_control_rects(card);
        let padded = card.shrink(8.0);
        let rects = [
            controls.natural_button,
            controls.balanced_button,
            controls.hq_button,
            controls.workers_slider,
            controls.amount_knob,
        ];

        for r in rects {
            assert!(padded.contains(r.min), "control left/top must stay padded");
            assert!(
                padded.contains(r.max),
                "control right/bottom must stay padded"
            );
        }

        for (i, a) in rects.iter().enumerate() {
            for b in rects.iter().skip(i + 1) {
                assert!(!a.intersects(*b), "Denoise controls must not crowd");
            }
        }
    }

    #[test]
    fn advanced_dereverb_panel_round_trips_session_controls() {
        let mut session = Session::new();
        let panel = AdvancedDeReverbPanel::from_session(&session);

        assert_eq!(panel.strength, 1.2);
        assert_eq!(panel.tail_preservation, 0.1);
        assert_eq!(panel.rt60_label(), "RT60 --");
        assert_eq!(panel.amount, session.stage_mix(Stage::DeReverb));

        let edited = AdvancedDeReverbPanel {
            strength: 1.55,
            tail_preservation: 0.42,
            rt60_s: panel.rt60_s,
            amount: 0.58,
        };
        edited.apply_to_session(&mut session);

        assert_eq!(session.dereverb_settings(), (1.55, 0.42));
        assert_eq!(session.stage_mix(Stage::DeReverb), 0.58);
        assert_eq!(AdvancedDeReverbPanel::from_session(&session), edited);
    }

    #[test]
    fn advanced_dereverb_controls_fit_without_overlapping() {
        let card = AdvancedLayout::new().module_cards[6];
        let controls = advanced_dereverb_control_rects(card);
        let padded = card.shrink(8.0);
        let rects = [
            controls.strength_slider,
            controls.tail_slider,
            controls.amount_knob,
        ];

        for r in rects {
            assert!(padded.contains(r.min), "control left/top must stay padded");
            assert!(
                padded.contains(r.max),
                "control right/bottom must stay padded"
            );
        }

        for (i, a) in rects.iter().enumerate() {
            for b in rects.iter().skip(i + 1) {
                assert!(!a.intersects(*b), "De-reverb controls must not crowd");
            }
        }
    }

    #[test]
    fn advanced_deess_panel_round_trips_session_controls() {
        let mut session = Session::new();
        let panel = AdvancedDeEssPanel::from_session(&session);

        assert_eq!(panel.mode, DeEssMode::Split);
        assert_eq!(panel.mode_label(), "Split");
        assert_eq!(panel.ess_ratio, 0.45);
        assert_eq!(panel.min_gain, 0.1);
        assert_eq!(panel.hf_label(), "HF --");
        assert_eq!(panel.amount, session.stage_mix(Stage::DeEss));

        let edited = AdvancedDeEssPanel {
            mode: DeEssMode::Wide,
            ess_ratio: 0.32,
            min_gain: 0.08,
            hf_reduction_db: panel.hf_reduction_db,
            amount: 0.48,
        };
        edited.apply_to_session(&mut session);

        assert_eq!(session.deess_settings(), (DeEssMode::Wide, 0.32, 0.08));
        assert_eq!(session.stage_mix(Stage::DeEss), 0.48);
        assert_eq!(
            AdvancedDeEssPanel::from_session(&session).mode_label(),
            "Wide"
        );
        assert_eq!(AdvancedDeEssPanel::from_session(&session), edited);
    }

    #[test]
    fn advanced_deess_controls_fit_without_overlapping() {
        let card = AdvancedLayout::new().module_cards[7];
        let controls = advanced_deess_control_rects(card);
        let padded = card.shrink(8.0);
        let rects = [
            controls.split_button,
            controls.wide_button,
            controls.ess_ratio_slider,
            controls.min_gain_slider,
            controls.amount_knob,
        ];

        for r in rects {
            assert!(padded.contains(r.min), "control left/top must stay padded");
            assert!(
                padded.contains(r.max),
                "control right/bottom must stay padded"
            );
        }

        for (i, a) in rects.iter().enumerate() {
            for b in rects.iter().skip(i + 1) {
                assert!(!a.intersects(*b), "De-ess controls must not crowd");
            }
        }
    }

    #[test]
    fn advanced_breath_panel_round_trips_session_controls() {
        let mut session = Session::new();
        let panel = AdvancedBreathPanel::from_session(&session);

        assert_eq!(panel.strength_db, 6.0);
        assert_eq!(panel.sidechain_label(), "HF --");
        assert_eq!(panel.amount, session.stage_mix(Stage::Breath));

        let edited = AdvancedBreathPanel {
            strength_db: 4.5,
            shared_hf_reduction_db: panel.shared_hf_reduction_db,
            amount: 0.37,
        };
        edited.apply_to_session(&mut session);

        assert_eq!(session.breath_strength_db(), 4.5);
        assert_eq!(session.stage_mix(Stage::Breath), 0.37);
        assert_eq!(AdvancedBreathPanel::from_session(&session), edited);
    }

    #[test]
    fn advanced_breath_controls_fit_without_overlapping() {
        let card = AdvancedLayout::new().module_cards[8];
        let controls = advanced_breath_control_rects(card);
        let padded = card.shrink(8.0);
        let rects = [controls.strength_slider, controls.amount_knob];

        for r in rects {
            assert!(padded.contains(r.min), "control left/top must stay padded");
            assert!(
                padded.contains(r.max),
                "control right/bottom must stay padded"
            );
        }

        assert!(
            !controls.strength_slider.intersects(controls.amount_knob),
            "Breath controls must not crowd"
        );
    }

    #[test]
    fn advanced_finish_panel_round_trips_session_controls() {
        let mut session = Session::new();
        let panel = AdvancedFinishPanel::from_session(&session);

        assert!(panel.presence_air_enabled);
        assert!(panel.leveling_enabled);
        assert!(panel.loudness_enabled);
        assert_eq!(panel.presence_db, 1.5);
        assert_eq!(panel.air_db, 0.0);
        assert_eq!(panel.presence_air_amount, session.presence_air_amount());
        assert_eq!(panel.leveling_amount, session.leveling_amount());
        assert_eq!(panel.loudness_target, LoudnessTarget::Podcast);

        let edited = AdvancedFinishPanel {
            presence_air_enabled: true,
            leveling_enabled: false,
            loudness_enabled: true,
            presence_db: 2.25,
            air_db: 0.75,
            presence_air_amount: 0.62,
            leveling_amount: 0.31,
            loudness_target: LoudnessTarget::Streaming,
        };
        edited.apply_to_session(&mut session);

        assert!(session.presence_air_enabled());
        assert!(!session.leveling_enabled());
        assert!(session.loudness_enabled());
        assert_eq!(session.presence_air_eq(), (2.25, 0.75));
        assert_eq!(session.presence_air_amount(), 0.62);
        assert_eq!(session.leveling_amount(), 0.31);
        assert_eq!(session.loudness_target(), LoudnessTarget::Streaming);
        assert_eq!(AdvancedFinishPanel::from_session(&session), edited);
    }

    #[test]
    fn advanced_finish_controls_fit_without_overlapping() {
        let card = AdvancedLayout::new().module_cards[9];
        let controls = advanced_finish_control_rects(card);
        let labels = advanced_finish_label_rects(card);
        let padded = card.shrink(8.0);
        let rects = [
            controls.presence_toggle,
            controls.presence_slider,
            controls.air_slider,
            controls.presence_amount_knob,
            controls.leveling_toggle,
            controls.leveling_slider,
            controls.loudness_toggle,
            controls.target_button,
            controls.loudness_knob,
        ];
        let label_rects = [
            labels.presence,
            labels.presence_value,
            labels.air,
            labels.air_value,
            labels.presence_amount,
            labels.leveling,
            labels.leveling_value,
            labels.loudness,
            labels.target,
            labels.limiter,
        ];

        for r in rects {
            assert!(padded.contains(r.min), "control left/top must stay padded");
            assert!(
                padded.contains(r.max),
                "control right/bottom must stay padded"
            );
        }

        for (i, a) in rects.iter().enumerate() {
            for b in rects.iter().skip(i + 1) {
                assert!(!a.intersects(*b), "Finish controls must not crowd");
            }
        }

        for label in label_rects {
            assert!(
                padded.contains(label.min),
                "label left/top must stay padded"
            );
            assert!(
                padded.contains(label.max),
                "label right/bottom must stay padded"
            );
            for control in rects {
                assert!(
                    !label.intersects(control),
                    "Finish labels must not overlap controls"
                );
            }
        }

        for (i, a) in label_rects.iter().enumerate() {
            for b in label_rects.iter().skip(i + 1) {
                assert!(!a.intersects(*b), "Finish labels must not crowd");
            }
        }
    }

    #[test]
    fn loudness_labels_cover_every_target() {
        for t in [
            LoudnessTarget::Podcast,
            LoudnessTarget::Streaming,
            LoudnessTarget::MatchInput,
            LoudnessTarget::PeakNormalize,
            LoudnessTarget::Custom(-18.5),
        ] {
            assert!(!loudness_label(t).is_empty());
        }
    }

    #[test]
    fn cycle_loudness_visits_every_target_and_returns_to_start() {
        // The Loudness Target selector (#24) cycles through the product.md §7
        // list including Custom, and wraps.
        let mut t = LoudnessTarget::Podcast;
        let seen: Vec<LoudnessTarget> = (0..5)
            .map(|_| {
                let cur = t;
                t = cycle_loudness(t);
                cur
            })
            .collect();
        assert_eq!(t, LoudnessTarget::Podcast, "cycle wraps back to start");
        assert!(seen.contains(&LoudnessTarget::Streaming));
        assert!(seen.contains(&LoudnessTarget::MatchInput));
        assert!(seen.contains(&LoudnessTarget::PeakNormalize));
        assert!(
            seen.iter().any(|t| matches!(t, LoudnessTarget::Custom(_))),
            "cycle includes a Custom target"
        );
    }

    #[test]
    fn output_field_chrome_only_marks_mutable_rows_as_selectors() {
        assert_eq!(output_field_chrome("Format"), OutputFieldChrome::Selector);
        assert_eq!(
            output_field_chrome("Bit Depth"),
            OutputFieldChrome::Selector
        );
        assert_eq!(
            output_field_chrome("Loudness Target"),
            OutputFieldChrome::Selector
        );
        assert_eq!(
            output_field_chrome("Sample Rate"),
            OutputFieldChrome::ReadOnly
        );
    }

    #[test]
    fn preset_gen_badge_follows_quality_mode() {
        let mut session = Session::new();

        session.set_quality_mode(QualityMode::Natural);
        assert!(!preset_gen_badge_visible(&session));

        session.set_quality_mode(QualityMode::Balanced);
        assert!(preset_gen_badge_visible(&session));

        session.set_quality_mode(QualityMode::RestorationHq);
        assert!(preset_gen_badge_visible(&session));
    }

    #[test]
    fn audible_cache_key_tracks_source_controls_and_render_generation() {
        let base = audible_cache_key(AbView::Processed, false, 0.25, false, 7);

        assert_eq!(base.playback_source(), PlaybackSource::Processed);
        assert_ne!(
            base,
            audible_cache_key(AbView::Original, false, 0.25, false, 7)
        );
        assert_ne!(
            base,
            audible_cache_key(AbView::Processed, true, 0.25, false, 7)
        );
        assert_ne!(
            base,
            audible_cache_key(AbView::Processed, false, 0.25, false, 8)
        );

        let compare = audible_cache_key(AbView::CompareSnapshot, false, 0.25, false, 7);
        assert_eq!(compare.playback_source(), PlaybackSource::CompareSnapshot);
        assert_ne!(base, compare);

        let audition = audible_cache_key(AbView::Original, false, 0.25, true, 7);
        assert_eq!(audition.playback_source(), PlaybackSource::Reblend);
        assert_ne!(base, audition);
    }

    #[test]
    fn reblend_drags_only_invalidate_the_reblend_source() {
        // #36: dragging the reblend slider while auditing Original/Processed
        // must not rebuild the audible buffer (and its LUFS) every drag frame —
        // the slider only shapes the Reblend source's mix.
        let a = audible_cache_key(AbView::Processed, false, 0.25, false, 7);
        let b = audible_cache_key(AbView::Processed, false, 0.50, false, 7);
        assert_eq!(a, b, "A/B/C sources must ignore reblend drags");

        let mix_a = audible_cache_key(AbView::Processed, false, 0.25, true, 7);
        let mix_b = audible_cache_key(AbView::Processed, false, 0.50, true, 7);
        assert_ne!(mix_a, mix_b, "the audible reblend mix follows the slider");
    }

    #[test]
    fn unchanged_audible_cache_key_does_not_refresh_player_source() {
        let key = audible_cache_key(AbView::Processed, false, 0.25, false, 7);

        assert!(should_refresh_player_source(None, key));
        assert!(!should_refresh_player_source(Some(key), key));
        assert!(should_refresh_player_source(
            Some(key),
            audible_cache_key(AbView::Original, false, 0.25, false, 7)
        ));
    }

    #[test]
    fn advanced_spectrogram_grid_stays_dense_but_bounded() {
        let l = AdvancedLayout::new();
        let spec = rect(
            l.spectrogram.left() + 20.0,
            l.spectrogram.top() + 52.0,
            l.spectrogram.width() - 40.0,
            l.spectrogram.height() - 76.0,
        );
        let (columns, rows) = advanced_spectrogram_dimensions(spec);

        assert!((64..=240).contains(&columns));
        assert!((32..=96).contains(&rows));
        assert!(columns >= 160, "Advanced needs enough time detail");
        assert!(rows >= 40, "Advanced needs enough frequency detail");
    }

    #[test]
    fn spectrogram_cache_key_tracks_audible_source_and_grid() {
        let audible = audible_cache_key(AbView::Processed, false, 0.25, false, 7);
        let key = spectrogram_cache_key(audible, 192, 72);

        assert_eq!(key.audible, audible);
        assert_ne!(key, spectrogram_cache_key(audible, 160, 72));
        assert_ne!(key, spectrogram_cache_key(audible, 192, 48));
        assert_ne!(
            key,
            spectrogram_cache_key(
                audible_cache_key(AbView::Original, false, 0.25, false, 7),
                192,
                72,
            )
        );
        assert_ne!(
            key,
            spectrogram_cache_key(
                audible_cache_key(AbView::Processed, true, 0.25, false, 7),
                192,
                72,
            )
        );
        assert_ne!(
            key,
            spectrogram_cache_key(
                audible_cache_key(AbView::Processed, false, 0.25, false, 8),
                192,
                72,
            )
        );
    }

    #[test]
    fn batch_queue_summary_counts_pending_done_and_failed() {
        let mut queue = BatchQueueUi::default();
        queue.enqueue(PathBuf::from("a.wav"), PathBuf::from("a_YesVox.wav"));
        queue.enqueue(PathBuf::from("b.wav"), PathBuf::from("b_YesVox.wav"));
        queue.rows[0].status = JobStatus::Done(PathBuf::from("a_YesVox.wav"));
        queue.rows[1].status = JobStatus::Failed("decode failed".to_owned());

        assert_eq!(
            queue.counts(),
            BatchQueueCounts {
                pending: 0,
                done: 1,
                failed: 1
            }
        );
        assert_eq!(queue.summary_label(), "0 queued - 1 done - 1 failed");
    }

    #[test]
    fn batch_queue_rejects_duplicate_pending_inputs_only() {
        let mut queue = BatchQueueUi::default();
        assert!(queue.enqueue(PathBuf::from("a.wav"), PathBuf::from("a_YesVox.wav")));
        assert!(!queue.enqueue(PathBuf::from("a.wav"), PathBuf::from("a_YesVox_2.wav")));
        queue.rows[0].status = JobStatus::Failed("decode failed".to_owned());

        assert!(queue.enqueue(PathBuf::from("a.wav"), PathBuf::from("a_YesVox_retry.wav")));
        assert_eq!(queue.pending_count(), 1);
    }

    #[test]
    fn gui_batch_mode_maps_to_core_render_queue_mode() {
        let mut queue = BatchQueueUi::default();
        assert_eq!(
            queue.preset_mode(Preset::NARRATION),
            BatchPresetMode::Force(Preset::NARRATION)
        );

        queue.mode = GuiBatchPresetMode::DetectPerFile;
        assert_eq!(
            queue.preset_mode(Preset::NARRATION),
            BatchPresetMode::DetectPerFile {
                fallback: Preset::NARRATION
            }
        );
    }

    #[test]
    fn batch_output_path_uses_input_folder_stem_and_export_extension() {
        let path = batch_output_path(Path::new("clips/Lead Vocal.wav"), OutputFormat::Flac);

        assert_eq!(path, PathBuf::from("clips/Lead Vocal_YesVox.flac"));
    }

    #[test]
    fn batch_status_reports_cancel_separately_from_failures() {
        let summary = BatchSummary {
            done: 1,
            failed: 0,
            canceled: true,
            failures: Vec::new(),
        };

        assert_eq!(
            format_batch_status(&summary),
            "Batch canceled - 1 exported - 0 failed"
        );
    }

    #[test]
    fn middle_truncation_keeps_both_ends() {
        assert_eq!(truncate_middle("short", 12), "short");
        assert_eq!(truncate_middle("C:/very/long/location", 12), "C:/v...ation");
    }

    /// Move the app's session onto a worker held live by the returned sender —
    /// exactly what `render_now` does — so tests can exercise the mid-job
    /// settings lock (#36).
    fn hold_render_job(app: &mut YesVoxApp) -> std::sync::mpsc::Sender<()> {
        let (release, gate) = std::sync::mpsc::channel::<()>();
        let moved = std::mem::replace(&mut app.session, Session::new());
        app.render_job = Some(spawn(move |_ctx| {
            gate.recv().ok(); // hold the job live until the test releases it
            Ok(RenderDone {
                session: moved,
                ok: true,
                status: "Rendered".to_owned(),
            })
        }));
        release
    }

    /// Pump job events until the render job finishes (bounded wait).
    fn pump_until_idle(app: &mut YesVoxApp) {
        let ctx = egui::Context::default();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while app.render_job.is_some() {
            assert!(
                std::time::Instant::now() < deadline,
                "render job never finished"
            );
            app.pump_jobs(&ctx);
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }

    #[test]
    fn standard_import_compare_and_export_smoke() {
        use egui::{DroppedFile, Event, Modifiers, PointerButton};
        use yesvox_core::{read_audio, write_wav, SampleEncoding};

        fn frame(
            app: &mut YesVoxApp,
            ctx: &egui::Context,
            raw: egui::RawInput,
        ) -> egui::FullOutput {
            ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(rect(0.0, 0.0, 1280.0, 720.0)),
                    ..raw
                },
                |ui| app.draw_screen(ui),
            )
        }

        fn painted_text(output: egui::FullOutput) -> String {
            output
                .shapes
                .into_iter()
                .filter_map(|shape| match shape.shape {
                    egui::Shape::Text(shape) => Some(shape.galley.text().to_owned()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n")
        }

        fn click(app: &mut YesVoxApp, ctx: &egui::Context, x: f32, y: f32) {
            let pos = egui::pos2(x, y) * (2.0 / 3.0);
            frame(app, ctx, egui::RawInput::default());
            for pressed in [true, false] {
                frame(
                    app,
                    ctx,
                    egui::RawInput {
                        events: vec![
                            Event::PointerMoved(pos),
                            Event::PointerButton {
                                pos,
                                button: PointerButton::Primary,
                                pressed,
                                modifiers: Modifiers::NONE,
                            },
                        ],
                        ..Default::default()
                    },
                );
            }
        }

        let root =
            std::env::temp_dir().join(format!("yesvox-standard-smoke-{}", std::process::id()));
        std::fs::create_dir(&root).expect("create isolated fixture directory");
        let source = root.join("test vocal.wav");
        let output = root.join("test vocal_YesVox.wav");
        let sample_rate = 48_000;
        let samples = (0..sample_rate / 2)
            .map(|i| 0.15 * (std::f32::consts::TAU * 220.0 * i as f32 / sample_rate as f32).sin())
            .collect();
        write_wav(
            &source,
            &AudioBuffer::new(sample_rate, 24, SampleEncoding::Int, vec![samples]),
        )
        .expect("write test vocal");
        let original_bytes = std::fs::read(&source).unwrap();

        let ctx = egui::Context::default();
        install_theme(&ctx);
        let mut app = YesVoxApp::from_session(YesVoxApp::default_session());
        let empty = painted_text(frame(&mut app, &ctx, Default::default()));
        assert!(empty.contains("Import Vocal"), "{empty}");
        assert!(
            !empty.contains("Lead Vocal.wav") && !empty.contains("Export Vocal"),
            "{empty}"
        );

        let invalid = painted_text(frame(
            &mut app,
            &ctx,
            egui::RawInput {
                dropped_files: vec![DroppedFile {
                    path: Some(root.join("missing.wav")),
                    ..Default::default()
                }],
                ..Default::default()
            },
        ));
        assert!(
            invalid.contains("Import failed") && invalid.contains("Import Vocal"),
            "{invalid}"
        );

        frame(
            &mut app,
            &ctx,
            egui::RawInput {
                dropped_files: vec![DroppedFile {
                    path: Some(source.clone()),
                    ..Default::default()
                }],
                ..Default::default()
            },
        );
        assert!(
            app.has_vocal_workspace(),
            "render must not flash the import screen"
        );
        pump_until_idle(&mut app);
        assert!(app.session.output().is_some());
        let standard = painted_text(frame(&mut app, &ctx, Default::default()));
        for label in [
            "test vocal.wav",
            "Original",
            "Processed",
            "Export Vocal",
            "Effect strength",
            "More Controls",
        ] {
            assert!(standard.contains(label), "missing {label} in {standard}");
        }
        assert!(!standard.contains("BATCH QUEUE") && !standard.contains("De-reverb"));

        click(&mut app, &ctx, 375.0, 292.0);
        assert_eq!(app.ab, AbView::Original);
        click(&mut app, &ctx, 527.0, 292.0);
        assert_eq!(app.ab, AbView::Processed);

        click(&mut app, &ctx, 210.0, 829.0);
        assert_eq!(app.session.settings().content_mode, ContentMode::Voiceover);
        assert_eq!(app.ab, AbView::Original);
        assert!(app.session.output().is_none());
        click(&mut app, &ctx, 1588.0, 349.0);
        assert!(
            app.export_job.is_none() && !output.exists(),
            "exported without a current render"
        );

        click(&mut app, &ctx, 1170.0, 292.0);
        pump_until_idle(&mut app);
        assert!(app.session.output().is_some());
        click(&mut app, &ctx, 1588.0, 349.0);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while app.export_job.is_some() && std::time::Instant::now() < deadline {
            app.pump_jobs(&ctx);
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert!(app.status.starts_with("Exported"), "{}", app.status);
        let exported = read_audio(&output).expect("decode exported vocal");
        assert_eq!(exported.frames(), sample_rate as usize / 2);
        assert_eq!(std::fs::read(&source).unwrap(), original_bytes);

        click(&mut app, &ctx, 1210.0, 723.0);
        assert!(app.show_standard_controls);
        let controls = painted_text(frame(&mut app, &ctx, Default::default()));
        assert!(controls.contains("BATCH QUEUE") && controls.contains("Back to Standard"));

        // Merge guard (2026-11-23): the Aug 25 brand pass centered a narrower
        // view switch, and the Nov 14 Standard workspace renames its left
        // segment "Back to Standard". Both labels must fit their segments, and
        // no other text may sit on the switch (the old top-bar section labels
        // did once both histories met).
        fn painted_text_rects(output: egui::FullOutput) -> Vec<(String, egui::Rect)> {
            output
                .shapes
                .into_iter()
                .filter_map(|shape| match shape.shape {
                    egui::Shape::Text(text) => {
                        Some((text.galley.text().to_owned(), text.visual_bounding_rect()))
                    }
                    _ => None,
                })
                .collect()
        }
        let to_screen =
            |r: egui::Rect| egui::Rect::from_min_max(r.min * (2.0 / 3.0), r.max * (2.0 / 3.0));
        let [standard_segment, advanced_segment] = YesVoxApp::top_bar_view_rects();
        let switch = to_screen(standard_segment.union(advanced_segment));
        let assert_switch_label = |painted: &[(String, egui::Rect)], label: &str, segment| {
            let (_, bounds) = painted
                .iter()
                .find(|(text, _)| text == label)
                .unwrap_or_else(|| panic!("{label:?} is not painted"));
            let inside = to_screen(segment).shrink(2.0);
            assert!(
                inside.contains_rect(*bounds),
                "{label:?} at {bounds:?} overflows its segment {inside:?}"
            );
            for (text, bounds) in painted {
                if !["Standard", "Back to Standard", "Advanced"].contains(&text.as_str()) {
                    assert!(
                        !switch.intersects(*bounds),
                        "{text:?} at {bounds:?} overlaps the view switch {switch:?}"
                    );
                }
            }
        };
        let controls = painted_text_rects(frame(&mut app, &ctx, Default::default()));
        assert_switch_label(&controls, "Back to Standard", standard_segment);
        assert_switch_label(&controls, "Advanced", advanced_segment);
        let advanced_center = advanced_segment.center();
        click(&mut app, &ctx, advanced_center.x, advanced_center.y);
        assert!(app.view_mode == ViewMode::Advanced && !app.show_standard_controls);
        let advanced = painted_text_rects(frame(&mut app, &ctx, Default::default()));
        assert_switch_label(&advanced, "Standard", standard_segment);
        assert_switch_label(&advanced, "Advanced", advanced_segment);

        std::fs::remove_file(output).unwrap();
        std::fs::remove_file(source).unwrap();
        std::fs::remove_dir(root).unwrap();
    }

    #[test]
    fn canceled_render_status_names_the_audible_fallback() {
        // A canceled render keeps the prior output; the status must say what
        // playback is still auditioning instead of being silently correct (#36).
        assert_eq!(canceled_render_status(false), "Render canceled");
        assert_eq!(
            canceled_render_status(true),
            "Render canceled - showing previous processed"
        );
    }

    #[test]
    fn preview_rate_badge_only_appears_on_a_rate_mismatch() {
        assert_eq!(preview_rate_badge(48_000, 48_000), None);
        assert_eq!(preview_rate_badge(44_100, 44_100), None);
        assert_eq!(
            preview_rate_badge(48_000, 44_100).as_deref(),
            Some("Preview 48.0 kHz - source 44.1 kHz")
        );
    }

    #[test]
    fn build_stamp_shows_version_and_the_active_engine() {
        // #20/W1: the always-visible stamp reports version + hash + the engine
        // that actually ran, so a stale build or a neural->light fallback is
        // visible in the app. The painted chip is visual; this pins the text.
        use yesvox_core::{write_wav, AudioBuffer, SampleEncoding};
        let mut app = YesVoxApp::from_session(YesVoxApp::default_session());

        // Before any render: version + hash + a neutral engine placeholder.
        let before = app.build_stamp_text();
        assert!(
            before.starts_with(&format!("v{}", yesvox_core::version::VERSION)),
            "stamp must start with the version: {before:?}"
        );
        assert!(
            before.contains('—'),
            "no engine word before a render: {before:?}"
        );

        // After a render, the actual engine word appears (Light in the default,
        // no-`neural`-feature build).
        let mut path = std::env::temp_dir();
        path.push(format!("yesvox-gui-stamp-{}.wav", std::process::id()));
        let sr = 48_000u32;
        let samples: Vec<f32> = (0..(sr as usize / 4))
            .map(|i| 0.1 * (2.0 * std::f32::consts::PI * 220.0 * (i as f32 / sr as f32)).sin())
            .collect();
        write_wav(
            &path,
            &AudioBuffer::new(sr, 24, SampleEncoding::Int, vec![samples]),
        )
        .expect("write fixture");
        app.session.open(&path).expect("open");
        app.session.render().expect("render");
        assert!(
            app.build_stamp_text().contains("Light"),
            "stamp must show the engine after render: {:?}",
            app.build_stamp_text()
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn undo_redo_keep_the_pinned_compare_snapshot() {
        // The pinned C is an audio slot, not a setting: undo/redo restore
        // settings snapshots and must leave it pinned (#36 regression guard).
        use yesvox_core::{write_wav, AudioBuffer, SampleEncoding};
        let mut path = std::env::temp_dir();
        path.push(format!("yesvox-gui-pin-c-{}.wav", std::process::id()));
        let sr = 48_000u32;
        let samples: Vec<f32> = (0..(sr as usize / 2))
            .map(|i| {
                let t = i as f32 / sr as f32;
                0.15 * (2.0 * std::f32::consts::PI * 220.0 * t).sin()
            })
            .collect();
        write_wav(
            &path,
            &AudioBuffer::new(sr, 24, SampleEncoding::Int, vec![samples]),
        )
        .expect("write fixture");

        let mut app = YesVoxApp::from_session(YesVoxApp::default_session());
        app.session.open(&path).expect("open fixture");
        app.session.render().expect("render fixture");
        app.pin_compare_snapshot();
        assert!(app.session.compare_snapshot().is_some(), "pin failed");

        assert!(app.apply_edit("flow", |s| s.set_flow_amount(0.7)));
        app.undo();
        assert!(
            app.session.compare_snapshot().is_some(),
            "undo cleared the pinned C"
        );
        app.redo();
        assert!(
            app.session.compare_snapshot().is_some(),
            "redo cleared the pinned C"
        );

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn escape_cancels_the_live_render_from_any_view() {
        // #36: the Standard sidebar's cancel button is unreachable from
        // Advanced — Esc (and the top-bar button wired to the same
        // `cancel_render`) must work regardless of the active view.
        let mut app = YesVoxApp::from_session(YesVoxApp::default_session());
        let idle_status = app.status.clone();

        // Esc with no render running is a no-op, not a status complaint.
        app.apply_shortcuts(false, false, true);
        assert_eq!(app.status, idle_status);

        let release = hold_render_job(&mut app);
        app.view_mode = ViewMode::Advanced;
        app.apply_shortcuts(false, false, true);
        assert_eq!(app.status, "Render cancel requested");

        release.send(()).ok();
        pump_until_idle(&mut app);
    }

    #[test]
    fn top_bar_render_cancel_stays_inside_the_bar_and_clear_of_neighbors() {
        let top = ReferenceLayout::new().top;
        let progress = YesVoxApp::top_bar_render_progress_rect();
        let cancel = YesVoxApp::top_bar_render_cancel_rect();
        let ab_button = rect(1485.0, 14.0, 68.0, 40.0);
        let window_controls = YesVoxApp::top_bar_window_control_rects();

        for r in [progress, cancel] {
            assert!(
                top.contains(r.min) && top.contains(r.max),
                "render chrome must stay inside the top bar"
            );
            assert!(
                !r.intersects(ab_button)
                    && window_controls
                        .iter()
                        .all(|control| !r.intersects(*control)),
                "render chrome must not crowd the A/B or window-control buttons"
            );
        }
        assert!(!progress.intersects(cancel));
    }

    #[test]
    fn borderless_window_chrome_stays_reachable_and_clear() {
        let top = ReferenceLayout::new().top;
        let drag = YesVoxApp::top_bar_drag_rect();
        let presets = YesVoxApp::top_bar_presets_rect();
        let controls = YesVoxApp::top_bar_window_control_rects();
        let progress = YesVoxApp::top_bar_render_progress_rect();
        let cancel = YesVoxApp::top_bar_render_cancel_rect();

        for r in [drag, presets].into_iter().chain(controls) {
            assert!(
                top.contains(r.min) && top.contains(r.max),
                "borderless-window chrome must stay inside the top bar"
            );
        }
        for (index, control) in controls.iter().enumerate() {
            for other in controls.iter().skip(index + 1) {
                assert!(!control.intersects(*other));
            }
            assert!(!control.intersects(drag));
            assert!(!control.intersects(presets));
            assert!(!control.intersects(progress));
            assert!(!control.intersects(cancel));
        }
        assert!(!drag.intersects(presets));
    }

    #[test]
    fn edits_while_render_job_live_are_refused_and_not_recorded() {
        // #36: while a render runs, `session` is a `Session::new()` placeholder
        // (the real session moved onto the worker). An edit made then must not
        // survive adopt-back and must not leave a history entry.
        let mut app = YesVoxApp::from_session(YesVoxApp::default_session());
        let flow_before = app.session.flow_amount();
        assert_eq!(app.history.undo_depth(), 0);

        let release = hold_render_job(&mut app);
        assert!(app.edits_locked());

        assert!(!app.apply_edit("flow", |s| s.set_flow_amount(0.11)));
        assert_eq!(app.history.undo_depth(), 0, "refused edit left history");
        assert_eq!(app.status, SETTINGS_LOCKED_STATUS);

        release.send(()).ok();
        pump_until_idle(&mut app);

        // The refused edit did not land on the adopted session.
        assert_eq!(app.session.flow_amount(), flow_before);
        assert_eq!(app.history.undo_depth(), 0);

        // Edits work again once the job is gone.
        assert!(app.apply_edit("flow", |s| s.set_flow_amount(0.3)));
        assert_eq!(app.session.flow_amount(), 0.3);
        assert_eq!(app.history.undo_depth(), 1);
    }

    #[test]
    fn undo_and_redo_lock_while_a_render_job_is_live() {
        // Undo/redo restore settings snapshots — onto the placeholder, mid-job
        // (#36): the restore would vanish on adopt-back while the history
        // cursor moved, desyncing the two. Both must refuse while locked.
        let mut app = YesVoxApp::from_session(YesVoxApp::default_session());
        assert!(app.apply_edit("flow", |s| s.set_flow_amount(0.4)));
        assert_eq!(app.history.undo_depth(), 1);

        let release = hold_render_job(&mut app);

        app.undo();
        assert_eq!(app.history.undo_depth(), 1, "locked undo moved the cursor");
        assert_eq!(app.status, SETTINGS_LOCKED_STATUS);
        app.status.clear();
        app.redo();
        assert_eq!(app.status, SETTINGS_LOCKED_STATUS);

        release.send(()).ok();
        pump_until_idle(&mut app);
        assert_eq!(app.session.flow_amount(), 0.4);

        // Unlocked again: undo restores the pre-edit flow.
        app.undo();
        assert_eq!(app.session.flow_amount(), 1.0);
    }
}
