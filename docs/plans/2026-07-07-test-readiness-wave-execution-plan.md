# Test-readiness wave — execution plan (W1–W4)

**What this is:** the decision-complete execution plan for the wave that makes
YesVox v1 ready for Dan's hand-testing (#20 clean-VM smoke, #21 real-stem
corpus, and general GUI hand-testing). Decided 2026-07-07 in a planning
session against a verified-green `cargo xtask gate` on the office PC. Serves
[`docs/STATUS.md`](../STATUS.md) "Next steps" and closes toward #20/#21.

**Drift rule:** if this plan contradicts current code/docs reality, verify
reality, follow reality, and note the discrepancy in the deviation log — do
**not** improvise a new decision.

**Executor:** a Claude Opus 4.8 session (or equivalent). Read
[`CLAUDE.md`](../../CLAUDE.md) and [`docs/product.md`](../product.md) first;
the engineering-discipline section there is binding (mechanical verification,
one canonical gate, goldens never re-blessed as a side effect).

---

## Decisions locked (D1–D10)

| # | Decision | Consequence |
|---|----------|-------------|
| D1 | Wave scope = **W1–W4 only**. The per-render diagnostics log ("W5") is **parked** — revisit only if Dan's testing shows bug reports are too thin. | No new logging/telemetry surface in this wave. |
| D2 | **Commits as small as stays green, never squash, push after every commit.** Dan is in a GitHub contribution contest — many small green commits are a *feature*. A docs file may land section-by-section; a code slice lands as build-script / API / CLI / GUI / test commits wherever each is independently green. | High commit count is correct behavior, not noise. Conventional prefixes scoped as in D9. |
| D3 | This plan lives at `docs/plans/2026-07-07-test-readiness-wave-execution-plan.md`; STATUS.md "Next steps" points at it. | Kickoff prompts reference this file, not the conversation. |
| D4 | **W1 build stamp**: CLI gets `--version`; the GUI shows a small always-visible stamp = version + short git hash (+ `-dirty` when the tree is dirty) + **active engine** (Neural / Light, sourced from `Session` state incl. `engine_note` fallback reason). Git hash falls back to `unknown` when git is unavailable at build time — the build must never fail for lack of git. | Kills stale-build ghost bugs; makes neural→light fallback visible in the GUI (today it is CLI-only — verified 2026-07-07: `yesvox-gui` never reads `Session::engine_note`). |
| D5 | **Crate versions bump from 0.0.0 to `1.0.0-rc.2`** for `yesvox-core`, `yesvox-app`, `yesvox-gui` (xtask stays 0.0.0). The W2 installer sets `YESVOX_PACKAGE_VERSION=1.0.0-rc.2` to match. (Rationale: the 07-04 installer was `1.0.0-rc1`; the rebuilt one with the stamp is rc.2.) **Dan may veto the string at Checkpoint 1** — it is user-visible. | One version string everywhere the user can read one. |
| D6 | **W2 rebuilds the installer on the current machine, after W1**, so the smoked build carries the stamp. Inno Setup 6 gets installed via winget (free software — no spend approval needed). | The artifact Dan smokes on the VM is stamp-carrying and proves the packaging recipe is machine-portable. |
| D7 | **CI minutes are exhausted** on this GitHub account. `cargo xtask gate` locally **is** the merge bar. Never push red; do not touch `.github/workflows` to "fix" CI. | Gate before every push. |
| D8 | `gh` is **not authenticated** on the office PC. No new GitHub issues are created for this wave; slice IDs are W1–W4 in this doc. | Commit scopes per D9 instead of new issue numbers. |
| D9 | Commit-scope mapping: W1 → `feat(#20)`/`test(#20)` (the stamp ships in the installer), W2 → `chore(#20)`/`fix(#20)`, W3 → `docs:`, W4 → `feat(#21)`/`test(#21)`/`docs:`. Version bump commit → `chore(#20): bump crates to 1.0.0-rc.2`. | Ticket-scoped history stays consistent with the repo convention. |
| D10 | Slice order: **W3 → W1 → Checkpoint 1 → W4 → W2 → final report.** Docs-first so Dan can grade cheaply before code lands; W1 before W2 so the installer contains the stamp; W2 last because it is environment-heavy and its output is the direct handoff to Dan. | Executor stops at Checkpoint 1 and at the end; nowhere else unless a stop trigger fires. |

---

## Lanes

**Owner lane (Dan — nothing here blocks W1–W4):**
- Clean-Windows-VM smoke of the W2 installer (#20). W3 gives him the script.
- Assemble `YESVOX_EVAL_CORPUS` from his real stems (#21). W4 gives him the recipe.
- Optional non-blocking listen ([`docs/listening-checklist.md`](../listening-checklist.md)).
- Veto window on the `1.0.0-rc.2` string at Checkpoint 1 (D5).
- (Housekeeping, any time: `gh auth login` on the office PC; decide whether to renew GitHub Actions minutes.)

**Executor lane:** everything below.

---

## Slices

### W3 — Hand-test runbook (docs only; land first)

**Deliverable:** `docs/hand-test-runbook.md`.

Structure (every step written as "do X → you should see Y"; plain English,
Dan is not a coder):

0. **What am I running** — how to read the version stamp (GUI chip and
   `yesvox --version`), and the rule: *if the stamp doesn't match what you
   expect, stop testing and rebuild — you are chasing a ghost.* (Until W1
   lands, write this section to the W1 spec in D4.)
1. **#20 clean-VM smoke script** — exact steps from
   [`packaging/windows/README.md`](../../packaging/windows/README.md):
   copy installer → SmartScreen "More info → Run anyway" expectation →
   install (per-user, no UAC) → launch GUI → import a WAV → render with the
   default preset → confirm the stamp shows **Neural** → export → play the
   export → uninstall via Settings → Apps. Explicit PASS/FAIL criteria per
   step.
2. **Standard view tour** — import, content-mode toggle, Smart Assist,
   preset gallery, each of the 9 cards (what audible/visible change to
   expect at extreme settings), Flow Amount at 0 (should null to input),
   Restoration Focus / Polish Intensity, A/B + Volume Match, reblend strip +
   difference monitor, Vocal Isolation Check verdicts, undo/redo
   (Ctrl+Z/Shift+Z/Y and toolbar), transport/seek.
3. **Advanced view tour** — module rack in canonical order, A/B/C pinned
   snapshots (loudness-matched), spectrogram, per-parameter spot checks per
   [`docs/advanced-param-coverage.md`](../advanced-param-coverage.md).
4. **Batch/queue** — build a queue, force-preset vs per-file detect, pause /
   resume / reorder, cancel mid-batch (unprocessed rows stay pending),
   retry-failed, crash-recovery (kill the app mid-batch → relaunch →
   queue restores paused).
5. **Export matrix** — WAV/FLAC × bit depths × dither, FLAC level 0 vs 8,
   metadata round-trip spot check, same-path export lock, locked-file error.
6. **Error paths** — corrupt/truncated file, >2 h file, >192 kHz file,
   zero-length file, missing/corrupt model (light-engine fallback message),
   locked output file. Each with the *friendly* message the user should see.
7. **How to report a bug** — template: version stamp, mode, preset, settings
   touched, source file facts, what you did / expected / got, timestamp.

**Ground rules:** every control named must exist in the current shell —
derive from the actual `crates/yesvox-gui` source and the #24 wiring sweep;
**do not invent features**. Where a "you should see" depends on W1 (the
stamp), write it to the D4 spec and mark it `(lands with W1)`.

**Verification (floor, not ceiling):** grep-check that every named control /
message string exists in the code; `cargo xtask gate` still green (docs-only,
but run it anyway before push). Commit granularity: one commit per major
section is welcome (D2), e.g. `docs: hand-test runbook — batch/queue section`.

### W1 — Build stamp + engine surface

**Spec:**
- A single source of truth for the stamp string in `yesvox-core` (suggested:
  a small `version` module), fed by `CARGO_PKG_VERSION` plus a build-script
  git probe (`git rev-parse --short HEAD`, `-dirty` suffix when the tree is
  dirty, literal `unknown` when git is absent). The build must succeed
  without git installed.
- `yesvox --version` (and `-V`) prints e.g. `yesvox 1.0.0-rc.2 (a1b2c3d)`.
  Keep whatever CLI-arg handling already exists; do not pull in a new arg
  parser just for this.
- GUI: a small, dim, always-visible stamp (both Standard and Advanced
  views; exact placement is executor judgment — bottom corner of the fixed
  1920×1080 canvas is the obvious spot) showing `v1.0.0-rc.2 · a1b2c3d ·
  Neural`/`· Light`. Engine text reflects the session's actual last-render
  engine, including the `Session::engine_note` fallback reason — surface the
  note (e.g. tooltip or appended short text) so a neural→light fallback is
  **visible in the GUI**, not just the CLI.
- Version bump per D5 (its own commit).

**Tests (mechanical, floor not ceiling):**
- CLI integration test: `--version` output contains `CARGO_PKG_VERSION` and
  a token matching `[0-9a-f]{7,}(-dirty)?|unknown`.
- Core unit test on the stamp-string builder (version + hash + engine label
  composition, fallback-note inclusion).
- GUI logic stays tested in core per repo convention; the painted chip is
  visual (Dan's eye at checkpoint).
- Re-check the repo's conditional-verify rules against the actual diff: the
  offline-guarantee source scan must stay green (a build-script `git`
  invocation is a process spawn, not a net symbol — but verify, don't
  assume); determinism gates are about rendered audio and must be untouched.

### CHECKPOINT 1 (stop, report, wait for Dan)

Report: W3 runbook ready to read, W1 stamp demo (`cargo run -p yesvox --
--version` output pasted; screenshot or described GUI chip), the version
string for veto (D5), the deviation log so far. Dan grades before W4/W2
start.

### W4 — Corpus kit for #21

**Spec:**
- `docs/eval-corpus.md`: how Dan assembles `YESVOX_EVAL_CORPUS` — folder
  layout the harness actually accepts (loose `.wav`/`.flac` **or**
  `manifest.tsv` — read `xtask/src/main.rs` `run_eval`/`eval_stem` for the
  real contract and document *that*, including how content mode is chosen
  per stem), where the report lands (`target/eval/report.json`), and what
  pass/fail means per stem.
- `cargo xtask eval --check` (name is executor judgment; `--check` fits the
  existing `xtask` verb style): a preflight that, **without rendering**,
  lists every stem it would run with the mode/preset it would use, and for a
  malformed corpus says exactly what is wrong (bad extension, unreadable
  manifest row, empty dir…). Keep the existing skip-safe behavior: unset
  env → exit 0 with the explicit skip notice (that behavior is load-bearing
  for the gate).
- Friendly failure text is part of the spec: Dan will run this himself.

**Tests:** xtask unit/integration tests for the preflight against temp-dir
fixtures (empty dir, loose files, manifest, malformed manifest). Gate green.

### W2 — Rebuild the installer on this machine (stamp included)

**Spec** — follow [`packaging/windows/README.md`](../../packaging/windows/README.md)
exactly; it is the verified recipe. Environment setup on this machine:
1. Install Inno Setup 6: `winget install -e --id JRSoftware.InnoSetup`
   (verify `ISCC.exe` on PATH or use its install dir explicitly).
2. Regenerate the model payload via `spikes/mossformer2-onnx`
   (`pip install -r requirements.txt`, `python export_and_verify.py` — it
   downloads the checkpoint and self-gates GO/NO-GO). Stage the fp32 ONNX as
   `target\model-bundle\mossformer2_se_48k.onnx`.
3. **Hash invariant:** the staged file's SHA-256 must equal the `bundled`
   row in `licenses/model-weights.tsv`. `cargo xtask package-audit` enforces
   this — if it fails on hash mismatch, **stop trigger S2 fires** (below).
   Never edit the tsv or the integrity manifest to make it pass.
4. Stage the ONNX Runtime DLLs (`onnxruntime.dll`,
   `onnxruntime_providers_shared.dll`) into `target\runtime-bundle\`,
   version-matched to the `ort` crate pinned in `Cargo.lock` (fetch from the
   official microsoft/onnxruntime release for that version).
5. `cargo build --release -p yesvox-gui -p yesvox-app --features neural` →
   `cargo xtask package-audit` → `$env:YESVOX_PACKAGE_VERSION="1.0.0-rc.2"` →
   `ISCC.exe packaging\windows\yesvox.iss`.
6. Local smoke (PowerShell, **not** Git Bash — MSYS mangles `/FLAGS`):
   silent install → installed CLI renders a WAV with `denoiser Neural` in
   the output **and** `--version` shows the rc.2 stamp → silent uninstall.

**Deliverable:** `target\package\yesvox-v1-unsigned.exe` (gitignored — report
its full path, size, and SHA-256 to Dan; he carries it to the VM). Commit any
recipe/doc corrections discovered along the way (small commits, D2).

**Verification:** the six steps above are the acceptance test; also re-run
`cargo xtask gate` before any push (the release/neural build must not have
perturbed anything).

### Final report

Deviation log (every judgment call, every plan-vs-reality mismatch, every
ripple beyond spec), installer path+hash, updated STATUS.md "Next steps"
(mark W-slices done, leave Dan's owner-lane items as the only open v1 work),
everything pushed.

---

## Do-not-touch (absolute)

- **Goldens** (`crates/yesvox-core/tests/goldens/*`): never regenerate; a red
  golden is a bug in your change. `YESVOX_BLESS` stays untriggered.
- `licenses/model-weights.tsv`, the load-time weight-integrity manifest, and
  `THIRD-PARTY-NOTICES.txt` — read-only this wave (S2 covers the mismatch
  case).
- `deny.toml` license exceptions; `.github/workflows` (D7).
- The guarantee tests' semantics (null, same-length, loudness, offline,
  non-destructive, batch, singing gate, determinism) — code may be touched
  only to *keep* them green, never to weaken an assertion.
- DSP behavior: nothing in this wave changes what the user **hears**. If a
  slice starts to, it has drifted — stop.
- The approved reference PNGs and the Advanced structure (#26 decision).
- No new product features beyond the D4 stamp surface and the W4 preflight
  (product.md guardrails; "W5" is parked per D1).

## Stop-and-ask triggers (the only reasons to contact Dan)

- **S1** — Checkpoint 1 and the final report (scheduled stops).
- **S2** — model-payload SHA-256 mismatch vs `licenses/model-weights.tsv`
  (means the export is not reproducing the audited artifact — a
  license/integrity question, Dan's call).
- **S3** — anything that would spend money, publish anything, or require an
  account/credential the executor doesn't have.
- **S4** — a do-not-touch item blocks a slice (report the conflict; do not
  route around it).
- **S5** — a gate failure that cannot be fixed without violating S4 or
  changing what the user hears.

Everything else: execute without asking, log it in the deviation log.

## Deviation log (required)

Maintain a running list in your reports (not committed to the repo): where
the plan was unclear or wrong, what you did about it, and any slice whose
footprint grew beyond its spec (which grows its verify obligations with it —
re-check CLAUDE.md's discipline section against the actual diff each time).

## Parking lot (explicitly NOT this wave)

- W5 per-render diagnostics log (D1).
- Code signing, merchant-of-record, any commerce plumbing (v1.1 lane).
- DFN3 or any second model in the bundle (#20 decision: unwired).
- Bandwidth extension / creative reverb / pitch-time / brush-heal (post-v1).
- User-preset store, suite handoff automation ("Save as New Preset" seams).
- CI-minutes remediation (Dan's account decision).
- Renaming/re-nesting `spikes/` or packaging refactors beyond what W2 needs.
