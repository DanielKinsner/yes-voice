//! `yesvox-core` — the host-agnostic, RT-safe DSP/ML core for YesVox.
//!
//! v1 ships only the offline render path: standalone YesVox feeds whole files
//! through one [`VocalProcessor`] (plan §1.1). This crate currently provides the
//! processing boundary, the f32-planar audio contract, and WAV decode/encode —
//! the open → process → export skeleton that the four guarantee tests pin down
//! before any DSP exists.
//!
//! See `docs/plans/2026-06-29-feat-yesvox-v1-restoration-workstation-plan.md`.

pub mod audio;
pub mod io;
pub mod processor;

pub use audio::{AudioBuffer, ChannelLayout, SampleEncoding};
pub use io::{read_wav, write_wav, IoError};
pub use processor::{Passthrough, VocalProcessor};
