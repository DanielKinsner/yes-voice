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
pub mod dsp;
pub mod io;
pub mod loudness;
pub mod processor;
pub mod settings;

pub use analysis::{
    analyze, analyze_file, AnalysisError, AnalysisReport, ContentGuess, HumEstimate,
};
pub use audio::{AudioBuffer, ChannelLayout, SampleEncoding};
pub use io::{
    export, read_audio, read_flac, read_wav, write_flac_with, write_wav, write_wav_with, IoError,
};
pub use processor::{Passthrough, VocalProcessor};
pub use settings::{
    ContentMode, ExportOptions, ExportSettings, Settings, SettingsError, SingingSettings,
    SCHEMA_VERSION,
};
