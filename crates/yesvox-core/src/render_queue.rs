//! The non-destructive batch render queue (plan §5.6): apply one configuration
//! across many vocals, never overwriting, skipping (and recording) per-file
//! failures so one bad file can't sink the batch.
//!
//! Each item is opened in its own [`Session`] seeded from the queue's settings +
//! preset, rendered, and exported (the writer auto-suffixes on collision). A
//! failure is captured in the item's [`JobStatus`] and the run continues.

use std::io::Write;
use std::path::{Path, PathBuf};

use crate::analysis::AnalysisReport;
use crate::preset::Preset;
use crate::session::Session;
use crate::settings::Settings;
use serde::{Deserialize, Serialize};

const QUEUE_SCHEMA_VERSION: u32 = 1;
const DETECTED_PRESET_CONFIDENCE: f32 = 0.70;

/// The outcome of a queued render.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "status", content = "detail")]
pub enum JobStatus {
    /// Not yet processed.
    Pending,
    /// Written to this path.
    Done(PathBuf),
    /// Failed with this message; the batch continued.
    Failed(String),
}

/// One queued vocal: where it comes from, where it goes, and how it went.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueItem {
    /// Source file.
    pub input: PathBuf,
    /// Requested output path (auto-suffixed on collision by the writer).
    pub output: PathBuf,
    /// Current status.
    pub status: JobStatus,
}

/// One failed queue item for end-of-batch retry/summary UI.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BatchFailure {
    /// Index in the queue at the time the run processed this item.
    pub index: usize,
    /// Source file.
    pub input: PathBuf,
    /// Requested output path.
    pub output: PathBuf,
    /// Failure message captured from the renderer.
    pub error: String,
}

/// Tally of a batch run, including the failed items a UI can surface for retry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BatchSummary {
    /// Files written.
    pub done: usize,
    /// Files that failed (skipped + logged).
    pub failed: usize,
    /// True when the caller stopped the run before all pending files were tried.
    pub canceled: bool,
    /// Per-file failures from this run only.
    pub failures: Vec<BatchFailure>,
}

/// A UI/agent progress snapshot emitted before and after queue items run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BatchProgress {
    /// Total queue length, including already-done/failed rows.
    pub total: usize,
    /// Queue rows that are no longer pending at this point.
    pub completed: usize,
    /// The item about to run; `None` means the queue is between files or done.
    pub current_index: Option<usize>,
    /// Files written during this run.
    pub done: usize,
    /// Files failed during this run.
    pub failed: usize,
}

/// Control response from a progress observer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BatchControl {
    /// Keep processing pending files.
    Continue,
    /// Stop before starting the next pending file. The currently reported item
    /// remains pending if cancellation is requested from a pre-file callback.
    Cancel,
}

/// How a batch chooses the preset for each queued file.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BatchPresetMode {
    /// Apply one explicit preset to every file in the queue.
    Force(Preset),
    /// Let each file's analysis choose its content-mode default when confidence
    /// is high; otherwise use the explicit fallback preset.
    DetectPerFile { fallback: Preset },
}

impl BatchPresetMode {
    fn preset_for_report(self, report: &AnalysisReport) -> Preset {
        match self {
            BatchPresetMode::Force(preset) => preset,
            BatchPresetMode::DetectPerFile { fallback } => {
                if report.content.confidence >= DETECTED_PRESET_CONFIDENCE {
                    Preset::default_for(report.content.mode)
                } else {
                    fallback
                }
            }
        }
    }

    fn fallback_preset(self) -> Preset {
        match self {
            BatchPresetMode::Force(preset)
            | BatchPresetMode::DetectPerFile { fallback: preset } => preset,
        }
    }
}

impl BatchSummary {
    fn empty() -> Self {
        BatchSummary {
            done: 0,
            failed: 0,
            canceled: false,
            failures: Vec::new(),
        }
    }

    #[cfg(test)]
    fn with_counts(mut self, done: usize, failed: usize) -> Self {
        self.done = done;
        self.failed = failed;
        self
    }
}

/// Errors loading or saving crash-recoverable queue state.
#[derive(Debug)]
pub enum QueueStateError {
    /// Filesystem error.
    Io(std::io::Error),
    /// JSON serialization or parsing error.
    Json(serde_json::Error),
    /// The queue file names a preset this build does not know.
    UnknownPreset(String),
}

impl std::fmt::Display for QueueStateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            QueueStateError::Io(e) => write!(f, "queue state io error: {e}"),
            QueueStateError::Json(e) => write!(f, "queue state json error: {e}"),
            QueueStateError::UnknownPreset(name) => {
                write!(f, "queue state references unknown preset: {name}")
            }
        }
    }
}

impl std::error::Error for QueueStateError {}

impl From<std::io::Error> for QueueStateError {
    fn from(e: std::io::Error) -> Self {
        QueueStateError::Io(e)
    }
}

impl From<serde_json::Error> for QueueStateError {
    fn from(e: serde_json::Error) -> Self {
        QueueStateError::Json(e)
    }
}

#[derive(Serialize, Deserialize)]
struct RenderLogEntry {
    schema_version: u32,
    queue_index: usize,
    input: PathBuf,
    output: PathBuf,
    status: JobStatus,
}

/// A batch render queue applying one settings + preset across many files.
pub struct RenderQueue {
    settings: Settings,
    preset_mode: BatchPresetMode,
    paused: bool,
    items: Vec<QueueItem>,
}

impl RenderQueue {
    /// A queue that renders every file with `settings` + `preset` (force-preset
    /// batch mode).
    pub fn new(settings: Settings, preset: Preset) -> Self {
        RenderQueue {
            settings,
            preset_mode: BatchPresetMode::Force(preset),
            paused: false,
            items: Vec::new(),
        }
    }

    /// A queue that lets each file's analysis choose the Voiceover/Singing
    /// default preset when confidence is high, with an explicit fallback for
    /// low-confidence files.
    pub fn detect_per_file(settings: Settings, fallback: Preset) -> Self {
        RenderQueue {
            settings,
            preset_mode: BatchPresetMode::DetectPerFile { fallback },
            paused: false,
            items: Vec::new(),
        }
    }

    /// Append a file to the queue.
    pub fn enqueue(&mut self, input: impl Into<PathBuf>, output: impl Into<PathBuf>) {
        self.items.push(QueueItem {
            input: input.into(),
            output: output.into(),
            status: JobStatus::Pending,
        });
    }

    /// Number of queued items.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Whether the queue is empty.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// The queued items and their statuses.
    pub fn items(&self) -> &[QueueItem] {
        &self.items
    }

    /// The queue's batch preset-selection mode.
    pub fn preset_mode(&self) -> BatchPresetMode {
        self.preset_mode
    }

    /// Whether the queue is currently paused. A paused queue keeps accepting
    /// appends/reorders, but [`RenderQueue::run`] leaves all pending items alone.
    pub fn is_paused(&self) -> bool {
        self.paused
    }

    /// Pause processing after the current run boundary.
    pub fn pause(&mut self) {
        self.paused = true;
    }

    /// Resume processing of pending items.
    pub fn resume(&mut self) {
        self.paused = false;
    }

    /// Move a queued item from `from` to `to`, preserving its status. Returns
    /// `false` when either index is outside the queue.
    pub fn move_item(&mut self, from: usize, to: usize) -> bool {
        if from >= self.items.len() || to >= self.items.len() {
            return false;
        }
        if from == to {
            return true;
        }
        let item = self.items.remove(from);
        self.items.insert(to, item);
        true
    }

    /// Mark selected failed items as pending so the next run retries them.
    ///
    /// Done and already-pending items are left untouched; out-of-range indices
    /// are ignored. Returns the number of failed items selected for retry.
    pub fn retry_failed(&mut self, indices: impl IntoIterator<Item = usize>) -> usize {
        let mut selected = 0;
        for index in indices {
            if let Some(item) = self.items.get_mut(index) {
                if matches!(item.status, JobStatus::Failed(_)) {
                    item.status = JobStatus::Pending;
                    selected += 1;
                }
            }
        }
        selected
    }

    /// Serialize the queue to pretty JSON for crash recovery.
    pub fn to_json(&self) -> Result<String, QueueStateError> {
        Ok(serde_json::to_string_pretty(&PersistentQueue::from_queue(
            self,
        ))?)
    }

    /// Parse a queue from JSON and force it to paused recovery mode.
    pub fn from_json(s: &str) -> Result<Self, QueueStateError> {
        PersistentQueue::into_queue(serde_json::from_str(s)?)
    }

    /// Save `queue.json` atomically and keep a one-rotation `queue.json.bak`.
    pub fn save(&self, path: &Path) -> Result<(), QueueStateError> {
        let json = self.to_json()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let backup = queue_backup_path(path);
        if path.exists() {
            let _ = std::fs::remove_file(&backup);
            std::fs::copy(path, &backup)?;
        }

        let tmp = queue_temp_path(path);
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    /// Load `queue.json`; if the primary is absent/corrupt, recover from
    /// `queue.json.bak`. Recovered queues always start paused.
    pub fn load_or_empty(path: &Path) -> Result<Self, QueueStateError> {
        match load_queue(path) {
            Ok(q) => Ok(q),
            Err(QueueStateError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {
                let backup = queue_backup_path(path);
                match load_queue(&backup) {
                    Ok(q) => Ok(q),
                    Err(QueueStateError::Io(be)) if be.kind() == std::io::ErrorKind::NotFound => {
                        Ok(Self::new(
                            Settings::default(),
                            Preset::default_for(Default::default()),
                        ))
                    }
                    Err(e) => Err(e),
                }
            }
            Err(_) => load_queue(&queue_backup_path(path)),
        }
    }

    /// Process every pending item, skipping (and recording) failures. Returns a
    /// tally of the run.
    pub fn run(&mut self) -> BatchSummary {
        self.run_inner(None)
            .expect("in-memory queue run cannot fail")
    }

    /// Process every pending item and append one JSON object per processed file
    /// to `log_path`.
    pub fn run_with_log(&mut self, log_path: &Path) -> Result<BatchSummary, QueueStateError> {
        self.run_inner(Some(log_path))
    }

    /// Process pending items while reporting progress and allowing a UI/agent to
    /// cancel at file boundaries.
    pub fn run_with_progress(
        &mut self,
        mut progress: impl FnMut(BatchProgress) -> BatchControl,
    ) -> BatchSummary {
        self.run_inner_with_progress(None, &mut progress)
            .expect("in-memory queue run cannot fail")
    }

    fn run_inner(&mut self, log_path: Option<&Path>) -> Result<BatchSummary, QueueStateError> {
        self.run_inner_with_progress(log_path, &mut |_| BatchControl::Continue)
    }

    fn run_inner_with_progress(
        &mut self,
        log_path: Option<&Path>,
        progress: &mut dyn FnMut(BatchProgress) -> BatchControl,
    ) -> Result<BatchSummary, QueueStateError> {
        if self.paused {
            return Ok(BatchSummary::empty());
        }

        let mut done = 0;
        let mut failed = 0;
        let mut canceled = false;
        let mut failures = Vec::new();
        let mut log = match log_path {
            Some(path) => Some(open_render_log(path)?),
            None => None,
        };
        let total = self.items.len();
        for index in 0..self.items.len() {
            if !matches!(self.items[index].status, JobStatus::Pending) {
                continue; // already processed (resume-safe)
            }
            let completed_before = completed_items(&self.items);
            if progress(BatchProgress {
                total,
                completed: completed_before,
                current_index: Some(index),
                done,
                failed,
            }) == BatchControl::Cancel
            {
                canceled = true;
                break;
            }
            let item = &mut self.items[index];
            item.status = render_one(&self.settings, self.preset_mode, &item.input, &item.output);
            if let Some(file) = log.as_mut() {
                append_render_log(file, index, item)?;
            }
            match &item.status {
                JobStatus::Done(_) => done += 1,
                JobStatus::Failed(error) => {
                    failed += 1;
                    failures.push(BatchFailure {
                        index,
                        input: item.input.clone(),
                        output: item.output.clone(),
                        error: error.clone(),
                    });
                }
                JobStatus::Pending => {}
            }
            if progress(BatchProgress {
                total,
                completed: completed_before + 1,
                current_index: None,
                done,
                failed,
            }) == BatchControl::Cancel
            {
                canceled = true;
                break;
            }
        }
        Ok(BatchSummary {
            done,
            failed,
            canceled,
            failures,
        })
    }
}

#[derive(Serialize, Deserialize)]
struct PersistentQueue {
    schema_version: u32,
    // Kept for backward compatibility with v1 queue.json files written before
    // explicit batch preset modes existed.
    settings: Settings,
    preset: String,
    #[serde(default)]
    preset_mode: Option<PersistentPresetMode>,
    paused: bool,
    items: Vec<QueueItem>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "mode")]
enum PersistentPresetMode {
    Force { preset: String },
    DetectPerFile { fallback_preset: String },
}

impl PersistentQueue {
    fn from_queue(queue: &RenderQueue) -> Self {
        let fallback = queue.preset_mode.fallback_preset().name.to_owned();
        PersistentQueue {
            schema_version: QUEUE_SCHEMA_VERSION,
            settings: queue.settings.clone(),
            preset: fallback.clone(),
            preset_mode: Some(PersistentPresetMode::from_mode(queue.preset_mode)),
            paused: queue.paused,
            items: queue.items.clone(),
        }
    }

    fn into_queue(saved: PersistentQueue) -> Result<RenderQueue, QueueStateError> {
        let preset_mode = match saved.preset_mode {
            Some(mode) => mode.into_mode()?,
            None => BatchPresetMode::Force(preset_by_name(&saved.preset)?),
        };
        Ok(RenderQueue {
            settings: saved.settings,
            preset_mode,
            // Recovery must not render until the app/user explicitly resumes.
            paused: true,
            items: saved.items,
        })
    }
}

impl PersistentPresetMode {
    fn from_mode(mode: BatchPresetMode) -> Self {
        match mode {
            BatchPresetMode::Force(preset) => PersistentPresetMode::Force {
                preset: preset.name.to_owned(),
            },
            BatchPresetMode::DetectPerFile { fallback } => PersistentPresetMode::DetectPerFile {
                fallback_preset: fallback.name.to_owned(),
            },
        }
    }

    fn into_mode(self) -> Result<BatchPresetMode, QueueStateError> {
        Ok(match self {
            PersistentPresetMode::Force { preset } => {
                BatchPresetMode::Force(preset_by_name(&preset)?)
            }
            PersistentPresetMode::DetectPerFile { fallback_preset } => {
                BatchPresetMode::DetectPerFile {
                    fallback: preset_by_name(&fallback_preset)?,
                }
            }
        })
    }
}

fn preset_by_name(name: &str) -> Result<Preset, QueueStateError> {
    Preset::ALL
        .iter()
        .copied()
        .find(|p| p.name == name)
        .ok_or_else(|| QueueStateError::UnknownPreset(name.to_owned()))
}

fn load_queue(path: &Path) -> Result<RenderQueue, QueueStateError> {
    RenderQueue::from_json(&std::fs::read_to_string(path)?)
}

fn queue_temp_path(path: &Path) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(".yesvox-tmp");
    PathBuf::from(s)
}

fn queue_backup_path(path: &Path) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(".bak");
    PathBuf::from(s)
}

fn open_render_log(path: &Path) -> Result<std::fs::File, QueueStateError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    Ok(std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?)
}

fn append_render_log(
    file: &mut std::fs::File,
    queue_index: usize,
    item: &QueueItem,
) -> Result<(), QueueStateError> {
    let entry = RenderLogEntry {
        schema_version: QUEUE_SCHEMA_VERSION,
        queue_index,
        input: item.input.clone(),
        output: item.output.clone(),
        status: item.status.clone(),
    };
    serde_json::to_writer(&mut *file, &entry)?;
    file.write_all(b"\n")?;
    Ok(())
}

fn completed_items(items: &[QueueItem]) -> usize {
    items
        .iter()
        .filter(|item| !matches!(item.status, JobStatus::Pending))
        .count()
}

/// Render one file with the given configuration (a free fn so `run` can borrow
/// `self.items` mutably without aliasing the config).
fn render_one(
    settings: &Settings,
    preset_mode: BatchPresetMode,
    input: &Path,
    output: &Path,
) -> JobStatus {
    let mut session = Session::with_config(settings.clone(), preset_mode.fallback_preset());
    let result = (|| {
        let report = *session.open(input)?;
        session.select_preset(preset_mode.preset_for_report(&report));
        session.render()?;
        session.export(output)
    })();
    match result {
        Ok(path) => JobStatus::Done(path),
        Err(e) => JobStatus::Failed(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::{ContentGuess, HumEstimate};
    use crate::audio::{AudioBuffer, ChannelLayout, SampleEncoding};
    use crate::io::write_wav;
    use crate::settings::ContentMode;
    use std::f32::consts::PI;

    const SR: u32 = 48_000;

    fn fixture(path: &Path) {
        let n = 24_000;
        let x: Vec<f32> = (0..n)
            .map(|i| 0.3 * (2.0 * PI * 220.0 * i as f32 / SR as f32).sin())
            .collect();
        let buf = AudioBuffer::new(SR, 24, SampleEncoding::Int, vec![x]);
        write_wav(path, &buf).expect("write fixture");
    }

    fn temp(tag: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("yesvox-queue-{tag}-{}.wav", std::process::id()));
        let _ = std::fs::remove_file(&p);
        p
    }

    fn temp_json(tag: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("yesvox-queue-{tag}-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&p);
        let _ = std::fs::remove_file(queue_backup_path(&p));
        p
    }

    fn report(mode: ContentMode, confidence: f32) -> AnalysisReport {
        AnalysisReport {
            channel_mode: ChannelLayout::Mono,
            integrated_lufs: -20.0,
            true_peak_dbtp: -3.0,
            noise_floor_db: -65.0,
            clip_sample_count: 0,
            clip_fraction: 0.0,
            click_rate_hz: 0.0,
            hum: HumEstimate {
                frequency: 0.0,
                harmonic_count: 0,
                strength_db: -120.0,
            },
            reverb_time_s: 0.0,
            content: ContentGuess { mode, confidence },
            residual_nonvocal: 0.0,
            resampled_for_batch: false,
        }
    }

    #[test]
    fn forced_batch_mode_uses_the_selected_preset_for_every_report() {
        let mode = BatchPresetMode::Force(Preset::NARRATION);

        assert_eq!(
            mode.preset_for_report(&report(ContentMode::Singing, 0.99)),
            Preset::NARRATION
        );
    }

    #[test]
    fn detected_batch_mode_uses_high_confidence_file_content() {
        let mode = BatchPresetMode::DetectPerFile {
            fallback: Preset::PODCAST,
        };

        assert_eq!(
            mode.preset_for_report(&report(ContentMode::Singing, 0.85)),
            Preset::LEAD_VOCAL
        );
        assert_eq!(
            mode.preset_for_report(&report(ContentMode::Voiceover, 0.85)),
            Preset::PODCAST
        );
    }

    #[test]
    fn detected_batch_mode_falls_back_when_confidence_is_low() {
        let mode = BatchPresetMode::DetectPerFile {
            fallback: Preset::NATURAL_VOCAL,
        };

        assert_eq!(
            mode.preset_for_report(&report(ContentMode::Voiceover, 0.69)),
            Preset::NATURAL_VOCAL
        );
    }

    #[test]
    fn renders_every_file_and_summarizes() {
        let in1 = temp("a-in");
        let in2 = temp("b-in");
        let out1 = temp("a-out");
        let out2 = temp("b-out");
        fixture(&in1);
        fixture(&in2);

        let mut q = RenderQueue::new(Settings::default(), Preset::PODCAST);
        q.enqueue(&in1, &out1);
        q.enqueue(&in2, &out2);
        let summary = q.run();

        assert_eq!(summary, BatchSummary::empty().with_counts(2, 0));
        assert!(q
            .items()
            .iter()
            .all(|i| matches!(i.status, JobStatus::Done(_))));
        assert!(out1.exists() && out2.exists());

        for p in [in1, in2, out1, out2] {
            let _ = std::fs::remove_file(p);
        }
    }

    #[test]
    fn a_failed_file_is_skipped_not_fatal() {
        let good_in = temp("good-in");
        let good_out = temp("good-out");
        fixture(&good_in);
        let missing = temp("missing-does-not-exist");
        let _ = std::fs::remove_file(&missing); // ensure it's absent

        let mut q = RenderQueue::new(Settings::default(), Preset::PODCAST);
        q.enqueue(&missing, temp("missing-out")); // will fail to open
        q.enqueue(&good_in, &good_out); // must still run
        let summary = q.run();

        assert_eq!(summary.done, 1);
        assert_eq!(summary.failed, 1);
        assert_eq!(summary.failures.len(), 1);
        assert_eq!(summary.failures[0].index, 0);
        assert_eq!(summary.failures[0].input, missing);
        assert!(matches!(q.items()[0].status, JobStatus::Failed(_)));
        assert!(matches!(q.items()[1].status, JobStatus::Done(_)));
        assert!(good_out.exists());

        for p in [good_in, good_out] {
            let _ = std::fs::remove_file(p);
        }
    }

    #[test]
    fn paused_queue_does_not_process_pending_items_until_resumed() {
        let input = temp("paused-in");
        let output = temp("paused-out");
        fixture(&input);

        let mut q = RenderQueue::new(Settings::default(), Preset::PODCAST);
        q.enqueue(&input, &output);
        q.pause();
        assert!(q.is_paused());

        let paused = q.run();
        assert_eq!(paused, BatchSummary::empty());
        assert!(matches!(q.items()[0].status, JobStatus::Pending));
        assert!(
            !output.exists(),
            "paused queue must not render pending files"
        );

        q.resume();
        assert!(!q.is_paused());
        let resumed = q.run();
        assert_eq!(resumed, BatchSummary::empty().with_counts(1, 0));
        assert!(matches!(q.items()[0].status, JobStatus::Done(_)));
        assert!(output.exists());

        for p in [input, output] {
            let _ = std::fs::remove_file(p);
        }
    }

    #[test]
    fn reorder_preserves_items_and_statuses() {
        let mut q = RenderQueue::new(Settings::default(), Preset::PODCAST);
        q.enqueue("a.wav", "a-out.wav");
        q.enqueue("b.wav", "b-out.wav");
        q.enqueue("c.wav", "c-out.wav");
        q.items[1].status = JobStatus::Failed("decode failed".to_owned());

        assert!(q.move_item(1, 0));
        assert_eq!(q.items()[0].input, PathBuf::from("b.wav"));
        assert!(matches!(q.items()[0].status, JobStatus::Failed(_)));
        assert_eq!(q.items()[1].input, PathBuf::from("a.wav"));
        assert_eq!(q.items()[2].input, PathBuf::from("c.wav"));

        assert!(q.move_item(2, 2));
        assert!(!q.move_item(3, 0));
        assert!(!q.move_item(0, 3));
    }

    #[test]
    fn queue_json_round_trips_items_statuses_and_recovers_paused() {
        let mut q = RenderQueue::new(Settings::default(), Preset::LEAD_VOCAL);
        q.enqueue("lead.wav", "lead-yesvox.wav");
        q.enqueue("bad.wav", "bad-yesvox.wav");
        q.items[0].status = JobStatus::Done(PathBuf::from("lead-yesvox.wav"));
        q.items[1].status = JobStatus::Failed("decode failed".to_owned());

        let recovered = RenderQueue::from_json(&q.to_json().unwrap()).unwrap();

        assert!(recovered.is_paused(), "crash recovery must land paused");
        assert_eq!(recovered.items().len(), 2);
        assert_eq!(recovered.items()[0].input, PathBuf::from("lead.wav"));
        assert_eq!(
            recovered.items()[0].status,
            JobStatus::Done(PathBuf::from("lead-yesvox.wav"))
        );
        assert!(matches!(recovered.items()[1].status, JobStatus::Failed(_)));
    }

    #[test]
    fn queue_json_round_trips_detected_batch_mode() {
        let mut q = RenderQueue::detect_per_file(Settings::default(), Preset::NARRATION);
        q.enqueue("first.wav", "first-yesvox.wav");

        let recovered = RenderQueue::from_json(&q.to_json().unwrap()).unwrap();

        assert_eq!(
            recovered.preset_mode(),
            BatchPresetMode::DetectPerFile {
                fallback: Preset::NARRATION
            }
        );
        assert!(recovered.is_paused(), "crash recovery must land paused");
        assert_eq!(recovered.items()[0].input, PathBuf::from("first.wav"));
    }

    #[test]
    fn legacy_queue_json_without_preset_mode_recovers_as_forced_preset() {
        let json = r#"{
          "schema_version": 1,
          "settings": { "schema_version": 1 },
          "preset": "Lead Vocal",
          "paused": false,
          "items": []
        }"#;

        let recovered = RenderQueue::from_json(json).unwrap();

        assert_eq!(
            recovered.preset_mode(),
            BatchPresetMode::Force(Preset::LEAD_VOCAL)
        );
        assert!(recovered.is_paused(), "crash recovery must land paused");
    }

    #[test]
    fn load_or_empty_recovers_from_backup_when_primary_is_corrupt() {
        let path = temp_json("recover");

        let mut q = RenderQueue::new(Settings::default(), Preset::PODCAST);
        q.enqueue("first.wav", "first-yesvox.wav");
        q.save(&path).unwrap();

        q.enqueue("second.wav", "second-yesvox.wav");
        q.save(&path).unwrap();
        std::fs::write(&path, "{ this is not json").unwrap();

        let recovered = RenderQueue::load_or_empty(&path).unwrap();

        assert!(recovered.is_paused(), "backup recovery must land paused");
        assert_eq!(recovered.items().len(), 1);
        assert_eq!(recovered.items()[0].input, PathBuf::from("first.wav"));

        let _ = std::fs::remove_file(queue_backup_path(&path));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn run_with_log_appends_jsonl_entries_and_returns_failure_details() {
        let good_in = temp("log-good-in");
        let good_out = temp("log-good-out");
        let missing = temp("log-missing");
        let missing_out = temp("log-missing-out");
        let log = temp_json("render-log");
        fixture(&good_in);
        let _ = std::fs::remove_file(&missing);

        let mut q = RenderQueue::new(Settings::default(), Preset::PODCAST);
        q.enqueue(&missing, &missing_out);
        q.enqueue(&good_in, &good_out);
        let summary = q.run_with_log(&log).unwrap();

        assert_eq!(summary.done, 1);
        assert_eq!(summary.failed, 1);
        assert_eq!(summary.failures.len(), 1);
        assert_eq!(summary.failures[0].index, 0);
        assert_eq!(summary.failures[0].input, missing);
        assert!(!summary.failures[0].error.is_empty());

        let lines: Vec<String> = std::fs::read_to_string(&log)
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect();
        assert_eq!(lines.len(), 2);
        let first: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
        let second: serde_json::Value = serde_json::from_str(&lines[1]).unwrap();
        assert_eq!(first["schema_version"], QUEUE_SCHEMA_VERSION);
        assert_eq!(first["queue_index"], 0);
        assert_eq!(first["status"]["status"], "failed");
        assert_eq!(second["queue_index"], 1);
        assert_eq!(second["status"]["status"], "done");

        for p in [good_in, good_out, missing_out, log] {
            let _ = std::fs::remove_file(p);
        }
    }

    #[test]
    fn retry_failed_marks_only_selected_failed_items_pending() {
        let first_in = temp("retry-first-in");
        let first_out = temp("retry-first-out");
        let retry_in = temp("retry-missing-in");
        let retry_out = temp("retry-missing-out");
        let unselected_in = temp("retry-unselected-in");
        let unselected_out = temp("retry-unselected-out");
        fixture(&first_in);
        let _ = std::fs::remove_file(&retry_in);
        let _ = std::fs::remove_file(&unselected_in);

        let mut q = RenderQueue::new(Settings::default(), Preset::PODCAST);
        q.enqueue(&first_in, &first_out);
        q.enqueue(&retry_in, &retry_out);
        q.enqueue(&unselected_in, &unselected_out);

        let first = q.run();
        assert_eq!(first.done, 1);
        assert_eq!(first.failed, 2);
        assert!(matches!(q.items()[0].status, JobStatus::Done(_)));
        assert!(matches!(q.items()[1].status, JobStatus::Failed(_)));
        assert!(matches!(q.items()[2].status, JobStatus::Failed(_)));

        fixture(&retry_in);
        let selected = q.retry_failed([1, 1, 0, 99]);
        assert_eq!(selected, 1, "only the selected failed item should retry");
        assert!(matches!(q.items()[0].status, JobStatus::Done(_)));
        assert!(matches!(q.items()[1].status, JobStatus::Pending));
        assert!(matches!(q.items()[2].status, JobStatus::Failed(_)));

        let retry = q.run();
        assert_eq!(retry.done, 1);
        assert_eq!(retry.failed, 0);
        assert!(matches!(q.items()[0].status, JobStatus::Done(_)));
        assert!(matches!(q.items()[1].status, JobStatus::Done(_)));
        assert!(matches!(q.items()[2].status, JobStatus::Failed(_)));
        assert!(first_out.exists());
        assert!(retry_out.exists());
        assert!(!unselected_out.exists());

        for p in [
            first_in,
            first_out,
            retry_in,
            retry_out,
            unselected_in,
            unselected_out,
        ] {
            let _ = std::fs::remove_file(p);
        }
    }

    #[test]
    fn run_with_progress_reports_file_boundaries() {
        let in1 = temp("progress-a-in");
        let in2 = temp("progress-b-in");
        let out1 = temp("progress-a-out");
        let out2 = temp("progress-b-out");
        fixture(&in1);
        fixture(&in2);

        let mut q = RenderQueue::new(Settings::default(), Preset::PODCAST);
        q.enqueue(&in1, &out1);
        q.enqueue(&in2, &out2);
        let mut events = Vec::new();
        let summary = q.run_with_progress(|progress| {
            events.push(progress);
            BatchControl::Continue
        });

        assert_eq!(summary, BatchSummary::empty().with_counts(2, 0));
        assert_eq!(
            events,
            vec![
                BatchProgress {
                    total: 2,
                    completed: 0,
                    current_index: Some(0),
                    done: 0,
                    failed: 0,
                },
                BatchProgress {
                    total: 2,
                    completed: 1,
                    current_index: None,
                    done: 1,
                    failed: 0,
                },
                BatchProgress {
                    total: 2,
                    completed: 1,
                    current_index: Some(1),
                    done: 1,
                    failed: 0,
                },
                BatchProgress {
                    total: 2,
                    completed: 2,
                    current_index: None,
                    done: 2,
                    failed: 0,
                },
            ]
        );

        for p in [in1, in2, out1, out2] {
            let _ = std::fs::remove_file(p);
        }
    }

    #[test]
    fn run_with_progress_can_cancel_before_next_file() {
        let first_in = temp("cancel-first-in");
        let second_in = temp("cancel-second-in");
        let third_in = temp("cancel-third-in");
        let first_out = temp("cancel-first-out");
        let second_out = temp("cancel-second-out");
        let third_out = temp("cancel-third-out");
        fixture(&first_in);
        fixture(&second_in);
        fixture(&third_in);

        let mut q = RenderQueue::new(Settings::default(), Preset::PODCAST);
        q.enqueue(&first_in, &first_out);
        q.enqueue(&second_in, &second_out);
        q.enqueue(&third_in, &third_out);
        let summary = q.run_with_progress(|progress| {
            if progress.current_index == Some(1) {
                BatchControl::Cancel
            } else {
                BatchControl::Continue
            }
        });

        assert_eq!(summary.done, 1);
        assert_eq!(summary.failed, 0);
        assert!(summary.canceled);
        assert!(matches!(q.items()[0].status, JobStatus::Done(_)));
        assert!(matches!(q.items()[1].status, JobStatus::Pending));
        assert!(matches!(q.items()[2].status, JobStatus::Pending));
        assert!(first_out.exists());
        assert!(!second_out.exists());
        assert!(!third_out.exists());

        for p in [
            first_in, second_in, third_in, first_out, second_out, third_out,
        ] {
            let _ = std::fs::remove_file(p);
        }
    }
}
