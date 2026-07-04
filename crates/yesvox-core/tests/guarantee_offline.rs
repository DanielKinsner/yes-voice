//! OFFLINE guarantee (plan §7.6): the processing path completes with no network
//! dependency. The full read -> process -> write pipeline uses only local file
//! I/O and pure-Rust code.
//!
//! The runtime pipeline test alone is *tautological* (it only proves an output
//! file appears — an HTTP call mid-process would still pass), so the real
//! enforcement (#27, finding 1) is a **source-level scan**: `yesvox-core`'s
//! source must contain no networking symbols at all. A self-test proves the
//! scanner actually detects network code, so it can never silently pass.
//! Strict network-namespace enforcement (running the suite under `unshare
//! --net`) is an optional belt-and-suspenders CI job, not a gate on a flaky
//! sandbox primitive.

mod common;

use std::path::{Path, PathBuf};

use yesvox_core::{read_wav, write_wav, Chain, VocalProcessor};

/// Networking identifiers that must never appear in the offline processing
/// crate. Chosen to be code tokens that do not occur in ordinary prose, and the
/// scanner strips comments before matching, so a stray mention cannot trip it.
const NETWORK_SYMBOLS: &[&str] = &[
    "std::net",
    "TcpStream",
    "TcpListener",
    "UdpSocket",
    "tokio::net",
    "async_std::net",
    "reqwest",
    "hyper::",
    "ureq",
    "isahc",
    "socket2",
    "curl::",
];

/// Strip `//` line comments (covers `///` doc and `//!` inner-doc comments too)
/// and crude `/* … */` block comments so only real code is scanned. Truncating
/// at `//` can only *remove* candidate matches, never invent one — a network
/// symbol after a `//` is, by definition, inside a comment.
fn strip_comments(src: &str) -> String {
    // Remove block comments first (non-nested; the core uses these rarely).
    let mut no_block = String::with_capacity(src.len());
    let bytes = src.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if i + 1 < bytes.len() && bytes[i] == b'/' && bytes[i + 1] == b'*' {
            i += 2;
            while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                i += 1;
            }
            i += 2;
            no_block.push(' ');
            continue;
        }
        no_block.push(bytes[i] as char);
        i += 1;
    }
    no_block
        .lines()
        .map(|line| match line.find("//") {
            Some(idx) => &line[..idx],
            None => line,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Network symbols found in `code` (after comment stripping).
fn network_symbols_in(code: &str) -> Vec<&'static str> {
    let stripped = strip_comments(code);
    NETWORK_SYMBOLS
        .iter()
        .copied()
        .filter(|sym| stripped.contains(sym))
        .collect()
}

/// Every `.rs` file under `dir`, recursively.
fn rust_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).expect("read_dir").flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(rust_files(&path));
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            out.push(path);
        }
    }
    out
}

#[test]
fn core_source_contains_no_network_symbols() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut offenders = Vec::new();
    for file in rust_files(&src) {
        let code = std::fs::read_to_string(&file).expect("read source");
        let hits = network_symbols_in(&code);
        if !hits.is_empty() {
            offenders.push(format!("{}: {:?}", file.display(), hits));
        }
    }
    assert!(
        offenders.is_empty(),
        "yesvox-core processing crate references networking — OFFLINE guarantee broken:\n{}",
        offenders.join("\n")
    );
}

#[test]
fn workspace_sources_contain_no_network_symbols() {
    // The shipping binary is `yesvox-gui`, not `yesvox-core` — a network dep
    // added to the GUI (telemetry, auto-update) would break the OFFLINE promise
    // while the core-only scan stayed green. Scan every workspace crate we own.
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root");
    let mut offenders = Vec::new();
    for crate_src in [
        "crates/yesvox-gui/src",
        "crates/yesvox-app/src",
        "xtask/src",
    ] {
        let dir = root.join(crate_src);
        assert!(
            dir.is_dir(),
            "scan target missing (crate moved? update this list): {}",
            dir.display()
        );
        for file in rust_files(&dir) {
            let code = std::fs::read_to_string(&file).expect("read source");
            let hits = network_symbols_in(&code);
            if !hits.is_empty() {
                offenders.push(format!("{}: {:?}", file.display(), hits));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "workspace source references networking — OFFLINE guarantee broken:\n{}",
        offenders.join("\n")
    );
}

#[test]
fn the_network_scanner_actually_detects_network_code() {
    // Proof the scan is not a no-op: it must flag real networking code…
    let bad = r#"
        use std::net::TcpStream;
        fn phone_home() { let _ = reqwest::blocking::get("http://example.com"); }
    "#;
    let hits = network_symbols_in(bad);
    assert!(
        hits.contains(&"std::net") && hits.contains(&"reqwest") && hits.contains(&"TcpStream"),
        "scanner missed networking symbols: {hits:?}"
    );
    // …and must ignore the same symbols when they appear only in comments.
    let commented = "// this code never opens a TcpStream or calls reqwest::get\nfn f() {}";
    assert!(
        network_symbols_in(commented).is_empty(),
        "scanner false-positived on a comment"
    );
}

#[test]
fn full_pipeline_runs_without_network() {
    let in_path = common::temp_wav("offline-in");
    let out_path = common::temp_wav("offline-out");
    common::write_fixture(&in_path);

    let mut buffer = read_wav(&in_path).expect("read");
    let mut chain = Chain::voiceover_default(2);
    chain.prepare(
        buffer.sample_rate(),
        buffer.frames().max(1),
        buffer.layout(),
    );
    chain.reset();
    chain.process(&mut buffer);
    write_wav(&out_path, &buffer).expect("write");

    assert!(out_path.exists(), "offline pipeline produced no output");

    common::remove(&[&in_path, &out_path]);
}
