//! The core processing boundary (plan §1.1).
//!
//! Standalone YesVox feeds whole files through one [`VocalProcessor`] offline.
//! v1's only processor is [`Passthrough`], the all-bypass identity that proves
//! the open → process → export path and anchors the NULL guarantee before any
//! DSP exists. As real modules land they implement the same trait so the host
//! can chain, bypass, and null-test them uniformly.

use crate::audio::{AudioBuffer, ChannelLayout};

/// The single host-agnostic processing contract. Heavy work happens in
/// [`process`](VocalProcessor::process); [`prepare`](VocalProcessor::prepare) is
/// the only place allocation is allowed.
pub trait VocalProcessor {
    /// Prepare for a stream at `sample_rate`, with blocks up to `max_block`
    /// frames and the given channel `layout`.
    fn prepare(&mut self, sample_rate: u32, max_block: usize, layout: ChannelLayout);

    /// Process a buffer in place.
    fn process(&mut self, buffer: &mut AudioBuffer);

    /// Flush all internal state (called per file).
    fn reset(&mut self);

    /// Added latency in samples at the input sample rate.
    fn latency_samples(&self) -> u32 {
        0
    }
}

/// A no-op processor: the sample-accurate identity the NULL gate asserts.
#[derive(Debug, Default)]
pub struct Passthrough;

impl VocalProcessor for Passthrough {
    fn prepare(&mut self, _sample_rate: u32, _max_block: usize, _layout: ChannelLayout) {}

    fn process(&mut self, _buffer: &mut AudioBuffer) {}

    fn reset(&mut self) {}
}
