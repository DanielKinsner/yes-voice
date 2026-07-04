//! Neural-rack plumbing guarantees (#27, finding 3). The NULL and SAME-LENGTH
//! gates only ever saw `Chain::all_bypassed` / `voiceover_default`, which
//! hardcode the light `SpectralDenoiser`; the neural engine is a *different*
//! `Denoiser` impl behind a cargo feature + fetched weights, so the guarantees
//! never exercised the rack around it and the neural CI job is compile-only.
//!
//! This gate closes that hole *without weights*: a mock `Denoiser` that honors
//! the same-length contract but is deliberately unlike `SpectralDenoiser`
//! (delayed + attenuated, and call-counted) is injected into the full rack via
//! `Preset::build_chain`. It proves the host plumbing — chunking, worker pool,
//! reassembly, bypass — preserves NULL and SAME-LENGTH around ANY denoiser. The
//! real-weights parity test stays `#[ignore]`d / env-gated as before.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use yesvox_core::{AudioBuffer, Denoiser, Preset, SampleEncoding, Stage, VocalProcessor};

const SR: u32 = 48_000;

/// A stand-in for a foreign (e.g. neural) engine: honors the `Denoiser`
/// same-length contract, but delays the block by `lag` samples and attenuates
/// it (a deliberate "algorithmic latency + change"), and counts every call so a
/// test can prove the injected engine actually ran on the rack's worker pool.
struct LaggyMockDenoiser {
    lag: usize,
    calls: AtomicUsize,
}

impl LaggyMockDenoiser {
    fn new(lag: usize) -> Self {
        LaggyMockDenoiser {
            lag,
            calls: AtomicUsize::new(0),
        }
    }
}

impl Denoiser for LaggyMockDenoiser {
    fn denoise(&self, input: &[f32], _sample_rate: f32) -> Vec<f32> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        // Same length as required by the trait, but content shifted by `lag`
        // (front zero-filled) and attenuated — clearly not the identity and not
        // SpectralDenoiser.
        let mut out = vec![0.0f32; input.len()];
        for i in self.lag..input.len() {
            out[i] = 0.5 * input[i - self.lag];
        }
        out
    }
}

fn voice_like(seconds: f32) -> Vec<f32> {
    use std::f32::consts::PI;
    let n = (seconds * SR as f32) as usize;
    (0..n)
        .map(|i| {
            let t = i as f32 / SR as f32;
            let amp = 0.18 * (1.0 + 0.2 * (2.0 * PI * 4.0 * t).sin());
            amp * ((2.0 * PI * 150.0 * t).sin() + 0.4 * (2.0 * PI * 300.0 * t).sin())
        })
        .collect()
}

fn buffer(samples: Vec<f32>) -> AudioBuffer {
    AudioBuffer::new(SR, 24, SampleEncoding::Int, vec![samples])
}

#[test]
fn all_bypassed_rack_is_identity_around_a_foreign_denoiser() {
    // NULL guarantee around ANY denoiser: build the *full* rack with the mock
    // engine plugged in, bypass every stage, and require bit-exact identity —
    // and that a bypassed denoise stage never even calls the engine.
    let input = voice_like(0.5);
    let mut buf = buffer(input.clone());

    let mock = Arc::new(LaggyMockDenoiser::new(64));
    let dyn_mock: Arc<dyn Denoiser> = mock.clone();
    let mut chain = Preset::PODCAST.build_chain(dyn_mock, 2);
    for stage in Stage::ALL {
        chain.set_stage_enabled(stage, false);
    }
    chain.prepare(SR, buf.frames().max(1), buf.layout());
    chain.reset();
    chain.process(&mut buf);

    let out = buf.channel(0);
    assert_eq!(out.len(), input.len(), "bypassed rack changed length");
    for (a, b) in out.iter().zip(&input) {
        assert!(
            (a - b).abs() <= f32::EPSILON,
            "null gate: sample drifted around the mock denoiser ({a} vs {b})"
        );
    }
    assert_eq!(
        mock.calls.load(Ordering::SeqCst),
        0,
        "a bypassed denoise stage must not invoke the engine"
    );
}

#[test]
fn active_rack_preserves_length_and_runs_the_foreign_denoiser() {
    // SAME-LENGTH guarantee around ANY denoiser: the full rack with the mock
    // engine *active* must return exactly as many frames as it received, and the
    // injected engine must actually have run on the rack (proving it isn't an
    // accidental no-op that would make length preservation trivial).
    let input = voice_like(2.0);
    let mut buf = buffer(input.clone());

    let mock = Arc::new(LaggyMockDenoiser::new(64));
    let dyn_mock: Arc<dyn Denoiser> = mock.clone();
    let mut chain = Preset::PODCAST.build_chain(dyn_mock, 3);
    chain.set_stage_enabled(Stage::Denoise, true);
    chain.prepare(SR, 4096, buf.layout());
    chain.reset();
    chain.process(&mut buf);

    assert_eq!(
        buf.frames(),
        input.len(),
        "same-length gate: rack changed frame count around the mock denoiser"
    );
    assert_eq!(buf.channel_count(), 1, "channel count changed");
    assert_eq!(buf.sample_rate(), SR, "sample rate changed");
    assert!(
        mock.calls.load(Ordering::SeqCst) > 0,
        "the injected denoiser never ran — length preservation would be trivial"
    );
}

/// The mock itself must honor the same-length contract, or the gates above would
/// be testing a broken stand-in rather than the rack.
#[test]
fn mock_denoiser_honors_the_same_length_contract() {
    let mock = LaggyMockDenoiser::new(64);
    let x = voice_like(0.1);
    let y = mock.denoise(&x, SR as f32);
    assert_eq!(x.len(), y.len(), "mock changed length");
    assert!(
        x.iter().zip(&y).any(|(a, b)| (a - b).abs() > 1e-3),
        "mock must change the signal"
    );
    assert_eq!(mock.calls.load(Ordering::SeqCst), 1);
}
