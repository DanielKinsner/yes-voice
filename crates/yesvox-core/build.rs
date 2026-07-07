//! Injects the short git hash of the build into the compile as the
//! `YESVOX_GIT_HASH` env var, which `src/version.rs` reads for the version
//! stamp (#20/W1).
//!
//! Git is optional: if git is not installed, this is not a git checkout, or the
//! command fails for any reason, the hash falls back to `unknown` and the build
//! still succeeds — the build must never depend on git. A `git` process spawn is
//! not a network symbol, so it does not affect the OFFLINE guarantee (the
//! offline scan covers `src/`, not this build script, and forbids net symbols,
//! not process spawns).

use std::process::Command;

fn main() {
    let hash = git_hash().unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=YESVOX_GIT_HASH={hash}");

    // Re-probe when the checked-out commit or the index (dirty state) changes.
    // A missing path just means "always re-run", which is harmless for a tiny
    // script and correct for a non-standard git layout (e.g. worktrees).
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/index");
}

/// The short hash, suffixed `-dirty` when the working tree has uncommitted
/// changes; `None` when git is unavailable or reports nothing usable.
fn git_hash() -> Option<String> {
    let short = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()?;
    if !short.status.success() {
        return None;
    }
    let hash = String::from_utf8(short.stdout).ok()?.trim().to_string();
    if hash.is_empty() {
        return None;
    }
    let dirty = Command::new("git")
        .args(["status", "--porcelain"])
        .output()
        .map(|out| !out.stdout.is_empty())
        .unwrap_or(false);
    Some(if dirty { format!("{hash}-dirty") } else { hash })
}
