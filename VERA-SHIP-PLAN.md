# YesVox (yes-voice) — Ship Plan

*Vera, 2026-07-12 (Fable session). ⚠️ Read against `origin/main` @ 6dacc08 — the local clone on this machine (`471a136`) is **233 commits stale**; pull before working here.*

## Correction to the record first
Every status note (README, the CC memory index, product.md §12) still says YesVox is "planning stage / no application code yet." **That is stale and wrong.** origin is at **v1.0-rc.2**, a real Rust app, with a Windows unsigned installer built and locally smoke-tested (install → --version → neural render → uninstall) on 2026-07-07. It's the **second-most-complete** app in the suite, not the earliest.

## What it is
Offline vocal **restoration + polish** — denoise (MossFormer2 ONNX), de-reverb, de-click/pop, de-hum, declip, de-ess, breath control, content-aware presets with a first-class **Voiceover/Singing toggle**. Rust workspace (yesvox-core / -app CLI / -gui egui / xtask). Deep test discipline: guarantee-gates (null/offline/nondestructive/same-length/batch/loudness/neural-plumbing), fuzzing, determinism, perf budget, a real singing-preservation mechanical gate + golden files. `cargo xtask gate` (fmt→clippy -D→test→cargo-deny) is green locally.

## What stands between here and v1.0 ship
Both remaining items are **HITL — need you, not code** (tracked as GitHub issues):
1. **#20 — clean-Windows-VM installer smoke test.** Local smoke is done; a truly clean machine/VM run is the last verification.
2. **#21 — real-stem eval corpus.** The mechanical gates exist but need real vocal stems in `YESVOX_EVAL_CORPUS` (`docs/eval-corpus.md`; preview with `cargo xtask eval --check`) + the full suite round-trip **YesStems → YesVox → yes-master** on a clean machine.

Then the same money-gate as the rest: **code-signing** (shares whatever cert/accounts yes-master sets up) to avoid the SmartScreen wall.

## One engineering decision worth making
STATUS.md flags that the ONNX payload is **regenerated, not stored** on each installer rebuild → risks a re-bless every build. Recommend **storing the audited artifact** instead. Small, worth doing before release. I can do this.

## Doc hygiene (I can fix these)
product.md §12 still lists "tech stack final pick" as open though Rust+egui+ort is settled and built; README "Status" still says "no application code." These mislead any future planner (including me). Say the word and I'll reconcile the docs to reality.

## Decisions I need from you
- Do #20/#21 now, or is this repo paused while you focus on yes-master?
- Can you supply real vocal stems for the eval corpus? (That's the one thing only you have.)
- Want me to store the ONNX artifact + fix the stale docs? (both safe, additive)

## Suite context
Ships **third** (after yes-master proves the pipeline, and alongside/after yes-stems since the two chain together). It's close — the gap is your verification, not build work.
