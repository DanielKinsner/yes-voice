# YesVox — current state

> **Living document.** Update this whenever the app's state changes meaningfully
> (a slice lands, a decision is made, a blocker clears). It's the fast briefing
> for the next person/agent. Keep it honest — "done" means gate-green on `main`.
>
> Last updated: 2026-06-29 (after the #9 spike + #10 increment 1).

## TL;DR

The **entire v1 restoration/finish core is built, tested, and green on `main`**
(Windows/macOS/Linux CI). It's a host-agnostic Rust crate (`yesvox-core`) with a
thin CLI shell (`yesvox-app`); the **egui GUI is not built yet** (that's #17/#18,
see [`docs/ui/yesvox-v1-ui-reference.md`](docs/ui/yesvox-v1-ui-reference.md) for
the visual target). The high-quality neural denoiser path is proven (ONNX export
GO) and being integrated. Remaining work is either the GUI, the neural
integration, or human-gated (listening / signing accounts).

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
| MossFormer2 STFT/iSTFT back-end (Rust, torch-golden) | `neural::stft` | #10 | 🟡 in progress |

## Architecture (orientation)

- **`crates/yesvox-core`** — all DSP/ML. Pure Rust + small codec deps (`hound`,
  `claxon`, `flacenc`, `serde`, `rustfft`). One trait: `VocalProcessor`
  (`prepare`/`process`/`reset`).
- **`crates/yesvox-app`** — thin CLI: `read_audio → analyze → process → export`.
- **`denoise::Denoiser` trait = the engine seam.** v1 light engine is
  `SpectralDenoiser` (STFT spectral gating). The neural MossFormer2 engine drops
  in here behind the off-by-default `neural` feature (#10).
- **`neural/`** — the batch-engine integration. The model is **feature→mask**;
  the STFT/fbank/iSTFT live host-side and are reimplemented in Rust around the
  ONNX graph.
- **`spikes/mossformer2-onnx/`** — the #9 export+numerics spike (Python; outside
  the cargo workspace; weights gitignored).

## Issue status

- **Closed (done):** #1–#8, #12–#15 (rack), **#9** (ONNX export spike → GO).
- **In progress:** **#10** — Rust STFT back-end landed (torch-golden); next:
  Kaldi-fbank front-end, then `ort` inference behind `neural` + 48 kHz resample.
- **Blocked on #10/#11 (engine chain):** #11 (quality-mode binding).
- **Needs Dan — cannot be automated:**
  - **#16** singing quality gate — blind listening (≥3 ears) + license-clean
    ≥12-stem reference set. Gates the singing UI; #17/#19 wait on it.
  - **#18** Advanced view + spectrogram (UI sign-off).
  - **#20** packaging/signing — Apple notarization + Windows Authenticode under
    Dan's developer identities.
  - **#21** final eval — listening on the two reference laptops.
- **GUI not started:** #17 (Standard view), #18 (Advanced view) — target layout
  in `docs/ui/`.

## #10 roadmap (the neural HQ engine)

1. ✅ **STFT/iSTFT back-end** in Rust, byte-faithful to `torch.stft`.
2. ⏭ **Kaldi-fbank front-end** — 60 mels + Δ + ΔΔ, golden-tested vs
   `torchaudio.compliance.kaldi.fbank` (dither=0 for determinism). *Fiddliest
   piece — Kaldi conventions: pre-emphasis, windowing, mel formula, framing.*
3. ⏭ **`ort` inference** behind the `neural` feature (default build stays
   pure-Rust/green) + the contained **48 kHz resample island** (`rubato`).
4. ⏭ Wire as a `Denoiser` impl; bind to Restoration-HQ quality mode (#11).

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
- **`ort` is opt-in.** ONNX Runtime is a native dep — keep it behind `neural` so
  the default build/CI stay pure-Rust and the 3-OS gate stays cheap/green.
- **#9 spike Python (3.13) dep pins:** `numpy>=2.2,<2.3` (numba/librosa cap) +
  `ml_dtypes>=0.5.1` (onnx needs `float4`); legacy TorchScript exporter
  (`dynamo=False`, opset 17), not the dynamo path. See
  `spikes/mossformer2-onnx/`.
- **Singing is gated.** Ships only if #16 passes; schema slot reserved either way.
