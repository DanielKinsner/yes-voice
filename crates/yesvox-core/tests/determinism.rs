//! Whole-chain determinism gate (#34). Goldens, crash-recovery re-renders, and
//! user trust all assume: same input + same settings → same output. This asserts
//! it end-to-end through the full rack — including the parallel worker pool,
//! where thread-count variation is the real risk (chunks finish out of order).
//!
//! Same-machine determinism (twice, and across worker counts) is a **hard gate**
//! here and therefore runs on every CI platform. Cross-*platform* byte-agreement
//! is not yet asserted: a fingerprint is printed so the per-OS gate logs record
//! it (informational first — see docs/TESTING.md).

use std::sync::Arc;

use yesvox_core::neural::sha256_hex;
use yesvox_core::{
    AudioBuffer, Denoiser, Preset, SampleEncoding, SpectralDenoiser, VocalProcessor,
};

const SR: u32 = 48_000;

/// A deterministic voice-like fixture (fundamental + harmonics + tremolo).
fn fixture() -> Vec<f32> {
    use std::f32::consts::PI;
    let n = 96_000; // 2 s
    (0..n)
        .map(|i| {
            let t = i as f32 / SR as f32;
            let amp = 0.18 * (1.0 + 0.2 * (2.0 * PI * 4.0 * t).sin());
            amp * ((2.0 * PI * 150.0 * t).sin()
                + 0.5 * (2.0 * PI * 300.0 * t).sin()
                + 0.3 * (2.0 * PI * 450.0 * t).sin())
        })
        .collect()
}

/// Render the fixture through the full Podcast rack with `workers` pool threads.
fn render(workers: usize) -> Vec<f32> {
    let denoiser: Arc<dyn Denoiser> = Arc::new(SpectralDenoiser::default());
    let mut chain = Preset::PODCAST.build_chain(denoiser, workers);
    let mut buf = AudioBuffer::new(SR, 24, SampleEncoding::Int, vec![fixture()]);
    chain.prepare(SR, 4096, buf.layout());
    chain.reset();
    chain.process(&mut buf);
    buf.channel(0).to_vec()
}

fn fingerprint(samples: &[f32]) -> String {
    let mut bytes = Vec::with_capacity(samples.len() * 4);
    for s in samples {
        bytes.extend_from_slice(&s.to_le_bytes());
    }
    sha256_hex(&bytes)
}

#[test]
fn same_settings_render_twice_is_byte_identical() {
    let a = render(4);
    let b = render(4);
    assert_eq!(a.len(), b.len());
    assert_eq!(
        a, b,
        "two identical renders drifted — the chain is not deterministic"
    );
}

#[test]
fn worker_count_does_not_change_the_output() {
    // The pool reassembles chunk results by index, so 1 vs many workers must be
    // bit-identical despite the out-of-order completion.
    let one = render(1);
    let many = render(8);
    assert_eq!(one.len(), many.len());
    assert_eq!(
        one, many,
        "worker count changed the output — chunk reassembly is order-dependent"
    );
}

#[test]
fn cross_platform_fingerprint_is_stable_and_recorded() {
    // Same-machine stability of the fingerprint (a hard check), and print it so
    // each CI platform's gate log records its hash for the informational
    // cross-platform comparison (#34, docs/TESTING.md).
    let hash = fingerprint(&render(4));
    assert_eq!(hash, fingerprint(&render(4)), "fingerprint unstable");
    println!(
        "DETERMINISM_FINGERPRINT os={} arch={} sha256={hash}",
        std::env::consts::OS,
        std::env::consts::ARCH,
    );
}
