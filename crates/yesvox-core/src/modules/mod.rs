//! The deterministic-DSP repair rack (plan §3–§4).
//!
//! Every module implements [`crate::processor::VocalProcessor`] and follows one
//! rule that the per-module null gates pin down: **when bypassed, a module is
//! the sample-accurate identity** so the NULL guarantee survives the whole
//! chain. Modules are added one slice at a time and chained by the host.

pub mod breath;
pub mod debleed;
pub mod declick;
pub mod declip;
pub mod deess;
pub mod dehum;
pub mod depop;
pub mod dereverb;
pub mod finish;
pub mod transient;

pub use breath::BreathControl;
pub use debleed::{BleedVerdict, DeBleed};
pub use declick::DeClick;
pub use declip::DeClip;
pub use deess::{DeEss, DeEssMode};
pub use dehum::DeHum;
pub use depop::DePop;
pub use dereverb::DeReverb;
pub use finish::{FinishStage, LoudnessTarget};
pub use transient::{Transient, TransientDetector, TransientKind};
