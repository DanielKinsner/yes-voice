//! SAME-LENGTH guarantee (plan §7.6): output sample count == input sample count
//! per channel on the non-resampling path. Verified across a full
//! read -> process -> write -> read round-trip — and now through an **active**
//! chain (every module running), since "same take, same length out" must hold
//! while the rack is actually processing, not only when bypassed.

mod common;

use yesvox_core::{read_wav, write_wav, Chain, VocalProcessor};

#[test]
fn output_matches_input_length_rate_and_channels() {
    let in_path = common::temp_wav("samelen-in");
    let out_path = common::temp_wav("samelen-out");
    let input = common::write_fixture(&in_path);

    let mut buffer = read_wav(&in_path).expect("read input");
    let mut chain = Chain::voiceover_default(2);
    chain.prepare(
        buffer.sample_rate(),
        buffer.frames().max(1),
        buffer.layout(),
    );
    chain.reset();
    chain.process(&mut buffer);
    write_wav(&out_path, &buffer).expect("write output");

    let output = read_wav(&out_path).expect("read output");
    assert_eq!(output.frames(), input.frames(), "frame count changed");
    assert_eq!(
        output.sample_rate(),
        input.sample_rate(),
        "sample rate changed"
    );
    assert_eq!(
        output.channel_count(),
        input.channel_count(),
        "channel count changed"
    );

    common::remove(&[&in_path, &out_path]);
}
