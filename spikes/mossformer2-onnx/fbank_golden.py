#!/usr/bin/env python
"""Golden generator for the Rust Kaldi-fbank front-end (#10).

Reproduces MossFormer2_SE_48K's exact feature stack — the 180-dim input the
ONNX net consumes — on a deterministic, noise-free signal so the Rust port in
`crates/yesvox-core/src/neural/fbank.rs` can be pinned byte-faithfully.

The stack (mirrors clearvoice `compute_fbank` + `decode_one_audio_*`):
    fbank = kaldi.fbank(dither=0, frame_length=40ms, frame_shift=8ms,
                        num_mel_bins=60, sample_frequency=48000,
                        window_type='hamming')          # [T, 60]
    d1 = compute_deltas(fbank.T); d2 = compute_deltas(d1)
    feats = cat([fbank, d1.T, d2.T], dim=1)             # [T, 180]

dither is forced to 0 here (and in Rust) for determinism — the model is robust
to it and the offline engine must be reproducible. Prints reference values that
get embedded in the Rust golden test.

Run:  python fbank_golden.py
"""

import numpy as np
import torch
import torchaudio
import torchaudio.compliance.kaldi as kaldi

SR = 48000
WIN_LEN = 1920   # args.win_len
WIN_INC = 384    # args.win_inc
NUM_MELS = 60

# Deterministic, noise-free speech-like signal (sum of formant tones). No RNG,
# so the golden is stable across machines/torch versions.
N = WIN_LEN + 6 * WIN_INC  # -> 7 frames
i = torch.arange(N, dtype=torch.float32)
audio = (
    0.4 * torch.sin(2 * np.pi * 140.0 * i / SR)
    + 0.2 * torch.sin(2 * np.pi * 700.0 * i / SR)
    + 0.15 * torch.sin(2 * np.pi * 1700.0 * i / SR)
).float()

frame_length = WIN_LEN / SR * 1000.0  # 40.0 ms
frame_shift = WIN_INC / SR * 1000.0   # 8.0 ms

fbanks = kaldi.fbank(
    audio.unsqueeze(0),
    dither=0.0,
    frame_length=frame_length,
    frame_shift=frame_shift,
    num_mel_bins=NUM_MELS,
    sample_frequency=SR,
    window_type="hamming",
)  # [T, 60]

fb_t = torch.transpose(fbanks, 0, 1)            # [60, T]
d1 = torchaudio.functional.compute_deltas(fb_t)
d2 = torchaudio.functional.compute_deltas(d1)
d1 = torch.transpose(d1, 0, 1)                  # [T, 60]
d2 = torch.transpose(d2, 0, 1)                  # [T, 60]
feats = torch.cat([fbanks, d1, d2], dim=1).numpy()  # [T, 180]

T = feats.shape[0]
print(f"frames (T)        = {T}")
print(f"feat dim          = {feats.shape[1]}")
print("--- point samples (frame, col) = value ---")
pts = [(0, 0), (0, 30), (0, 59),
       (3, 0), (3, 30), (3, 70), (3, 130),
       (T - 1, 59), (T - 1, 119), (T - 1, 179)]
for (f, c) in pts:
    print(f"feats[{f}][{c:3d}] = {feats[f][c]:.6f}")
print("--- block checksums ---")
print(f"sum(fbank)  = {feats[:, 0:60].sum():.6f}")
print(f"sum(delta1) = {feats[:, 60:120].sum():.6f}")
print(f"sum(delta2) = {feats[:, 120:180].sum():.6f}")
print(f"sum(all)    = {feats.sum():.6f}")
print(f"max(fbank)  = {feats[:, 0:60].max():.6f}")
print(f"min(fbank)  = {feats[:, 0:60].min():.6f}")
