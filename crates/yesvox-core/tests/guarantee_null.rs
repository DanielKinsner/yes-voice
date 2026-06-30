//! NULL guarantee (plan §7.6): the assembled chain with **every module
//! bypassed** reproduces the input within float epsilon. This now drives the
//! real [`yesvox_core::Chain`] (not a no-op `Passthrough`), so the gate locks
//! that composing the whole rack and bypassing it is still bit-exact identity —
//! a regression here means some module's bypass stopped being a true identity.

mod common;

use yesvox_core::{Chain, VocalProcessor};

#[test]
fn all_bypassed_chain_is_sample_accurate_identity() {
    let path = common::temp_wav("null");
    let original = common::write_fixture(&path);

    let mut processed = original.clone();
    let mut chain = Chain::all_bypassed(1);
    chain.prepare(
        processed.sample_rate(),
        processed.frames().max(1),
        processed.layout(),
    );
    chain.reset();
    chain.process(&mut processed);

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
