//! W1 (#20): the build/version stamp is the one source of truth the CLI prints
//! and the GUI paints. These tests pin the *string composition* — the git hash
//! itself is injected by `build.rs` and is environment-dependent, so we assert
//! its shape (short hex, optional `-dirty`, or the literal `unknown`) rather
//! than a fixed value.

use yesvox_core::version;

/// The git hash token is a short hex string, optionally `-dirty`, or `unknown`
/// when git was unavailable at build time (the build must never require git).
fn is_valid_hash_token(token: &str) -> bool {
    if token == "unknown" {
        return true;
    }
    let core = token.strip_suffix("-dirty").unwrap_or(token);
    core.len() >= 7 && core.bytes().all(|b| b.is_ascii_hexdigit())
}

#[test]
fn git_hash_is_short_hex_or_unknown() {
    assert!(
        is_valid_hash_token(version::GIT_HASH),
        "GIT_HASH must be a >=7 char hex hash, `hash-dirty`, or `unknown`, got {:?}",
        version::GIT_HASH
    );
}

#[test]
fn version_line_contains_version_and_hash() {
    let line = version::version_line();
    assert!(
        line.contains(version::VERSION),
        "version_line must contain the crate version {:?}: {line:?}",
        version::VERSION
    );
    assert!(
        line.contains(version::GIT_HASH),
        "version_line must contain the git hash {:?}: {line:?}",
        version::GIT_HASH
    );
}

#[test]
fn gui_stamp_composes_version_hash_and_engine() {
    assert_eq!(
        version::gui_stamp("Neural"),
        format!("v{} · {} · Neural", version::VERSION, version::GIT_HASH)
    );
    // The engine word is passed through verbatim so a Light fallback is visible.
    assert!(version::gui_stamp("Light").contains("Light"));
}
