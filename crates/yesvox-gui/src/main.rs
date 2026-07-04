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

use audio::Player;
use eframe::egui;
use yesvox_core::worker::{spawn, JobEvent, JobHandle};
use yesvox_core::{
    AudioBuffer, BatchControl, BatchPresetMode, BatchSummary, CardId, ContentMode, History,
    JobStatus, LoudnessTarget, OutputFormat, PlaybackSource, Preset, RenderQueue, Session,
    SessionError, SessionSnapshot, VocalIsolationCheck,
};

/// Undo/redo depth cap (#25).
const HISTORY_CAP: usize = 100;

const DEFAULT_VIEWPORT_SIZE: [f32; 2] = [1920.0, 1080.0];
const MIN_VIEWPORT_SIZE: [f32; 2] = [1280.0, 720.0];

// eframe storage keys for persistence-lite (#24).
const STORAGE_LAST_DIR: &str = "yesvox_last_output_dir";
const STORAGE_LAST_FORMAT: &str = "yesvox_last_output_format";
const CANVAS: egui::Vec2 = egui::vec2(1920.0, 1080.0);

const BG_TOP: egui::Color32 = egui::Color32::from_rgb(3, 10, 18);
const BG_BOTTOM: egui::Color32 = egui::Color32::from_rgb(7, 17, 29);
const PANEL: egui::Color32 = egui::Color32::from_rgb(8, 21, 36);
const PANEL_2: egui::Color32 = egui::Color32::from_rgb(10, 27, 45);
const CARD: egui::Color32 = egui::Color32::from_rgb(11, 27, 45);
const FIELD: egui::Color32 = egui::Color32::from_rgb(9, 20, 34);
const BORDER: egui::Color32 = egui::Color32::from_rgb(29, 62, 93);
const BORDER_DIM: egui::Color32 = egui::Color32::from_rgb(19, 42, 65);
const TEXT: egui::Color32 = egui::Color32::from_rgb(229, 238, 255);
const TEXT_MUTED: egui::Color32 = egui::Color32::from_rgb(145, 159, 187);
const TEXT_DIM: egui::Color32 = egui::Color32::from_rgb(100, 119, 151);
const BLUE: egui::Color32 = egui::Color32::from_rgb(0, 131, 255);
const CYAN: egui::Color32 = egui::Color32::from_rgb(41, 214, 255);
const PURPLE: egui::Color32 = egui::Color32::from_rgb(141, 62, 255);
const VIOLET: egui::Color32 = egui::Color32::from_rgb(103, 67, 238);
const AMBER: egui::Color32 = egui::Color32::from_rgb(241, 211, 91);

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size(DEFAULT_VIEWPORT_SIZE)
            .with_min_inner_size(MIN_VIEWPORT_SIZE)
            .with_decorations(false)
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
                rect(1544.0, 474.0, 268.0, 160.0),
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
        reblend_bits: reblend_amount.clamp(0.0, 1.0).to_bits(),
        auditioning_reblend,
        generation,
    }
}

fn should_refresh_player_source(current: Option<AudibleCacheKey>, next: AudibleCacheKey) -> bool {
    current != Some(next)
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

/// Result of an off-thread batch run: the per-row status updates to apply and
/// the run summary.
struct BatchDone {
    updates: Vec<(usize, JobStatus)>,
    summary: BatchSummary,
}

struct YesVoxApp {
    session: Session,
    view_mode: ViewMode,
    tab: Tab,
    ab: AbView,
    path_input: String,
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
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        install_theme(&cc.egui_ctx);
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

        let history = History::new(session.snapshot(), HISTORY_CAP);
        YesVoxApp {
            session,
            view_mode: ViewMode::Standard,
            tab: Tab::Restore,
            ab: AbView::Processed,
            path_input: String::new(),
            source_path: None,
            source_name: "Lead Vocal.wav".to_owned(),
            name_input: "Lead Vocal_YesVox".to_owned(),
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

    fn bump_audio_generation(&mut self) {
        self.audio_generation = self.audio_generation.wrapping_add(1);
        self.audible_cache = None;
        self.player_source_key = None;
    }

    /// Undo the last settings edit (if any), restoring the session.
    fn undo(&mut self) {
        if let Some(snapshot) = self.history.undo().cloned() {
            self.session.restore_snapshot(&snapshot);
            self.stop_playback();
            self.bump_audio_generation();
            // The verdict described the pre-undo render; don't show it against
            // reverted settings.
            self.isolation_check = None;
            self.status = "Undo".to_owned();
        }
    }

    /// Redo the last undone edit (if any).
    fn redo(&mut self) {
        if let Some(snapshot) = self.history.redo().cloned() {
            self.session.restore_snapshot(&snapshot);
            self.stop_playback();
            self.bump_audio_generation();
            self.isolation_check = None;
            self.status = "Redo".to_owned();
        }
    }

    /// Draw the preset dropdown (when open) over everything else, and apply a
    /// selection. Rendered last in `ui()` so it sits above the panels (#24).
    fn draw_preset_menu(&mut self, ui: &mut egui::Ui, painter: &egui::Painter, c: CanvasMap) {
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
            self.session.select_preset(preset);
            self.commit_edit("preset");
            self.stop_playback(); // preset changes the render
            self.preset_menu_open = false;
            self.status = format!("Preset - {}", preset.name);
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
            .pick_file()
            .or_else(|| {
                let typed = self.path_input.trim();
                (!typed.is_empty()).then(|| PathBuf::from(typed))
            });
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
        self.stop_playback(); // new audio invalidates the current stream
        match self.session.open(&path) {
            Ok(_) => {
                self.source_path = Some(path.clone());
                self.path_input = path.display().to_string();
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
                self.ab = AbView::Processed;
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
                Err(SessionError::Canceled) => (false, "Render canceled".to_owned()),
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
        // Drain background-job events and repaint while work is in flight (#22).
        self.pump_jobs(ui.ctx());
        // Undo/redo shortcuts (#25): Ctrl/Cmd+Z, Ctrl/Cmd+Shift+Z, Ctrl/Cmd+Y.
        let (do_undo, do_redo) = ui.input(|i| {
            let cmd = i.modifiers.command;
            let undo = cmd && !i.modifiers.shift && i.key_pressed(egui::Key::Z);
            let redo = cmd
                && ((i.modifiers.shift && i.key_pressed(egui::Key::Z))
                    || (!i.modifiers.shift && i.key_pressed(egui::Key::Y)));
            (undo, redo)
        });
        if do_undo {
            self.undo();
        }
        if do_redo {
            self.redo();
        }
        let available = ui.max_rect();
        let canvas = CanvasMap::new(available);
        let painter = ui.painter().clone();

        painter.rect_filled(available, egui::CornerRadius::ZERO, BG_BOTTOM);
        paint_v_gradient(&painter, canvas.rect, BG_TOP, BG_BOTTOM, 72);
        let layout = ReferenceLayout::new();
        self.draw_top_bar(ui, &painter, canvas, layout);
        match self.view_mode {
            ViewMode::Standard => {
                self.draw_left_sidebar(ui, &painter, canvas, layout);
                self.draw_center(ui, &painter, canvas, layout);
                self.draw_right_sidebar(ui, &painter, canvas, layout);
                self.draw_status_bar(&painter, canvas, layout);
            }
            ViewMode::Advanced => {
                let advanced = AdvancedLayout::new();
                self.draw_advanced(ui, &painter, canvas, advanced);
                self.draw_status_bar(&painter, canvas, ReferenceLayout::new());
            }
        }
        self.draw_preset_menu(ui, &painter, canvas); // overlay, drawn last
    }
}

impl YesVoxApp {
    fn draw_top_bar(
        &mut self,
        ui: &mut egui::Ui,
        painter: &egui::Painter,
        c: CanvasMap,
        l: ReferenceLayout,
    ) {
        paint_panel(painter, c, l.top, 0, alpha(PANEL, 235), BORDER_DIM);
        self.draw_wordmark(painter, c, egui::pos2(18.0, 35.0));

        if self.segment_button(
            ui,
            painter,
            c,
            rect(314.0, 14.0, 124.0, 40.0),
            "Standard",
            self.view_mode == ViewMode::Standard,
            "view_standard",
        ) {
            self.view_mode = ViewMode::Standard;
        }
        if self.segment_button(
            ui,
            painter,
            c,
            rect(440.0, 14.0, 124.0, 40.0),
            "Advanced",
            self.view_mode == ViewMode::Advanced,
            "view_advanced",
        ) {
            self.view_mode = ViewMode::Advanced;
        }

        let restore = rect(742.0, 13.0, 184.0, 44.0);
        let polish = rect(928.0, 13.0, 184.0, 44.0);
        if self.view_mode == ViewMode::Standard {
            if self.segment_button(
                ui,
                painter,
                c,
                restore,
                "Restore",
                self.tab == Tab::Restore,
                "top_restore",
            ) {
                self.tab = Tab::Restore;
            }
            if self.segment_button(
                ui,
                painter,
                c,
                polish,
                "Polish",
                self.tab == Tab::Polish,
                "top_polish",
            ) {
                self.tab = Tab::Polish;
            }
        } else {
            self.section_label(painter, c, 742.0, 35.0, "ADVANCED");
            self.small_text(
                painter,
                c,
                846.0,
                35.0,
                "module depth + spectrogram",
                TEXT_MUTED,
            );
        }

        // Undo/redo (#25): enabled state reflects real stack depth; clicks drive
        // the tested History.
        if self.icon_button(
            ui,
            painter,
            c,
            rect(1386.0, 14.0, 38.0, 40.0),
            "Undo",
            self.history.can_undo(),
        ) {
            self.undo();
        }
        if self.icon_button(
            ui,
            painter,
            c,
            rect(1434.0, 14.0, 38.0, 40.0),
            "Redo",
            self.history.can_redo(),
        ) {
            self.redo();
        }
        if self.outline_button(
            ui,
            painter,
            c,
            rect(1485.0, 14.0, 68.0, 40.0),
            "A / B",
            "top_ab",
        ) {
            self.toggle_ab_view(); // instant A/B at the shared playhead
        }
        // Preset picker (#24): the top-bar dropdown is the only way to change
        // preset. Gear/Help/"Back to Suite" were PNG-only dead chrome — removed.
        if self.outline_button(
            ui,
            painter,
            c,
            rect(1775.0, 14.0, 130.0, 40.0),
            "Presets v",
            "top_presets",
        ) {
            self.preset_menu_open = !self.preset_menu_open;
        }
    }

    fn draw_left_sidebar(
        &mut self,
        ui: &mut egui::Ui,
        painter: &egui::Painter,
        c: CanvasMap,
        l: ReferenceLayout,
    ) {
        paint_panel(painter, c, l.left, 8, alpha(PANEL, 225), BORDER_DIM);
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
            alpha(BLUE, 82),
            BLUE,
        );
        self.body_text(painter, c, 38.0, 171.0, "01", CYAN);
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
            self.session.set_content_mode(ContentMode::Voiceover);
            self.commit_edit("content_mode"); // undoable — it resets the rack (#25)
            self.stop_playback();
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
            self.session.set_content_mode(ContentMode::Singing);
            self.commit_edit("content_mode"); // undoable — it resets the rack (#25)
            self.stop_playback();
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
        paint_panel(painter, c, l.hero, 8, alpha(PANEL, 210), BORDER_DIM);
        let source = self.source_name.to_uppercase();
        self.title_text(painter, c, 322.0, 106.0, &source);
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
            self.session.set_flow_amount(flow);
            self.commit_edit("flow");
        }
        self.body_text(painter, c, 1288.0, 120.0, "x1.00", TEXT);

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
        self.draw_spectrum(painter, c, spec, self.session.difference().as_ref());
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
        for (card, module) in l.module_cards.iter().zip(modules) {
            self.advanced_module_card(painter, c, *card, module);
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

    fn advanced_module_card(
        &self,
        painter: &egui::Painter,
        c: CanvasMap,
        r: egui::Rect,
        (index, title, line1, line2): (&str, &str, &str, &str),
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
        self.micro_text(
            painter,
            c,
            r.left() + 12.0,
            r.top() + 56.0,
            line1,
            TEXT_MUTED,
        );
        self.micro_text(
            painter,
            c,
            r.left() + 12.0,
            r.top() + 78.0,
            line2,
            TEXT_MUTED,
        );
        paint_panel(
            painter,
            c,
            rect(r.left() + 12.0, r.bottom() - 18.0, r.width() - 24.0, 5.0),
            3,
            FIELD,
            egui::Color32::TRANSPARENT,
        );
        paint_h_gradient(
            painter,
            c,
            rect(
                r.left() + 12.0,
                r.bottom() - 18.0,
                (r.width() - 24.0) * 0.58,
                5.0,
            ),
            BLUE,
            CYAN,
            14,
            3,
        );
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
                    self.session.set_restoration_focus(value);
                    self.commit_edit("restoration_focus");
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
                    self.session.set_polish_intensity(value);
                    self.commit_edit("polish");
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
        let mut on = self.session.card_enabled(card);
        if self.toggle(
            ui,
            painter,
            c,
            rect(r.right() - 43.0, r.top() + 18.0, 34.0, 20.0),
            on,
            ("card_toggle", card.label()),
        ) {
            on = !on;
            self.session.set_card_enabled(card, on);
            self.commit_edit(&format!("card_toggle:{}", card.label()));
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
                self.session.set_card_amount(card, next);
                self.commit_edit(&format!("card_amount:{}", card.label()));
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
        paint_panel(painter, c, l.right, 8, alpha(PANEL, 225), BORDER_DIM);
        let preset = rect(1588.0, 70.0, 304.0, 292.0);
        paint_panel(painter, c, preset, 8, alpha(PANEL_2, 230), BORDER_DIM);
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
        self.small_text(painter, c, 1608.0, 212.0, "DESCRIPTION", TEXT_MUTED);
        self.small_text(
            painter,
            c,
            1608.0,
            246.0,
            "Balanced restoration for singing vocals.",
            TEXT_MUTED,
        );
        self.small_text(
            painter,
            c,
            1608.0,
            272.0,
            "Preserves character and natural ambience.",
            TEXT_MUTED,
        );
        // "Save as New Preset" removed (#24): no user-preset store exists.
        let out = rect(1588.0, 372.0, 304.0, 294.0);
        paint_panel(painter, c, out, 8, alpha(PANEL_2, 230), BORDER_DIM);
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
            self.session.set_dither(next);
            self.commit_edit("dither");
        }

        let file = rect(1588.0, 680.0, 304.0, 216.0);
        paint_panel(painter, c, file, 8, alpha(PANEL_2, 230), BORDER_DIM);
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
        let name_resp = ui.put(
            c.rect(rect(1620.0, 780.0, 244.0, 24.0)),
            egui::TextEdit::singleline(&mut self.name_input).text_color(TEXT),
        );
        if name_resp.changed() {
            self.session.set_output_name(&self.name_input);
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
            .unwrap_or_else(|| "/Users/You/Music/YesVox".to_owned());
        let location_short = truncate_middle(&location, 30);
        self.body_text(painter, c, 1622.0, 858.0, &location_short, TEXT);
        self.folder_icon(painter, c, egui::pos2(1850.0, 856.0));
        // Click the Location field to pick an output folder (#24, rfd).
        if self
            .response(ui, c, rect(1608.0, 840.0, 264.0, 34.0), "location_pick")
            .clicked()
        {
            if let Some(dir) = rfd::FileDialog::new()
                .set_title("Output Location")
                .pick_folder()
            {
                self.session.set_output_location(&dir);
            }
        }
        // "Add to Yes Suite Library" removed (#24): the suite handoff is a
        // designed-for seam, not a built-in v1 feature.

        paint_panel(
            painter,
            c,
            rect(1588.0, 906.0, 304.0, 86.0),
            8,
            alpha(PANEL_2, 230),
            BORDER_DIM,
        );
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
        if self.gradient_button(
            ui,
            painter,
            c,
            rect(1588.0, 1010.0, 304.0, 28.0),
            "EXPORT VOCAL",
            "export_vocal",
        ) {
            if self.busy() {
                self.status = "Working - please wait".to_owned();
            } else {
                match self.session.prepare_export() {
                    Ok(prepared) => {
                        // Write off the UI thread; the session stays put (export
                        // is read-only) — #22.
                        self.job_progress = 0.0;
                        self.status = "Exporting...".to_owned();
                        self.export_job = Some(spawn(move |ctx| {
                            ctx.progress(0.1);
                            let path = prepared.write().map_err(|e| e.to_string())?;
                            ctx.progress(1.0);
                            Ok(path)
                        }));
                    }
                    Err(e) => self.status = format!("Export failed: {e}"),
                }
            }
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
        self.small_text(painter, c, 1662.0, 1066.0, &self.status, TEXT_DIM);
        self.badge(
            painter,
            c,
            rect(1760.0, 1049.0, 142.0, 24.0),
            &self.processing_label().to_ascii_uppercase(),
            true,
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
        if self.checkbox(
            ui,
            painter,
            c,
            rect(1136.0, 374.0, 18.0, 18.0),
            self.volume_match,
            "volume_match",
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
        let selection = rect(r.left(), r.top(), r.width() * 0.20, r.height());
        paint_h_gradient(
            painter,
            c,
            selection,
            alpha(BLUE, 90),
            alpha(PURPLE, 75),
            24,
            4,
        );
        for i in 0..buckets {
            let x = sr.left() + i as f32;
            let t = i as f32 / buckets.max(1) as f32;
            let (lo, hi) = peaks
                .as_ref()
                .and_then(|p| p.get(i).copied())
                .unwrap_or_else(|| synthetic_peak(t));
            let color = lerp_color(CYAN, PURPLE, t);
            painter.line_segment(
                [
                    egui::pos2(x, mid - hi.abs().max(0.03) * height),
                    egui::pos2(x, mid + lo.abs().max(0.03) * height),
                ],
                egui::Stroke::new(c.v(1.0), color),
            );
        }
        painter.line_segment(
            [
                c.pos(r.left() + r.width() * 0.20, r.top()),
                c.pos(r.left() + r.width() * 0.20, r.bottom()),
            ],
            egui::Stroke::new(c.v(2.0), PURPLE),
        );
    }

    fn draw_time_labels(&self, painter: &egui::Painter, c: CanvasMap, r: egui::Rect) {
        let duration = self.input_duration().unwrap_or(208.0);
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
        let mut reblend = self.reblend_amount;
        reblend = self.slider(
            ui,
            painter,
            c,
            rect(r.left() + 628.0, r.top() + 74.0, 185.0, 8.0),
            reblend,
            "reblend",
            BLUE,
            BLUE,
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
        painter.text(
            c.pos(x, y),
            egui::Align2::LEFT_CENTER,
            text,
            c.font(15.0),
            CYAN,
        );
    }

    fn title_text(&self, painter: &egui::Painter, c: CanvasMap, x: f32, y: f32, text: &str) {
        painter.text(
            c.pos(x, y),
            egui::Align2::LEFT_CENTER,
            text,
            c.font(20.0),
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
        let clicked = self.response(ui, c, r, id).clicked();
        if selected {
            paint_h_gradient(painter, c, r, VIOLET, CYAN, 24, 7);
        } else {
            paint_panel(painter, c, r, 7, FIELD, BORDER_DIM);
        }
        painter.text(
            c.rect(r).center(),
            egui::Align2::CENTER_CENTER,
            label,
            c.font(14.0),
            if selected { TEXT } else { TEXT_MUTED },
        );
        clicked
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
        let end = if resp.hovered() { CYAN } else { BLUE };
        paint_h_gradient(painter, c, r, PURPLE, end, 32, 6);
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
        let resp = self.response(ui, c, r, id);
        if selected {
            paint_h_gradient(painter, c, r, VIOLET, BLUE, 24, 6);
        } else {
            paint_panel(painter, c, r, 6, alpha(PANEL_2, 230), BORDER_DIM);
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
        resp.clicked()
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
                            self.session.set_output_format(next);
                            self.commit_edit("format");
                        }
                        "Bit Depth" => {
                            let current = self
                                .session
                                .settings()
                                .export
                                .bit_depth_override
                                .or_else(|| self.session.input().map(|b| b.bits_per_sample()))
                                .unwrap_or(24);
                            self.session
                                .set_bit_depth(Some(if current >= 24 { 16 } else { 24 }));
                            self.commit_edit("bit_depth");
                        }
                        "Loudness Target" => {
                            let next = cycle_loudness(self.session.loudness_target());
                            self.session.set_loudness_target(next);
                            self.commit_edit("loudness");
                            self.stop_playback(); // target change invalidates the render
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
        let hit = r.expand(10.0);
        let resp = ui.interact(
            c.rect(hit),
            ui.make_persistent_id(id),
            egui::Sense::click_and_drag(),
        );
        let mut next = value.clamp(0.0, 1.0);
        if (resp.clicked() || resp.dragged()) && resp.interact_pointer_pos().is_some() {
            let pos = resp.interact_pointer_pos().unwrap();
            let sr = c.rect(r);
            next = ((pos.x - sr.left()) / sr.width()).clamp(0.0, 1.0);
        }
        paint_panel(
            painter,
            c,
            r,
            3,
            egui::Color32::from_rgb(21, 44, 70),
            egui::Color32::TRANSPARENT,
        );
        let filled = rect(r.left(), r.top(), r.width() * next, r.height());
        paint_h_gradient(painter, c, filled, start, end, 20, 3);
        let knob = c.pos(r.left() + r.width() * next, r.center().y);
        painter.circle_filled(knob, c.v(6.0), egui::Color32::from_rgb(190, 168, 255));
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
        let resp = ui.interact(
            c.rect(r),
            ui.make_persistent_id(id),
            egui::Sense::click_and_drag(),
        );
        let mut next = value.clamp(0.0, 1.0);
        if (resp.clicked() || resp.dragged()) && resp.interact_pointer_pos().is_some() {
            let p = resp.interact_pointer_pos().unwrap();
            let center = c.pos(r.center().x, r.top() + 80.0);
            let angle = (p.y - center.y).atan2(p.x - center.x);
            let start = -225.0_f32.to_radians();
            let end = 45.0_f32.to_radians();
            let mut t = (angle - start) / (end - start);
            if t < -0.2 {
                t += 1.0;
            }
            next = t.clamp(0.0, 1.0);
        }
        let center = c.pos(r.center().x, r.top() + 78.0);
        let radius = c.v(48.0);
        arc(
            painter,
            center,
            radius,
            -225.0,
            45.0,
            egui::Stroke::new(c.v(8.0), egui::Color32::from_rgb(30, 47, 80)),
        );
        arc(
            painter,
            center,
            radius,
            -225.0,
            -225.0 + 270.0 * next,
            egui::Stroke::new(c.v(8.0), BLUE),
        );
        arc(
            painter,
            center,
            radius * 0.84,
            0.0,
            45.0,
            egui::Stroke::new(c.v(4.0), PURPLE),
        );
        let angle = (-225.0 + 270.0 * next).to_radians();
        let needle = center + egui::vec2(angle.cos() * radius * 0.74, angle.sin() * radius * 0.74);
        painter.line_segment([center, needle], egui::Stroke::new(c.v(2.0), TEXT));
        painter.circle_filled(center, c.v(4.0), CYAN);
        painter.text(
            center,
            egui::Align2::CENTER_CENTER,
            format!("{:.0}%", next * 100.0),
            c.font(17.0),
            TEXT,
        );
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
        resp.clicked()
    }

    fn checkbox(
        &self,
        ui: &mut egui::Ui,
        painter: &egui::Painter,
        c: CanvasMap,
        r: egui::Rect,
        checked: bool,
        id: impl Hash + Debug,
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
        resp.clicked()
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
        painter.circle_stroke(sc, r, egui::Stroke::new(c.v(1.0), BORDER_DIM));
        arc(
            painter,
            sc,
            r,
            -210.0,
            -210.0 + 270.0 * value.clamp(0.0, 1.0),
            egui::Stroke::new(c.v(3.0), CYAN),
        );
        painter.text(
            sc,
            egui::Align2::CENTER_CENTER,
            format!("{:.0}%", value * 100.0),
            c.font(14.0),
            TEXT,
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
        let hit = rect(center.x - 30.0, center.y - 30.0, 60.0, 60.0);
        let resp = ui.interact(
            c.rect(hit),
            ui.make_persistent_id(id),
            egui::Sense::click_and_drag(),
        );
        let mut next = value.clamp(0.0, 1.0);
        if resp.dragged() {
            // Screen-space vertical drag; ~250 px sweeps the full range.
            next = (next - resp.drag_delta().y * 0.004).clamp(0.0, 1.0);
        }
        self.percent_knob(painter, c, center, next);
        next
    }

    fn loudness_knob(&self, painter: &egui::Painter, c: CanvasMap, center: egui::Pos2) {
        let sc = c.pos(center.x, center.y);
        let r = c.v(27.0);
        painter.circle_stroke(sc, r, egui::Stroke::new(c.v(1.0), BORDER_DIM));
        arc(
            painter,
            sc,
            r,
            -210.0,
            60.0,
            egui::Stroke::new(c.v(3.0), CYAN),
        );
        painter.text(sc, egui::Align2::CENTER_CENTER, "-14.0", c.font(13.0), TEXT);
        painter.text(
            sc + egui::vec2(0.0, c.v(14.0)),
            egui::Align2::CENTER_CENTER,
            "LUFS",
            c.font(9.0),
            TEXT_MUTED,
        );
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
                .unwrap_or_else(|| {
                    (0.12 + (t * 31.0).sin().abs() * 0.50 + (t * 113.0).cos().abs() * 0.28)
                        * r.height()
                });
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

fn synthetic_peak(t: f32) -> (f32, f32) {
    let env = if t < 0.18 {
        0.82
    } else if t < 0.36 {
        0.44
    } else if t < 0.72 {
        0.35 + 0.20 * (t * 18.0).sin().abs()
    } else {
        0.24 + 0.16 * (t * 30.0).sin().abs()
    };
    let fast = (t * 180.0).sin().abs() * 0.55 + (t * 430.0).cos().abs() * 0.45;
    let hi = (env * fast).clamp(0.04, 0.95);
    (-hi * (0.68 + 0.25 * (t * 21.0).cos().abs()), hi)
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
    painter.rect_filled(sr, egui::CornerRadius::same(radius), fill);
    if stroke != egui::Color32::TRANSPARENT {
        painter.rect_stroke(
            sr,
            egui::CornerRadius::same(radius),
            egui::Stroke::new(c.v(1.0), stroke),
            egui::StrokeKind::Inside,
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

fn lerp_u8(a: u8, b: u8, t: f32) -> u8 {
    (a as f32 + (b as f32 - a as f32) * t).round() as u8
}

fn alpha(c: egui::Color32, a: u8) -> egui::Color32 {
    let [r, g, b, _] = c.to_array();
    egui::Color32::from_rgba_premultiplied(r, g, b, a)
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
    visuals.widgets.hovered.bg_fill = PANEL_2;
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
            audible_cache_key(AbView::Processed, false, 0.50, false, 7)
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
}
