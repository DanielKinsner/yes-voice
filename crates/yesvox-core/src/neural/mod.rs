//! Batch-engine (MossFormer2_SE_48K) integration — the high-quality offline
//! denoiser (plan §2.2, slice #10).
//!
//! The #9 spike proved the net exports to ONNX faithfully and is a clean
//! **feature → mask** graph: the STFT, mel filter-bank, Δ/ΔΔ, mask application,
//! and iSTFT all live in host code, so they are reimplemented here in Rust
//! around the (later) `ort` inference call. Per the integration decision, this
//! host-side DSP is **always-on pure Rust** and golden-tested in the normal
//! gate; only the ONNX Runtime inference itself sits behind the off-by-default
//! `neural` cargo feature, so the default build stays pure-Rust and green.
//!
//! Status: STFT/iSTFT back-end + Kaldi log-mel fbank front-end landed (both
//! torch-golden). The `ort` inference + 48 kHz resample island follow as the
//! remaining #10 increments.

pub mod fbank;
pub mod stft;

pub use fbank::MossformerFbank;
pub use stft::MossformerStft;
