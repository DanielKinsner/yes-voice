//! Long-file streaming + RAM preflight (plan §1.4).
//!
//! Non-causal denoisers can't hold a 30-minute file in working memory, so the
//! file is processed as overlapping chunks denoised on the [`WorkerPool`] and
//! recombined with a raised-cosine **equal-gain** crossfade. Equal-gain (the two
//! weights sum to 1) is what lets the chunked result match a single-shot
//! reference within tolerance, since neighbouring chunks agree across the
//! overlap. A RAM preflight estimates peak memory and warns before a file would
//! blow the budget.

use super::engine::Denoiser;
use super::pool::WorkerPool;
use std::sync::Arc;

/// Chunking policy for streamed processing.
#[derive(Clone, Copy, Debug)]
pub struct ChunkPolicy {
    /// Chunk length, seconds.
    pub chunk_s: f32,
    /// Overlap between adjacent chunks, seconds.
    pub overlap_s: f32,
    /// Working-set multiplier `K` for the RAM estimate (peak / file size).
    pub working_set_k: f32,
}

impl Default for ChunkPolicy {
    fn default() -> Self {
        ChunkPolicy {
            chunk_s: 8.0,
            overlap_s: 0.5,
            working_set_k: 8.0,
        }
    }
}

/// The result of a memory preflight.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RamPreflight {
    /// Estimated peak working-set bytes.
    pub estimated_bytes: u64,
    /// The budget compared against (70 % of available).
    pub budget_bytes: u64,
    /// Whether the estimate exceeds the budget (a warning is warranted).
    pub exceeds: bool,
}

impl RamPreflight {
    /// A user-facing warning if the budget is exceeded, else `None`.
    pub fn warning(&self) -> Option<String> {
        if self.exceeds {
            Some(format!(
                "this file may need ~{} MB to process, over the ~{} MB budget — close other apps or split the file.",
                self.estimated_bytes / 1_000_000,
                self.budget_bytes / 1_000_000,
            ))
        } else {
            None
        }
    }
}

impl ChunkPolicy {
    /// Estimate peak memory and compare it to 70 % of `available_bytes`.
    pub fn preflight(
        &self,
        frames: usize,
        channels: usize,
        _sample_rate: f32,
        available_bytes: u64,
    ) -> RamPreflight {
        let file_bytes = frames as u64 * channels as u64 * 4;
        let estimated_bytes = (file_bytes as f64 * self.working_set_k as f64) as u64;
        let budget_bytes = (available_bytes as f64 * 0.70) as u64;
        RamPreflight {
            estimated_bytes,
            budget_bytes,
            exceeds: estimated_bytes > budget_bytes,
        }
    }
}

/// Denoise `input` by streaming overlapping chunks through `pool` and
/// recombining with an equal-gain crossfade. Returns a same-length result.
pub fn process_streamed(
    input: &[f32],
    sample_rate: f32,
    policy: &ChunkPolicy,
    pool: &WorkerPool,
    denoiser: &Arc<dyn Denoiser>,
) -> Vec<f32> {
    let n = input.len();
    let chunk = (policy.chunk_s * sample_rate) as usize;
    let overlap = (policy.overlap_s * sample_rate) as usize;
    if n == 0 {
        return Vec::new();
    }
    // Short enough for one shot.
    if n <= chunk || overlap == 0 || overlap >= chunk {
        return pool
            .denoise_chunks(vec![input.to_vec()], sample_rate, denoiser)
            .pop()
            .unwrap_or_default();
    }

    let stride = chunk - overlap;
    // Chunk [start, end) boundaries.
    let mut bounds: Vec<(usize, usize)> = Vec::new();
    let mut start = 0;
    while start < n {
        let end = (start + chunk).min(n);
        bounds.push((start, end));
        if end == n {
            break;
        }
        start += stride;
    }

    let chunks: Vec<Vec<f32>> = bounds.iter().map(|&(s, e)| input[s..e].to_vec()).collect();
    let processed = pool.denoise_chunks(chunks, sample_rate, denoiser);

    // Equal-gain overlap-add: weights sum to 1 across each overlap so coherent
    // chunks reproduce the single-shot result.
    let mut out = vec![0.0f32; n];
    let mut wsum = vec![0.0f32; n];
    for (&(s, e), chunk_out) in bounds.iter().zip(&processed) {
        let len = e - s;
        for (j, &v) in chunk_out.iter().enumerate().take(len) {
            let idx = s + j;
            // Ramp in over the first `overlap` samples, out over the last.
            let into = j;
            let outof = len - 1 - j;
            let w_in = ramp(into, overlap);
            let w_out = ramp(outof, overlap);
            let w = w_in.min(w_out);
            out[idx] += w * v;
            wsum[idx] += w;
        }
    }
    for i in 0..n {
        if wsum[i] > 1.0e-6 {
            out[i] /= wsum[i];
        } else {
            out[i] = input[i];
        }
    }
    out
}

/// Raised-cosine ramp: 0→1 over `width` samples, flat 1 beyond.
fn ramp(pos: usize, width: usize) -> f32 {
    if width == 0 || pos >= width {
        return 1.0;
    }
    let t = (pos as f32 + 0.5) / width as f32;
    0.5 - 0.5 * (std::f32::consts::PI * t).cos()
}

#[cfg(test)]
mod tests {
    use super::super::engine::SpectralDenoiser;
    use super::*;
    use std::f32::consts::PI;

    const SR: f32 = 48_000.0;

    fn noisy_tone(n: usize) -> Vec<f32> {
        let mut seed = 0xDEAD_BEEFu32;
        (0..n)
            .map(|i| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let noise = (seed >> 9) as f32 / (1u32 << 23) as f32 - 0.5;
                0.4 * (2.0 * PI * 250.0 * i as f32 / SR).sin() + 0.05 * noise
            })
            .collect()
    }

    #[test]
    fn chunked_matches_single_shot_within_tolerance() {
        let n = 48_000 * 3; // 3 s → multiple chunks
        let x = noisy_tone(n);
        let den: Arc<dyn Denoiser> = Arc::new(SpectralDenoiser::default());
        let pool = WorkerPool::new(3);
        // Force several chunks.
        let policy = ChunkPolicy {
            chunk_s: 0.6,
            overlap_s: 0.2,
            working_set_k: 8.0,
        };

        let single = den.denoise(&x, SR);
        let chunked = process_streamed(&x, SR, &policy, &pool, &den);
        assert_eq!(chunked.len(), single.len(), "length preserved");

        // Mean absolute difference small on the interior.
        let a = &chunked[4096..n - 4096];
        let b = &single[4096..n - 4096];
        let mad: f32 = a.iter().zip(b).map(|(x, y)| (x - y).abs()).sum::<f32>() / a.len() as f32;
        assert!(mad < 0.01, "chunked should match single-shot: MAD {mad}");
    }

    #[test]
    fn silent_boundaries_stay_silent() {
        let n = 48_000 * 2;
        let x = vec![0.0f32; n];
        let den: Arc<dyn Denoiser> = Arc::new(SpectralDenoiser::default());
        let pool = WorkerPool::new(2);
        let policy = ChunkPolicy {
            chunk_s: 0.5,
            overlap_s: 0.2,
            working_set_k: 8.0,
        };
        let out = process_streamed(&x, SR, &policy, &pool, &den);
        assert!(
            out.iter().all(|&v| v.abs() < 1.0e-6),
            "silence stays silent"
        );
    }

    #[test]
    fn preflight_warns_only_when_over_budget() {
        let policy = ChunkPolicy::default();
        // 30-minute stereo 96 kHz file vs a 1 GB budget → over.
        let frames = 96_000 * 60 * 30;
        let big = policy.preflight(frames, 2, 96_000.0, 1_000_000_000);
        assert!(big.exceeds, "huge file must warn");
        assert!(big.warning().is_some());

        // A 30-second mono 48 kHz file vs 8 GB → fine.
        let small = policy.preflight(48_000 * 30, 1, 48_000.0, 8_000_000_000);
        assert!(!small.exceeds, "small file must not warn");
        assert!(small.warning().is_none());
    }
}
