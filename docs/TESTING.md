# Testing & engineering discipline

The canonical reference for how YesVox is verified. Mirrors the Yes-suite conventions (yes-master / yes-daw) and adds the gaps they were missing. The governing rule, from [`CLAUDE.md`](../CLAUDE.md): **every check is mechanical (exit 0/1) — Dan does not read diffs or hand-verify by ear/eye.** The only exceptions are the explicit blind-listening gates (#16 singing, #21 final eval), which are `hitl`.

## The one gate

```
cargo xtask gate
```

Runs, in order, failing fast:

1. `cargo fmt --check` — formatting is enforced, not advisory.
2. `cargo clippy --all-targets -- -D warnings` — warnings are errors on our code.
3. `cargo test` — unit + integration + the guarantee tests.
4. `cargo deny check` — code-license + advisory (`cargo-audit`) hygiene.

CI (`.github/workflows/ci.yml`) runs the **identical** command on Windows + macOS + Linux on every push and PR. A slice is "done" only when the gate is green. Fix red before stopping.

## The four guarantee gates (plan §7.6) — wired from #1

These convert the product's promises into contracts and run on every push. They must stay green through every later slice.

| Gate | Asserts | Test |
|---|---|---|
| **NULL** | engine with all modules bypassed reproduces the input within float epsilon | `tests/guarantee_null.rs` |
| **OFFLINE** | zero network syscalls between decode and export | `tests/guarantee_offline.rs` |
| **NON-DESTRUCTIVE** | input file's bytes (SHA-256) are identical before/after a full session | `tests/guarantee_nondestructive.rs` |
| **SAME-LENGTH** | output sample count == input sample count per channel (non-resampling path) | `tests/guarantee_same_length.rs` |

As modules land, each adds its own mechanical gate (per-module null, no-double-repair, loudness within ±0.3 LU, metadata round-trip, fp16 numerics, …). See the plan's acceptance-criteria matrix.

## Goldens — bless deliberately, never silently

DSP byte-identity buffers and analysis-report goldens regenerate **only** via:

```
YESVOX_BLESS=1 cargo test <golden_test>      # or: cargo xtask bless
```

An autonomous build loop must **never** make a red golden green by re-blessing it — that hides a real DSP change. Re-bless only when a tuning change is intentional, and commit the golden diff as its own `chore(golden):` commit.

## Lanes (grow as the engine grows)

- **Fast lane** — `cargo xtask gate`. Always green; what the loop runs every slice.
- **Property lane** — `proptest` over value types (format/SR/channel contract, param ranges). Added with #1's data contract.
- **Perf lane** — Criterion benches for the <60 s drop-to-export budget (plan §8.4); tracked once the denoise engines land (#8/#10).
- **Listening lane** (`hitl`) — blind A/B sign-off for the singing gate (#16) and final eval (#21). Automated tests cannot approve taste; capture the listening note before any preset-calibration change.

## Commit discipline

Small green chunks straight to `main`, never squashed, each independently green (`git bisect run cargo test` must work). Conventional-commit prefixes scoped to the ticket — `feat(#3): …`, `test(#1): …`, `chore(ci): …`. A pre-commit hook (`.githooks/pre-commit`, enabled via `git config core.hooksPath .githooks`) runs `cargo fmt --check` + `clippy` on staged Rust so red never reaches `main`.
