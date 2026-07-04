//! Device-independent playback transport (#23).
//!
//! The audible A/B + difference-monitor trust loop (product.md §6, success
//! criteria §9.1–9.3) needs real playback. The **device** side (a `cpal` output
//! stream) lives in the GUI; everything auditable in isolation — the transport
//! position state machine and which buffer each A/B source maps to — lives here
//! so it is tested without an audio device, keeping CI headless-green.
//!
//! All A/B/C sources (Original, Processed, CompareSnapshot, Reblend, Difference) are the *same
//! length* (same take — the SAME-LENGTH guarantee), so a single frame position
//! is shared across them: switching Original↔Processed continues from the exact
//! same playhead, no restart.

use crate::audio::AudioBuffer;

/// Which buffer the transport auditions. Maps to the existing monitor buffers on
/// [`crate::session::Session`]; see `Session::playback_buffer`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlaybackSource {
    /// The dry input (loudness-matched when Volume Match is on).
    Original,
    /// The processed output.
    Processed,
    /// A user-pinned processed output snapshot for Advanced A/B/C compare.
    CompareSnapshot,
    /// A reblend of processed + removed signal.
    Reblend,
    /// The difference monitor — what was removed.
    Difference,
}

/// A play/pause/seek transport over a fixed number of frames. Holds only the
/// position and run state; the audio callback hands it the current source buffer
/// each fill, so switching source never disturbs the playhead.
#[derive(Clone, Debug)]
pub struct Transport {
    len: usize,
    position: usize,
    playing: bool,
}

impl Transport {
    /// A paused transport at position 0 over `len` frames.
    pub fn new(len: usize) -> Self {
        Transport {
            len,
            position: 0,
            playing: false,
        }
    }

    /// Total length in frames.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether there are no frames to play.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Current playhead in frames.
    pub fn position(&self) -> usize {
        self.position
    }

    /// Whether the transport is advancing.
    pub fn is_playing(&self) -> bool {
        self.playing
    }

    /// Start playing. If already at the end, rewind to the start first so the
    /// play button always produces sound.
    pub fn play(&mut self) {
        if self.position >= self.len {
            self.position = 0;
        }
        self.playing = true;
    }

    /// Pause without moving the playhead.
    pub fn pause(&mut self) {
        self.playing = false;
    }

    /// Toggle play/pause.
    pub fn toggle(&mut self) {
        if self.playing {
            self.pause();
        } else {
            self.play();
        }
    }

    /// Move the playhead to `frame`, clamped to `[0, len]`. Does not change the
    /// play/pause state.
    pub fn seek(&mut self, frame: usize) {
        self.position = frame.min(self.len);
    }

    /// Normalized playhead in `[0, 1]` (0 when empty) — for the progress bar.
    pub fn fraction(&self) -> f32 {
        if self.len == 0 {
            0.0
        } else {
            self.position as f32 / self.len as f32
        }
    }

    /// Fill `out` (interleaved, `source.channel_count()` channels) from `source`
    /// at the current position, advancing the playhead while playing. Frames past
    /// the end — and everything while paused — are silence. Auto-stops (pauses)
    /// when the playhead reaches the end. Returns the number of frames sourced.
    pub fn fill(&mut self, source: &AudioBuffer, out: &mut [f32]) -> usize {
        for s in out.iter_mut() {
            *s = 0.0;
        }
        let channels = source.channel_count().max(1);
        let out_frames = out.len() / channels;
        if !self.playing || out_frames == 0 {
            return 0;
        }
        // The end is the shorter of the timeline and the source: if a shorter
        // source is ever swapped in past its end, the transport auto-stops
        // instead of wedging at `playing == true` with a frozen playhead.
        let end = self.len.min(source.frames());
        let readable = end.saturating_sub(self.position);
        let n = out_frames.min(readable);
        for f in 0..n {
            let src_frame = self.position + f;
            for ch in 0..channels {
                out[f * channels + ch] = source.channel(ch)[src_frame];
            }
        }
        self.position += n;
        if self.position >= end {
            self.playing = false; // auto-stop at end (no looping in v1)
        }
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::SampleEncoding;

    fn ramp(n: usize) -> AudioBuffer {
        let ch: Vec<f32> = (0..n).map(|i| i as f32).collect();
        AudioBuffer::new(48_000, 24, SampleEncoding::Int, vec![ch])
    }

    fn stereo_ramp(n: usize) -> AudioBuffer {
        let l: Vec<f32> = (0..n).map(|i| i as f32).collect();
        let r: Vec<f32> = (0..n).map(|i| -(i as f32)).collect();
        AudioBuffer::new(48_000, 24, SampleEncoding::Int, vec![l, r])
    }

    #[test]
    fn advances_by_the_number_of_frames_pulled() {
        let src = ramp(100);
        let mut t = Transport::new(100);
        t.play();
        let mut out = [0.0f32; 10];
        assert_eq!(t.fill(&src, &mut out), 10);
        assert_eq!(t.position(), 10);
        assert_eq!(out, [0., 1., 2., 3., 4., 5., 6., 7., 8., 9.]);
    }

    #[test]
    fn switching_source_preserves_the_playhead_exactly() {
        let a = ramp(100);
        // A second source with distinct values at the same positions.
        let b = {
            let ch: Vec<f32> = (0..100).map(|i| 1000.0 + i as f32).collect();
            AudioBuffer::new(48_000, 24, SampleEncoding::Int, vec![ch])
        };
        let mut t = Transport::new(100);
        t.play();
        let mut out = [0.0f32; 10];
        t.fill(&a, &mut out);
        assert_eq!(t.position(), 10);
        // Switch to B mid-stream: same playhead, B's samples from frame 10.
        t.fill(&b, &mut out);
        assert_eq!(t.position(), 20);
        assert_eq!(out[0], 1010.0);
        assert_eq!(out[9], 1019.0);
    }

    #[test]
    fn seek_clamps_to_the_valid_range() {
        let mut t = Transport::new(100);
        t.seek(50);
        assert_eq!(t.position(), 50);
        t.seek(10_000);
        assert_eq!(t.position(), 100, "seek past end clamps to len");
        t.seek(0);
        assert_eq!(t.position(), 0);
    }

    #[test]
    fn pause_and_resume_are_sample_accurate() {
        let src = ramp(100);
        let mut t = Transport::new(100);
        t.play();
        let mut out = [0.0f32; 10];
        t.fill(&src, &mut out); // frames 0..10
        assert_eq!(t.position(), 10);

        t.pause();
        let mut silent = [9.9f32; 10];
        assert_eq!(t.fill(&src, &mut silent), 0);
        assert!(silent.iter().all(|&s| s == 0.0), "paused output is silence");
        assert_eq!(t.position(), 10, "pause does not move the playhead");

        t.play();
        t.fill(&src, &mut out); // frames 10..20, exactly where it left off
        assert_eq!(out[0], 10.0);
        assert_eq!(t.position(), 20);
    }

    #[test]
    fn auto_stops_at_the_end_with_a_silent_tail() {
        let src = ramp(100);
        let mut t = Transport::new(100);
        t.seek(95);
        t.play();
        let mut out = [-1.0f32; 10];
        assert_eq!(t.fill(&src, &mut out), 5, "only 5 frames remain");
        assert_eq!(&out[0..5], &[95., 96., 97., 98., 99.]);
        assert!(out[5..].iter().all(|&s| s == 0.0), "tail is silence");
        assert_eq!(t.position(), 100);
        assert!(!t.is_playing(), "auto-stops at the end");
    }

    #[test]
    fn a_shorter_source_auto_stops_instead_of_wedging() {
        // Defensive: sources are same-length by guarantee, but if a shorter one
        // ever appears the transport must stop, not freeze at playing==true.
        let short = ramp(50);
        let mut t = Transport::new(100);
        t.seek(60); // already past the short source's end
        t.play();
        let mut out = [0.0f32; 10];
        assert_eq!(t.fill(&short, &mut out), 0);
        assert!(!t.is_playing(), "must auto-stop past a short source's end");
    }

    #[test]
    fn play_from_end_rewinds_to_start() {
        let mut t = Transport::new(100);
        t.seek(100);
        t.play();
        assert_eq!(t.position(), 0, "play at end rewinds");
        assert!(t.is_playing());
    }

    #[test]
    fn stereo_output_is_interleaved() {
        let src = stereo_ramp(100);
        let mut t = Transport::new(100);
        t.play();
        let mut out = [0.0f32; 6]; // 3 frames × 2 channels
        assert_eq!(t.fill(&src, &mut out), 3);
        // L0,R0,L1,R1,L2,R2 = 0,-0,1,-1,2,-2
        assert_eq!(out, [0.0, 0.0, 1.0, -1.0, 2.0, -2.0]);
        assert_eq!(t.position(), 3);
    }

    #[test]
    fn a_paused_transport_fills_silence_without_advancing() {
        let src = ramp(100);
        let mut t = Transport::new(100); // paused by default
        let mut out = [7.0f32; 8];
        assert_eq!(t.fill(&src, &mut out), 0);
        assert!(out.iter().all(|&s| s == 0.0));
        assert_eq!(t.position(), 0);
    }

    #[test]
    fn toggle_flips_play_state() {
        let mut t = Transport::new(10);
        assert!(!t.is_playing());
        t.toggle();
        assert!(t.is_playing());
        t.toggle();
        assert!(!t.is_playing());
    }
}
