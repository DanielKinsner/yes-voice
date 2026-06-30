# Vocal Polish — 4th Deep-Research Pass: Newest Tech (mid-2026), DSP Math, and Best-of-Breed Stack

Date: 2026-06-29. Scope: a premium, **offline**, standalone desktop app for vocal restoration/polish (denoise, de-click/pop, de-reverb, reverb removal, de-hum, de-ess, declip, bandwidth extension), for both **voiceover and singing**, processing already-recorded material. No cloud/upload; local ML model weights and local DSP are in scope. **Quality > real-time** (batch/offline first; near-real-time preview later). A DAW-plugin path is a secondary "nice to have."

**How this pass differs from the prior three:** the existing `consolidated-voice-restoration-research.md` is an excellent architecture brief, but it predates several mid-2026 developments and is light on (a) the *weights-licensing reality*, (b) the *ONNX-export engineering reality*, and (c) the *DSP math* you specifically asked for. This document foregrounds those three, updates the model rosters to mid-2026, pins exact library versions, and revises two of the prior plan's headline recommendations. Treat it as a delta + deep-dive on top of the consolidated brief, not a replacement of it.

Confidence convention used throughout: **[verified]** = read primary source (LICENSE file, model-card YAML, repo, paper); **[reported]** = single credible source, not independently re-verified; **[blog-grade]** = secondary/anecdotal — directional only.

---

## 0. TL;DR — what changed, and the headline picks

1. **New denoise centerpiece for batch quality: `MossFormer2_SE_48K`** (Alibaba ClearerVoice-Studio) — **Apache-2.0 on code *and* weights [verified]**, genuine **48 kHz full-band**, VoiceBank+DEMAND **PESQ ≈ 3.15–3.16**, beating DeepFilterNet (3.03) and Resemble Enhance (2.84) on the same set. The catch: **no official ONNX** (you export it yourself). Pair with `MossFormer2_SR_48K` (Apache-2.0) for bandwidth extension.
2. **DeepFilterNet3 stays — as the preview/low-footprint engine**, because it is the **only** full-band enhancer that **ships ONNX out of the box** and runs natively in Rust via `tract`. Dual **MIT OR Apache-2.0**, ~2.1 M params, 48 kHz. Use DFN3 for responsive preview + CPU-only tier; use MossFormer2_SE_48K for the heavy "final render."
3. **Separation recommendation reversed: lead with `BS PolarFormer`, not Demucs.** Demucs *code* is MIT but its **weights are author-confirmed "scientific purposes only / non-commercial"** [verified — Défossez, issue #327]. BS PolarFormer is the new SOTA-class vocal isolator (**11.0–12.0 dB vocal SDR**, 51 M params, **reported MIT code+weights**, **ONNX FP16/FP32 already published**, 103/201 MB). Verify its weight license directly before shipping.
4. **Licensing of *weights* is the dominant constraint.** Most high-SDR RoFormer community checkpoints have **no license at all** (= all-rights-reserved); MUSDB18-trained weights are non-commercial. Only **two** cleanly bundleable high-quality vocal checkpoints were confirmable: `KimberleyJSN/melbandroformer` (**MIT [verified]**) and BS PolarFormer (reported MIT). The clean long-term path is **train/commission your own separator on permissively-licensed multitracks** using the MIT architecture code.
5. **The ONNX-export discipline is the #1 engineering risk** and reshapes model choice: ONNX has an `STFT` op but **no `ISTFT` and no usable complex-number support**; diffusion/flow models must export **one sampler step** with the loop kept in host (Rust) code; **Mamba/SSM models effectively cannot be exported** today (selective-scan custom kernels — verified across multiple primary issues). Prefer conv/attention models with clean STFT front-ends.
6. **A fully permissive pitch/time stack exists now:** `SwiftF0` (MIT, ONNX-native F0) + `Signalsmith Stretch` (MIT, formant-aware pitch/time) + `WORLD` (modified-BSD, patent-free formant-preserving resynthesis). Buy Rubber Band (£590) or license élastique only if you want the absolute top vocal-shift fidelity.
7. **Bandwidth extension that's actually license-clean: `AP-BWE`** (MIT code+weights, extends to 48 kHz, all-convolutional → ONNX-friendly, ~292× real-time). It replaces AudioSR, which remains **commercially blocked** (CC-BY-NC AudioLDM lineage).
8. **Stack:** the prior Rust + `ort` + Tauri lean is still defensible, but with two caveats now sharpened: for the **heavy interactive spectrogram editor**, **egui (Rust-native, Rerun precedent)** is a lower-risk renderer than Tauri's WebView (WebGPU-on-WKWebView is unconfirmed in mid-2026); and **`ort` is still 2.0.0-rc** (pin it, disable build-time binary download). If a DAW plugin is near-term, go **C++/JUCE** (or a CLAP-first core) instead.

---

# Part I — The newest tech (mid-2026), by problem class

For the deterministic modules (de-click, declip, de-hum, de-ess, de-pop, reverb synthesis) the *methods* haven't changed — the math is in **Part III**. This part covers where ML has moved.

## 1. Denoising / speech enhancement

The competitive picture for **commercially-usable, offline, full-band** denoisers in mid-2026:

| Model | License (code / weights) | SR | Params | ONNX | Quality (VB+DEMAND PESQ) | Verdict |
|---|---|---|---|---:|---|---|
| **MossFormer2_SE_48K** (ClearerVoice) | Apache-2.0 / **Apache-2.0** [verified] | **48 kHz** | tens of M (undisclosed) | not shipped → export yourself | **3.15–3.16** | **Best full-band quality pick** |
| **DeepFilterNet3** | MIT∨Apache / same [verified] | **48 kHz** | ~2.1 M | **ships ONNX** ✅ | ~3.0 (optimized for low-distortion, not peak PESQ) | **Preview / CPU tier / tract-native** |
| MossFormerGAN_SE_16K | Apache-2.0 / Apache-2.0 | 16 kHz | undisclosed | no | **3.47** (highest in family) | Great but wrong band unless mid-stage |
| MP-SENet | MIT / MIT [verified] | 16 kHz | ~2.05 M | export feasible | **3.60** (among highest published) | Quality reference; band-extend after |
| GTCRN | MIT / MIT [verified] | 16 kHz | **48.2 K** | **ships ONNX** (sherpa-onnx) ✅ | 2.87 (modest) | Ultra-light fallback tier only |
| FRCRN_SE_16K | Apache-2.0 | 16 kHz | undisclosed | no | 3.23 | Solid 16 kHz option |

**Key facts and why they matter:**

- **ClearerVoice-Studio licensing is genuinely clean** — the agent read the repo `LICENSE` and every weights model-card YAML byte-for-byte: Apache-2.0 with **no** non-commercial rider on code *or* weights [verified, raw cards]. The one caveat is undisclosed "private" training data (a normal, non-contractual risk; the Apache grant on the weights still stands). The intended product pipeline is even spelled out on the SR card: **denoise (`SE_48K`) → super-resolve (`SR_48K`)** — which maps exactly onto "vocal polish." Source: [ClearerVoice-Studio](https://github.com/modelscope/ClearerVoice-Studio), [MossFormer2_SE_48K card](https://huggingface.co/alibabasglab/MossFormer2_SE_48K), [Interspeech 2025 paper (arXiv 2506.19398)](https://arxiv.org/html/2506.19398v1).
- **The MossFormer2 ONNX gap is the main integration cost.** Multiple open, unanswered issues (#29/#93/#106/#114) ask for ONNX; none resolved. A community MLX export exists (`starkdmi/MossFormer2-SE`, fp32/16/int8, ~30× real-time on M4) — *proof the graph is exportable/quantizable* — but it's Apple-only MLX, not ONNX. **Budget engineering time to own the `torch.onnx.export` path** (see Part IV §2 for the techniques).
- **DeepFilterNet3 remains uniquely deployable.** It is the current release (there is **no DeepFilterNet 4** as of mid-2026 [verified via search]), ships ONNX *and* a Rust `libdeepfilter`/`tract` path, and is the lowest-risk way to get a 48 kHz neural denoiser running natively. Use it for the preview chain and the CPU-only tier even though its peak PESQ trails MossFormer2/MP-SENet — it is tuned for *low-distortion, voice-preserving* denoise, which is exactly right for a non-destructive preview. Source: [DeepFilterNet repo](https://github.com/Rikorose/DeepFilterNet) (`models/` contains ONNX variants).

**The research frontier (track, don't ship yet):**
- **URGENT Challenge** is the field's universal-SE benchmark now that the **Microsoft DNS Challenge ended at its 5th edition (ICASSP 2023)** [verified]. URGENT 2025 (Interspeech) finding: the **top system was purely *discriminative*** (a ~102 M sub-band RNN), while most competitive systems were hybrid; URGENT 2026 (ICASSP) is dominated by **hybrid discriminative+generative** designs. The *winning* systems are unreleased research artifacts, but the **official baselines are Apache-2.0 with HF weights** — `BSRNN`, `BSRNN-Flow/FlowSE`, `TF-GridNet` — a clean, current reference architecture if you ever train your own. Source: [URGENT 2025 (arXiv 2505.23212)](https://arxiv.org/html/2505.23212v2), [URGENT 2026 track-1 baselines (Apache-2.0)](https://github.com/urgent-challenge/urgent2026_challenge_track1).
- **Mamba / state-space enhancers (SEMamba, MambAttention, USEMamba, Mamba-SEUNet, SEMamba++) are an export trap.** Code is usually MIT, and some reach high PESQ (Mamba-SEUNet 3.59), but **(a)** they are almost all **16 kHz** (only USEMamba is 48 kHz/SFI, and it has no public code), **(b)** their **weights are unconfirmed / SEMamba's own artifacts are stamped "non-commercial research,"** and **(c)** the selective-scan (S6) operator **has no working ONNX export** — verified across `state-spaces/mamba`#200 (open), PyTorch#146835, MambaIR#105 (silent numerical errors), and MambaLite-Micro (arXiv 2509.05488), which had to **reimplement the scan in C** because it "lacks an ONNX export path." **Recommendation: do not bet the product on Mamba.** If you want the recurrent-efficiency benefits without the export wall, `xLSTM-SENet` exports far more cleanly. Sources: [mamba#200](https://github.com/state-spaces/mamba/issues/200), [MambaLite-Micro (arXiv 2509.05488)](https://arxiv.org/html/2509.05488v1).
- **One-step generative SE** (Schrödinger-Bridge Mamba, MeanSE mean-flow, FlowSE NFE≈5) is closing the quality/speed gap, but licenses/weights are mostly unreleased. Watch.

**Classical baselines** (spectral subtraction, Wiener, log-MMSE) remain useful as a CPU-only fallback and for surgical broadband hiss — math in **Part III §1–5**.

## 2. Dereverberation

Still the highest-value premium feature, and still best as a **hybrid**:

- **WPE (Weighted Prediction Error)** for transparent light-to-moderate dereverb — interpretable, low-distortion, multichannel-friendly (math in **Part III §8**). This is your "safe" tier.
- **Diffusion / Schrödinger-bridge** for hard single-channel cases: **SGMSE+ / StoRM** (sp-uhh, **MIT code**; dedicated dereverb checkpoints incl. 48 kHz EARS-Reverb). Generative methods specifically win on "non-additive" distortions like dereverb and bandwidth extension. Run them only in the offline "max quality" render path.
- **The few-step revolution makes generative dereverb practical offline.** New 2025–26 models cut the network-function-evaluation count from ~30–60 down to **1–5 steps**: **`MeanFlowSE`** (one-step MeanFlow, **MIT code / CC-BY-4.0 weights [verified]** — the cleanest license combo, NCSN++ backbone = easiest ONNX, but 16 kHz/denoise-focused), **SB-UFOGen** (1-step Schrödinger-bridge, denoise *and* dereverb, 48 kHz-capable — but unreleased, Supertone-affiliated), **ROSE-CD** (1-step consistency distillation that beats its 30-step teacher — unreleased). **Export pattern for all of these:** keep STFT/iSTFT *outside* the graph, export only the score/velocity network, and **run the N-step sampler loop in host (Rust) code**; the NCSN++ net itself exports cleanly.
- **Watch the license, not just the quality:** the Hamburg streaming successors to StoRM — **`StreamFM`** (Dec 2025, causal flow-matching, 5-step) — ship weights but are **AGPL-3.0 (commercially hostile)** and rely on `torch.compile`/CUDA-graphs (ONNX-hard). Microsoft's 20 ms-latency restoration is a great *blueprint* (tiny 6 M ConvGLU-UNet, 5-step) but has **no released code/weights**. Treat these as the future low-latency monitoring tier, not v1 dependencies.
- **Singing dereverb training data:** the **ReverbFX / Singing-ReverbFX** RIR set (sp-uhh) — RIRs captured from reverb *plugins*, shown to outperform natural RIRs for sung vocals. **Dataset license is contested (CC0 vs CC-BY-NC) — confirm with the authors before commercial use.** A useful 2026 training insight from NVIDIA's RE-USE work: **train dereverb against *time-shifted anechoic clean* targets, not early-reflected speech** (the latter hurts perceptual quality/ASR). Sources: [SGMSE](https://github.com/sp-uhh/sgmse), [ReverbFX (arXiv 2505.20533)](https://arxiv.org/abs/2505.20533), [RE-USE (arXiv 2603.02641)](https://arxiv.org/abs/2603.02641).

## 3. Bandwidth extension / general restoration

Restoring HF lost to telephone/MP3/old media, and joint multi-distortion repair:

- **`AP-BWE` — the clean commercial answer [verified MIT code + MIT weights].** All-convolutional dual-stream GAN that explicitly recovers HF **amplitude *and* phase** in parallel (no vocoder), native **48 kHz** configs (8/12/16/24k→48k), **~18× real-time on a single CPU**, only ~18 GFLOPs (≈80–130× lighter than NU-Wave2/UDM+). Uniquely it ships a **dedicated `weights_LICENSE.txt` granting free commercial use** [verified] — the cleanest dual-MIT result found. All-conv = **the most ONNX-friendly** BWE model (the only complex/STFT ops are in the training-only discriminators). Speech-only (VCTK-trained) → fine-tune or pair with a separator for music/singing. This is what lets you avoid **AudioSR** (CC-BY-NC, blocked). Source: [AP-BWE repo (MIT code+weights)](https://github.com/yxlu-0102/AP-BWE), [paper (arXiv 2401.06387)](https://arxiv.org/abs/2401.06387).
- **`UniverSR` — higher-quality generative BWE tier.** **MIT code / CC-BY-4.0 weights [verified]**, **vocoder-free flow matching directly in the complex STFT domain** (no BigVGAN dependency), 8/12/16/24k→**48 kHz**, ~4 ODE steps, unified across speech/music/SFX; reports beating AudioSR/NVSR/FlowHigh/AP-BWE. Honor CC-BY attribution. Source: [UniverSR (arXiv 2510.00771)](https://arxiv.org/abs/2510.00771).
- **`LavaSR` — Apache-2.0 baseline that actually ships ONNX.** Vocos-based BWE+refiner (not diffusion), **Apache-2.0 code *and* weights [verified]**, community **ONNX builds with DirectML/NPU**, 8–48k→**48 kHz**, ~50 MB, single-pass, ~50× real-time CPU, optional denoise flag. The one entry with an end-to-end clean deployment story today. Source: LavaSR (GitHub `ysharma3501/LavaSR`, HF `YatharthS/LavaSR`).
- **`MossFormer2_SR_48K`** (Apache-2.0) — speech super-resolution to 48 kHz (HiFi-SR architecture); the natural partner to `MossFormer2_SE_48K`. **Designed for clean input → run denoise first.** Other BSD-code options: `mdctGAN` (BSD, but a heavy 415 MB generator) and `NU-Wave2` (BSD-3, tiny 1.7M but 8-step diffusion; weights license unstated → retrain).
- **`Resemble Enhance`** (MIT, weights in-repo) — denoise + enhancement/bandwidth extension at 44.1 kHz; clean for commercial use; good general "one-click polish" for spoken voice.
- **`VoiceFixer`** — joint denoise/dereverb/declip/super-res; **code MIT but weights license is a blank template** → confirm in writing before bundling.
- **Music/vocal-specific restoration:** **A2SB** (NVIDIA, 44.1 kHz music BWE + inpainting) and **Latent Bridge Models** (any→48/192 kHz, music-capable, SOTA on Song-Describer) are the quality frontier — but **A2SB is NVIDIA-NonCommercial** and LBM's weight license needs confirmation. **Apollo** (ICASSP 2025) repairs codec/separation artifacts on stems but its **weights are CC-BY-SA 4.0** (copyleft — legal review before bundling in a closed product). Sources: [A2SB](https://github.com/NVIDIA/diffusion-audio-restoration), [LBM (arXiv 2509.17609)](https://arxiv.org/abs/2509.17609), [Apollo (arXiv 2409.08514)](https://arxiv.org/abs/2409.08514).
- **Universal restoration foundation models — the "MIT code / non-commercial weights" trap is pervasive.** Almost every *newest/best* universal restorer is blocked at the weights level: **AnyEnhance** full 363 M model (CC-BY-NC-ND + largely unreleased), **Amphion Metis-SE** (MIT toolkit / CC-BY-NC weights), **NVIDIA RE-USE** (technically ideal — 9.6 M, SFI 8–48 kHz, ONNX-friendly regression — but NSCLv1 **non-commercial**), **Miipher-2** (Google, fully closed), and **VoiceBridge** (48 kHz, one-step, *best quality on paper* — but **no code/weights released yet**; watch closely). **What you can actually ship today (open weights, permissive):** `VoiceFixer`/`VoiceFixer2` (MIT, 44.1 kHz, dated but deployable), `Resemble Enhance` (MIT, 44.1 kHz), and the **AnyEnhance MIT *challenge baseline*** (`anyenhance-v1-ccf-aatc`, 82 M, 44.1 kHz, singing-capable — a legally-bundleable cut-down of the blocked full model). **UniPASE** (Apr 2026, 545.7 M, up to 48 kHz, **CC-BY-4.0 code+weights**, WavLM+Vocos) is the most capable *permissive* open universal restorer — heavy, offline-only, worth a benchmark. Note: token/codec masked-generative and LM-style restorers (AnyEnhance, Metis, GenSE, Miipher) are **hard to export to ONNX** (iterative discrete sampling + RVQ codec) — ship them as bundled PyTorch with offline iterative decoding, or prefer regression/UNet models. Sources: [VoiceBridge (arXiv 2509.25275)](https://arxiv.org/abs/2509.25275), [RE-USE (HF, NSCLv1)](https://huggingface.co/nvidia/RE-USE), [Metis (HF, CC-BY-NC)](https://huggingface.co/amphion/Metis), [UniPASE (arXiv 2604.14606)](https://arxiv.org/abs/2604.14606).

## 4. Music source separation (vocal isolation)

For "sung vocal inside a full mix," separate first, then restore the stem. **Two SDR scales appear in the literature — do not cross-compare them:** academic **MUSDB18-HQ museval SDR** (top vocals ~10–11 dB) vs **MVSep "Multisong" SDR** (runs ~1 dB higher). Numbers below note which scale.

| Model | Arch | Vocal SDR | Params | License (code / weights) | ONNX | Verdict |
|---|---|---|---:|---|---|---|
| **BS PolarFormer** | BS-RoFormer + Polar PE | **11.0 (Multisong); up to 12.0 (MVSep 124-band)** | 51 M | reported **MIT / MIT** | **shipped FP32 201 MB / FP16 103 MB** ✅ | **Lead pick — verify weight license** |
| MelBand RoFormer (KimberleyJSN) | Mel-Band RoFormer | 10.98 (Multisong) | ~ | code MIT / **weights MIT [verified]** | exportable | **The one fully-confirmed clean checkpoint** |
| SCNet-large / XL | SCNet | 10.86 (academic) / 11.1 (Multisong, XL-IHF) | 41 M / — | **MIT code** / weights unstated | community WASM exists | Compact 4-stem; verify weights |
| BS-RoFormer (viperx) | BS-RoFormer | 10.87 (Multisong) | ~ | code MIT / **no license** | yes | Popular but **non-redistributable** |
| Demucs v4 / HTDemucs | hybrid transformer | ~9 (academic) | ~ | **MIT code / weights NON-COMMERCIAL** [verified] | export solved (2026) | **Do NOT bundle weights** |

**The decisive findings:**
- **`BS PolarFormer` is the new lead** — highest vocal SDR among *bundleable* models, MIT (reported), and **ONNX FP16/FP32 already exported and published** (FP32 ≈ 1e-7 mask error vs PyTorch). 44.1 kHz stereo, 51 M params. Source: [HF bgkb/bs_polarformer](https://huggingface.co/bgkb/bs_polarformer), [ZFTurbo model zoo](https://github.com/ZFTurbo/Music-Source-Separation-Training/blob/main/docs/pretrained_models.md), [MVSep algo 128](https://mvsep.com/algorithms/128).
- **The licensing reality (critical):** a dedicated investigation found that **architecture *code* is almost universally MIT** (lucidrains BS/Mel-RoFormer, ZFTurbo MSST, SCNet, UVR GUI, Demucs) but **the high-quality *weights* are the problem.** Demucs weights are author-confirmed research-only [Défossez, [demucs#327](https://github.com/facebookresearch/demucs/issues/327)]; **20+ surveyed RoFormer HF checkpoints (viperx, anvuew, Gabox, unwa, the entire UVR model catalog, the ZFTurbo zoo) returned `license: None`** = all-rights-reserved by default; MUSDB18 itself is non-commercial (MedleyDB CC-BY-NC-SA lineage), tainting any model trained on it. **Only `KimberleyJSN/melbandroformer` was independently confirmed MIT.** The "honor the MIT license / give credit" line in UVR is a credit *request*, not a redistribution grant. Source: [MUSDB18 terms](https://sigsep.github.io/datasets/musdb.html), [KimberleyJSN card (MIT)](https://huggingface.co/KimberleyJSN/melbandroformer).
- **Cleanest long-term path:** train (or commission) a Mel-Band/BS-RoFormer on **owned or permissively-licensed multitrack data only** (explicitly excluding MUSDB18/MedleyDB-NC) using the MIT architecture code — then you own the weights outright and eliminate both the no-license and MUSDB-taint risks.
- **Stem restoration post-pass:** **Apollo** repairs separation/codec artifacts (CC-BY-SA weights — review), or watch **DTT-BSR** (2026) as a non-copyleft alternative.

## 5. Singing-specific restoration

Singing is a distinct domain, now with hard evidence:

- **`SingVERSE` (Sep 2025) is the key benchmark** — 3,929 real-world paired clips, 19 acoustic scenarios, 44.1 kHz, **CC-BY-4.0**, evaluation-only. Its headline result: **speech-enhancement models degrade significantly on singing — statistically significant drops in 22 of 27 comparisons** (wider pitch range + stable extended harmonics break speech-tuned models). It also quantifies a quality-vs-intelligibility tradeoff: aggressive generative models raise perceptual scores but destroy lyric content (WER blows up). **This is the empirical justification for a dedicated singing path**, and the right eval set for it. Source: [SingVERSE (arXiv 2509.20969)](https://arxiv.org/abs/2509.20969), [dataset](https://huggingface.co/datasets/amphion/SingVERSE).
- **`SRS` (Smule Renaissance Small) remains the centerpiece** — **MIT code + weights [verified]**, 10.4 M params, **48 kHz**, complex-STFT single-stage, 10.5× real-time on an iPhone 12 CPU. On the **Extreme Degradation Bench (EDB)** it surpasses all open-source baselines on singing and matches commercial systems. Conventional ops (band-split STFT + temporal-conv + SwiGLU) → should be ONNX-traceable. **This is the only singing-trained, commercially-licensable, tiny, offline model that ticks every box.** Source: [SRS (arXiv 2510.21659)](https://arxiv.org/abs/2510.21659), [HF card (MIT)](https://huggingface.co/smulelabs/Smule-Renaissance-Small).
- **`AnyEnhance` is the most capable singing-capable universal model but is BLOCKED** — **CC-BY-NC-ND-4.0**, 363 M params, offline-only. Excellent results (singing SIM 0.915, denoise+dereverb+declip+SR+TSE in one model with a self-critic loop), but the license kills it for a paid product unless you license commercially or retrain the architecture. Source: [AnyEnhance (arXiv 2501.15417)](https://arxiv.org/abs/2501.15417).
- **No new singing-only dereverb *model*** shipped in 2025–26; it's handled inside the universal models or trained with ReverbFX RIRs. A Jan-2026 shallow-diffusion model repairs **pitch-shift artifacts** specifically (relevant only if you add transpose features; no license yet).
- **Expectation-setting:** v1 will be markedly better on **voiceover and dry-vocal cleanup** than on **full-mix singing restoration**, unless you invest in singing-specific data and the separate evaluation set (SingVERSE + EDB).

## 6. Pitch & time (for the singing/creative tier)

A fully permissive stack is now available:

- **F0 estimation — `SwiftF0` (primary).** **MIT code + MIT weights [verified], ONNX-native** (the model *is* an ONNX file), 95.8 K params, ~42× faster than CREPE, SOTA noise-robustness (91.8% harmonic mean at 10 dB SNR vs CREPE 79%). On clean solo singing RMVPE still edges it (MIR-1K 95.0 vs 96.0), so: use **`FCPE`** (MIT code; 96.8% RPA on MIR-1K — best small-model singing accuracy; weights license unconfirmed → export from MIT code yourself) when singing accuracy is paramount, and **`RMVPE`** only for tracking vocals bleeding through un-separated mixes. **Avoid `PESTO` (LGPL-3.0)** for a proprietary binary. Source: [SwiftF0 (arXiv 2508.18440)](https://arxiv.org/abs/2508.18440), [FCPE (arXiv 2509.15140)](https://arxiv.org/abs/2509.15140).
- **Pitch-shift / time-stretch — `Signalsmith Stretch` (primary).** **MIT**, C++11 header-only, **formant-aware** (`setFormantBase()` takes an F0 estimate — wire SwiftF0 into it), WASM/Python/Rust bindings, already vendored into Qt 6. For top-tier vocal fidelity on large shifts, either **buy Rubber Band** (now public flat pricing: **£590 standard** w/ attribution, £1,490 small-publisher non-attribution; GPL build can't ship on App Stores) or license **z-plane élastique Pro** (proprietary per-unit royalty SDK, industry-reference quality). Source: [Signalsmith Stretch](https://github.com/Signalsmith-Audio/signalsmith-stretch), [Rubber Band pricing](https://breakfastquay.com/rubberband/license.html).
- **Formant-preserving resynthesis / gentle correction — `WORLD`** (modified-BSD, explicitly patent-free): Harvest F0 + CheapTrick envelope + D4C aperiodicity natively decouples pitch and formants — ideal for monophonic pitch correction. Note the classic result that formant shift is negligible within ±100 cents, so for gentle auto-tune you often don't need formant preservation at all; reserve WORLD/Signalsmith/élastique for larger transpositions. Source: [WORLD](https://github.com/mmorise/World).
- **Neural pitch correction** (Neurodyne, BERT-APC, 2025–26) — promising "infer-the-intended-note" auto-tune, but **no released code/weights yet**. Watch-list only.

## 7. Deterministic DSP modules

De-click/de-crackle (AR/LSAR), declip (SPADE), de-hum (adaptive notch/comb), de-ess (sidechain dynamics), de-pop (localized linear-phase HPF), and reverb synthesis (FDN/convolution) remain **pure-DSP, no-ML** and are your zero-license-risk differentiators. The rigorous math is in **Part III**. The competitive landscape confirms this split: even iZotope RX keeps De-click, De-hum, and Spectral Repair classical while going neural for dialogue isolate/de-reverb/separation.

---

# Part II — The licensing reality (the gating constraint)

The capability exists; **the right to ship it is the bottleneck.** Verify every dependency at **both** code and weights level before locking the stack. Master table (✅ bundleable in a paid closed-source app, ⚠️ verify, ❌ blocked):

| Component | Role | Code | Weights | Ship? |
|---|---|---|---|:--:|
| MossFormer2_SE_48K / SR_48K | Denoise / BWE (48 kHz) | Apache-2.0 | **Apache-2.0** [verified] | ✅ |
| DeepFilterNet3 | Denoise preview (48 kHz, ONNX) | MIT∨Apache | MIT∨Apache [verified] | ✅ |
| GTCRN | Ultra-light denoise (16 kHz, ONNX) | MIT | MIT [verified] | ✅ |
| MP-SENet | Denoise quality ref (16 kHz) | MIT | MIT [verified] | ✅ |
| Resemble Enhance | General polish / BWE (44.1 kHz) | MIT | MIT | ✅ |
| AP-BWE | Bandwidth extension (48 kHz, ONNX-friendly) | MIT | **MIT** (dedicated weights license) | ✅ |
| UniverSR | Generative BWE (48 kHz, vocoder-free) | MIT | CC-BY-4.0 | ✅ (attribution) |
| LavaSR | BWE/restore (48 kHz, **ships ONNX**) | Apache-2.0 | Apache-2.0 | ✅ |
| MeanFlowSE | 1-step generative denoise (ONNX-friendly) | MIT | CC-BY-4.0 | ✅ (16 kHz) |
| VoiceFixer / VoiceFixer2 | Multi-distortion restore (44.1 kHz) | MIT | MIT | ✅ (dated) |
| AnyEnhance *challenge baseline* (82 M) | Universal, singing-capable | MIT | MIT | ✅ (cut-down model) |
| SGMSE+/StoRM | Diffusion denoise/dereverb (max-quality) | MIT | check checkpoints | ⚠️ |
| StreamFM (Hamburg) | Streaming flow-matching restore | **AGPL-3.0** | AGPL-3.0 | ❌ copyleft |
| NVIDIA RE-USE | Universal SFI restore (9.6 M, ONNX-friendly) | — | **NSCLv1 non-commercial** | ❌ |
| VoiceBridge | One-step 48 kHz universal (SOTA on paper) | unreleased | unreleased | ⏳ watch |
| SRS (Smule Renaissance Small) | Singing restoration (48 kHz) | MIT | **MIT** [verified] | ✅ |
| BS PolarFormer | Vocal separation (ONNX shipped) | MIT [reported] | MIT [reported] | ⚠️ verify card |
| KimberleyJSN MelBand RoFormer | Vocal separation | MIT | **MIT** [verified] | ✅ |
| SCNet-large/XL | Vocal/4-stem separation | MIT | unstated | ⚠️ |
| Demucs v4 / HTDemucs | Vocal separation | MIT | **non-commercial** [verified] | ❌ weights |
| viperx / UVR / ZFTurbo-zoo checkpoints | Separation | (MIT) | **no license** | ❌ until permission |
| Apollo | Stem/codec restoration | open | **CC-BY-SA 4.0** | ⚠️ copyleft |
| SwiftF0 | F0 estimation (ONNX-native) | MIT | MIT [verified] | ✅ |
| FCPE | F0 (best singing) | MIT | unstated | ⚠️ export from code |
| RMVPE / Basic Pitch | F0 / pitch-MIDI | Apache-2.0 | Apache-2.0 | ✅ |
| Signalsmith Stretch | Pitch/time + formants | MIT | n/a | ✅ |
| WORLD | Formant-preserving resynthesis | modified-BSD | n/a | ✅ |
| Rubber Band | Pitch/time (premium) | GPL **or** commercial | n/a | ⚠️ buy £590+ |
| AnyEnhance | Universal (singing) | CC-BY-NC-ND | CC-BY-NC-ND | ❌ |
| A2SB | Music BWE/inpaint | NVIDIA-NC | NVIDIA-NC | ❌ |
| AudioSR | Audio super-res | — | CC-BY-NC (AudioLDM) | ❌ |
| UniPASE | Universal restoration | CC-BY-4.0 | CC-BY-4.0 | ✅ (heavy) |
| Mamba SE (SEMamba etc.) | Denoise | usually MIT | unconfirmed/NC-flagged | ❌ (+ no ONNX) |
| ort / ONNX Runtime | Inference | MIT | n/a | ✅ |
| tract / candle / cpal / symphonia(MPL-2.0) / rubato / realfft / fundsp | Engine + DSP | permissive | n/a | ✅ |
| JUCE | Plugin/standalone framework | AGPLv3 **or** commercial | n/a | ⚠️ buy Indie/Pro |

**Operating rule:** pin exact commits/revisions and **archive the LICENSE file + HF model-card YAML** for every shipped model, because repos get reorganized and cards get edited. A "license: mit" claim that rests only on a card (e.g., a separator whose training data is undocumented) carries residual MUSDB-taint risk — prefer models with both a permissive license *and* disclosed permissive training data, or train your own.

---

# Part III — DSP Math Reference

A rigorous, implementation-oriented reference for the deterministic modules. Notation: `x[n]` clean signal, `d[n]` noise, `y[n] = x[n] + d[n]` observation; `Y(t,f)` STFT at frame `t`, bin `f`; `f_s` sample rate. Every classical module reduces to one of three cores: **(A) STFT-domain gain estimation** (denoise, de-ess), **(B) linear-prediction / autoregressive modeling** (declick, dereverb), **(C) constrained reconstruction** (declip, inpainting).

## 0. The STFT, COLA, and perfect reconstruction

With analysis window `w[n]` of length `N`, hop `H`:

```
Y(t,f) = Σ_{n=0}^{N-1} w[n]·y[tH+n]·exp(-j 2π f n / N)
```

Resynthesis is weighted overlap-add with synthesis window `s[n]`. **Perfect reconstruction (COLA)** requires the analysis×synthesis windows to tile to a constant:

```
Σ_t w[n - tH]·s[n - tH] = C    for all n
```

Safe default for restoration: **√Hann** analysis+synthesis at **75% overlap** (`H = N/4`) — satisfies COLA and splits window gain symmetrically so you can modulate gains between analysis and synthesis without artifacts.

**Tradeoffs that drive quality:**
- **Window length `N`** sets the time/frequency tradeoff (uncertainty principle). Large `N` (4096–8192 @ 48 kHz, 85–170 ms): fine frequency resolution for hiss/hum/sustained sung notes, but smears transients. Small `N` (256–512): localizes clicks/plosives/consonants, blurs frequency. **A premium offline app should run multiple resolutions** — no single window is optimal for both hum and clicks.
- **Hop `H`**: smaller hop (75–87.5% overlap) reduces modulation artifacts when gains vary quickly.
- **Phase is the deep problem.** Magnitude methods reuse the noisy phase `∠Y` and only change magnitude; at low SNR the phase error is audible. This is the magnitude-only ceiling neural complex/deep-filter/waveform models break (see §13).

## 1. Spectral subtraction (Boll, 1979)

```
|X̂(t,f)|^p = max( |Y(t,f)|^p − α(t,f)·|D̂(f)|^p ,  β·|Y(t,f)|^p )
X̂(t,f)     = |X̂(t,f)|·exp( j·∠Y(t,f) )
```

`p=1` magnitude, `p=2` power; `α ≥ 1` over-subtraction factor (often SNR-adaptive); `β ∈ [0,1)` spectral floor. The half-wave rectification leaves isolated surviving bins that flicker frame-to-frame → **musical noise** ("birdies"). Mitigate with larger `α`, higher floor, frame averaging, or switch to MMSE (§3). Keep as a fast CPU-only fallback, not the quality ceiling.

## 2. Wiener filter

Per-bin MMSE linear estimator (uncorrelated `x`,`d`):

```
G_W(f) = S_x(f) / (S_x(f) + S_d(f)) = ξ/(1+ξ)
```

`ξ = S_x/S_d` is the a-priori SNR. Smooth, low musical noise, but needs `S_x` you don't have → decision-directed (§3).

## 3. Ephraim–Malah MMSE-STSA and log-MMSE (classical quality leaders)

These optimal short-time spectral-amplitude estimators (Gaussian DFT model) inherently suppress musical noise. Define:

```
γ(t,f) = |Y(t,f)|² / λ_d(f)        # a-posteriori SNR
ξ(t,f) = λ_x(f) / λ_d(f)          # a-priori SNR
ν(t,f) = ξ/(1+ξ)·γ
```

**MMSE-STSA gain** (modified Bessel `I₀`,`I₁`; verified against Loizou, *Speech Enhancement: Theory and Practice*):

```
G_STSA = (√π/2)·(√ν/γ)·exp(−ν/2)·[ (1+ν)·I₀(ν/2) + ν·I₁(ν/2) ]
```

**log-MMSE gain** (usually preferred perceptually; `E₁` = exponential integral):

```
G_logMMSE = ξ/(1+ξ)·exp( ½·E₁(ν) ),    E₁(ν) = ∫_ν^∞ (e^{−t}/t) dt
```

**Decision-directed a-priori SNR (the crucial trick):**

```
ξ̂(t,f) = α·|X̂(t−1,f)|²/λ_d(f) + (1−α)·max(γ(t,f)−1, 0),    α ≈ 0.98
```

The high `α` heavily smooths `ξ` over time — that temporal smoothing is what actually kills musical noise. **Implementation:** precompute the gain on a 2-D `(ξ,γ)` log-spaced lookup grid to avoid per-bin Bessel/`E₁` evaluation.

## 4. Noise PSD tracking: Minimum Statistics & IMCRA

`λ_d(f)` must be tracked continuously (real noise is non-stationary; no clean preamble).
- **Minimum Statistics (Martin 2001):** track the minimum of the smoothed periodogram over a sliding ~1.5 s window; noise ≈ minimum × bias-compensation.
- **IMCRA (Cohen):** recursively average noisy power weighted by speech-*absence* probability `(1−p(t,f))`; speech-likely bins freeze the estimate. The standard pairing with log-MMSE.

## 5. Spectral gating ("broadband denoise", the RX-style feel)

Learn a per-bin threshold `T(f)` from a noise profile (`mean + k·std` of `|Y|` over a noise region); attenuate below it:

```
mask(t,f) = 1 if |Y| > T(f) ;  else g_min  (e.g. −24 dB, not 0 — avoid spectral holes)
```

Then **smooth the mask** in time+frequency (small Gaussian/median 2-D kernel) before applying — the smoothing is what removes musical noise. Expose: noise-profile capture, reduction amount, threshold offset `k`, smoothing radii.

## 6. De-click / de-crackle — AR detection + LSAR interpolation (core B)

Impulsive defects are sparse/structural → deterministic AR repair is gold-standard (interpretable, sample-accurate, no hallucination).

**AR model** (order `p ≈ 20–50` for voice @ 48 kHz): `x[n] = Σ_{k=1}^p a_k x[n−k] + e[n]`. Estimate `{a_k}` via Levinson–Durbin or **Burg** (better on short frames).

**Detection (whitened residual):** `e[n] = x[n] − Σ a_k x[n−k]`. The all-pole inverse filter whitens signal but amplifies impulses; flag `|e[n]| > c·σ_e` (`c≈3–5`, robust `σ_e` via MAD).

**Reconstruction — LSAR (Janssen/Vaseghi/Veldhuis):** let `U` = corrupted indices, `K` = reliable. With prediction-error filter `b=[1,−a_1,…,−a_p]` and its banded Toeplitz convolution matrix `A`, total excitation energy is `‖Ax‖² = xᵀRx`, `R = AᵀA`. Minimizing over the unknowns gives the **normal equations**:

```
R_UU · x_U = − R_UK · x_K
```

A single linear solve fills the gap using **both sides jointly** (vs one-sided LPC extrapolation). **Janssen's algorithm** iterates (estimate `a` from filled signal → solve `x_U` → repeat 2–4×). Dense vinyl **crackle** is the same detector at a finer threshold. This is the iZotope RX De-click / Spectral-Repair lineage.

## 7. Declip — constrained sparse reconstruction (core C; SPADE family)

Clipped samples are *known-missing-above-threshold* (you know the sign and a lower bound `θ`). Exploit sparsity in a transform `D` (DFT/DCT/Gabor):

```
min_z ‖z‖₀  s.t.  x = Dz,  and consistency:
   x[n]=y[n] (reliable n),  x[n]≥θ (clipped high),  x[n]≤−θ (clipped low)
```

`‖·‖₀` is NP-hard → **SPADE** approximates via ADMM + hard thresholding, relaxing the sparsity level until consistency holds (**A-SPADE** = analysis/cosparse variant, usually preferred). It cannot invent energy contradicting the surviving samples or clip bounds. Mild clipping (<~10% of samples) restores near-transparently; severe clipping wants a generative fallback. Refs: Kitić et al. 2015; Záviška et al. "S-SPADE Done Right" 2018.

## 8. Dereverberation — WPE (core B)

Reverb is convolution `y = x*h`. WPE models the **late tail** as linear prediction from past STFT frames and subtracts it. Per bin:

```
y_t = d_t + Σ_{τ=Δ}^{Δ+L−1} g_τ*·y_{t−τ}
```

`d_t` = desired (direct+early); `Δ` = **prediction delay** protecting the direct path (the key quality knob — too small dries/distorts the direct path, too large leaves residual reverb); `L` = filter length. Estimate `g` by minimizing the **variance-normalized** (weighted) prediction error, weight = time-varying desired power `λ_t`:

```
min_g  Σ_t |y_t − Σ_τ g_τ*·y_{t−τ}|² / λ_t
```

Alternating optimization: (1) estimate `λ_t=|d_t|²` from current estimate; (2) solve weighted normal equations for `g`; (3) subtract the tail; iterate 2–5×. Transparent for mild–moderate reverb (esp. multichannel). Hard single-channel cases → generative neural (§Part I.2).

## 9. De-hum — harmonic notch / comb / adaptive cancellation (core A/B)

Mains hum = partials at `f₀` (50/60 Hz) and harmonics.

**(a) IIR notch** at each `ω_k = 2π k f₀/f_s`:

```
H_k(z) = (1 − 2cos(ω_k)z⁻¹ + z⁻²) / (1 − 2r·cos(ω_k)z⁻¹ + r²z⁻²)
```

`r→1⁻` narrows the notch (−3 dB BW ≈ `(1−r)·f_s/π`). Cascade over `f₀,2f₀,…`.

**(b) Comb** to hit all harmonics: `H(z) = (1 − z^{−M})/(1 − r z^{−M})`, `M = f_s/f₀`.

**(c) Adaptive LMS cancellation** (most transparent; tracks drift): synthesize per-harmonic references and adapt amplitudes:

```
ĥ[n] = Σ_k ( a_k[n]cos(ω_k n) + b_k[n]sin(ω_k n) )
a_k[n+1] = a_k[n] + μ·e[n]·cos(ω_k n),   e[n] = y[n] − ĥ[n]   (likewise b_k)
```

Estimate `f₀` precisely first and follow it. Low-risk, fully explainable.

## 10. De-ess — sidechain detection + dynamic attenuation (core A)

Sibilance (~3–10 kHz, up to 16). (1) Band-pass sidechain → detector level `L_sb(t)`. (2) Gain computer with threshold `T`, ratio `R`:

```
GR_dB(t) = min( 0, (T − L_sb_dB(t))·(1 − 1/R) )
```

(3) Apply **wide-band** (whole signal ducks — natural on a solo lead) or **split-band** (only highs attenuate — avoids darkening, better on buses; can lisp if overdone). **Pro trick (level-independent detection):** gate on the *spectral shape* (high-band-to-total energy ratio) normalized by level, so soft and loud passages de-ess consistently; use linear-phase/short-latency band split to avoid phase smear.

## 11. De-pop / plosive removal (core A)

Plosives = LF air bursts, mostly < ~120 Hz. Detect the localized low-band energy spike (LP sidechain over threshold), then over **only the plosive region** apply a **linear-phase FIR high-pass** (~80–120 Hz) or a transient-triggered dynamic low-shelf cut, preserving vocal body elsewhere. Hard case: plosive energy overlapping the vocal fundamental (low male voices) → some thinning is unavoidable.

## 12. Reverb synthesis (creative "add a space" module)

**(a) Convolution reverb:** `y = x*h` with a measured IR; partitioned FFT convolution (uniform size `B`, FFT `2B`, overlap-add; non-uniform/Gardner for low latency). **Offline → use large partitions / one big FFT.** Quality is in the IR.

**(b) FDN (Feedback Delay Network):** `N` delay lines `m_i` fed back through a **unitary (lossless) matrix** `A`:

```
s_i(n) = x(n)·b_i + Σ_j A_ij·( g_j·s_j(n − m_j) )
y(n)   = Σ_i c_i·s_i(n − m_i) + d·x(n)
```

`A` unitary ⇒ lossless dense tail (use **Householder** `A = I − (2/N)·11ᵀ` or normalized **Hadamard**). Per-line decay to a target RT60: `g_i = 10^( −3·m_i/(f_s·RT60) )`. Replace scalar `g_i` with a per-line **low-pass** for frequency-dependent decay (air absorption); delay lengths `m_i` mutually prime to maximize echo density. **Schroeder** primitives: comb `z^{−m}/(1−g z^{−m})`, allpass `(−g+z^{−m})/(1−g z^{−m})`, same `g`↔RT60 mapping. Keep "remove bad room" and "add chosen space" separate in the UX.

## 13. Phase reconstruction & the magnitude-only ceiling

- **Griffin–Lim:** iterate `x ← ISTFT(|S|·exp(j·∠STFT(x)))` — reduces magnitude inconsistency but sounds metallic; a poor ceiling.
- **Complex Ideal Ratio Mask (cIRM):** estimate complex `M`, apply `Ŝ = M⊙Y` (recovers magnitude *and* phase):

```
M_r = (Y_r S_r + Y_i S_i)/(Y_r²+Y_i²),   M_i = (Y_r S_i − Y_i S_r)/(Y_r²+Y_i²)
```

- **Deep filtering (DeepFilterNet):** per low-freq bin predict a short complex multi-frame FIR and convolve across frames:

```
Ŝ(t,f) = Σ_{i=0}^{N−1} C_i(t,f)·Y(t−i+l, f)   (N≈5 taps, l look-ahead)
```

exploiting inter-frame correlation to restore periodicity/phase a single-frame mask cannot. **Product takeaway:** magnitude DSP for *surgical, local, structural* defects (clicks, hum, plosives, clipping — exact + interpretable); neural complex/generative for *broadband, phase-coupled, ill-posed* restoration (low-SNR denoise, single-channel dereverb, BWE). Route accordingly.

## 14. Perceptual scales (why bands are spaced as they are)

```
Mel:  m  = 2595·log10(1 + f/700)
Bark: z  = 13·arctan(0.00076 f) + 3.5·arctan((f/7500)²)
ERB:  ERB(f) = 24.7·(4.37·f/1000 + 1)        (Glasberg–Moore)
```

DeepFilterNet's 32 ERB bands and RNNoise's 22 Bark bands exploit this: coarse perceptual-band gains capture most audible structure with few parameters, with fine complex processing reserved for low frequencies where pitch lives. **Simultaneous masking** means residual noise below the masking threshold is inaudible — a principled stop on how hard to denoise.

## 15. Evaluation metrics (so "quality" is measurable)

- **SI-SDR** — time-domain reconstruction fidelity (separation/enhancement default).
- **PESQ (P.862) / POLQA (P.863)** — intrusive speech quality (log exact version; PESQ saturates and disagrees with humans on generative output).
- **STOI / ESTOI** — intelligibility.
- **SRMR** — reference-free, reverb-sensitive (good for dereverb without a clean reference).
- **DNSMOS / NISQA / UTMOS** — learned no-reference MOS (good for generative output where SI-SDR/PESQ mislead; validate against listening tests).
- **SDR/SIR/SAR** (museval) — separation stems.

**Rule:** generative restoration can *raise* perceptual MOS while *lowering* SI-SDR (plausible, not identical, reconstruction). Always pair metrics with blinded listening tests for clicks, clipped consonants, reverb tails, and — critically — **sung vibrato/sustain preservation**, for which no good objective metric yet exists. **Eval buckets:** speech SE (DNS/EARS), singing (**SingVERSE** + **EDB**), vocal-in-mix (MUSDB18-HQ), localized repair (inject controlled clip/click/dropout/bandlimit), creative preservation (breath/vibrato/intentional reverb).

---

# Part IV — Tech stack (mid-2026): best of the best

## 1. Inference engine

**Default: `ort` (ONNX Runtime Rust bindings) — broadest hardware reach from one artifact.**
- Current: **`ort` 2.0.0-rc.12** (2026-03-05), wrapping **ONNX Runtime 1.26** (multiversioning lets you target ORT 1.17→1.26); ONNX Runtime itself is at **1.27** (June 2026). **It is still a release candidate — pin the exact rc.** Execution providers expose **CUDA, TensorRT, DirectML, CoreML, OpenVINO, QNN, WebGPU, XNNPACK** + more.
- **Two gotchas to engineer around now:** (a) `ort` **downloads a prebuilt ONNX Runtime binary at build time by default** — for a reproducible/offline premium build, disable `download-binaries` and **vendor/static-link** your own runtime; (b) inference is **not** real-time-safe — keep it off the audio callback (Part IV §5).
- **Execution-provider reality (per-OS):** **CoreML** is the only Apple GPU/ANE path (no Metal EP) — use **`ModelFormat=MLProgram` + static input shapes** and *validate FP16 numerically* (the legacy NeuralNetwork format can silently run FP32 in FP16). **DirectML** covers any DX12 GPU on Windows but is in **"sustained engineering"** — architect toward **Windows ML (WinML)** for longevity. **QNN** (Copilot+/Snapdragon NPUs) shipped via Windows Update and is a real target now. **CPU is the always-present floor** — keep it fast.
- **When to reach past `ort`:** **`tract`** (pure-Rust, zero C deps, static-link, no binary download — what DeepFilterNet uses; best for small CPU models like DFN3); **`candle`** (HuggingFace, Metal+CUDA, best for big diffusion/transformer models if you hand-write the sampler loop); **`tch-rs`/libtorch** only as a last resort (huge runtime). Versions (mid-2026): tract 0.23.3, candle-core 0.11.0, burn 0.21.0, tch 0.24 (libtorch 2.11).

## 2. The ONNX-export discipline (the #1 engineering risk — read this before picking models)

ONNX has an `STFT` op (opset 17) but **no `ISTFT`** and **no usable complex-number support**. The 2026-standard fixes (all verified against working HT-Demucs/RoFormer exports):
- **Rewrite `torch.stft` → a `Conv1d` with sin/cos kernels** emitting two real channels; **iSTFT → `ConvTranspose1d`** (~5e-6 error vs original).
- **De-fuse attention:** fused `aten::_native_multi_head_attention` has no ONNX symbolic → reimplement as plain `Linear`/`bmm`/`softmax` (~1e-6 error).
- **Strip Python artifacts** from the traced path (`fractions.Fraction`, `random.randrange`, data-dependent control flow).
- **Diffusion / flow-matching: export ONE sampler step**, run the predictor-corrector / ODE loop **in Rust host code**, feeding step `t` as an input. Never bake the loop into the graph. This single pattern removes most "ONNX can't do diffusion" objections.
- **Target opset 17+ with the legacy exporter** (`dynamo_export` still flaky for these); **fix a processing chunk length** (CoreML/NPUs strongly prefer static shapes for variable-length audio).
- **Mamba/SSM models do not export** (selective-scan Triton kernels; silent numerical errors when forced) — avoid, or budget a custom C/sequential-scan reimplementation.

**Model-selection consequence:** prefer **conv/attention models with clean in-graph STFT** (DeepFilterNet ships ONNX; BS PolarFormer ships ONNX; SRS/MossFormer2 are conventional and traceable; AP-BWE is all-conv). Treat anything Mamba-based, or any model that leaves STFT outside the graph, as high-risk.

## 3. Rust DSP / audio crates (mid-2026 versions)

| Crate | Version | Role | Notes |
|---|---|---|---|
| `cpal` | 0.18.1 | Audio device I/O | WASAPI/ASIO/CoreAudio/ALSA; keep callback alloc-free |
| `symphonia` | 0.6.0 | Decode WAV/FLAC/ALAC/MP3/AAC | **decode-only** (use `hound`/FLAC encoder to write); WAV demux now via `symphonia-format-riff` |
| `rubato` | 3.0.0 | Sample-rate conversion | RT-safe, SIMD; 3.0 changed the buffer API |
| `realfft` / `rustfft` | 3.5.0 / 6.4.1 | STFT/spectral FFT | use `realfft` for audio; prebuilt plan is RT-safe, planning is not |
| `fundsp` | 0.23.0 | Filter graphs (EQ/reverb/de-ess) | graph DSP, RT-safe once built |
| `dasp` | 0.11.0 (**STALE, 2020**) | Sample primitives | effectively unmaintained — use only `dasp_sample` if needed |

You implement STFT/ISTFT, AR declick, WPE, notch de-hum, FDN/convolution reverb, and the de-ess/de-pop dynamics natively in Rust/C++ (Part III).

## 4. UI framework decision (for a heavy spectrogram editor)

The hard requirement is an **RX-class zoomable, interactive spectrogram + waveform editor** moving large data Rust→GPU. None of the options has a true pro-audio-editor precedent, so this is partly trailblazing. Ranked by fit:

| Option | Spectrogram GPU path | Maturity proof | Bundle / RAM | DSP bridge | Best when |
|---|---|---|---|---|---|
| **egui** (Rust, immediate) | **Strong** — `egui_wgpu` paint callback + GPU texture; used by **Rerun** (heavy GPU pro app) | Rerun Viewer | few MB / tight | **native, zero-FFI** to Rust core | **Rust core + custom editor (recommended)** |
| **iced** (Rust, Elm) | Strong — `Shader`/`Primitive` widget; manual wgpu integration; used by **COSMIC desktop** | System76 COSMIC | few MB / tight | native, zero-FFI | Rust core, cleaner retained architecture |
| **Qt 6.11 / 6.8 LTS** (C++) | **Strongest cross-platform GPU path** — `QRhiWidget` / `QSGRenderNode` abstract Vulkan/Metal/D3D; one shader path everywhere | **DaVinci Resolve** (GPU pro editor) | ~70 MB / modest | **C++ native**; Rust via **CXX-Qt** (KDAB) | **C++-native premium editor; richest editor chrome** |
| **JUCE 8** (C++) | Weaker — Direct2D accelerates 2D *on Windows only*; custom spectrogram = hand-rolled `OpenGLContext` (deprecated on macOS) | audio plugins/apps (thin UI toolkit) | **small static binary** / low | C++ native; Rust via cbindgen+Corrosion / `cxx-juce` | **Plugin-first; cheapest license** ($3.5k perpetual) |
| **Electron** | **Guaranteed WebGPU+WebGL** (version-pinned Chromium); Figma GPU precedent | VS Code, Figma | 150–250 MB / ~400 MB | C++ via N-API (native), Rust via napi-rs | Web UI velocity / C++ core, footprint OK |
| **Tauri v2** | **Risk** — WebGPU on **WKWebView is unconfirmed** in 2026; WebView2 OK; native wgpu overlay is R&D | consumer/utility apps only | ~8 MB / lower | **native Rust backend** (great) | Small Rust app *if* you accept the WebGPU/IPC caveats |
| **Flutter** | Feasible (fragment shaders + `sampler2D`), but **Impeller not default on Windows**; multi-window new/buggy | mobile-first; thin desktop | 25–40 MB | `flutter_rust_bridge` (good) | Not recommended for this editor in 2026 |
| **Dioxus + Blitz** | **Weak** — no per-canvas wgpu surface (#3725); Blitz experimental | core mature (webview) | <6 MB | native | Avoid for this specific app |

**Recommendation:** three viable lanes, by core language and priorities:
- **Rust core, standalone-first → `egui`.** Immediate-mode is the natural fit for constant custom hit-testing; Rerun proves the heavy-GPU-viz architecture; zero-FFI to your Rust DSP. Gaps: native macOS menus (`muda`) and file dialogs (`rfd`).
- **C++ core, premium native editor → Qt 6 (`QRhiWidget`).** The strongest *cross-platform* GPU spectrogram path (one Vulkan/Metal/D3D shader path), the richest editor chrome (dockable panels, dialogs, theming), and a real precedent (DaVinci Resolve). Cost: LGPLv3 obligations (dynamic-link, provide Qt source, no static link) or unpublished commercial (~$3.6–4k/dev/yr); LTS patches are commercial-only. Rust DSP bridges via CXX-Qt.
- **Web-UI velocity or quickest polish → Electron** (guaranteed WebGPU, N-API for a C++ core, Figma precedent) — accept 150–250 MB / ~400 MB.

**Tauri** is defensible for a small Rust app but you must verify WebGPU-on-WKWebView hands-on and engineer a custom-protocol/tiled data path (default JSON IPC is ~11× slower than binary; WebView2 lacks chunked streaming). **JUCE** is the pick only if a plugin is near-term (cheapest license at $3,500 perpetual Pro, batteries-included audio), but its custom-spectrogram GPU path is hand-rolled OpenGL that's deprecated on macOS — weaker than Qt's RHI for this specific editor.

## 5. Real-time-safe architecture (keep ML off the audio thread)

The governing rule (Bencina): **nothing with unbounded execution time on the audio callback** — no locks, no allocation, no syscalls, no waiting on other threads. ANIRA's RADSan analysis confirmed **every** inference engine (ONNX Runtime, LibTorch, TFLite) allocates and locks *inside* each inference call, and the first few inferences are dramatically slower (warm-up needed).

**Reference design (ANIRA, arXiv 2506.12665):**
- Audio callback only reads/writes **preallocated** buffers.
- A **lock-free SPSC ring buffer** (`rtrb` 0.3.3 or `ringbuf` 0.4.8 in Rust; JUCE `AbstractFifo` / `moodycamel::ReaderWriterQueue` in C++) ferries frames to a **static worker thread pool**.
- **All allocation happens at construction**, never in `process()`; chunk audio independently of host buffer size; compute added latency (Letz algorithm) and report it for compensation; **warm up** the model before audio starts.
- **Offline render path** has *no* RT constraint — run the big non-causal model on a normal background thread, allocate freely, use multiple ORT intra-op threads, process the whole file with full context. This is where the heavy weights live; your "quality > real-time" priority makes this the primary path.

## 6. Model packaging & code-signing for offline distribution

- **Bundle weights in the installer** (the only thing that *guarantees* offline operation) — optionally a small hybrid (core model bundled, optional model packs as in-app downloads). Your entire value proposition is local/offline.
- **mmap the weights:** ship as **page-aligned ONNX external-data** (or ORT format) and load via **file path** (not `CreateSessionFromArray`) with `session.use_memory_mapped_ort_model` / external-initializer mmap options. The default external-data conversion is **not** page-aligned — convert *with alignment on* or mmap silently won't engage. mmap cuts resident RAM and cold-start time. For multiple sessions sharing weights, use a prepacked-weights container.
- **macOS:** notarization is mandatory and requires Hardened Runtime; **sign inside-out** (every embedded `.dylib` incl. `libonnxruntime` before the `.app`), notarize the `.dmg`/`.pkg`, and **staple** so Gatekeeper validates **offline**. Budget hours for multi-GB notarization (field reports ~3.5–4.5 h, flaky S3 uploads) — notarize the disk image, not loose files.
- **Windows:** Authenticode + **reputation note: EV certs no longer grant instant SmartScreen reputation** (2024 change) — keep the **same signing identity** across releases so reputation accrues; plan for cloud-HSM/USB-token signing (CA/B hardware-key mandate).
- **Update cost:** bundled weights mean app updates re-ship the whole payload unless you implement delta updates or externalize weights into separately-versioned, separately-verified (signed-manifest/checksum) files. Place weights **outside** the ASAR/qrc/executable (Tauri `bundle.resources`, Electron `extraResources`) so they're mmap-able and signable as part of the bundle.

## 7. Three concrete stack options

**Option A — Rust-native (recommended for standalone-first, no near-term plugin):**
`ort` (pinned rc, vendored ORT) + `tract` (for DFN3 CPU) · DSP in Rust (`realfft`/`rustfft`, `rubato`, `fundsp`, `symphonia`+`hound`) · **egui** editor · `cpal` I/O · `rtrb` ring buffers · ANIRA-style worker pool. Small, memory-safe, clean offline story, zero-FFI core↔UI. Risk: you're trailblazing the egui spectrogram editor; `ort` is pre-stable.

**Option B — C++ / JUCE (recommended if a VST3/CLAP/AU plugin is near-term):**
JUCE 8.0.14 (`dsp` module + FFTW/pffft) standalone *and* plugin from one `AudioProcessor` · ONNX Runtime C++ (or libtorch) · **JUCE WebView UI** (React/Svelte shared between app and plugin) · CLAP today via `clap-juce-extensions`, native CLAP in JUCE 9. Best path to DAW formats and the most pro-audio precedent. Cost: JUCE commercial license (Indie **$800 perpetual / $40 mo** <\$300k rev; Pro **$3,500 / $175 mo**), no memory safety, slower UI iteration.

**Option C — Web-UI velocity (if a polished UI fast matters most, C++ core OK):**
Electron 43 (Chromium 150, guaranteed WebGPU) · DSP/ML core as a C++ N-API addon (or Rust via napi-rs) · ArrayBuffer→WebGPU texture spectrogram. Fastest UI iteration and lowest spectrogram-rendering risk; cost is 150–250 MB bundle / ~400 MB RAM and a less "premium-native" feel.

**Primary recommendation:** **Option A** for the standalone-first premium vision, with the **core DSP/ML kept as a host-agnostic library** so you can later wrap it for plugins (Part V). If you discover the plugin market is the priority, switch the lead to **Option B** — that decision should be made before, not after, you build the UI layer.

---

# Part V — DAW plugin path (keep it cheap to add later)

**Keep the DSP/ML in a standalone, host-agnostic core** (a Rust crate or header-only C++ lib with no host/IO/GUI deps), exposing `prepare(sr, maxBlock)` / `process(buffer)` / params / reset / latency. The *same* RT-safe `process` serves both the plugin callback and the offline batch path. Then:

- **CLAP-first is the most license-clean, future-proof hub.** CLAP (MIT ABI, by Bitwig+u-he) is supported by Reaper, Bitwig, FL Studio, Studio One, and 70+ vendors; **`clap-wrapper` fans one CLAP out to VST3, AU, and standalone (AAX in progress)**. Note VST3 wrapping still requires accepting Steinberg's GPLv3-or-proprietary terms, and Logic/Pro Tools need AU/AAX respectively.
- **C++ route:** **JUCE** produces standalone + all plugin formats from one codebase; CLAP via `clap-juce-extensions` now, native in **JUCE 9**. JUCE 8.0.14 (June 2026) adds shared **WebView UI** so app and plugin share one frontend.
- **Rust route:** use **`nice-plug`** (the maintained RustAudio fork of nih-plug — the original went to maintenance mode March 2026) for standalone + CLAP + VST3, or build CLAP-first with **Clack** + `clap-wrapper`. Keep VST3 export optional (its bindings are GPLv3).
- DAW plugins impose strict real-time safety, host compatibility, automation, and preset constraints — valuable for DAW users but unnecessary friction for the first offline product. Defer, but don't architect yourself out of it.

---

# Part VI — Updated recommendation & roadmap

**Build Vocal Polish v1 as a standalone, offline, modular desktop app**, with the DSP/ML core as a host-agnostic library. The revised stack:

1. **Denoise:** `DeepFilterNet3` (ONNX/tract) for responsive preview + CPU tier; **`MossFormer2_SE_48K` (Apache-2.0, export to ONNX yourself)** for the heavy final-render quality. Classical log-MMSE as CPU-only fallback / surgical hiss tool.
2. **Deterministic repair (your zero-license-risk differentiators):** AR/LSAR de-click/de-crackle, A-SPADE declip, localized linear-phase de-pop, adaptive-notch de-hum, level-independent split-band de-ess (Part III). Expose them as controllable tools, not hidden side-effects.
3. **Dereverb:** WPE light tier + an **SGMSE+/StoRM** diffusion "deep dereverb" for the offline max-quality path (verify checkpoints; keep the sampler loop in host code).
4. **Bandwidth extension / restoration:** `MossFormer2_SR_48K` and/or **`AP-BWE` (MIT, 48 kHz, ONNX-friendly)** — **not** AudioSR. `Resemble Enhance` as a general one-click polish.
5. **Singing path:** optional **`BS PolarFormer`** separation (MIT, ONNX shipped) → **`SRS`** (MIT) singing restoration → ReverbFX-trained singing dereverb. Treat singing as its own domain; gate full-mix singing quality on the SingVERSE/EDB eval set. **Do not bundle Demucs or unlicensed RoFormer weights.**
6. **Pitch/time (creative tier):** `SwiftF0` (MIT, ONNX) + `Signalsmith Stretch` (MIT, formants) + `WORLD` (BSD); buy Rubber Band only for premium shift fidelity.
7. **Engine/threading:** `ort` (pinned, vendored) default; `tract` for DFN3; ANIRA-style worker pool with lock-free ring buffers; **never** run inference on the audio callback; offline render path runs the big models freely.
8. **UI:** **egui** (Rust core) for the spectrogram editor, or Electron if you want web-UI velocity / a C++ core. JUCE only if plugin-first.

**Roadmap (unchanged in shape from the consolidated brief, updated in content):**
- **Phase 1 — File-based MVP:** WAV/FLAC I/O; DFN3 + MossFormer2_SE_48K denoise; AR declick; A-SPADE declip; de-pop/de-hum/de-ess; convolution+FDN reverb; loudness + limiter; A/B with gain-match; non-destructive render queue. Prove the UX + offline packaging before heavy creative workflows.
- **Phase 2 — Singing & stems:** BS PolarFormer separation; SRS dry-vocal restoration; WPE + diffusion singing dereverb; SwiftF0/RMVPE analysis; WORLD/Signalsmith pitch-time; SingVERSE/EDB evaluation.
- **Phase 3 — Low-latency preview:** causal DFN3 preview on worker threads; device settings; ring-buffered audio exchange; preview intentionally simpler than final render.
- **Phase 4 — Advanced batch + plugin:** UniPASE/universal restorers; diffusion BWE; train your own license-clean separator on permissive multitracks; wrap the core as CLAP → VST3/AU via `clap-wrapper`.

**Release gate:** improvement on ≥1 fidelity metric per eval bucket, no catastrophic intelligibility regressions, and blinded listening checks for mouth clicks, clipped consonants, reverb tails, and **sung-vibrato preservation**. Label generative restoration as *reconstructive* for any archival/forensic use.

---

## Sources

**Denoise / enhancement**
- ClearerVoice-Studio / MossFormer2 — [repo (Apache-2.0)](https://github.com/modelscope/ClearerVoice-Studio) · [SE_48K card](https://huggingface.co/alibabasglab/MossFormer2_SE_48K) · [SR_48K card](https://huggingface.co/alibabasglab/MossFormer2_SR_48K) · [Interspeech 2025 paper](https://arxiv.org/html/2506.19398v1)
- DeepFilterNet — [repo (MIT∨Apache, ships ONNX)](https://github.com/Rikorose/DeepFilterNet) · [DF2 paper](https://arxiv.org/pdf/2205.05474)
- MP-SENet — [repo (MIT)](https://github.com/yxlu-0102/MP-SENet) · [paper](https://arxiv.org/abs/2305.13686) · GTCRN — [repo (MIT, ONNX)](https://github.com/Xiaobin-Rong/gtcrn)
- URGENT — [2025 (arXiv 2505.23212)](https://arxiv.org/html/2505.23212v2) · [2026 baselines Apache-2.0](https://github.com/urgent-challenge/urgent2026_challenge_track1) · [DNS-Challenge (ended 2023)](https://github.com/microsoft/DNS-Challenge)
- Mamba export blockers — [mamba#200](https://github.com/state-spaces/mamba/issues/200) · [MambaLite-Micro (arXiv 2509.05488)](https://arxiv.org/html/2509.05488v1) · SEMamba — [repo (MIT code)](https://github.com/RoyChao19477/SEMamba)
- AP-BWE (MIT, 48 kHz BWE) · Resemble Enhance (MIT) · VoiceFixer (code MIT, weights unclear) · SGMSE/StoRM — [sp-uhh/sgmse (MIT)](https://github.com/sp-uhh/sgmse) · UniPASE — [arXiv 2604.14606 (CC-BY-4.0)](https://arxiv.org/abs/2604.14606)
- MMSE-STSA/log-MMSE — Loizou, *Speech Enhancement: Theory and Practice*; Ephraim & Malah 1984/1985 · SPADE — [Kitić et al. 2015](https://inria.hal.science/hal-01159700/file/Declipping.pdf), [Záviška "S-SPADE Done Right" 2018](https://ar5iv.labs.arxiv.org/html/1809.09847)

**Separation / singing / pitch**
- BS PolarFormer — [HF bgkb/bs_polarformer](https://huggingface.co/bgkb/bs_polarformer) · [ZFTurbo zoo](https://github.com/ZFTurbo/Music-Source-Separation-Training/blob/main/docs/pretrained_models.md) · [MVSep 128](https://mvsep.com/algorithms/128)
- KimberleyJSN MelBand RoFormer (MIT weights) — [HF card](https://huggingface.co/KimberleyJSN/melbandroformer) · SCNet — [repo (MIT)](https://github.com/starrytong/SCNet) · Apollo — [repo](https://github.com/JusperLee/Apollo) / [HF (CC-BY-SA)](https://huggingface.co/JusperLee/Apollo)
- Demucs weights non-commercial — [demucs#327](https://github.com/facebookresearch/demucs/issues/327) · MUSDB18 terms — [SigSep](https://sigsep.github.io/datasets/musdb.html) / [Zenodo](https://zenodo.org/records/1117372)
- SingVERSE — [arXiv 2509.20969](https://arxiv.org/abs/2509.20969) / [dataset](https://huggingface.co/datasets/amphion/SingVERSE) · SRS — [arXiv 2510.21659](https://arxiv.org/abs/2510.21659) / [HF (MIT)](https://huggingface.co/smulelabs/Smule-Renaissance-Small) · AnyEnhance — [arXiv 2501.15417 (CC-BY-NC-ND)](https://arxiv.org/abs/2501.15417)
- SwiftF0 — [arXiv 2508.18440 (MIT)](https://arxiv.org/abs/2508.18440) · FCPE — [arXiv 2509.15140](https://arxiv.org/abs/2509.15140) · Signalsmith Stretch — [repo (MIT)](https://github.com/Signalsmith-Audio/signalsmith-stretch) · WORLD — [repo (BSD)](https://github.com/mmorise/World) · Rubber Band — [pricing](https://breakfastquay.com/rubberband/license.html)

**Stack / engineering**
- `ort` — [crate](https://crates.io/crates/ort) / [EP docs](https://ort.pyke.io/perf/execution-providers) · ONNX Runtime — [releases](https://github.com/microsoft/onnxruntime/releases) · CoreML EP — [docs](https://onnxruntime.ai/docs/execution-providers/CoreML-ExecutionProvider.html) · DirectML "sustained engineering" / WinML — [WinML EP](https://learn.microsoft.com/en-us/windows/ai/new-windows-ml/supported-execution-providers)
- ONNX STFT (no ISTFT) — [STFT op](https://onnx.ai/onnx/operators/onnx__STFT.html) / [ISTFT proposal #4777](https://github.com/onnx/onnx/issues/4777) · HT-Demucs export — [sevagh/demucs.onnx](https://github.com/sevagh/demucs.onnx) · ZFTurbo ONNX/TensorRT — [MSS_ONNX_TensorRT](https://github.com/ZFTurbo/MSS_ONNX_TensorRT)
- Rust crates — tract [repo](https://github.com/sonos/tract) · candle [repo](https://github.com/huggingface/candle) · cpal [repo](https://github.com/RustAudio/cpal) · symphonia [repo](https://github.com/pdeljanov/Symphonia) · rubato [repo](https://github.com/HEnquist/rubato) · realfft [crate](https://crates.io/crates/realfft) · fundsp [repo](https://github.com/SamiPerttu/fundsp)
- UI — egui [repo](https://github.com/emilk/egui) / [egui_wgpu CallbackTrait](https://docs.rs/egui-wgpu/) · iced [0.14](https://github.com/iced-rs/iced/releases) · Tauri [IPC concept](https://v2.tauri.app/concept/inter-process-communication/) · Electron [schedule](https://releases.electronjs.org/schedule)
- Plugin — CLAP [repo](https://github.com/free-audio/clap) · clap-wrapper [repo](https://github.com/free-audio/clap-wrapper) · JUCE [releases](https://github.com/juce-framework/JUCE/releases) / [pricing](https://juce.com/get-juce/) / [JUCE 9 CLAP roadmap](https://juce.com/blog/juce-roadmap-update-q3-2025/) · nice-plug [RustAudio fork](https://codeberg.org/BillyDM/nih-plug)
- RT architecture — Bencina, ["Real-time audio programming 101"](https://lwn.net/Articles/452630/) · ANIRA — [arXiv 2506.12665](https://arxiv.org/abs/2506.12665) / [repo](https://github.com/anira-project/anira) · ring buffers `rtrb` [repo](https://github.com/mgeier/rtrb), `ringbuf` [repo](https://github.com/agerasev/ringbuf)
- Packaging — ONNX external data [spec](https://onnx.ai/onnx/repo-docs/ExternalData.html) · ORT session options [header](https://github.com/microsoft/onnxruntime/blob/main/include/onnxruntime/core/session/onnxruntime_session_options_config_keys.h) · Apple [notarization](https://developer.apple.com/documentation/security/notarizing-macos-software-before-distribution) · Windows [SmartScreen reputation](https://learn.microsoft.com/en-us/windows/apps/package-and-deploy/smartscreen-reputation) · Tauri [resources](https://v2.tauri.app/develop/resources/)

## Gaps & further verification before locking the stack
- **Confirm BS PolarFormer's *weights* license** directly on its HF card (only KimberleyJSN MelBand was independently verified MIT).
- **Own the ONNX export** for MossFormer2_SE_48K and SRS (no official export); validate FP16 numerics for any generative/iterative model.
- **Confirm the ReverbFX dataset license** (CC0 vs CC-BY-NC conflict) before using it to train shipped singing-dereverb weights.
- **Decide the plugin question early** — it flips the stack from Rust/egui (Option A) to C++/JUCE (Option B).
- **Singing objective metrics don't exist for vibrato/sustain** — plan blinded listening tests; budget for training a license-clean separator on permissive multitracks if full-mix singing becomes core.
