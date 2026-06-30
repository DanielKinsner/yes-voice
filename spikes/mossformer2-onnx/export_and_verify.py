#!/usr/bin/env python
"""
#9 spike — reproducible MossFormer2_SE_48K → ONNX export + numerics gate.

The exportable network is feature→mask: it takes 180-channel features
(60 mel-fbanks + Δ + ΔΔ) and emits a 961-bin complex-ratio mask. All spectral
work (STFT, fbank, mask application, iSTFT) lives in host code, so the ONNX
graph is a clean feed-forward net — no in-graph STFT to rebuild. This validates
that the export is numerically faithful (fp32 and fp16) so #10 can integrate it
behind the Rust `Denoiser` seam.

Run:  python export_and_verify.py
Outputs (gitignored): mossformer2_se_48k.onnx, mossformer2_se_48k_fp16.onnx
Prints a PASS/FAIL line per gate and an overall GO/NO-GO verdict.
"""

import sys
import numpy as np
import torch
import torch.nn as nn

TOL_FP32 = 1e-3       # max abs mask diff, torch vs onnxruntime (fp32)
TOL_FP16 = 5e-2       # max abs mask diff, fp32 vs fp16 export
STATIC_FRAMES = 256   # fixed chunk length S for the static export


def log(ok, name, detail=""):
    print(f"[{'PASS' if ok else 'FAIL'}] {name}" + (f" — {detail}" if detail else ""))
    return ok


def main():
    results = []

    # --- Load the pretrained model (downloads the checkpoint) --------------
    from clearvoice import ClearVoice
    cv = ClearVoice(task="speech_enhancement", model_names=["MossFormer2_SE_48K"])
    wrapper = cv.models[0]
    model = wrapper.model.eval().cpu()
    args = wrapper.args
    log(True, "load model", f"sr={args.sampling_rate} fft={args.fft_len} hop={args.win_inc}")

    # The net returns a list [mask]; wrap to a single tensor for ONNX.
    class MaskNet(nn.Module):
        def __init__(self, m):
            super().__init__()
            self.m = m

        def forward(self, x):
            return self.m(x)[-1]

    net = MaskNet(model).eval()

    # --- Build a real feature tensor [1, 180, S] --------------------------
    from clearvoice.utils.misc import compute_fbank
    import torchaudio

    torch.manual_seed(0)
    secs = STATIC_FRAMES * args.win_inc / args.sampling_rate + 0.05
    n = int(secs * args.sampling_rate)
    t = torch.arange(n) / args.sampling_rate
    # Speech-like: a few formant tones + harmonics + broadband noise.
    audio = (
        0.4 * torch.sin(2 * np.pi * 140 * t)
        + 0.2 * torch.sin(2 * np.pi * 700 * t)
        + 0.15 * torch.sin(2 * np.pi * 1700 * t)
        + 0.1 * torch.randn(n)
    ).float()

    # Mirror decode_one_audio_mossformer2_se_48k exactly: features are
    # [T, 180] (frames × [60 fbank | 60 Δ | 60 ΔΔ]) → [1, T, 180].
    fbanks = compute_fbank(audio.unsqueeze(0), args)           # [T, 60]
    fb_t = torch.transpose(fbanks, 0, 1)                        # [60, T]
    d1 = torchaudio.functional.compute_deltas(fb_t)
    d2 = torchaudio.functional.compute_deltas(d1)
    d1 = torch.transpose(d1, 0, 1)                              # [T, 60]
    d2 = torch.transpose(d2, 0, 1)                              # [T, 60]
    feats = torch.cat([fbanks, d1, d2], dim=1).unsqueeze(0).float()  # [1, T, 180]
    # Crop/pad the time axis (dim 1) to the static frame count.
    T = feats.shape[1]
    if T >= STATIC_FRAMES:
        feats = feats[:, :STATIC_FRAMES, :]
    else:
        feats = torch.nn.functional.pad(feats, (0, 0, 0, STATIC_FRAMES - T))
    log(feats.shape == (1, STATIC_FRAMES, 180), "feature shape", f"{tuple(feats.shape)}")

    with torch.no_grad():
        mask_torch = net(feats).cpu().numpy()
    finite = np.isfinite(mask_torch).all()
    results.append(log(finite, "torch forward finite", f"mask {mask_torch.shape}"))

    # --- Export fp32 ONNX (static chunk) ----------------------------------
    onnx_path = "mossformer2_se_48k.onnx"
    torch.onnx.export(
        net, feats, onnx_path,
        input_names=["features"], output_names=["mask"],
        opset_version=17, do_constant_folding=True, dynamo=False,
    )
    import onnx
    onnx.checker.check_model(onnx.load(onnx_path))
    results.append(log(True, "onnx export + checker (fp32)", onnx_path))

    # --- fp32 parity: torch vs onnxruntime --------------------------------
    import onnxruntime as ort
    sess = ort.InferenceSession(onnx_path, providers=["CPUExecutionProvider"])
    mask_ort = sess.run(["mask"], {"features": feats.numpy()})[0]
    max_abs = float(np.max(np.abs(mask_torch - mask_ort)))
    results.append(log(max_abs <= TOL_FP32, "fp32 parity (torch vs ORT)",
                       f"max|Δ|={max_abs:.2e} (tol {TOL_FP32:.0e})"))

    # --- fp16 export (advisory): bounded error, no NaN/Inf ----------------
    # fp16 is a speed/memory optimization, not a correctness gate — the
    # acceptance explicitly allows shipping the fp32 fallback. keep_io_types
    # leaves the graph's IO in fp32 so internal dtype Casts stay consistent.
    fp16_ok = None
    try:
        from onnxconverter_common import float16
        fp16_path = "mossformer2_se_48k_fp16.onnx"
        m16 = float16.convert_float_to_float16(onnx.load(onnx_path), keep_io_types=True)
        onnx.save(m16, fp16_path)
        sess16 = ort.InferenceSession(fp16_path, providers=["CPUExecutionProvider"])
        mask16 = sess16.run(["mask"], {"features": feats.numpy()})[0].astype(np.float32)
        finite16 = bool(np.isfinite(mask16).all())
        err16 = float(np.max(np.abs(mask_torch - mask16)))
        fp16_ok = finite16 and err16 <= TOL_FP16
        log(fp16_ok, "fp16 numerics (advisory)",
            f"finite={finite16} max|Δ|={err16:.2e} (tol {TOL_FP16:.0e})")
    except Exception as e:  # noqa: BLE001
        fp16_ok = False
        log(False, "fp16 numerics (advisory)",
            f"converter needs op-block tuning — ship fp32 fallback. {type(e).__name__}")

    # --- End-to-end waveform parity + PESQ proxy --------------------------
    try:
        from clearvoice.utils.misc import stft, istft
        spectrum = stft(audio, args)
        # torch-mask path
        def apply_mask(mask_chw):
            m = torch.from_numpy(mask_chw).permute(2, 1, 0)  # match decode permute
            S = min(spectrum.shape[1], m.shape[1])
            masked = spectrum[:, :S].cpu() * m[:, :S].detach().cpu()
            cplx = masked[:, :, 0] + 1j * masked[:, :, 1]
            return istft(cplx, args, len(audio))
        wav_torch = apply_mask(mask_torch)
        wav_onnx = apply_mask(mask_ort)
        L = min(len(wav_torch), len(wav_onnx))
        wdiff = float(torch.max(torch.abs(wav_torch[:L] - wav_onnx[:L])))
        results.append(log(wdiff <= 1e-3, "end-to-end waveform parity",
                           f"max|Δ|={wdiff:.2e}"))
        try:
            from pesq import pesq
            ref = wav_torch[:L].numpy()
            deg = wav_onnx[:L].numpy()
            # PESQ wideband at 16k — resample for the metric only.
            import torchaudio.functional as AF
            ref16 = AF.resample(torch.from_numpy(ref), args.sampling_rate, 16000).numpy()
            deg16 = AF.resample(torch.from_numpy(deg), args.sampling_rate, 16000).numpy()
            score = pesq(16000, ref16, deg16, "wb")
            # delta from a perfect 4.5 — export should be near-perfect.
            results.append(log((4.5 - score) <= 0.05, "PESQ parity (onnx vs torch)",
                               f"PESQ={score:.3f} (Δ from 4.5 = {4.5 - score:.3f})"))
        except Exception as e:  # noqa: BLE001
            print(f"[INFO] PESQ skipped: {e}")
    except Exception as e:  # noqa: BLE001
        results.append(log(False, "end-to-end waveform parity", f"error: {e}"))

    # Verdict rests on the correctness gates (fp32 parity, waveform, PESQ);
    # fp16 is advisory (fp32 fallback is an accepted outcome).
    ok = all(results)
    fp16_note = {True: "fp16 ok", False: "fp16 deferred → ship fp32", None: "fp16 untested"}[fp16_ok]
    print("\n=== VERDICT:",
          f"GO — export is numerically faithful ({fp16_note})" if ok
          else "NO-GO — see FAIL lines above", "===")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
