# MossFormer2_SE_48K Weights License

Distribution decision: **bundled in the v1.0 Windows package** under
`Apache-2.0`.

Shipped artifact (v1.0):
- File name: `mossformer2_se_48k.onnx`
- Shipped SHA-256:
  `e1281db76f705da81641ded332752c5d95dc90f8b4eb6590e5cf7b2fdd20a51d`
- Provenance: the fp32 ONNX export produced by
  `spikes/mossformer2-onnx/export_and_verify.py` from the pinned source
  checkpoint below. This is the exact artifact the engine's host chain is
  golden-pinned against (`ort_mask_matches_golden`, #10), which is why it —
  and not the unvalidated fp16 or dynamic-axis exports — is the v1.0 payload.
- Runtime integrity: the shipped hash is pinned in the load-time
  integrity manifest (`crates/yesvox-core/src/neural/integrity.rs`, #33), so a
  corrupt install degrades to the light engine with a reinstall message.

Source evidence:
- Hugging Face model page:
  https://huggingface.co/alibabasglab/MossFormer2_SE_48K
- Pinned Hugging Face revision: `eff8c97925c8bec812af707814b3e5d777fd4503`
- Model-card front matter declares `license: apache-2.0`.
- Model card describes these as the MossFormer2_SE_48K model weights for
  48 kHz speech enhancement in ClearerVoice-Studio.
- ClearerVoice-Studio repository:
  https://github.com/modelscope/ClearerVoice-Studio
- Pinned ClearerVoice-Studio commit observed for this audit:
  `6b3774dc79c46ae8bed2a4fa5f706f0ac8c75c61`
- Source checkpoint:
  `last_best_checkpoint.pt`
- Source SHA-256:
  `03692b9f773bbd6bb43b9c5a41f96b1e28affd66e13796b7bec66ad3d8b227c6`

Packaging obligations (met):
- The full Apache-2.0 license text ships in
  `packaging/windows/THIRD-PARTY-NOTICES.txt`, which the installer displays
  and installs next to the app (enforced by
  `xtask/tests/windows_package_contract.rs`).
- The model-card license/provenance is preserved in this record, which the
  installer also installs.
- The pinned Hugging Face revision publishes no separate NOTICE file; the
  attribution above is the applicable upstream provenance.

Re-pinning rule: if the shipped ONNX is ever re-exported, record the new
artifact's SHA-256 in `licenses/model-weights.tsv` **and** the integrity
manifest in the same commit — `cargo xtask package-audit` and the weights
audit tests enforce the pairing.
