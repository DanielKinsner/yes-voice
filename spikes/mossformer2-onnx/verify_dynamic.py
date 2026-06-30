#!/usr/bin/env python
"""Spike result: dynamic-time ONNX export is NOT viable for MossFormer2_SE_48K
via the legacy tracer — the engine must chunk into fixed 256-frame windows (#10).

Motivation: the #9 static export fixes T=256 frames, which can't process whole
files (T varies with length). The obvious fix is `dynamic_axes` on the time
dimension. This script tries exactly that and demonstrates it FAILS: the
MossFormer block contains a Reshape whose target shape constant-folds the frame
count (256) during tracing, so the exported graph only runs at T=256:

    onnxruntime ... Reshape ... Input shape:{1,128,1024},
    requested shape:{1,128,256,1024}   # 256 is baked in; fails at T!=256

Conclusion (matches clearvoice's own decode, which feeds fixed-size windows):
the neural engine must run the fixed 256-frame ONNX over **overlapping windows**
and stitch the outputs (a 256-frame analogue of `decode_one_audio_*`'s sliding
window). A truly dynamic graph would need the dynamo exporter or hand-edited
reshapes — deferred. Writes to a throwaway path so the canonical static model
(export_and_verify.py → mossformer2_se_48k.onnx) is left untouched.

Run:  python verify_dynamic.py   # expect: dynamic export runs at T=256 only
"""

import sys
import numpy as np
import torch
import torch.nn as nn

OUT = "mossformer2_se_48k_dynamic_attempt.onnx"  # throwaway, gitignored


def main():
    from clearvoice import ClearVoice
    cv = ClearVoice(task="speech_enhancement", model_names=["MossFormer2_SE_48K"])
    model = cv.models[0].model.eval().cpu()

    class MaskNet(nn.Module):
        def __init__(self, m):
            super().__init__()
            self.m = m

        def forward(self, x):
            return self.m(x)[-1]

    net = MaskNet(model).eval()
    torch.onnx.export(
        net, torch.randn(1, 256, 180), OUT,
        input_names=["features"], output_names=["mask"],
        opset_version=17, do_constant_folding=True, dynamo=False,
        dynamic_axes={"features": {1: "frames"}, "mask": {1: "frames"}},
    )
    print(f"[info] exported with dynamic_axes: {OUT}")

    import onnxruntime as ort
    sess = ort.InferenceSession(OUT, providers=["CPUExecutionProvider"])
    in_name = sess.get_inputs()[0].name
    runs_at = []
    for t in (128, 256, 320):
        feats = np.random.RandomState(0).randn(1, t, 180).astype(np.float32)
        try:
            mask = sess.run(None, {in_name: feats})[0]
            runs_at.append(t)
            print(f"[runs ] T={t}: mask {mask.shape}")
        except Exception as e:  # noqa: BLE001
            print(f"[fails] T={t}: {type(e).__name__} (baked-in 256 reshape)")

    only_256 = runs_at == [256]
    print("\n=== RESULT:",
          "CONFIRMED -- graph is 256-frame-locked -> chunked decode required"
          if only_256 else f"unexpected: runs at {runs_at}", "===")
    # The expected/known outcome is "256 only"; exit 0 when that holds.
    return 0 if only_256 else 1


if __name__ == "__main__":
    sys.exit(main())
