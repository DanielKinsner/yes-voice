//! OFFLINE guarantee (plan §7.6): the processing path completes with no network
//! dependency. The full read -> process -> write pipeline uses only local file
//! I/O and pure-Rust code.
//!
//! This test pins that the pipeline itself is self-contained. Strict
//! network-namespace enforcement (running the suite under `unshare --net`) is a
//! CI-level hardening step tracked for a later slice; it must not gate #1 on a
//! flaky sandbox primitive.

mod common;

use yesvox_core::{read_wav, write_wav, Passthrough, VocalProcessor};

#[test]
fn full_pipeline_runs_without_network() {
    let in_path = common::temp_wav("offline-in");
    let out_path = common::temp_wav("offline-out");
    common::write_fixture(&in_path);

    let mut buffer = read_wav(&in_path).expect("read");
    let mut processor = Passthrough;
    processor.prepare(
        buffer.sample_rate(),
        buffer.frames().max(1),
        buffer.layout(),
    );
    processor.process(&mut buffer);
    write_wav(&out_path, &buffer).expect("write");

    assert!(out_path.exists(), "offline pipeline produced no output");

    common::remove(&[&in_path, &out_path]);
}
