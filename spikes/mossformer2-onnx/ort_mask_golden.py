#!/usr/bin/env python
"""Golden generator for the Rust `ort` adapter integration test (#10).

Runs the real MossFormer2_SE_48K ONNX graph through onnxruntime on the *exact*
features the Rust engine feeds it — the 180-dim Kaldi fbank+Δ+ΔΔ stack
(dither=0) computed on the 32768-scaled deterministic signal — and dumps a few
mask points. The Rust `ort_mask_matches_golden` test feeds the same input
through `OrtMaskModel` and asserts these values, closing the loop from Rust
features → real ONNX mask.

Requires the exported model (default ./mossformer2_se_48k.onnx, the static
256-frame export). Run:  python ort_mask_golden.py
"""

import os
import sys
import numpy as np
import torch
import torchaudio
import torchaudio.compliance.kaldi as kaldi
import onnxruntime as ort

SR = 48000
WIN_LEN = 1920
WIN_INC = 384
NUM_MELS = 60
FRAMES = 256
MODEL = os.environ.get("YESVOX_MODEL_PATH", "mossformer2_se_48k.onnx")

# Same deterministic signal as the Rust test, scaled by MAX_WAV_VALUE (32768).
n = WIN_LEN + (FRAMES - 1) * WIN_INC
i = torch.arange(n, dtype=torch.float32)
audio = 32768.0 * (
    0.4 * torch.sin(2 * np.pi * 140.0 * i / SR)
    + 0.2 * torch.sin(2 * np.pi * 700.0 * i / SR)
    + 0.15 * torch.sin(2 * np.pi * 1700.0 * i / SR)
).float()

fbanks = kaldi.fbank(
    audio.unsqueeze(0), dither=0.0,
    frame_length=WIN_LEN / SR * 1000.0, frame_shift=WIN_INC / SR * 1000.0,
    num_mel_bins=NUM_MELS, sample_frequency=SR, window_type="hamming",
)  # [T, 60]
fb_t = torch.transpose(fbanks, 0, 1)
d1 = torchaudio.functional.compute_deltas(fb_t)
d2 = torchaudio.functional.compute_deltas(d1)
feats = torch.cat(
    [fbanks, torch.transpose(d1, 0, 1), torch.transpose(d2, 0, 1)], dim=1
).unsqueeze(0).numpy().astype(np.float32)  # [1, T, 180]
T = feats.shape[1]
assert T == FRAMES, f"expected {FRAMES} frames, got {T}"

sess = ort.InferenceSession(MODEL, providers=["CPUExecutionProvider"])
in_name = sess.get_inputs()[0].name
mask = sess.run(None, {in_name: feats})[0]  # [1, T, 961]
mask = np.squeeze(mask, axis=0)             # [T, 961]
print(f"mask shape (T,bins) = {mask.shape}")
print(f"finite={np.isfinite(mask).all()} min={mask.min():.5f} max={mask.max():.5f}")
amax = np.unravel_index(np.argmax(mask), mask.shape)
print(f"argmax (t,b) = {tuple(int(v) for v in amax)} -> {mask[amax]:.5f}")
print("--- GOLDEN_POINTS (t, b, value) ---")
# STFT bin width = 25 Hz: formants 140/700/1700 Hz land near bins 6/28/68.
pts = [(int(amax[0]), int(amax[1])), (128, 6), (128, 28), (128, 68), (64, 28), (200, 68)]
for (t, b) in pts:
    print(f"({t}, {b}, {mask[t][b]:.5f}),")
print(f"sum(mask)  = {mask.sum():.4f}")
print(f"mean(mask) = {mask.mean():.6f}")
sys.exit(0)
