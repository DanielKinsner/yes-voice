//! File decode/encode for the core (plan §1.2 / §1.3).
//!
//! v1 handles WAV (integer PCM 16/24/32 and 32-bit float) via `hound`, decoding
//! to f32 planar and re-encoding in the source format. Export is atomic: write a
//! temp file, then rename it into place. FLAC support follows in a later slice.

use crate::audio::{AudioBuffer, ChannelLayout, SampleEncoding};
use std::path::{Path, PathBuf};

/// Errors from decoding or encoding audio files.
#[derive(Debug)]
pub enum IoError {
    /// Underlying WAV codec error.
    Wav(hound::Error),
    /// Filesystem error (open, rename, ...).
    Io(std::io::Error),
    /// Channel count not supported in v1 (only mono/stereo).
    UnsupportedChannels(u16),
}

impl std::fmt::Display for IoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IoError::Wav(e) => write!(f, "wav error: {e}"),
            IoError::Io(e) => write!(f, "io error: {e}"),
            IoError::UnsupportedChannels(n) => {
                write!(f, "unsupported channel count: {n} (v1 is mono or stereo)")
            }
        }
    }
}

impl std::error::Error for IoError {}

impl From<hound::Error> for IoError {
    fn from(e: hound::Error) -> Self {
        IoError::Wav(e)
    }
}

impl From<std::io::Error> for IoError {
    fn from(e: std::io::Error) -> Self {
        IoError::Io(e)
    }
}

/// Decode a WAV file into an f32-planar [`AudioBuffer`], preserving sample rate,
/// bit depth, and int/float encoding.
pub fn read_wav(path: &Path) -> Result<AudioBuffer, IoError> {
    let mut reader = hound::WavReader::open(path)?;
    let spec = reader.spec();
    let layout = ChannelLayout::from_channels(spec.channels)
        .ok_or(IoError::UnsupportedChannels(spec.channels))?;
    let nch = layout.channels();
    let bits = spec.bits_per_sample;

    let (encoding, interleaved): (SampleEncoding, Vec<f32>) = match spec.sample_format {
        hound::SampleFormat::Float => {
            let samples = reader.samples::<f32>().collect::<Result<Vec<f32>, _>>()?;
            (SampleEncoding::Float, samples)
        }
        hound::SampleFormat::Int => {
            let scale = int_scale(bits);
            let samples = reader
                .samples::<i32>()
                .map(|s| s.map(|v| v as f32 / scale))
                .collect::<Result<Vec<f32>, _>>()?;
            (SampleEncoding::Int, samples)
        }
    };

    let frames = interleaved.len() / nch;
    let mut channels: Vec<Vec<f32>> = (0..nch).map(|_| Vec::with_capacity(frames)).collect();
    for (i, sample) in interleaved.into_iter().enumerate() {
        channels[i % nch].push(sample);
    }

    Ok(AudioBuffer::new(spec.sample_rate, bits, encoding, channels))
}

/// Encode an [`AudioBuffer`] to a WAV file atomically (temp file + rename).
pub fn write_wav(path: &Path, buffer: &AudioBuffer) -> Result<(), IoError> {
    let spec = hound::WavSpec {
        channels: buffer.channel_count() as u16,
        sample_rate: buffer.sample_rate(),
        bits_per_sample: buffer.bits_per_sample(),
        sample_format: match buffer.encoding() {
            SampleEncoding::Float => hound::SampleFormat::Float,
            SampleEncoding::Int => hound::SampleFormat::Int,
        },
    };

    let tmp = temp_path(path);
    {
        let mut writer = hound::WavWriter::create(&tmp, spec)?;
        let nch = buffer.channel_count();
        let frames = buffer.frames();
        match buffer.encoding() {
            SampleEncoding::Float => {
                for frame in 0..frames {
                    for ch in 0..nch {
                        writer.write_sample(buffer.channel(ch)[frame])?;
                    }
                }
            }
            SampleEncoding::Int => {
                let scale = int_scale(buffer.bits_per_sample());
                let max = scale as i64 - 1;
                let min = -(scale as i64);
                for frame in 0..frames {
                    for ch in 0..nch {
                        let raw = (buffer.channel(ch)[frame] * scale).round() as i64;
                        writer.write_sample(raw.clamp(min, max) as i32)?;
                    }
                }
            }
        }
        writer.finalize()?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// `2^(bits-1)`, the full-scale magnitude for integer PCM at `bits`.
fn int_scale(bits: u16) -> f32 {
    (1u64 << (bits - 1)) as f32
}

/// Sibling temp path for the atomic-write dance.
fn temp_path(path: &Path) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(".yesvox-tmp");
    PathBuf::from(s)
}
