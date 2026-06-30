//! The deterministic-DSP repair rack (plan §3–§4).
//!
//! Every module implements [`crate::processor::VocalProcessor`] and follows one
//! rule that the per-module null gates pin down: **when bypassed, a module is
//! the sample-accurate identity** so the NULL guarantee survives the whole
//! chain. Modules are added one slice at a time and chained by the host.

pub mod dehum;

pub use dehum::DeHum;
