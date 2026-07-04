//! cpal audio output for the playback trust loop (#23).
//!
//! The device-independent transport + source selection live in
//! `yesvox_core::playback`; this is only the glue that pumps the current source
//! buffer through a cpal output stream. All device fallibility is surfaced as a
//! `Result<_, String>` so the GUI can show a friendly error and disable
//! playback — never a panic.

use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::SampleFormat;
use yesvox_core::{AudioBuffer, Transport};

/// State shared between the UI thread and the audio callback.
struct Shared {
    transport: Transport,
    /// Source already adapted to the stream's channel count.
    source: AudioBuffer,
}

/// An open cpal output stream auditioning one source at a time. Dropping it
/// stops audio. Not `Send` (cpal streams aren't); it lives on the UI thread.
pub struct Player {
    _stream: cpal::Stream,
    shared: Arc<Mutex<Shared>>,
    channels: usize,
    sample_rate: u32,
}

impl Player {
    /// Open the default output device for `source`, starting **paused**. Every
    /// device failure becomes an `Err(String)`.
    pub fn new(source: &AudioBuffer) -> Result<Self, String> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or_else(|| "no output audio device".to_string())?;

        let sample_rate = source.sample_rate();
        // Prefer an f32 config that supports the source's sample rate, so no
        // resampling is needed and the callback stays a plain copy.
        let supported = device
            .supported_output_configs()
            .map_err(|e| format!("query output configs: {e}"))?;
        let chosen = supported
            .filter(|c| c.sample_format() == SampleFormat::F32)
            .find(|c| c.min_sample_rate().0 <= sample_rate && sample_rate <= c.max_sample_rate().0)
            .map(|c| c.with_sample_rate(cpal::SampleRate(sample_rate)))
            .ok_or_else(|| {
                format!("no f32 output config supports {sample_rate} Hz on this device")
            })?;

        let channels = chosen.channels() as usize;
        let config: cpal::StreamConfig = chosen.into();
        let adapted = adapt_channels(source, channels);
        let transport = Transport::new(adapted.frames());
        let shared = Arc::new(Mutex::new(Shared {
            transport,
            source: adapted,
        }));

        let cb_shared = Arc::clone(&shared);
        let stream = device
            .build_output_stream(
                &config,
                move |out: &mut [f32], _: &cpal::OutputCallbackInfo| match cb_shared.lock() {
                    Ok(mut s) => {
                        let Shared { transport, source } = &mut *s;
                        transport.fill(source, out);
                    }
                    Err(_) => out.iter_mut().for_each(|x| *x = 0.0),
                },
                |err| eprintln!("audio stream error: {err}"),
                None,
            )
            .map_err(|e| format!("open output stream: {e}"))?;
        stream.play().map_err(|e| format!("start stream: {e}"))?;

        Ok(Player {
            _stream: stream,
            shared,
            channels,
            sample_rate,
        })
    }

    fn with<R>(&self, f: impl FnOnce(&mut Shared) -> R) -> R {
        // A poisoned lock only happens if the audio callback panicked; recover
        // the guard so the UI stays responsive rather than propagating the panic.
        let mut guard = self.shared.lock().unwrap_or_else(|p| p.into_inner());
        f(&mut guard)
    }

    pub fn play(&self) {
        self.with(|s| s.transport.play());
    }

    pub fn toggle(&self) {
        self.with(|s| s.transport.toggle());
    }

    pub fn is_playing(&self) -> bool {
        self.with(|s| s.transport.is_playing())
    }

    pub fn position(&self) -> usize {
        self.with(|s| s.transport.position())
    }

    pub fn fraction(&self) -> f32 {
        self.with(|s| s.transport.fraction())
    }

    /// Seek to a normalized position in `[0, 1]`.
    pub fn seek_fraction(&self, fraction: f32) {
        self.with(|s| {
            let frame = (fraction.clamp(0.0, 1.0) * s.transport.len() as f32) as usize;
            s.transport.seek(frame);
        });
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Swap the audible source (A/B / reblend / difference) **without moving the
    /// playhead** — instant A/B at the shared playhead. The buffer is adapted to
    /// the stream's channel count.
    pub fn set_source(&self, source: &AudioBuffer) {
        let adapted = adapt_channels(source, self.channels);
        self.with(|s| s.source = adapted);
    }
}

/// Map `src`'s channels onto `target` output channels: mono is duplicated to
/// every channel, matching layouts pass through, and extra channels wrap. Keeps
/// [`Transport::fill`] a plain interleaving copy against a matching layout.
fn adapt_channels(src: &AudioBuffer, target: usize) -> AudioBuffer {
    let target = target.max(1);
    let sc = src.channel_count().max(1);
    let channels: Vec<Vec<f32>> = (0..target)
        .map(|ch| src.channel(ch % sc).to_vec())
        .collect();
    AudioBuffer::new(
        src.sample_rate(),
        src.bits_per_sample(),
        src.encoding(),
        channels,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use yesvox_core::SampleEncoding;

    fn buf(channels: Vec<Vec<f32>>) -> AudioBuffer {
        AudioBuffer::new(48_000, 24, SampleEncoding::Int, channels)
    }

    #[test]
    fn adapt_mono_to_stereo_duplicates() {
        let mono = buf(vec![vec![1.0, 2.0, 3.0]]);
        let stereo = adapt_channels(&mono, 2);
        assert_eq!(stereo.channel_count(), 2);
        assert_eq!(stereo.channel(0), stereo.channel(1));
        assert_eq!(stereo.channel(0), &[1.0, 2.0, 3.0]);
    }

    #[test]
    fn adapt_stereo_to_stereo_passes_through() {
        let stereo = buf(vec![vec![1.0, 2.0], vec![-1.0, -2.0]]);
        let out = adapt_channels(&stereo, 2);
        assert_eq!(out.channel(0), &[1.0, 2.0]);
        assert_eq!(out.channel(1), &[-1.0, -2.0]);
    }

    #[test]
    fn adapt_preserves_frame_count() {
        let mono = buf(vec![vec![0.0; 512]]);
        assert_eq!(adapt_channels(&mono, 4).frames(), 512);
        assert_eq!(adapt_channels(&mono, 1).frames(), 512);
    }
}
