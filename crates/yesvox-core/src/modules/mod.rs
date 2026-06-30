//! The deterministic-DSP repair rack (plan §3–§4).
//!
//! Every module implements [`crate::processor::VocalProcessor`] and follows one
//! rule that the per-module null gates pin down: **when bypassed, a module is
//! the sample-accurate identity** so the NULL guarantee survives the whole
//! chain. Modules are added one slice at a time and chained by the host.

pub mod declick;
pub mod declip;
pub mod deess;
pub mod dehum;
pub mod depop;
pub mod transient;

pub use declick::DeClick;
pub use declip::DeClip;
pub use deess::{DeEss, DeEssMode};
pub use dehum::DeHum;
pub use depop::DePop;
pub use transient::{Transient, TransientDetector, TransientKind};
