//! Shared transient detector (plan §3.1 / §3.2).
//!
//! De-click (#5) and de-pop (#6) detect impulsive events with **one** detector
//! so a given transient is found once and a duration/band rule decides which
//! repairer owns it — short impulses → de-click, mid-length high-frequency →
//! mouth-click, longer low-frequency bursts → plosive (de-pop). Because the
//! detector emits disjoint, singly-classified events, a transient can never be
//! repaired twice.

/// What kind of impulsive event a [`Transient`] is, which fixes its owner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransientKind {
    /// Short broadband impulse (vinyl click / digital tick) → de-click.
    Click,
    /// Mid-length high-frequency transient (tongue/lip/saliva) → mouth-click.
    MouthClick,
    /// Longer low-frequency burst (p/b plosive) → de-pop (#6).
    Plosive,
}

/// A detected impulsive event over the half-open sample range `[start, end)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Transient {
    /// First sample of the event.
    pub start: usize,
    /// One past the last sample of the event.
    pub end: usize,
    /// Classification (and therefore owner).
    pub kind: TransientKind,
}

impl Transient {
    /// Event length in samples.
    pub fn len(&self) -> usize {
        self.end - self.start
    }

    /// Whether the event is empty.
    pub fn is_empty(&self) -> bool {
        self.end <= self.start
    }
}

/// Tunables for the shared detector.
#[derive(Clone, Copy, Debug)]
pub struct DetectorConfig {
    /// Global threshold as a multiple of the residual's MAD.
    pub mad_factor: f32,
    /// Local threshold: a candidate must exceed this multiple of the local
    /// median residual (this is the voiced-frame guard — stationary voiced
    /// material never sticks this far out of its own neighbourhood).
    pub local_factor: f32,
    /// Merge events separated by fewer than this many samples.
    pub merge_gap: usize,
}

impl Default for DetectorConfig {
    fn default() -> Self {
        DetectorConfig {
            mad_factor: 6.0,
            local_factor: 8.0,
            merge_gap: 8,
        }
    }
}

/// The shared transient detector.
#[derive(Clone, Copy, Debug, Default)]
pub struct TransientDetector {
    config: DetectorConfig,
}

impl TransientDetector {
    /// Detector with explicit config.
    pub fn with_config(config: DetectorConfig) -> Self {
        TransientDetector { config }
    }

    /// Detect impulsive events in `x` at `sample_rate`. Events are disjoint,
    /// ordered, and each carries exactly one [`TransientKind`].
    pub fn detect(&self, x: &[f32], sample_rate: f32) -> Vec<Transient> {
        let n = x.len();
        if n < 4 {
            return Vec::new();
        }
        // Linear-prediction residual: a constant-acceleration predictor tracks a
        // sinusoid/voiced signal well, so its residual spikes only on impulses.
        let mut res = vec![0.0f32; n];
        for i in 2..n {
            let pred = 2.0 * x[i - 1] - x[i - 2];
            res[i] = (x[i] - pred).abs();
        }

        let global = self.config.mad_factor * mad(&res).max(1.0e-7);
        let local_win = ((0.020 * sample_rate) as usize).max(64);

        // Mark samples whose residual clears both the global and the local
        // (voiced-guard) threshold.
        let mut hot = vec![false; n];
        let mut win_start = 0;
        let mut local_med = local_median(&res[..local_win.min(n)]);
        for i in 2..n {
            if i >= win_start + local_win {
                win_start = i;
                let end = (i + local_win).min(n);
                local_med = local_median(&res[i..end.max(i + 1)]);
            }
            let local_thresh = self.config.local_factor * local_med.max(1.0e-7);
            if res[i] > global && res[i] > local_thresh {
                hot[i] = true;
            }
        }

        // Group hot samples into events, merging tiny gaps.
        let mut events: Vec<Transient> = Vec::new();
        let mut i = 0;
        while i < n {
            if !hot[i] {
                i += 1;
                continue;
            }
            let start = i;
            let mut end = i + 1;
            let mut gap = 0;
            let mut j = i + 1;
            while j < n {
                if hot[j] {
                    end = j + 1;
                    gap = 0;
                } else {
                    gap += 1;
                    if gap > self.config.merge_gap {
                        break;
                    }
                }
                j += 1;
            }
            // Pad the event slightly so the repair spans the whole glitch.
            let pad = 2;
            let start = start.saturating_sub(pad);
            let end = (end + pad).min(n);
            let kind = classify(x, start, end, sample_rate);
            events.push(Transient { start, end, kind });
            i = j;
        }
        events
    }
}

/// Interpolate across the half-open gap `[start, end)` with a cubic-Hermite
/// spline anchored to two good samples (value + slope) on each side — the
/// shared impulse-repair primitive for de-click and mouth-click. Falls back to
/// linear / hold when there isn't room for anchors.
pub fn fill_gap(x: &mut [f32], start: usize, end: usize) {
    let n = x.len();
    if start >= end || end > n {
        return;
    }
    if start < 2 || end + 1 >= n {
        // Not enough anchors: linear ramp between whatever neighbours exist.
        let left = if start > 0 {
            x[start - 1]
        } else {
            x[end.min(n - 1)]
        };
        let right = if end < n { x[end] } else { left };
        let span = (end - start + 1) as f32;
        for (k, idx) in (start..end).enumerate() {
            let t = (k + 1) as f32 / span;
            x[idx] = left + (right - left) * t;
        }
        return;
    }
    let l = start - 1;
    let r = end; // first good sample after the gap
    let y_l = x[l];
    let y_r = x[r];
    let m_l = x[l] - x[l - 1];
    let m_r = x[r + 1] - x[r];
    let span = (r - l) as f32;
    for (off, sample) in x[start..end].iter_mut().enumerate() {
        let s = (start + off - l) as f32 / span;
        let s2 = s * s;
        let s3 = s2 * s;
        let h00 = 2.0 * s3 - 3.0 * s2 + 1.0;
        let h10 = s3 - 2.0 * s2 + s;
        let h01 = -2.0 * s3 + 3.0 * s2;
        let h11 = s3 - s2;
        *sample = h00 * y_l + h10 * (m_l * span) + h01 * y_r + h11 * (m_r * span);
    }
}

/// Classify an event by duration and band content.
fn classify(x: &[f32], start: usize, end: usize, sample_rate: f32) -> TransientKind {
    let len_s = (end - start) as f32 / sample_rate;
    if len_s > 0.004 {
        // Long event: low-frequency energy → plosive, else mouth-click.
        if low_freq_dominant(&x[start..end]) {
            return TransientKind::Plosive;
        }
        return TransientKind::MouthClick;
    }
    if len_s > 0.0009 {
        TransientKind::MouthClick
    } else {
        TransientKind::Click
    }
}

/// Whether most of a slice's energy is low-frequency (slow zero-crossing rate).
fn low_freq_dominant(seg: &[f32]) -> bool {
    if seg.len() < 4 {
        return true;
    }
    let mut crossings = 0;
    for w in seg.windows(2) {
        if (w[0] >= 0.0) != (w[1] >= 0.0) {
            crossings += 1;
        }
    }
    // < ~2 zero-crossings over the event → low frequency.
    crossings <= 2
}

/// Median absolute deviation of a slice (robust spread).
fn mad(x: &[f32]) -> f32 {
    let med = local_median(x);
    let mut dev: Vec<f32> = x.iter().map(|v| (v - med).abs()).collect();
    dev.sort_by(|a, b| a.partial_cmp(b).unwrap());
    dev.get(dev.len() / 2).copied().unwrap_or(0.0)
}

/// Median of a slice (by value).
fn local_median(x: &[f32]) -> f32 {
    if x.is_empty() {
        return 0.0;
    }
    let mut v: Vec<f32> = x.to_vec();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[v.len() / 2]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    const SR: f32 = 48_000.0;

    fn tone(freq: f32, amp: f32, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| amp * (2.0 * PI * freq * i as f32 / SR).sin())
            .collect()
    }

    #[test]
    fn finds_injected_clicks() {
        let mut x = tone(200.0, 0.4, 48_000);
        let positions = [3000usize, 9000, 15000, 21000, 27000, 33000];
        for &p in &positions {
            x[p] += 0.8;
        }
        let det = TransientDetector::default();
        let events = det.detect(&x, SR);
        assert!(
            events.len() >= positions.len(),
            "should find all {} clicks, found {}",
            positions.len(),
            events.len()
        );
        // Every injected position lies inside some detected event.
        for &p in &positions {
            assert!(
                events.iter().any(|e| p >= e.start && p < e.end),
                "click at {p} not covered"
            );
        }
    }

    #[test]
    fn events_are_disjoint() {
        let mut x = tone(180.0, 0.3, 48_000);
        // Clicks + crackle (many tiny clicks).
        for k in 0..40 {
            x[500 + k * 1000] += 0.6;
        }
        let events = TransientDetector::default().detect(&x, SR);
        for pair in events.windows(2) {
            assert!(
                pair[0].end <= pair[1].start,
                "events overlap: {:?} vs {:?}",
                pair[0],
                pair[1]
            );
        }
    }

    #[test]
    fn voiced_consonant_is_not_flagged() {
        // A buzzy voiced signal (fundamental + harmonics), no clicks.
        let n = 48_000;
        let mut x = vec![0.0f32; n];
        for (i, s) in x.iter_mut().enumerate() {
            let t = i as f32 / SR;
            *s = 0.3 * (2.0 * PI * 150.0 * t).sin()
                + 0.2 * (2.0 * PI * 300.0 * t).sin()
                + 0.12 * (2.0 * PI * 450.0 * t).sin()
                + 0.08 * (2.0 * PI * 600.0 * t).sin();
        }
        let events = TransientDetector::default().detect(&x, SR);
        assert!(
            events.len() <= 2,
            "stationary voiced material must not trip the detector, got {}",
            events.len()
        );
    }
}
