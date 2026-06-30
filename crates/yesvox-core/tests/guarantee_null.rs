//! NULL guarantee (plan §7.6): the engine with all modules bypassed reproduces
//! the input within float epsilon. With only `Passthrough` in v1 the bypass is
//! exact; this gate locks that property as real modules are added.

mod common;

use yesvox_core::{Passthrough, VocalProcessor};

#[test]
fn passthrough_is_sample_accurate_identity() {
    let path = common::temp_wav("null");
    let original = common::write_fixture(&path);

    let mut processed = original.clone();
    let mut processor = Passthrough;
    processor.prepare(
        processed.sample_rate(),
        processed.frames().max(1),
        processed.layout(),
    );
    processor.reset();
    processor.process(&mut processed);

    assert_eq!(processed.channel_count(), original.channel_count());
    assert_eq!(processed.frames(), original.frames());
    for ch in 0..original.channel_count() {
        for (a, b) in processed.channel(ch).iter().zip(original.channel(ch)) {
            assert!(
                (a - b).abs() <= f32::EPSILON,
                "null gate: sample drifted ({a} vs {b})"
            );
        }
    }

    common::remove(&[&path]);
}
