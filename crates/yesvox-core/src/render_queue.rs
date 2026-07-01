//! The non-destructive batch render queue (plan §5.6): apply one configuration
//! across many vocals, never overwriting, skipping (and recording) per-file
//! failures so one bad file can't sink the batch.
//!
//! Each item is opened in its own [`Session`] seeded from the queue's settings +
//! preset, rendered, and exported (the writer auto-suffixes on collision). A
//! failure is captured in the item's [`JobStatus`] and the run continues.

use std::path::{Path, PathBuf};

use crate::preset::Preset;
use crate::session::Session;
use crate::settings::Settings;
use serde::{Deserialize, Serialize};

const QUEUE_SCHEMA_VERSION: u32 = 1;

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
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct QueueItem {
    /// Source file.
    pub input: PathBuf,
    /// Requested output path (auto-suffixed on collision by the writer).
    pub output: PathBuf,
    /// Current status.
    pub status: JobStatus,
}

/// Tally of a batch run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BatchSummary {
    /// Files written.
    pub done: usize,
    /// Files that failed (skipped + logged).
    pub failed: usize,
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

/// A batch render queue applying one settings + preset across many files.
pub struct RenderQueue {
    settings: Settings,
    preset: Preset,
    paused: bool,
    items: Vec<QueueItem>,
}

impl RenderQueue {
    /// A queue that renders every file with `settings` + `preset` (force-preset
    /// batch mode).
    pub fn new(settings: Settings, preset: Preset) -> Self {
        RenderQueue {
            settings,
            preset,
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
        if self.paused {
            return BatchSummary { done: 0, failed: 0 };
        }

        let mut done = 0;
        let mut failed = 0;
        for item in &mut self.items {
            if !matches!(item.status, JobStatus::Pending) {
                continue; // already processed (resume-safe)
            }
            item.status = render_one(&self.settings, self.preset, &item.input, &item.output);
            match item.status {
                JobStatus::Done(_) => done += 1,
                JobStatus::Failed(_) => failed += 1,
                JobStatus::Pending => {}
            }
        }
        BatchSummary { done, failed }
    }
}

#[derive(Serialize, Deserialize)]
struct PersistentQueue {
    schema_version: u32,
    settings: Settings,
    preset: String,
    paused: bool,
    items: Vec<QueueItem>,
}

impl PersistentQueue {
    fn from_queue(queue: &RenderQueue) -> Self {
        PersistentQueue {
            schema_version: QUEUE_SCHEMA_VERSION,
            settings: queue.settings.clone(),
            preset: queue.preset.name.to_owned(),
            paused: queue.paused,
            items: queue.items.clone(),
        }
    }

    fn into_queue(saved: PersistentQueue) -> Result<RenderQueue, QueueStateError> {
        let preset = Preset::ALL
            .iter()
            .copied()
            .find(|p| p.name == saved.preset)
            .ok_or_else(|| QueueStateError::UnknownPreset(saved.preset.clone()))?;
        Ok(RenderQueue {
            settings: saved.settings,
            preset,
            // Recovery must not render until the app/user explicitly resumes.
            paused: true,
            items: saved.items,
        })
    }
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

/// Render one file with the given configuration (a free fn so `run` can borrow
/// `self.items` mutably without aliasing the config).
fn render_one(settings: &Settings, preset: Preset, input: &Path, output: &Path) -> JobStatus {
    let mut session = Session::with_config(settings.clone(), preset);
    match session.process_file(input, output) {
        Ok(path) => JobStatus::Done(path),
        Err(e) => JobStatus::Failed(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::{AudioBuffer, SampleEncoding};
    use crate::io::write_wav;
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

        assert_eq!(summary, BatchSummary { done: 2, failed: 0 });
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

        assert_eq!(summary, BatchSummary { done: 1, failed: 1 });
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
        assert_eq!(paused, BatchSummary { done: 0, failed: 0 });
        assert!(matches!(q.items()[0].status, JobStatus::Pending));
        assert!(
            !output.exists(),
            "paused queue must not render pending files"
        );

        q.resume();
        assert!(!q.is_paused());
        let resumed = q.run();
        assert_eq!(resumed, BatchSummary { done: 1, failed: 0 });
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
}
