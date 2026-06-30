//! Project automation. `cargo xtask gate` runs the full local gate, identical
//! to CI (docs/TESTING.md): fmt --check -> clippy -D warnings -> test -> deny.
//!
//! `cargo deny` is optional locally — if it isn't installed the step is skipped
//! with a warning, because CI installs and enforces it.

use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    let task = std::env::args().nth(1).unwrap_or_default();
    match task.as_str() {
        "gate" => gate(),
        other => {
            eprintln!("unknown xtask: {other:?}\n\ntasks:\n  gate   fmt --check + clippy -D warnings + test + deny");
            ExitCode::from(2)
        }
    }
}

/// One step of the gate: the command, its args, and whether it is required
/// (skipped-with-warning when the tool is absent if not required).
struct Step {
    args: &'static [&'static str],
    required: bool,
}

fn gate() -> ExitCode {
    let steps = [
        Step {
            args: &["fmt", "--all", "--check"],
            required: true,
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
            required: true,
        },
        Step {
            args: &["test", "--all"],
            required: true,
        },
        Step {
            args: &["deny", "check"],
            required: false,
        },
    ];

    for step in &steps {
        let pretty = format!("cargo {}", step.args.join(" "));
        if !step.required && !subcommand_available(step.args[0]) {
            eprintln!(
                "warning: skipping `{pretty}` (cargo-{} not installed; CI enforces it)",
                step.args[0]
            );
            continue;
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

/// Whether `cargo <sub> --version` runs, used to detect optional subcommands.
fn subcommand_available(sub: &str) -> bool {
    Command::new("cargo")
        .args([sub, "--version"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}
