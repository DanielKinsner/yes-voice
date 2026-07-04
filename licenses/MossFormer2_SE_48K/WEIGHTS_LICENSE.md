# MossFormer2_SE_48K Weights License

Distribution decision: **candidate-approved for v1.0 distribution** under
`Apache-2.0`, pending selection/export of the exact shipped installer artifact
and its shipped SHA-256.

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
- Candidate source checkpoint:
  `last_best_checkpoint.pt`
- Candidate source SHA-256:
  `03692b9f773bbd6bb43b9c5a41f96b1e28affd66e13796b7bec66ad3d8b227c6`

Packaging obligations:
- Ship Apache-2.0 license text in installer notices.
- Preserve the model-card license/provenance in release notices.
- Before changing `licenses/model-weights.tsv` from `candidate` to `bundled`,
  record the exact shipped artifact hash in `bundled_sha256`.

Blocked before release bundling:
- The planned v1 payload is an exported ONNX/ORT artifact, not the ignored local
  PyTorch checkpoint by itself.
- The shipped-artifact hash is therefore intentionally `TBD`.
