//! Internal audio representation (plan §1.2).
//!
//! The pipeline is f32 planar end-to-end and sample-rate preserving: one
//! `Vec<f32>` per channel, all the same length. Mono stays mono; stereo is
//! carried as dual-mono. The source bit depth and int/float encoding are kept
//! so export can honor the input format.

/// Channel topology of a take. v1 supports mono and stereo (dual-mono).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChannelLayout {
    /// One channel.
    Mono,
    /// Two channels, processed as independent mono lanes (plan §1.2).
    DualMono,
}

impl ChannelLayout {
    /// Number of channels in this layout.
    pub fn channels(self) -> usize {
        match self {
            ChannelLayout::Mono => 1,
            ChannelLayout::DualMono => 2,
        }
    }

    /// Map a raw channel count to a layout, or `None` if unsupported in v1.
    pub fn from_channels(n: u16) -> Option<Self> {
        match n {
            1 => Some(ChannelLayout::Mono),
            2 => Some(ChannelLayout::DualMono),
            _ => None,
        }
    }
}

/// How the source samples were stored, so export round-trips the input format.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SampleEncoding {
    /// Integer PCM at the buffer's bit depth (16/24/32).
    Int,
    /// IEEE 32-bit float.
    Float,
}

/// An f32-planar audio buffer plus the metadata needed to export losslessly.
#[derive(Clone, Debug, PartialEq)]
pub struct AudioBuffer {
    sample_rate: u32,
    bits_per_sample: u16,
    encoding: SampleEncoding,
    channels: Vec<Vec<f32>>,
}

impl AudioBuffer {
    /// Build a buffer from planar channels. All channels must be the same length
    /// and there must be at least one.
    pub fn new(
        sample_rate: u32,
        bits_per_sample: u16,
        encoding: SampleEncoding,
        channels: Vec<Vec<f32>>,
    ) -> Self {
        debug_assert!(
            !channels.is_empty(),
            "audio buffer needs at least one channel"
        );
        debug_assert!(
            channels.iter().all(|c| c.len() == channels[0].len()),
            "all channels must be the same length"
        );
        AudioBuffer {
            sample_rate,
            bits_per_sample,
            encoding,
            channels,
        }
    }

    /// Sample rate in Hz.
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Source bit depth, preserved for export.
    pub fn bits_per_sample(&self) -> u16 {
        self.bits_per_sample
    }

    /// Source int/float encoding, preserved for export.
    pub fn encoding(&self) -> SampleEncoding {
        self.encoding
    }

    /// Number of channels.
    pub fn channel_count(&self) -> usize {
        self.channels.len()
    }

    /// The channel topology.
    pub fn layout(&self) -> ChannelLayout {
        // channel_count is 1 or 2 by construction (io rejects other counts).
        ChannelLayout::from_channels(self.channels.len() as u16).unwrap_or(ChannelLayout::Mono)
    }

    /// Frames (samples per channel).
    pub fn frames(&self) -> usize {
        self.channels.first().map_or(0, Vec::len)
    }

    /// Read one channel's samples.
    pub fn channel(&self, index: usize) -> &[f32] {
        &self.channels[index]
    }

    /// Mutably borrow one channel's samples.
    pub fn channel_mut(&mut self, index: usize) -> &mut [f32] {
        &mut self.channels[index]
    }

    /// All channels as planar slices.
    pub fn channels(&self) -> &[Vec<f32>] {
        &self.channels
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_round_trips_supported_channel_counts() {
        assert_eq!(ChannelLayout::from_channels(1), Some(ChannelLayout::Mono));
        assert_eq!(
            ChannelLayout::from_channels(2),
            Some(ChannelLayout::DualMono)
        );
        assert_eq!(ChannelLayout::from_channels(3), None);
        assert_eq!(ChannelLayout::Mono.channels(), 1);
        assert_eq!(ChannelLayout::DualMono.channels(), 2);
    }

    #[test]
    fn buffer_reports_frames_and_layout() {
        let buf = AudioBuffer::new(
            48_000,
            16,
            SampleEncoding::Int,
            vec![vec![0.0; 8], vec![0.0; 8]],
        );
        assert_eq!(buf.frames(), 8);
        assert_eq!(buf.channel_count(), 2);
        assert_eq!(buf.layout(), ChannelLayout::DualMono);
        assert_eq!(buf.sample_rate(), 48_000);
    }
}
