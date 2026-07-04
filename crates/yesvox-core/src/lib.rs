//! `yesvox-core` — the host-agnostic, RT-safe DSP/ML core for YesVox.
//!
//! v1 ships only the offline render path: standalone YesVox feeds whole files
//! through one [`VocalProcessor`] (plan §1.1). This crate provides the
//! processing boundary, the f32-planar audio contract, WAV/FLAC decode/encode
//! with atomic never-overwrite export and TPDF dither, and the versioned
//! settings document — the open → process → export skeleton that the four
//! guarantee tests pin down before any DSP exists.
//!
//! See `docs/plans/2026-06-29-feat-yesvox-v1-restoration-workstation-plan.md`.

pub mod analysis;
pub mod audio;
pub mod build;
pub mod card;
pub mod chain;
pub mod denoise;
pub mod dsp;
#[cfg(test)]
pub(crate) mod golden;
pub mod io;
pub mod loudness;
pub mod modules;
pub mod monitor;
pub mod neural;
pub mod playback;
pub mod preset;
pub mod processor;
pub mod quality;
pub mod render_queue;
pub mod session;
pub mod settings;
pub mod singing;
pub mod worker;

pub use analysis::{
    analyze, analyze_file, AnalysisError, AnalysisReport, ContentGuess, HumEstimate,
};
pub use audio::{
    AudioBuffer, ChannelLayout, ContainerMetadata, FlacMetadataBlock, SampleEncoding,
    WavMetadataChunk,
};
pub use build::BuiltChain;
pub use card::CardId;
pub use chain::{Chain, Stage};
pub use denoise::{
    build_denoiser, BuiltEngine, ChunkPolicy, Denoise, Denoiser, RamPreflight, SpectralDenoiser,
    WorkerPool,
};
pub use io::{
    export, read_audio, read_flac, read_wav, write_flac_with, write_wav, write_wav_with, IoError,
};
pub use modules::{
    BleedVerdict, BreathControl, DeBleed, DeClick, DeClip, DeEss, DeEssMode, DeHum, DePop,
    DeReverb, FinishStage, LoudnessTarget,
};
pub use playback::{PlaybackSource, Transport};
pub use preset::Preset;
pub use processor::{Passthrough, VocalProcessor};
pub use quality::{
    linear_residual, resolve, wet_dry_blend, DiffMonitor, Engine, EngineBinding, QualityMode,
    Resolved,
};
pub use render_queue::{
    BatchControl, BatchFailure, BatchPresetMode, BatchProgress, BatchSummary, JobStatus, QueueItem,
    QueueStateError, RenderQueue,
};
pub use session::{
    IsolationVerdict, OutputFormat, PreparedExport, Session, SessionError, VocalIsolationCheck,
};
pub use settings::{
    ContentMode, ExportOptions, ExportSettings, Settings, SettingsError, SingingSettings,
    DEFAULT_FLAC_COMPRESSION_LEVEL, SCHEMA_VERSION,
};
