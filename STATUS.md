# YesVox — current state

> **Living document.** Update this whenever the app's state changes meaningfully
> (a slice lands, a decision is made, a blocker clears). It's the fast briefing
> for the next person/agent. Keep it honest — "done" means gate-green on `main`.
>
> Last updated: 2026-06-29 (after #10 — the full MossFormer2 neural engine).

## TL;DR

The **entire v1 restoration/finish core is built, tested, and green on `main`**
(Windows/macOS/Linux CI). It's a host-agnostic Rust crate (`yesvox-core`) with a
thin CLI shell (`yesvox-app`); the **egui GUI is not built yet** (that's #17/#18,
see [`docs/ui/yesvox-v1-ui-reference.md`](docs/ui/yesvox-v1-ui-reference.md) for
the visual target). The **high-quality neural denoiser (MossFormer2_SE_48K) is
now fully built and validated end-to-end** — Kaldi fbank front-end, STFT
back-end, host enhancement chain, 48 kHz resample island, the `ort` ONNX-Runtime
adapter (behind the off-by-default `neural` feature), and a windowed decoder for
the frame-locked graph; proven against the real model + runtime. Remaining work
is the quality-mode→engine binding (#11), the GUI, or human-gated (listening /
signing accounts).

## What's built (the `yesvox-core` rack)

Every module is a `VocalProcessor`; bypassed = sample-accurate identity (per-module null gate). All mechanically tested.

| Area | Module(s) | Issue | State |
|---|---|---|---|
| Open → export skeleton (WAV/FLAC, dither, never-overwrite, versioned settings) | `io`, `settings` | #1 | ✅ done |
| Whole-file analyzer + reject paths (LUFS, true-peak, hum, RT60, content guess, …) | `analysis` | #2 | ✅ done |
| Shared DSP primitives (biquads, Goertzel) + BS.1770 loudness/true-peak | `dsp`, `loudness` | #2 | ✅ done |
| De-hum (adaptive notch/comb, drift-tracking) | `modules::dehum` | #3 | ✅ done |
| Declip (constrained cubic-Hermite) | `modules::declip` | #4 | ✅ done |
| Shared transient detector + de-click/crackle/mouth-click | `modules::transient`, `declick` | #5 | ✅ done |
| De-pop (plosive, shared detector) | `modules::depop` | #6 | ✅ done |
| De-ess (level-independent, split/wide, publishes HF reduction) | `modules::deess` | #7 | ✅ done |
| Denoise engine seam + worker pool + long-file streaming + RAM preflight | `denoise` | #8 | ✅ done |
| De-reverb (light tier, tail-preserving) | `modules::dereverb` | #12 | ✅ done |
| Breath control (attenuate-not-silence, de-ess-aware) | `modules::breath` | #13 | ✅ done |
| Finish: presence/air EQ + leveler + loudness/true-peak limiter | `modules::finish` | #14 | ✅ done |
| De-bleed + mix-refusal guard | `modules::debleed` | #15 | ✅ done |
| MossFormer2 STFT/iSTFT back-end (Rust, torch-golden) | `neural::stft` | #10 | ✅ done |
| Kaldi log-mel fbank front-end (60 mel + Δ + ΔΔ, torchaudio-golden) | `neural::fbank` | #10 | ✅ done |
| Host enhancement chain + `MaskModel` seam (clearvoice-golden) | `neural::engine` | #10 | ✅ done |
| 48 kHz resample island (Kaiser-sinc) + neural `Denoiser` impl | `neural::resample` | #10 | ✅ done |
| `ort` ONNX-Runtime adapter + runtime EP negotiation (CPU fallback) | `neural::ort_model` | #10 | ✅ done |
| Windowed mask inference for the 256-frame-locked graph | `neural::chunk` | #10 | ✅ done |

## Architecture (orientation)

- **`crates/yesvox-core`** — all DSP/ML. Pure Rust + small codec deps (`hound`,
  `claxon`, `flacenc`, `serde`, `rustfft`). One trait: `VocalProcessor`
  (`prepare`/`process`/`reset`).
- **`crates/yesvox-app`** — thin CLI: `read_audio → analyze → process → export`.
- **`denoise::Denoiser` trait = the engine seam.** v1 light engine is
  `SpectralDenoiser` (STFT spectral gating). The neural MossFormer2 engine drops
  in here behind the off-by-default `neural` feature (#10).
- **`neural/`** — the batch-engine integration. The model is **feature→mask**
  (the ONNX graph emits a 961-bin **real** ratio mask; STFT/fbank/iSTFT/resample
  all live host-side in pure Rust around it). Pipeline: `resample→48k → ×32768 →
  fbank → WindowedMask(OrtMaskModel) → STFT → apply gains → iSTFT → ÷32768 →
  resample back`. Everything except the `ort` call is always-on and golden-tested;
  only `OrtMaskModel` sits behind the `neural` feature.
- **`spikes/mossformer2-onnx/`** — Python tooling (outside the cargo workspace;
  weights/onnx/logs gitignored): `export_and_verify.py` (#9 export+numerics gate),
  `fbank_golden.py` / `decode_chain_golden.py` / `ort_mask_golden.py` (bless the
  Rust goldens), `verify_dynamic.py` (documents the 256-frame lock).

## Issue status

- **Closed (done):** #1–#8, #12–#15 (rack), **#9** (ONNX export spike → GO),
  **#10** (full MossFormer2 neural engine + SR handling + EP negotiation).
- **Next, unblocked:** **#11** quality-mode→engine binding + slow-hardware
  downshift (Natural/Balanced → light `SpectralDenoiser`; Restoration HQ →
  neural `MossformerEngine` when the `neural` feature + model are present).
- **Needs Dan — cannot be automated:**
  - **#16** singing quality gate — blind listening (≥3 ears) + license-clean
    ≥12-stem reference set. Gates the singing UI; #17/#19 wait on it.
  - **#18** Advanced view + spectrogram (UI sign-off).
  - **#20** packaging/signing — Apple notarization + Windows Authenticode under
    Dan's developer identities.
  - **#21** final eval — listening on the two reference laptops.
- **GUI not started:** #17 (Standard view), #18 (Advanced view) — target layout
  in `docs/ui/`.

## #10 — done (the neural HQ engine)

1. ✅ **STFT/iSTFT back-end**, byte-faithful to `torch.stft` (= clearvoice's stft).
2. ✅ **Kaldi-fbank front-end** — 60 mels + Δ + ΔΔ, golden vs
   `torchaudio.compliance.kaldi.fbank` (dither=0; 2048-pt FFT, hamming, pre-emph
   0.97, DC-removal, log/f32-eps floor).
3. ✅ **Host enhancement chain** (`MossformerEngine` + `MaskModel` seam) — pinned
   to clearvoice's decode via a synthetic-mask golden (independently validates
   the iSTFT on a masked spectrum, partial-overlap edges included).
4. ✅ **48 kHz resample island** (hand-rolled Kaiser-windowed-sinc, no new deps)
   + the neural `Denoiser` impl (resamples non-48k in/out, fit-to-length).
5. ✅ **`ort` ONNX-Runtime adapter** behind the `neural` feature (`load-dynamic`,
   so `--features neural` needs no native download); runtime EP negotiation with
   a guaranteed CPU fallback. Validated end-to-end on the real model+runtime
   (ignored test, gated on `YESVOX_MODEL_PATH`+`ORT_DYLIB_PATH`): Rust features →
   real ONNX mask, argmax location bit-exact to the Python golden.
6. ✅ **Windowed decoder** (`WindowedMask`) — the ONNX is 256-frame-locked
   (legacy tracer baked the count into a Reshape; `verify_dynamic.py`), so longer
   signals run as overlapping 256-frame windows stitched on their central frames.

**Next (#11):** bind to quality modes (Natural/Balanced → light, Restoration HQ →
neural) with a slow-hardware downshift + Fast-mode badge.

## Build & verify

```
cargo xtask gate     # fmt --check → clippy -D warnings → test → cargo-deny
```

CI runs the identical command on Windows/macOS/Linux on every push. "Done" =
green. Pre-commit hook (`git config core.hooksPath .githooks`) runs fmt+clippy on
staged Rust. Goldens re-bless only via an explicit path (never silently).

## Key decisions & gotchas

- **License:** MossFormer2_SE_48K weights are **Apache-2.0** (verified) —
  bundleable; carry the NOTICE (plan §7.5).
- **fp16:** the blanket ONNX fp16 converter trips an internal `Cast` (no
  NaN/Inf). **Ship the fp32 graph** (accepted fallback); revisit fp16 as a perf
  win with per-op block-listing.
- **`ort` is opt-in + `load-dynamic`.** ONNX Runtime is a native dep — kept
  behind `neural` (default build/CI stay pure-Rust). `ort = 2.0.0-rc.12` with
  `load-dynamic`+`api-24`+`ndarray`+`std`+`tracing`, **no** `download-binaries`:
  `--features neural` compiles with no native download/link; the runtime
  `libonnxruntime` is found at run time via `ORT_DYLIB_PATH`. A compile-only
  `neural` CI lane keeps the feature-gated code from rotting. (`api-24` is
  required — bare default trips a compile error in ort's own VitisAI EP.)
- **Hardware accel = runtime EP negotiation, CPU guaranteed.** `OrtMaskModel`
  registers DirectML (Win) / CoreML (mac) / CUDA then CPU; absent EPs are skipped
  non-fatally (logged via `tracing`). Only the CPU path is validated here; GPU on
  real hardware is Dan's to confirm.
- **ONNX is 256-frame-locked.** The legacy tracer constant-folds the frame count
  into a MossFormer Reshape, so dynamic-T export does **not** work
  (`verify_dynamic.py`). The engine runs the fixed graph over overlapping
  256-frame windows (`neural::chunk::WindowedMask`).
- **Resampler is hand-rolled, not `rubato`.** rubato 3.0 dragged in a 6-crate
  `audioadapter` tree + a reworked buffer API; a ~120-line Kaiser-windowed-sinc
  resampler is smaller, dependency-free, and fully golden-tested (flat passband,
  ≥20 dB stopband, exact same-length contract).
- **#9 spike Python (3.13) dep pins:** `numpy>=2.2,<2.3` (numba/librosa cap) +
  `ml_dtypes>=0.5.1` (onnx needs `float4`); legacy TorchScript exporter
  (`dynamo=False`, opset 17), not the dynamo path. ONNX Runtime (Python) 1.27
  works as the `load-dynamic` runtime for ort rc.12.
- **Singing is gated.** Ships only if #16 passes; schema slot reserved either way.
