//! File decode/encode for the core (plan §1.2 / §1.3).
//!
//! v1 handles WAV (integer PCM 16/24/32 and 32-bit float) via `hound` and FLAC
//! (`claxon` decode, `flacenc` encode), decoding to f32 planar and re-encoding
//! in the source format. Export is atomic — write a temp file, then rename it
//! into place — and **never overwrites**: a name collision auto-suffixes
//! (`name_1.wav`, `name_2.wav`, …), so an input is never clobbered (plan §1.3).
//!
//! ≤24-bit integer export gets **TPDF dither** before quantization (plan §1.2),
//! generated from a fixed-seed PRNG so exports stay byte-reproducible for the
//! golden discipline.

use crate::audio::{
    AudioBuffer, ChannelLayout, ContainerMetadata, FlacMetadataBlock, SampleEncoding,
    WavMetadataChunk,
};
use crate::settings::ExportOptions;
use std::io::Read;
use std::path::{Path, PathBuf};

/// Errors from decoding or encoding audio files.
#[derive(Debug)]
pub enum IoError {
    /// Underlying WAV codec error.
    Wav(hound::Error),
    /// Underlying FLAC decode error.
    FlacDecode(String),
    /// Underlying FLAC encode error.
    FlacEncode(String),
    /// Filesystem error (open, rename, ...).
    Io(std::io::Error),
    /// Channel count not supported in v1 (only mono/stereo).
    UnsupportedChannels(u16),
    /// File extension the core does not handle.
    UnsupportedFormat(String),
}

impl std::fmt::Display for IoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IoError::Wav(e) => write!(f, "wav error: {e}"),
            IoError::FlacDecode(e) => write!(f, "flac decode error: {e}"),
            IoError::FlacEncode(e) => write!(f, "flac encode error: {e}"),
            IoError::Io(e) => write!(f, "io error: {e}"),
            IoError::UnsupportedChannels(n) => {
                write!(f, "unsupported channel count: {n} (v1 is mono or stereo)")
            }
            IoError::UnsupportedFormat(ext) => {
                write!(
                    f,
                    "unsupported file format: .{ext} (v1 reads/writes WAV and FLAC)"
                )
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

/// Decode any supported audio file (WAV or FLAC) into an f32-planar buffer,
/// dispatching on the file extension.
pub fn read_audio(path: &Path) -> Result<AudioBuffer, IoError> {
    match extension(path).as_deref() {
        Some("wav") | Some("wave") => read_wav(path),
        Some("flac") => read_flac(path),
        other => Err(IoError::UnsupportedFormat(other.unwrap_or("").to_string())),
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

    let metadata = read_wav_metadata(path)?;

    Ok(deinterleave(
        spec.sample_rate,
        bits,
        encoding,
        nch,
        interleaved,
        metadata,
    ))
}

/// Decode a FLAC file into an f32-planar [`AudioBuffer`] (integer PCM only;
/// FLAC has no float encoding).
pub fn read_flac(path: &Path) -> Result<AudioBuffer, IoError> {
    let mut reader =
        claxon::FlacReader::open(path).map_err(|e| IoError::FlacDecode(e.to_string()))?;
    let info = reader.streaminfo();
    let nch = ChannelLayout::from_channels(info.channels as u16)
        .ok_or(IoError::UnsupportedChannels(info.channels as u16))?
        .channels();
    let bits = info.bits_per_sample as u16;
    let scale = int_scale(bits);

    let mut interleaved = Vec::new();
    for sample in reader.samples() {
        let v = sample.map_err(|e| IoError::FlacDecode(e.to_string()))?;
        interleaved.push(v as f32 / scale);
    }

    // A fixed-block-size encoder zero-pads the final block; a spec-compliant
    // reader truncates to STREAMINFO's total inter-channel sample count so the
    // SAME-LENGTH guarantee survives a WAV→FLAC→WAV round-trip.
    if let Some(total) = info.samples {
        interleaved.truncate(total as usize * nch);
    }

    let metadata = read_flac_metadata(path)?;

    Ok(deinterleave(
        info.sample_rate,
        bits,
        SampleEncoding::Int,
        nch,
        interleaved,
        metadata,
    ))
}

/// Reshape an interleaved sample stream into planar channels.
fn deinterleave(
    sample_rate: u32,
    bits: u16,
    encoding: SampleEncoding,
    nch: usize,
    interleaved: Vec<f32>,
    metadata: ContainerMetadata,
) -> AudioBuffer {
    let frames = interleaved.len() / nch;
    let mut channels: Vec<Vec<f32>> = (0..nch).map(|_| Vec::with_capacity(frames)).collect();
    for (i, sample) in interleaved.into_iter().enumerate() {
        channels[i % nch].push(sample);
    }
    let mut buffer = AudioBuffer::with_metadata(sample_rate, bits, encoding, channels, metadata);
    // Decode-time numeric hygiene (#28): a float WAV can carry NaN/±Inf or
    // wildly out-of-range samples that would poison RMS/loudness accumulators
    // and panic the analyzer's sorts. Flush them here so nothing downstream sees
    // a hostile sample; the count rides along in the analysis report.
    buffer.sanitize();
    buffer
}

/// Encode an [`AudioBuffer`] to a WAV file atomically (temp file + rename),
/// **bit-exact** at the source format — no dither, no collision handling. Used
/// for fixtures and as the low-level writer; user export goes through
/// [`export`].
pub fn write_wav(path: &Path, buffer: &AudioBuffer) -> Result<(), IoError> {
    write_wav_with(
        path,
        buffer,
        ExportOptions {
            bit_depth: buffer.bits_per_sample(),
            encoding: buffer.encoding(),
            dither: false,
            flac_compression_level: crate::settings::DEFAULT_FLAC_COMPRESSION_LEVEL,
        },
    )
}

/// Encode to WAV atomically with explicit [`ExportOptions`] (target bit depth,
/// encoding, and dither).
pub fn write_wav_with(
    path: &Path,
    buffer: &AudioBuffer,
    opts: ExportOptions,
) -> Result<(), IoError> {
    let spec = hound::WavSpec {
        channels: buffer.channel_count() as u16,
        sample_rate: buffer.sample_rate(),
        bits_per_sample: opts.bit_depth,
        sample_format: match opts.encoding {
            SampleEncoding::Float => hound::SampleFormat::Float,
            SampleEncoding::Int => hound::SampleFormat::Int,
        },
    };

    let tmp = sibling_temp(path);
    {
        let mut writer = hound::WavWriter::create(&tmp, spec)?;
        let nch = buffer.channel_count();
        let frames = buffer.frames();
        match opts.encoding {
            SampleEncoding::Float => {
                for frame in 0..frames {
                    for ch in 0..nch {
                        writer.write_sample(buffer.channel(ch)[frame])?;
                    }
                }
            }
            SampleEncoding::Int => {
                let mut q = Quantizer::new(opts.bit_depth, opts.dither);
                for frame in 0..frames {
                    for ch in 0..nch {
                        writer.write_sample(q.quantize(buffer.channel(ch)[frame]))?;
                    }
                }
            }
        }
        writer.finalize()?;
    }
    preserve_wav_metadata(&tmp, buffer.metadata())?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Encode an [`AudioBuffer`] to FLAC atomically (integer PCM). Float buffers are
/// quantized to `opts.bit_depth` (FLAC stores integer samples only).
pub fn write_flac_with(
    path: &Path,
    buffer: &AudioBuffer,
    opts: ExportOptions,
) -> Result<(), IoError> {
    use flacenc::component::BitRepr;
    use flacenc::error::Verify;

    let nch = buffer.channel_count();
    let frames = buffer.frames();
    let bits = opts.bit_depth.min(24); // FLAC stores ≤24-bit in our config.

    let mut q = Quantizer::new(bits, opts.dither);
    let mut interleaved: Vec<i32> = Vec::with_capacity(frames * nch);
    for frame in 0..frames {
        for ch in 0..nch {
            interleaved.push(q.quantize(buffer.channel(ch)[frame]));
        }
    }

    let config = flac_encoder_config(opts.flac_compression_level)
        .into_verified()
        .map_err(|e| IoError::FlacEncode(format!("{e:?}")))?;
    let source = flacenc::source::MemSource::from_samples(
        &interleaved,
        nch,
        bits as usize,
        buffer.sample_rate() as usize,
    );
    let mut stream = flacenc::encode_with_fixed_block_size(&config, source, config.block_size)
        .map_err(|e| IoError::FlacEncode(format!("{e:?}")))?;
    for block in buffer.metadata().flac_blocks() {
        let data =
            flacenc::component::MetadataBlockData::new_unknown(block.block_type(), block.data())
                .map_err(|e| IoError::FlacEncode(format!("{e:?}")))?;
        stream.add_metadata_block(data);
    }

    let mut sink = flacenc::bitsink::ByteSink::new();
    stream
        .write(&mut sink)
        .map_err(|e| IoError::FlacEncode(format!("{e:?}")))?;

    let tmp = sibling_temp(path);
    std::fs::write(&tmp, sink.as_slice())?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

fn flac_encoder_config(level: u8) -> flacenc::config::Encoder {
    let level = level.min(8);
    let mut config = flacenc::config::Encoder::default();
    config.block_size = match level {
        0..=2 => 1024,
        3..=5 => 4096,
        _ => 8192,
    };
    config.subframe_coding.use_lpc = level >= 3;
    config.subframe_coding.qlpc.lpc_order = match level {
        0..=2 => 1,
        3..=5 => 10,
        _ => 12,
    };
    config
}

/// Export a processed buffer to `desired`, **never overwriting** an existing
/// file: on collision the name is auto-suffixed (`name_1.ext`, `name_2.ext`, …).
/// Dispatches on the destination extension (WAV or FLAC). Returns the path
/// actually written.
pub fn export(
    desired: &Path,
    buffer: &AudioBuffer,
    opts: ExportOptions,
) -> Result<PathBuf, IoError> {
    let target = unique_path(desired);
    match extension(&target).as_deref() {
        Some("wav") | Some("wave") => write_wav_with(&target, buffer, opts)?,
        Some("flac") => write_flac_with(&target, buffer, opts)?,
        other => return Err(IoError::UnsupportedFormat(other.unwrap_or("").to_string())),
    }
    Ok(target)
}

/// Quantizer from f32 to integer PCM, optionally with fixed-seed TPDF dither.
struct Quantizer {
    scale: f32,
    max: i64,
    min: i64,
    dither: bool,
    rng: XorShift64,
}

impl Quantizer {
    fn new(bits: u16, dither: bool) -> Self {
        let scale = int_scale(bits);
        Quantizer {
            scale,
            max: scale as i64 - 1,
            min: -(scale as i64),
            dither,
            // Fixed seed → reproducible dither → byte-stable goldens.
            rng: XorShift64::new(0x5642_4F58_5645_5331),
        }
    }

    fn quantize(&mut self, value: f32) -> i32 {
        let mut scaled = value * self.scale;
        if self.dither {
            // TPDF dither: triangular in (-1, +1) LSB, decorrelating the
            // quantization error (plan §1.2).
            scaled += self.rng.next_f32() - self.rng.next_f32();
        }
        (scaled.round() as i64).clamp(self.min, self.max) as i32
    }
}

/// Small deterministic PRNG for reproducible dither.
struct XorShift64 {
    state: u64,
}

impl XorShift64 {
    fn new(seed: u64) -> Self {
        XorShift64 {
            state: seed | 1, // never zero
        }
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        x
    }

    /// Uniform in [0, 1).
    fn next_f32(&mut self) -> f32 {
        // Top 24 bits → [0, 1) at f32 precision.
        (self.next_u64() >> 40) as f32 / (1u32 << 24) as f32
    }
}

/// `2^(bits-1)`, the full-scale magnitude for integer PCM at `bits`.
fn int_scale(bits: u16) -> f32 {
    (1u64 << (bits - 1)) as f32
}

/// Lowercased file extension, if any.
fn extension(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase())
}

fn read_wav_metadata(path: &Path) -> Result<ContainerMetadata, IoError> {
    let bytes = std::fs::read(path)?;
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Ok(ContainerMetadata::empty());
    }

    let mut chunks = Vec::new();
    let mut pos = 12usize;
    while pos + 8 <= bytes.len() {
        // The `pos + 8 <= len` guard makes these eight indices in-bounds, so read
        // the id + length bytes directly rather than via a fallible try_into (#29).
        let id: [u8; 4] = [bytes[pos], bytes[pos + 1], bytes[pos + 2], bytes[pos + 3]];
        let len = u32::from_le_bytes([
            bytes[pos + 4],
            bytes[pos + 5],
            bytes[pos + 6],
            bytes[pos + 7],
        ]) as usize;
        let data_start = pos + 8;
        let data_end = data_start.saturating_add(len);
        if data_end > bytes.len() {
            break;
        }
        if &id != b"fmt " && &id != b"data" {
            chunks.push(WavMetadataChunk::new(
                id,
                bytes[data_start..data_end].to_vec(),
            ));
        }
        pos = data_end + (len & 1);
    }
    Ok(ContainerMetadata::with_wav_chunks(chunks))
}

fn preserve_wav_metadata(path: &Path, metadata: &ContainerMetadata) -> Result<(), IoError> {
    if metadata.wav_chunks().is_empty() {
        return Ok(());
    }
    let bytes = std::fs::read(path)?;
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Ok(());
    }

    let mut out = Vec::with_capacity(bytes.len() + serialized_wav_metadata_len(metadata));
    out.extend_from_slice(&bytes);
    for chunk in metadata.wav_chunks() {
        out.extend_from_slice(&chunk.id());
        out.extend_from_slice(&(chunk.data().len() as u32).to_le_bytes());
        out.extend_from_slice(chunk.data());
        if chunk.data().len() % 2 == 1 {
            out.push(0);
        }
    }

    let riff_size = (out.len() - 8) as u32;
    out[4..8].copy_from_slice(&riff_size.to_le_bytes());
    std::fs::write(path, out)?;
    Ok(())
}

fn serialized_wav_metadata_len(metadata: &ContainerMetadata) -> usize {
    metadata
        .wav_chunks()
        .iter()
        .map(|chunk| 8 + chunk.data().len() + (chunk.data().len() & 1))
        .sum()
}

fn read_flac_metadata(path: &Path) -> Result<ContainerMetadata, IoError> {
    let mut file = std::fs::File::open(path)?;
    let mut magic = [0u8; 4];
    file.read_exact(&mut magic)?;
    if &magic != b"fLaC" {
        return Ok(ContainerMetadata::empty());
    }

    let mut blocks = Vec::new();
    loop {
        let mut header = [0u8; 4];
        file.read_exact(&mut header)?;
        let is_last = (header[0] & 0x80) != 0;
        let block_type = header[0] & 0x7f;
        let len = ((header[1] as usize) << 16) | ((header[2] as usize) << 8) | header[3] as usize;
        let mut data = vec![0u8; len];
        file.read_exact(&mut data)?;
        if block_type == 2 || block_type == 4 {
            blocks.push(FlacMetadataBlock::new(block_type, data));
        }
        if is_last {
            break;
        }
    }
    Ok(ContainerMetadata::with_flac_blocks(blocks))
}

/// Sibling temp path for the atomic-write dance.
fn sibling_temp(path: &Path) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(".yesvox-tmp");
    PathBuf::from(s)
}

/// First non-colliding variant of `desired`: `desired` itself if free, else
/// `stem_1.ext`, `stem_2.ext`, … This is the never-overwrite guarantee.
fn unique_path(desired: &Path) -> PathBuf {
    if !desired.exists() {
        return desired.to_path_buf();
    }
    let parent = desired.parent();
    let stem = desired
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("output");
    let ext = desired.extension().and_then(|e| e.to_str());
    for n in 1u32.. {
        let name = match ext {
            Some(ext) => format!("{stem}_{n}.{ext}"),
            None => format!("{stem}_{n}"),
        };
        let candidate = match parent {
            Some(p) => p.join(name),
            None => PathBuf::from(name),
        };
        if !candidate.exists() {
            return candidate;
        }
    }
    unreachable!("u32 range exhausted resolving a unique export path")
}
