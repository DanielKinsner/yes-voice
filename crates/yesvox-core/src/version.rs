//! The build/version stamp — the single source of truth for the version string
//! the CLI prints (`yesvox --version`) and the GUI paints in its always-visible
//! corner stamp (#20/W1).
//!
//! Its whole job is to kill stale-build ghost bugs: if the stamp on screen does
//! not match the build you meant to test, you are chasing a ghost. The short git
//! hash is injected by this crate's build script (`build.rs`) via the
//! `YESVOX_GIT_HASH` compile-time env var; when git is unavailable at build time
//! the hash is [`GIT_HASH`]`== "unknown"` and the build still succeeds (the build
//! must never depend on git).

/// The crate version (all three shipping crates — core/app/gui — share one
/// version and are bumped together), e.g. `1.0.0-rc.2`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The short git hash of the build: `a1b2c3d`, `a1b2c3d-dirty` (uncommitted
/// changes present at build time), or the literal `unknown` when git was
/// unavailable. Injected by `build.rs`.
pub const GIT_HASH: &str = match option_env!("YESVOX_GIT_HASH") {
    Some(hash) => hash,
    None => "unknown",
};

/// The tail of the CLI `--version` line: `"1.0.0-rc.2 (a1b2c3d)"`. The binary
/// prefixes its own name (`yesvox`).
pub fn version_line() -> String {
    format!("{VERSION} ({GIT_HASH})")
}

/// The always-visible GUI corner stamp: `"v1.0.0-rc.2 · a1b2c3d · Neural"`.
///
/// `engine` is the last-render engine label (`"Neural"` / `"Light"`), passed
/// through verbatim so a neural→light fallback is visible on screen. Before any
/// render the caller supplies a neutral placeholder.
pub fn gui_stamp(engine: &str) -> String {
    format!("v{VERSION} · {GIT_HASH} · {engine}")
}
