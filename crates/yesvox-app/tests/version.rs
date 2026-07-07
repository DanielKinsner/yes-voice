//! W1 (#20): `yesvox --version` / `-V` prints the version and build hash so a
//! tester can confirm which build they are running — the CLI half of the
//! stale-build ghost check. The hash is environment-dependent, so we assert its
//! shape (a >=7 char hex run, or `unknown`) rather than a fixed value.

use std::process::Command;

fn run(flag: &str) -> (bool, String) {
    let bin = env!("CARGO_BIN_EXE_yesvox");
    let out = Command::new(bin)
        .arg(flag)
        .output()
        .expect("run the yesvox binary");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    )
}

/// True when `s` contains a run of >=7 hex digits (the git short hash) or the
/// literal `unknown` (git-unavailable build).
fn contains_hash_token(s: &str) -> bool {
    if s.contains("unknown") {
        return true;
    }
    let mut run = 0usize;
    for b in s.bytes() {
        if b.is_ascii_hexdigit() {
            run += 1;
            if run >= 7 {
                return true;
            }
        } else {
            run = 0;
        }
    }
    false
}

#[test]
fn version_flag_prints_version_and_hash() {
    let (ok, out) = run("--version");
    assert!(ok, "yesvox --version exited non-zero: {out:?}");
    assert!(
        out.contains(env!("CARGO_PKG_VERSION")),
        "--version must contain the crate version {:?}: {out:?}",
        env!("CARGO_PKG_VERSION")
    );
    assert!(
        contains_hash_token(&out),
        "--version must contain a git hash token or `unknown`: {out:?}"
    );
}

#[test]
fn short_v_flag_matches_long() {
    let (ok_short, short) = run("-V");
    assert!(ok_short, "yesvox -V exited non-zero: {short:?}");
    let (_, long) = run("--version");
    assert_eq!(short, long, "-V and --version must print the same line");
}
