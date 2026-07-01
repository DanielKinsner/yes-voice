# Testing & engineering discipline

The canonical reference for how YesVox is verified. Mirrors the Yes-suite conventions (yes-master / yes-daw) and adds the gaps they were missing. The governing rule, from [`CLAUDE.md`](../CLAUDE.md): **every check is mechanical (exit 0/1) — Dan does not read diffs or hand-verify by ear/eye.** The singing gate (#16) is mechanical: the `singing` preservation metrics run in CI. The final evaluation (#21) is the only optional listening lane, and it is non-blocking.

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
- **Neural lane** (`--features neural`) — the `ort` ONNX engine is off by default, so the fast lane never builds it. A compile-only CI job (`cargo clippy -p yesvox-core --features neural -D warnings`, `load-dynamic` ⇒ no native download) stops the feature-gated code from rotting. Its real end-to-end parity check is an **`#[ignore]`d** integration test (`ort_mask_matches_golden`) — it needs the fetched ONNX weights (`YESVOX_MODEL_PATH`) and the onnxruntime shared lib (`ORT_DYLIB_PATH`), so it runs on a developer machine, not CI. The always-on host DSP (fbank/STFT/engine/resample/chunk) **is** golden-tested in the fast lane; goldens are blessed by the `spikes/mossformer2-onnx/*_golden.py` scripts.
- **Singing lane** — `tests/singing_gate.rs` runs the synthetic sung-character preservation checks in CI: F0 cents, vibrato rate/depth, HNR, spectral-envelope cosine, and tail survival.
- **Listening lane** (`hitl`) — optional final eval (#21) on the reference laptops. It can inform preset calibration, but it is not a ship-blocking replacement for the mechanical gate.

## Commit discipline

Small green chunks straight to `main`, never squashed, each independently green (`git bisect run cargo test` must work). Conventional-commit prefixes scoped to the ticket — `feat(#3): …`, `test(#1): …`, `chore(ci): …`. A pre-commit hook (`.githooks/pre-commit`, enabled via `git config core.hooksPath .githooks`) runs `cargo fmt --check` + `clippy` on staged Rust so red never reaches `main`.
