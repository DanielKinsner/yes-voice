#!/usr/bin/env python
"""Golden generator for the Rust mask-apply / iSTFT chain (#10).

Proves the Rust engine's host-side reconstruction — STFT → real magnitude
ratio mask → iSTFT — reproduces clearvoice's `decode_one_audio_mossformer2_se_48k`
*exactly*, using clearvoice's own `stft`/`istft`. The mask the real model emits
is a 961-bin **real** ratio mask of shape [1, T, 961]; the decode permutes it to
[961, T, 1] and broadcast-multiplies the [961, T, 2] complex spectrum, so the
same gain scales real and imag (phase-preserving). We feed a deterministic
**synthetic** ramp mask (no model needed) so the golden is tiny and stable, and
pins exactly that path.

The MAX_WAV_VALUE (=32768) scale/unscale around the chain is a pure linear
no-op for a ratio mask (stft and istft are linear), so it is omitted here and in
the Rust chain golden; it matters only for the *features* the real model sees.

Run:  python decode_chain_golden.py
"""

import numpy as np
import torch
from clearvoice import ClearVoice
from clearvoice.utils.misc import stft, istft

SR = 48000
WIN_LEN = 1920
WIN_INC = 384

cv = ClearVoice(task="speech_enhancement", model_names=["MossFormer2_SE_48K"])
args = cv.models[0].args

N = WIN_LEN + 11 * WIN_INC  # -> 12 frames
i = torch.arange(N, dtype=torch.float32)
audio = (
    0.4 * torch.sin(2 * np.pi * 140.0 * i / SR)
    + 0.2 * torch.sin(2 * np.pi * 700.0 * i / SR)
    + 0.15 * torch.sin(2 * np.pi * 1700.0 * i / SR)
).float()

spectrum = stft(audio, args)            # [961, T, 2]
F, T, _ = spectrum.shape
print(f"bins={F} frames={T}")

# Synthetic real ratio mask [1, T, 961], deterministic ramp over frequency.
freq = torch.arange(F, dtype=torch.float32) / (F - 1)   # 0..1 over 961 bins
pred_mask = freq.view(1, 1, F).expand(1, T, F).contiguous()  # [1, T, 961]

# Mirror decode: permute -> [961, T, 1], broadcast multiply, to complex, istft.
m = pred_mask.permute(2, 1, 0)                          # [961, T, 1]
masked = spectrum * m                                   # broadcast -> [961, T, 2]
masked_complex = masked[:, :, 0] + 1j * masked[:, :, 1]
out = istft(masked_complex, args, len(audio)).numpy()   # [N]

print(f"out_len={len(out)}")
for k in [0, 1, 1000, 3000, 3072, 5000, N - 1]:
    print(f"out[{k}] = {out[k]:.6f}")
print(f"sum(out)      = {out.sum():.6f}")
print(f"energy(out)   = {np.sum(out**2):.6f}")
print(f"max(abs(out)) = {np.max(np.abs(out)):.6f}")
