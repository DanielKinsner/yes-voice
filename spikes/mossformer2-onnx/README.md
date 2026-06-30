# #9 spike — MossFormer2_SE_48K → ONNX export + numerics gate

**Verdict: GO.** The 48 kHz batch denoiser exports to ONNX faithfully in fp32.
Integrate it behind the Rust `Denoiser` seam (#10). fp16 is deferred — ship the
fp32 graph as the fallback.

## Run (single command)

```bash
pip install -r requirements.txt
python export_and_verify.py        # downloads the checkpoint, exports, gates
```

Outputs (gitignored): `mossformer2_se_48k.onnx`, the fp16 attempt, and a
PASS/FAIL line per gate with an overall GO/NO-GO.

## What the export looks like (the good news)

The pretrained `MossFormer2_SE_48K` network is **feature → mask**, not
waveform → waveform:

- **Input:** `[1, T, 180]` — per frame, 60 mel filter-banks + Δ + ΔΔ.
- **Output:** `[1, T, 961]` — a complex-ratio mask over the 961 STFT bins
  (`fft_len 1920` → `1920/2 + 1`).

Everything spectral — STFT (`win 1920 / hop 384`, Hamming), the mel fbank, mask
application, and the iSTFT — lives in **host code** (`clearvoice/utils/misc.py`,
`decode.py`), *outside* the graph. So the plan's hardest worry ("rebuild the
in-graph spectral transforms as convolutions") **does not apply** — the ONNX
graph is a clean feed-forward net, and #10 reimplements the STFT/fbank/iSTFT in
Rust (we already have `rustfft`).

The two real export knobs were minor:
- **Static chunk:** export at a fixed `T` (256 frames here) — matches the Rust
  streamer's fixed chunks. No dynamic axes needed.
- **Exporter:** the new dynamo exporter failed on this graph; the legacy
  TorchScript exporter (`dynamo=False`, opset 17) exports cleanly.

## Numerics (measured)

| Gate | Result | Tolerance |
|---|---|---|
| fp32 mask parity (torch vs onnxruntime) | **max\|Δ\| 4.62e-06** | ≤ 1e-3 |
| End-to-end waveform parity (full STFT→mask→iSTFT pipeline) | **max\|Δ\| 1.14e-06** | ≤ 1e-3 |
| PESQ parity (onnx pipeline vs torch pipeline) | **4.644** | Δ from 4.5 ≤ 0.05 |
| fp16 | converter mishandles an internal `Cast` node | advisory |

## Recommendation / fallback

- **GO:** integrate the **fp32** ONNX graph in #10 behind the `Denoiser` seam.
- **fp16:** deferred. The `onnxconverter-common` blanket fp16 pass breaks a
  dtype `Cast` inside the MossFormer block; it needs per-op block-listing (or an
  ORT-side fp16 conversion). Not a model defect (no NaN/Inf) — purely a
  converter detail. **The fp32 graph is the identified fallback** and is what
  ships unless/until fp16 is tuned for a speed/memory win.
- **License:** the weights are Apache-2.0 (see #9 thread) — bundleable; carry
  the NOTICE per plan §7.5.

## Caveat for #10 (the Rust side)

This spike validates the *export*. Wiring `ort` (ONNX Runtime) into the
cross-platform Rust core is the next, heavier step: ONNX Runtime is a native
dependency that changes the build from pure-Rust to "pure-Rust + a bundled
native runtime + a model file," with real implications for the
Windows/macOS/Linux CI gate, binary size (~2 GB weight budget, plan §7.1), and
`cargo-deny`. That integration is #10, tracked separately.
