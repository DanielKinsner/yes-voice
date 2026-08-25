# Windows Packaging

This directory holds the source-controlled unsigned Windows setup recipe for
#20. It is not a final release artifact by itself because neural weights and
runtime binaries are staged outside git.

## Verified build and local smoke

```powershell
# From a clean worktree, this audits both payload bundles, runs the canonical
# gate, builds the neural release binaries, compiles the installer, performs an
# isolated install -> exact version -> neural render -> uninstall smoke, and
# writes target\package\smoke-<timestamp>\smoke-report.json.
.\packaging\windows\build-rc.ps1
```

The script defaults to `1.0.0-rc.3` and locates Inno Setup 6 from
`YESVOX_ISCC`, `PATH`, or its standard per-user install location. It refuses a
dirty worktree so the seven-character commit in the installed build stamp is
the exact source revision used for the artifact.

## Stage package inputs

```powershell
# 1. Stage the exact ONNX Runtime 1.26.0 Windows x64 release payload:
#    target\runtime-bundle\onnxruntime.dll
#    target\runtime-bundle\onnxruntime_providers_shared.dll
#    target\runtime-bundle\ONNX_RUNTIME_LICENSE.txt
#    target\runtime-bundle\ONNX_RUNTIME_THIRD_PARTY_NOTICES.txt
# Every filename and SHA-256 must match:
#    packaging\windows\onnxruntime-1.26.0-sha256.tsv

# 2. Stage the model payload:
#    target\model-bundle\mossformer2_se_48k.onnx (regenerate via
#      spikes\mossformer2-onnx\export_and_verify.py; must match the bundled
#      SHA-256 in licenses\model-weights.tsv or package-audit fails).

# 3. The build script sets both variables and runs this audit before building.
$env:YESVOX_MODEL_BUNDLE_DIR = "target\model-bundle"
$env:YESVOX_RUNTIME_BUNDLE_DIR = "target\runtime-bundle"
cargo xtask package-audit
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
SHA-256 in `licenses/model-weights.tsv`; every runtime file must match
`onnxruntime-1.26.0-sha256.tsv`; and the clean Windows VM smoke test must pass:
install, launch, render, uninstall.

Run `cargo xtask package-audit` after setting both bundle variables. It is safe
to run on normal checkouts: without `YESVOX_MODEL_BUNDLE_DIR`, it exits 0 with
an explicit skip notice. With staged bundles, it verifies the model and runtime
hashes plus the v1.0 2 GiB model-payload ceiling before setup is built.

## Installed layout the app relies on

- `{app}\yesvox-gui.exe`, `{app}\yesvox.exe`
- `{app}\onnxruntime.dll` — found at process start
  (`configure_bundled_ort_runtime`) so ort's load-dynamic probe never hangs.
- `{app}\ONNX_RUNTIME_LICENSE.txt` and
  `{app}\ONNX_RUNTIME_THIRD_PARTY_NOTICES.txt` — the license and notices copied
  verbatim from the pinned ONNX Runtime 1.26.0 Windows x64 release archive.
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
