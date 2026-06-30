//! NON-DESTRUCTIVE guarantee (plan §7.6): the input file's bytes are identical
//! before and after a full session. Output goes to a new path; the input is
//! never written. (Byte-for-byte comparison; a SHA-256 variant can swap in if a
//! hash crate is ever vendored, but exact compare is the strongest check.)

mod common;

use yesvox_core::{read_wav, write_wav, Passthrough, VocalProcessor};

#[test]
fn input_bytes_unchanged_after_session() {
    let in_path = common::temp_wav("nondestructive-in");
    let out_path = common::temp_wav("nondestructive-out");
    common::write_fixture(&in_path);

    let before = common::read_bytes(&in_path);

    let mut buffer = read_wav(&in_path).expect("read input");
    let mut processor = Passthrough;
    processor.prepare(
        buffer.sample_rate(),
        buffer.frames().max(1),
        buffer.layout(),
    );
    processor.process(&mut buffer);
    write_wav(&out_path, &buffer).expect("write output");

    let after = common::read_bytes(&in_path);
    assert_eq!(
        before, after,
        "input file bytes changed during a session (non-destructive guarantee)"
    );

    common::remove(&[&in_path, &out_path]);
}
