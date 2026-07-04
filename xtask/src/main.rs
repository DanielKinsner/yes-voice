//! Project automation. `cargo xtask gate` runs the full local gate, identical
//! to CI (docs/TESTING.md): fmt --check -> clippy -D warnings -> test -> deny.
//!
//! Every step is **required**. `cargo deny` is an external tool; if it is not
//! installed the gate fails with an install hint rather than silently skipping
//! the license/advisory check (a silent skip let supply-chain drift slip past
//! the local gate — #27, finding 8).

use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    let task = std::env::args().nth(1).unwrap_or_default();
    match task.as_str() {
        "gate" => gate(),
        "bless" => bless(),
        other => {
            eprintln!("unknown xtask: {other:?}\n\ntasks:\n  gate    fmt --check + clippy -D warnings + test + deny\n  bless   deliberately regenerate goldens (YESVOX_BLESS=1 cargo test)");
            ExitCode::from(2)
        }
    }
}

/// Deliberately regenerate golden reference files. This is the ONLY sanctioned
/// way to rewrite a golden: it sets `YESVOX_BLESS=1` for a `cargo test` run, so
/// the golden harness (`yesvox_core::golden`) overwrites the `.golden` files
/// instead of asserting against them. The normal gate never sets the variable,
/// so an autonomous loop cannot re-bless a red golden as a side-effect. Review
/// and commit the resulting golden diff as its own deliberate change.
fn bless() -> ExitCode {
    eprintln!("=== cargo xtask bless (YESVOX_BLESS=1) ===");
    eprintln!("Regenerating goldens — this is a deliberate action; commit the diff explicitly.");
    match Command::new("cargo")
        .args(["test", "--all"])
        .env("YESVOX_BLESS", "1")
        .status()
    {
        Ok(status) if status.success() => {
            eprintln!("\nbless: goldens regenerated");
            ExitCode::SUCCESS
        }
        Ok(status) => {
            eprintln!("bless run failed ({status})");
            ExitCode::FAILURE
        }
        Err(e) => {
            eprintln!("bless could not run cargo test: {e}");
            ExitCode::FAILURE
        }
    }
}

/// One step of the gate: the command, its args, and whether it is an external
/// cargo subcommand (an installed tool, not a rustup component). External tools
/// get an availability preflight so a missing install fails loudly with a hint.
struct Step {
    args: &'static [&'static str],
    external: bool,
}

fn gate() -> ExitCode {
    let steps = [
        Step {
            args: &["fmt", "--all", "--check"],
            external: false,
        },
        Step {
            args: &[
                "clippy",
                "--all-targets",
                "--all-features",
                "--",
                "-D",
                "warnings",
            ],
            external: false,
        },
        Step {
            args: &["test", "--all"],
            external: false,
        },
        Step {
            args: &["deny", "check"],
            external: true,
        },
    ];

    for step in &steps {
        let pretty = format!("cargo {}", step.args.join(" "));
        // Every step is required. An external tool that isn't installed fails the
        // gate with a hint — never a silent skip (#27, finding 8).
        if step.external && !subcommand_available(step.args[0]) {
            eprintln!(
                "gate: `{pretty}` cannot run — cargo-{tool} is required but not installed.\n\
                 Install it with `cargo install cargo-{tool}` (CI enforces this license/advisory gate).",
                tool = step.args[0]
            );
            return ExitCode::FAILURE;
        }
        eprintln!("\n=== {pretty} ===");
        match Command::new("cargo").args(step.args).status() {
            Ok(status) if status.success() => {}
            Ok(status) => {
                eprintln!("gate failed at `{pretty}` ({status})");
                return ExitCode::FAILURE;
            }
            Err(e) => {
                eprintln!("gate could not run `{pretty}`: {e}");
                return ExitCode::FAILURE;
            }
        }
    }

    eprintln!("\ngate: green");
    ExitCode::SUCCESS
}

/// Whether `cargo <sub> --version` runs, used to detect an installed external
/// cargo subcommand.
fn subcommand_available(sub: &str) -> bool {
    Command::new("cargo")
        .args([sub, "--version"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}
