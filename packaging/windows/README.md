# Windows Packaging

This directory holds the source-controlled unsigned Windows setup recipe for
#20. It is not a final release artifact by itself because neural weights and
runtime binaries are staged outside git.

Expected package-time inputs:

- `YESVOX_PACKAGE_VERSION`: version string for the setup metadata.
- `YESVOX_BIN_DIR`: directory containing release `yesvox-gui.exe` and
  `yesvox.exe` builds. Defaults to `target\release`.
- `YESVOX_RUNTIME_BUNDLE_DIR`: directory containing packaged runtime files such
  as ONNX Runtime DLLs. Defaults to `target\runtime-bundle`.
- `YESVOX_MODEL_BUNDLE_DIR`: directory containing approved model payloads.
  Defaults to `target\model-bundle`.

Before a package can be treated as releasable, every model file staged in
`YESVOX_MODEL_BUNDLE_DIR` must have a matching `bundled` row and shipped
SHA-256 in `licenses/model-weights.tsv`, and the clean Windows VM smoke test
must pass: install, launch, render, uninstall.
