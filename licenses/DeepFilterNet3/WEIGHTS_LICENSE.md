# DeepFilterNet3 Weights License

Distribution decision: **candidate-approved for v1.0 distribution** under
`MIT OR Apache-2.0`, pending selection of the exact shipped installer artifact
and its shipped SHA-256.

Source evidence:
- Upstream repository: https://github.com/Rikorose/DeepFilterNet
- Pinned repository commit: `d375b2d8309e0935d165700c91da9de862a99c31`
- Upstream README states the repository includes pretrained model weights under
  `DeepFilterNet` and `models`.
- Upstream README license section states DeepFilterNet is dual-licensed under
  MIT or Apache-2.0, at the user's option.
- Candidate source artifact:
  `models/DeepFilterNet3_onnx.tar.gz`
- Candidate source SHA-256:
  `c94d91f70911001c946e0fabb4aa9adc37045f45a03b56008cb0c8244cb63616`

Packaging obligations:
- Ship the selected MIT or Apache-2.0 license text in installer notices.
- Preserve upstream attribution and citation guidance in the release notices.
- Before changing `licenses/model-weights.tsv` from `candidate` to `bundled`,
  record the exact shipped artifact hash in `bundled_sha256`.

Blocked before release bundling:
- The installer has not yet selected the exact shipped DFN3 payload.
- The shipped-artifact hash is therefore intentionally `TBD`.
