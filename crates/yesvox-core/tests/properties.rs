//! Property lane (plan §7.7 / TESTING.md): the format/SR/channel I/O contract
//! and the value-type / param-range invariants, checked over generated inputs
//! rather than single fixtures. These complement the example-based guarantee
//! gates — proptest hunts the edge of the contract (odd lengths, both channel
//! topologies, every sample rate, the full intensity × estimate space).

use proptest::prelude::*;

use yesvox_core::{
    quality::{resolve, QualityMode},
    read_wav, write_wav, AudioBuffer, Chain, SampleEncoding, VocalProcessor,
};

/// Deterministic pseudo-random samples in [-1, 1) from a seed (proptest shrinks
/// the seed, not each sample, keeping failures readable).
fn samples(seed: u64, len: usize) -> Vec<f32> {
    let mut s = seed | 1;
    (0..len)
        .map(|_| {
            s = s
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            ((s >> 40) as f32 / (1u64 << 24) as f32) - 1.0
        })
        .collect()
}

proptest! {
    /// The format/SR/channel contract: a WAV write→read round-trip preserves
    /// frame count, channel count, and sample rate for any valid buffer.
    #[test]
    fn wav_roundtrip_preserves_shape(
        sr in prop::sample::select(vec![44_100u32, 48_000, 88_200, 96_000]),
        nch in 1usize..=2,
        len in 1usize..1_500,
        seed in any::<u64>(),
    ) {
        let chans: Vec<Vec<f32>> = (0..nch).map(|c| samples(seed ^ (c as u64 + 1), len)).collect();
        let buf = AudioBuffer::new(sr, 16, SampleEncoding::Int, chans);
        let path = std::env::temp_dir().join(format!(
            "yesvox-prop-{}-{sr}-{nch}-{len}.wav",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        write_wav(&path, &buf).expect("write");
        let back = read_wav(&path).expect("read");
        prop_assert_eq!(back.frames(), buf.frames());
        prop_assert_eq!(back.channel_count(), buf.channel_count());
        prop_assert_eq!(back.sample_rate(), buf.sample_rate());
        let _ = std::fs::remove_file(&path);
    }

    /// The all-bypass NULL guarantee holds for ANY signal, mono or dual-mono —
    /// not just the fixed fixture the guarantee gate uses.
    #[test]
    fn all_bypassed_chain_is_identity_for_any_signal(
        nch in 1usize..=2,
        len in 1usize..4_000,
        seed in any::<u64>(),
    ) {
        let chans: Vec<Vec<f32>> = (0..nch).map(|c| samples(seed ^ (c as u64 + 1), len)).collect();
        let original = chans.clone();
        let mut buf = AudioBuffer::new(48_000, 16, SampleEncoding::Int, chans);
        let mut chain = Chain::all_bypassed(1);
        chain.prepare(48_000, 4_096, buf.layout());
        chain.reset();
        chain.process(&mut buf);
        for (ch, orig) in original.iter().enumerate() {
            for (a, b) in buf.channel(ch).iter().zip(orig) {
                prop_assert!((a - b).abs() <= f32::EPSILON);
            }
        }
    }

    /// The slow-hardware downshift invariants (plan §2.4) over the whole
    /// intensity × render-estimate space: resolve never *upshifts*, Natural is a
    /// fixed point, a Fast-mode badge implies a downshift to Natural from a
    /// non-Natural request, and the binding always matches the resolved mode.
    #[test]
    fn resolve_only_ever_downshifts_to_natural(
        mode_idx in 0usize..3,
        est in 0.0f32..6_000.0,
    ) {
        let mode = [QualityMode::Natural, QualityMode::Balanced, QualityMode::RestorationHq][mode_idx];
        let r = resolve(mode, est);
        prop_assert!(r.mode == mode || r.mode == QualityMode::Natural);
        if mode == QualityMode::Natural {
            prop_assert_eq!(r.mode, QualityMode::Natural);
            prop_assert!(!r.fast_mode);
        }
        if r.fast_mode {
            prop_assert_eq!(r.mode, QualityMode::Natural);
            prop_assert_ne!(mode, QualityMode::Natural);
        }
        prop_assert_eq!(r.binding, r.mode.binding());
    }
}
