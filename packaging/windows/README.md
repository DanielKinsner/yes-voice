# Windows Packaging

This directory holds the source-controlled unsigned Windows setup recipe for
#20. It is not a final release artifact by itself because neural weights and
runtime binaries are staged outside git.

## Verified build steps (first exercised end-to-end 2026-07-04)

```powershell
# 1. Release binaries with the neural HQ engine (#20 passthrough feature):
cargo build --release -p yesvox-gui -p yesvox-app --features neural

# 2. Stage runtime + model payloads (defaults shown; env vars override):
#    target\runtime-bundle\  onnxruntime.dll, onnxruntime_providers_shared.dll
#    target\model-bundle\    mossformer2_se_48k.onnx (the bundled-hash artifact)

# 3. Audit the staged model payloads against licenses/model-weights.tsv:
cargo xtask package-audit          # requires YESVOX_MODEL_BUNDLE_DIR or the default dir

# 4. Compile the setup (Inno Setup 6):
$env:YESVOX_PACKAGE_VERSION = "1.0.0-rc1"
ISCC.exe packaging\windows\yesvox.iss
# -> target\package\yesvox-v1-unsigned.exe
```

The recipe sets `SourceDir=..\..`, so every relative path resolves from the
repo root — run ISCC against the .iss from anywhere.

Silent install / uninstall for smoke runs (**PowerShell, not Git Bash** — MSYS
mangles `/FLAGS` into paths):

```powershell
.\yesvox-v1-unsigned.exe /VERYSILENT /SUPPRESSMSGBOXES /NORESTART
& "$env:LOCALAPPDATA\Programs\YesVox\unins000.exe" /VERYSILENT /SUPPRESSMSGBOXES /NORESTART
```

## Package-time inputs

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

Run `cargo xtask package-audit` after staging `YESVOX_MODEL_BUNDLE_DIR`. It is
safe to run on normal checkouts: without `YESVOX_MODEL_BUNDLE_DIR`, it exits 0
with an explicit skip notice. With a staged bundle, it verifies shipped hashes
and the v1.0 2 GiB model-payload ceiling before the setup recipe is built.

## Installed layout the app relies on

- `{app}\yesvox-gui.exe`, `{app}\yesvox.exe`
- `{app}\onnxruntime.dll` — found at process start
  (`configure_bundled_ort_runtime`) so ort's load-dynamic probe never hangs.
- `{app}\models\mossformer2_se_48k.onnx` — found by `default_model_path()`
  (manifest-pinned names only) and hash-checked at load (#33); a corrupt file
  degrades to the light engine with a reinstall message.
- `{app}\THIRD-PARTY-NOTICES.txt`, `{app}\licenses\...` — license records,
  including the full Apache-2.0 text for the bundled MossFormer2 weights.

`YESVOX_MODEL_PATH` / `ORT_DYLIB_PATH` still override both discoveries for
development.

## SmartScreen expectations for testers (v1.0 is unsigned)

Windows SmartScreen will flag a downloaded copy of
`yesvox-v1-unsigned.exe` ("Windows protected your PC") because v1.0 is not
code-signed — signing is the v1.1 commerce lane. Testers should click
**More info → Run anyway**. A copy built locally (no Mark-of-the-Web) installs
without the prompt. The installer is per-user (`PrivilegesRequired=lowest`):
no UAC elevation, installs under `%LOCALAPPDATA%\Programs\YesVox`, removable
via Settings → Apps or `unins000.exe`.
