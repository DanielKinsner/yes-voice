# Model Weights License Audit

Status: **started for #20 on 2026-07-04**. This is the v1.0 distribution audit
for bundled neural weights. It records source-license approval and the remaining
release blockers. It does **not** claim an installer is ready.

## Scope

YesVox v1.0 may bundle only restoration weights needed for the offline vocal
restoration product in `docs/product.md`. Source/stem-separation weights remain
out of scope for YesVox.

## Current Inventory

| Model | Status | SPDX | Source pin | Source SHA-256 | Shipped hash |
|---|---|---|---|---|---|
| DeepFilterNet3 | Candidate-approved | `MIT OR Apache-2.0` | `Rikorose/DeepFilterNet@d375b2d8309e0935d165700c91da9de862a99c31` | `c94d91f70911001c946e0fabb4aa9adc37045f45a03b56008cb0c8244cb63616` | `TBD` |
| MossFormer2_SE_48K | Candidate-approved | `Apache-2.0` | `alibabasglab/MossFormer2_SE_48K@eff8c97925c8bec812af707814b3e5d777fd4503` | `03692b9f773bbd6bb43b9c5a41f96b1e28affd66e13796b7bec66ad3d8b227c6` | `TBD` |

The machine-local MossFormer2 checkpoint under `checkpoints/` is ignored by git
and is treated as source evidence only. The release bundle still needs final
exported artifacts plus shipped hashes.

## Source Evidence

- DeepFilterNet upstream includes pretrained model weights under
  `DeepFilterNet` and `models`, and its README states the repository is
  dual-licensed under MIT or Apache-2.0.
- MossFormer2_SE_48K's Hugging Face page declares `license: apache-2.0` and
  describes the artifact as model weights for 48 kHz speech enhancement in
  ClearerVoice-Studio.
- Apache-2.0 is the SPDX identifier published by the Apache Software Foundation
  for Apache License 2.0.

Per-model records live in:

- `licenses/DeepFilterNet3/WEIGHTS_LICENSE.md`
- `licenses/MossFormer2_SE_48K/WEIGHTS_LICENSE.md`

The mechanical manifest is `licenses/model-weights.tsv`.

## Windows Package Contract

`packaging/windows/yesvox.iss` is the source-controlled unsigned Inno Setup
recipe for the v1.0 Windows package. It requires package-time staging
directories for release binaries, runtime files, and model payloads:

- `YESVOX_BIN_DIR` for `yesvox-gui.exe` and `yesvox.exe`;
- `YESVOX_RUNTIME_BUNDLE_DIR` for ONNX Runtime or other runtime files;
- `YESVOX_MODEL_BUNDLE_DIR` for the approved neural model payloads.

The recipe installs `THIRD-PARTY-NOTICES.txt`, the model-weight manifest, and
the per-model license records next to the app. `THIRD-PARTY-NOTICES.txt` is
kept synchronized with every non-comment row in `licenses/model-weights.tsv`.
It is still a release blocker to append the exact upstream license/NOTICE text
that applies to the selected shipped artifacts.

## Exclusions

The v1.0 YesVox bundle must not include:

- Demucs or other source/stem-separation weights. Source separation is outside
  YesVox product scope and belongs to the separate YesStems engine.
- RoFormer community checkpoints. Their checkpoint licensing is not stable
  enough for this v1.0 distribution lane.
- Any optional future model pack unless it gets its own manifest row, source
  pin, SPDX decision, `WEIGHTS_LICENSE.md`, and shipped-artifact hash.

## Mechanical Check

`cargo test --all` includes `xtask/tests/weights_license_audit.rs`. The test
requires every manifest row to have:

- an approved SPDX expression;
- a pinned upstream revision;
- a source artifact SHA-256;
- a readable per-model `WEIGHTS_LICENSE.md` with a distribution decision and
  source evidence.

Rows marked `bundled` additionally require a shipped-artifact SHA-256. Current
rows are intentionally `candidate` because the installer payloads are not yet
selected/exported.

`cargo test --all` also includes `xtask/tests/windows_package_contract.rs`. The
test requires the Windows setup recipe to bundle a model directory, install the
notice file, and mention every model/status/SPDX/license-record entry from
`licenses/model-weights.tsv` in the notice text.

`cargo xtask package-audit` is the local package-payload audit. It is skip-safe
when `YESVOX_MODEL_BUNDLE_DIR` is unset; when set, it requires every staged
model file's SHA-256 to match a `bundled` row, requires every `bundled` row to
have a staged file, and enforces the v1.0 2 GiB model-payload ceiling. Tests
cover skip, unmanifested staged payloads, the byte ceiling, and a matching
bundled hash.

## Release Blockers

Before #20 can close:

- select/export the exact shipped DFN3 and MossFormer2 artifacts;
- record each shipped artifact's SHA-256 in `licenses/model-weights.tsv`;
- append the required final upstream license/NOTICE text for the selected
  payloads;
- update `crates/yesvox-core/src/neural/integrity.rs` with the shipped-weight
  manifest entries;
- smoke test install -> launch -> render -> uninstall on a clean Windows VM.
