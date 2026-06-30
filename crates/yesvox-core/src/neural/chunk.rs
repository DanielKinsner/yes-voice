//! Windowed mask inference for the frame-count-locked MossFormer2 graph (#10).
//!
//! The exported ONNX graph only runs at a fixed frame count (`verify_dynamic.py`
//! showed the legacy tracer bakes 256 into a Reshape), so it cannot mask a
//! whole file in one call. [`WindowedMask`] wraps any fixed-window
//! [`MaskModel`] and presents one that works for any number of frames: it slides
//! a fixed window with overlap, runs the inner model on each, and stitches the
//! **central** mask frames — discarding the poorly-contextualized give-up edges
//! exactly like clearvoice's `decode_one_audio_*` sliding window, but on mask
//! frames rather than samples (so the single whole-file iSTFT stays seamless).
//!
//! The give-up width is `(window - stride) / 2`, which makes adjacent kept
//! regions tile with no gap or overlap; the final window is clamped to the end
//! and keeps its tail (no later context exists to improve on it).

use super::engine::MaskModel;

/// A [`MaskModel`] that runs a fixed-window inner model over overlapping windows.
pub struct WindowedMask {
    inner: Box<dyn MaskModel>,
    window: usize,
    stride: usize,
}

impl WindowedMask {
    /// Wrap `inner` (which must be called with exactly `window` frames) and run
    /// it with the given hop `stride` (`stride < window`).
    pub fn new(inner: Box<dyn MaskModel>, window: usize, stride: usize) -> Self {
        assert!(
            stride > 0 && stride <= window,
            "stride must be in 1..=window"
        );
        assert!(
            (window - stride).is_multiple_of(2),
            "window - stride must be even so kept regions tile"
        );
        WindowedMask {
            inner,
            window,
            stride,
        }
    }

    /// Defaults for the MossFormer2_SE_48K export: 256-frame window, 75 % hop.
    pub fn mossformer(inner: Box<dyn MaskModel>) -> Self {
        WindowedMask::new(inner, 256, 192)
    }

    fn give_up(&self) -> usize {
        (self.window - self.stride) / 2
    }

    /// Run the inner model on exactly `window` frames, zero-padding a short tail.
    fn run_window(&self, frames: &[Vec<f32>]) -> Vec<Vec<f32>> {
        if frames.len() == self.window {
            return self.inner.run(frames);
        }
        let dim = frames.first().map_or(0, |r| r.len());
        let mut padded = frames.to_vec();
        padded.resize(self.window, vec![0.0; dim]);
        self.inner.run(&padded)
    }
}

impl MaskModel for WindowedMask {
    fn run(&self, features: &[Vec<f32>]) -> Vec<Vec<f32>> {
        let t = features.len();
        if t == 0 {
            return Vec::new();
        }
        // Short input: pad to one window, run, truncate back.
        if t <= self.window {
            let mut mask = self.run_window(features);
            mask.truncate(t);
            return mask;
        }

        let give_up = self.give_up();
        let mut out: Vec<Vec<f32>> = Vec::with_capacity(t);
        let mut next = 0; // next global frame still needing a mask
        let mut start = 0;
        loop {
            let s = start.min(t - self.window); // clamp the last window to the end
            let win_mask = self.inner.run(&features[s..s + self.window]);
            let last = s + self.window >= t;
            let hi = if last { t } else { s + self.window - give_up };
            // Fill [next, hi) from this window's frames (central region).
            for g in next..hi {
                out.push(win_mask[g - s].clone());
            }
            next = hi;
            if last {
                break;
            }
            start += self.stride;
        }
        debug_assert_eq!(out.len(), t);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    const BINS: usize = 4; // tiny mask width for the tests

    /// Inner model that asserts it always sees exactly `window` frames and
    /// returns each row filled with that row's *local* index — so the stitched
    /// output reveals which window each global frame was taken from.
    struct LocalIndexModel {
        window: usize,
        calls: Arc<AtomicUsize>,
    }
    impl MaskModel for LocalIndexModel {
        fn run(&self, features: &[Vec<f32>]) -> Vec<Vec<f32>> {
            assert_eq!(features.len(), self.window, "inner must get exactly window");
            self.calls.fetch_add(1, Ordering::Relaxed);
            (0..features.len()).map(|i| vec![i as f32; BINS]).collect()
        }
    }

    /// Inner model returning a constant gain regardless of input.
    struct ConstModel(usize, f32);
    impl MaskModel for ConstModel {
        fn run(&self, features: &[Vec<f32>]) -> Vec<Vec<f32>> {
            assert_eq!(features.len(), self.0);
            vec![vec![self.1; BINS]; features.len()]
        }
    }

    fn feats(t: usize) -> Vec<Vec<f32>> {
        (0..t).map(|i| vec![i as f32; 180]).collect()
    }

    #[test]
    fn exact_window_passes_straight_through() {
        let calls = Arc::new(AtomicUsize::new(0));
        let m = WindowedMask::new(
            Box::new(LocalIndexModel {
                window: 256,
                calls: calls.clone(),
            }),
            256,
            192,
        );
        let out = m.run(&feats(256));
        assert_eq!(out.len(), 256);
        assert_eq!(out[100][0], 100.0); // local == global in a single window
        assert_eq!(calls.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn short_input_is_padded_and_truncated() {
        let m = WindowedMask::new(Box::new(ConstModel(256, 0.5)), 256, 192);
        let out = m.run(&feats(100));
        assert_eq!(out.len(), 100);
        assert!(out.iter().flatten().all(|&g| g == 0.5));
    }

    #[test]
    fn long_input_is_fully_covered_no_gaps() {
        // Constant inner ⇒ every stitched frame must be the constant, proving
        // full coverage with no zero-filled gaps and exact length.
        let m = WindowedMask::new(Box::new(ConstModel(256, 0.7)), 256, 192);
        let out = m.run(&feats(1000));
        assert_eq!(out.len(), 1000);
        assert!(
            out.iter().flatten().all(|&g| g == 0.7),
            "all frames covered"
        );
    }

    #[test]
    fn central_frames_come_from_the_owning_window() {
        // window 256, stride 192 ⇒ give_up 32. First window owns global [0,224)
        // (local == global); the second window (start 192) owns [224,416) with
        // local index g-192, i.e. global 224 ⇒ local 32 (just past give-up).
        let calls = Arc::new(AtomicUsize::new(0));
        let m = WindowedMask::new(
            Box::new(LocalIndexModel {
                window: 256,
                calls: calls.clone(),
            }),
            256,
            192,
        );
        let out = m.run(&feats(600));
        assert_eq!(out.len(), 600);
        // First-window region: local index == global index.
        for g in [0, 100, 223] {
            assert_eq!(out[g][0], g as f32, "frame {g} should be from window 0");
        }
        // Second window starts at 192 → global 224 is its local frame 32.
        assert_eq!(out[224][0], 32.0);
        assert_eq!(out[300][0], (300 - 192) as f32);
        // Every kept local index is a valid, central-ish frame (< window).
        assert!(out.iter().all(|r| r[0] < 256.0));
        assert!(calls.load(Ordering::Relaxed) >= 3, "multiple windows run");
    }
}
