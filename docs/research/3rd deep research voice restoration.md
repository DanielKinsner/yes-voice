# Building a Premium Offline Desktop App for Vocal Polish & Voice Restoration: Technical Research Report (2025–2026)

## TL;DR
- **Build a hybrid pipeline, not a purist one.** Classical DSP wins decisively for de-click/de-crackle (autoregressive interpolation), de-hum (adaptive notch), de-pop (linear-phase HPF/dynamic EQ) and de-ess (split-band dynamics); neural/generative models now dominate denoising, dereverberation, and bandwidth extension/restoration. Top quality requires the best tool per task, so the mild pure-DSP preference should yield on denoise/dereverb/restoration.
- **Recommended stack: Rust core (realfft/rustfft, rubato, symphonia, dasp, fundsp) + ONNX Runtime via the `ort` crate for neural inference + Tauri UI.** This matches the user's existing Rust/Tauri suite, gives permissively-licensed neural inference with CUDA/Metal(CoreML)/DirectML execution providers, and keeps DSP in safe, fast native code. A C++/JUCE path is the alternative if a VST/AU plugin becomes a near-term priority.
- **Licensing is the gating constraint, not capability.** Commercially bundleable: DeepFilterNet (MIT/Apache-2.0), Resemble Enhance (MIT), SGMSE (MIT), Demucs/RoFormer *code* (MIT), and the new singing-focused SRS model (MIT). **AudioSR is blocked** (built on AudioLDM, CC-BY-NC 4.0). VoiceFixer weights, individual RoFormer/Demucs checkpoints, and the ReverbFX dataset license all need per-artifact verification.

## Key Findings

1. **Denoising:** Neural is state of the art. DeepFilterNet (two-stage ERB gain + deep filtering, written in Rust, MIT/Apache-2.0) is the single best bundleable workhorse. For maximum offline quality, diffusion regeneration (StoRM/SGMSE+) beats discriminative models on perceptual quality but is slower. RNNoise (BSD) is a lightweight hybrid baseline.
2. **De-click/de-crackle/declip:** Classical model-based DSP (autoregressive/LSAR interpolation) remains the gold standard — interpretable, sample-accurate, no hallucination. A genuine pure-DSP differentiation opportunity.
3. **De-pop (plosives):** Pure DSP — localized linear-phase high-pass / dynamic EQ around 80–120 Hz. Trivial math, no ML needed.
4. **De-reverb:** Hybrid. Classical WPE (multichannel linear prediction) for mild cases; diffusion/generative (SGMSE+, dxRevive-style, Supertone) for hard single-channel cases. The flagship advanced feature.
5. **Add reverb:** Pure DSP — partitioned FFT convolution with measured IRs plus algorithmic (FDN/Schroeder) reverbs.
6. **De-ess:** Pure DSP — split-band/dynamic-EQ sibilance detection and attenuation. ML offers only marginal gains.
7. **General polish & restoration:** Generative neural is SOTA — VoiceFixer, Resemble Enhance, Miipher/Miipher-2, neural bandwidth extension.
8. **Singing-specific:** A distinct problem. Use RoFormer-family separation as a front end, then singing-tuned diffusion/Schrödinger-bridge dereverb (ReverbFX) and the MIT-licensed SRS vocal-restoration model.

## Details

### 1. Denoising / Noise Reduction

**Classical DSP foundations.** The canonical framework is short-time spectral attenuation. Take the STFT of the noisy signal y[n] = x[n] + d[n], giving Y(t,f) = X(t,f) + D(t,f). Apply a real-valued gain mask G(t,f) and resynthesize via ISTFT with overlap-add:
- **Spectral subtraction** (Boll, 1979): |X̂(t,f)|^p = max(|Y(t,f)|^p − α·|D̂(f)|^p, β·|Y(t,f)|^p), where p=1 (magnitude) or p=2 (power), α is the over-subtraction factor (typically 1–5), and β the spectral floor. The noisy phase is reused for reconstruction. The flooring/half-wave rectification creates isolated, randomly distributed time-frequency islands perceived as **musical noise** — short-lived narrowband tones surrounded by gaps.
- **Wiener filter** (MMSE on the complex spectrum): G_W(f) = S_x(f) / (S_x(f) + S_d(f)) = ξ/(ξ+1), where ξ is the a priori SNR. Smoother than subtraction, less musical noise, but needs an estimate of the clean speech PSD (hence iterative/decision-directed approaches, Ephraim–Malah). A practical pipeline is spectral subtraction followed by a Wiener post-filter to smooth out subtraction's spectral irregularities.
- **Ephraim–Malah MMSE-STSA / log-MMSE** estimators with the decision-directed a priori SNR estimate are the classical quality leaders precisely because they suppress musical noise.
- **Subspace methods** (KLT/eigendecomposition) trade off noise reduction vs. signal distortion explicitly.

The fundamental hard problems: phase is left unprocessed (noisy phase reused), the noise PSD estimate is never the instantaneous noise, and the magnitude-only paradigm caps achievable quality. This is exactly where neural methods win.

**Neural SOTA.**
- **DeepFilterNet (1/2/3)** — the best bundleable choice. Two-stage, perceptually-motivated, full-band 48 kHz. Stage 1 estimates real-valued gains in 32 ERB bands (modeling human frequency resolution and keeping the network small); Stage 2 applies "deep filtering": instead of pointwise mask multiplication X̂ = G⊙Y, it predicts a complex multi-frame FIR filter per low-frequency bin (N=5 taps, 2 look-ahead + 3 causal), so X̂(t,f) = Σ_i C_i(t,f)·Y(t−i,f). This exploits inter-frame correlation and recovers periodicity/phase. Operates at 20 ms windows, 10 ms hop, ~40 ms latency. Written in Rust targeting the `tract` engine — directly relevant. **License: dual MIT OR Apache-2.0 ("at your option"), weights included under the same terms.**
- **RNNoise** (Valin) — hybrid DSP/RNN. Computes 22 Bark-scale band gains via a GRU, plus a pitch comb filter for fine structure; ~60× real-time on x86, 7× on a Raspberry Pi 3. **BSD-licensed.** Good as a fast preview/low-tier mode.
- **Diffusion/generative — SGMSE/SGMSE+ and StoRM** (Univ. Hamburg, sp-uhh). Formulate enhancement as a stochastic differential equation in the complex STFT domain: a forward SDE interpolates between clean x₀ and noisy y while adding Gaussian noise; a score network ∇ₓ log p_t(x) is trained, and a reverse-SDE sampler (predictor-corrector) recovers clean speech. Generative models "hallucinate" plausible fine detail, beating discriminative models on perceptual quality (NISQA) for non-additive distortions, at the cost of many network function evaluations. **StoRM** (Stochastic Regeneration) uses a predictive model (e.g., NCSN++M) to produce E[x₀|y], then a diffusion model corrects distortions — far fewer steps, no "vocalizing" artifacts. **License: MIT.**

**Verdict:** Neural wins. Ship DeepFilterNet as the default offline denoiser; offer a "maximum quality" diffusion mode (SGMSE+/StoRM-style) where processing time is acceptable. Keep a spectral-subtraction/Wiener module for surgical, predictable, broadband hiss reduction and as a CPU-only fallback.

### 2. De-click / De-crackle (impulsive noise)

**Classical DSP is the gold standard, and it's a differentiation opportunity.** Restoration is two phases: detection (find degraded sample locations) and reconstruction (replace them). The dominant model treats audio as locally autoregressive (AR): x[n] = Σ_{k=1}^p a_k x[n−k] + e[n], an all-pole IIR excited by white noise.
- **Detection:** fit AR coefficients (Levinson-Durbin/Burg), compute the prediction-error residual e[n]; clicks produce large excursions in the residual (matched-filter/kurtosis on the residual).
- **Reconstruction — LSAR (least-squares AR interpolation):** treat the corrupted samples as unknowns and minimize Σ e[n]² subject to the AR model, using *both* sides of the gap jointly (unlike one-sided LPC extrapolation). This yields a linear system solved in closed form. Janssen's algorithm (Vries/Janssen/Veldhuis) iterates AR-parameter estimation and missing-sample estimation. For long gaps, frequency-warped Burg and sinusoidal+AR models improve envelope recovery.
- **Mouth clicks / digital clicks** are short and handle well; **vinyl crackle** is dense low-level impulsive noise handled by the same detector at finer thresholds.
- **De-clip:** related — clipped samples are "known-missing above a threshold"; sparse-recovery/AR or sparsity-based declipping reconstructs them.

ML (U-Net image-to-image declipping) exists but offers little over AR for clicks and risks hallucinating transients. **Verdict: pure DSP (AR/LSAR).** iZotope RX's De-click and Spectral Repair (interpolation) descend from exactly this lineage.

### 3. De-pop / Plosive removal

Plosives (p, b, t, k, "p-pops") are bursts of low-frequency air energy, predominantly 20 Hz–~200 Hz with the bulk below ~120 Hz. **Pure DSP:** detect the localized low-frequency energy spike, then apply a *localized* high-pass / low-cut filter (linear-phase FIR to avoid smearing) with cutoff ~80–120 Hz, or a transient-triggered dynamic EQ that pulls down only the low band when the plosive crosses threshold. Apply only over the plosive region (clip-gain automation envelope) to preserve vocal body elsewhere. iZotope's RX De-plosive does exactly this. No ML needed; the only hard case is when plosive energy overlaps the vocal fundamental, where some thinning is unavoidable.

### 4. De-reverb (reverb removal) — flagship advanced feature

**Classical DSP.** The reverberant signal is x = s * h (clean speech convolved with the room impulse response). **WPE (Weighted Prediction Error)** is the workhorse: model the late-reverb tail via multichannel linear prediction in the STFT domain. Per frequency bin, x_{f,t} = Σ_{τ=0}^{L-1} Z_{f,τ} x_{f,t−Δ−τ}; estimate the AR matrix Z by minimizing a weighted prediction error using the time-varying speech variance r_{f,t} as the weight, with a prediction delay Δ that protects the direct path + early reflections (Δ trades residual reverberation against direct-path distortion). Iterative: estimate variance → estimate filter → subtract late reverb. Excellent and low-distortion, but works best multichannel and on mild-to-moderate reverb; single-channel WPE (e.g., 10 taps) works but is limited.

**Neural SOTA (single-channel hard cases).** Diffusion/generative models dominate. SGMSE+ has dedicated dereverberation checkpoints (WSJ0-REVERB, EARS-Reverb at 48 kHz). Generative approaches especially benefit "non-additive distortion models like dereverberation and bandwidth extension." Commercial leaders — **Supertone Clear** (proprietary NN separating voice/ambience/voice-reverb into three faders) and **iZotope RX Dialogue Isolate + Dialogue De-Reverb** (new neural net) — confirm the neural direction. In Production Expert's "Dialogue Noise Reduction Shootout 2025," the verdict was blunt: "The winners — Hush Pro, dxRevive Pro and Supertone Clear — are all relatively new to the game… neither iZotope or CEDAR led. RX didn't do badly, but no longer dominates." All three winners are neural and newer than RX/CEDAR.

**Verdict: hybrid.** WPE for transparent light dereverb; offer a diffusion-based "deep dereverb" for hard cases. This feature is the one most likely to win premium customers.

### 5. Reverb (adding natural reverb)

**Pure DSP, mature.** Two families: (a) **Convolution reverb** — convolve the dry signal with a measured room/plate IR using uniform/non-uniform partitioned FFT convolution (overlap-add/overlap-save). Offline batch removes any latency concern, so use large partitions for efficiency. (b) **Algorithmic** — Schroeder (comb + allpass networks) and **Feedback Delay Networks (FDN)** with a lossless feedback matrix and per-line damping filters for frequency-dependent decay. No ML needed; quality comes from good IRs and well-tuned FDN topologies/modulation.

### 6. De-ess (sibilance reduction)

**Pure DSP.** Sibilance ("s", "sh", "f", "ch") sits ~3–10 kHz (up to 16 kHz). Standard architecture: a sidechain band-pass filter isolates the sibilant band; when its level crosses a threshold, gain reduction is applied either **wide-band** (whole signal ducks — best for single vocals) or **split-band** (only highs attenuate — avoids darkening/lisping on buses). The strongest designs (e.g., FabFilter Pro-DS "Single Vocal" detection) look for the *wave pattern* of sibilance rather than just band energy, and operate independent of absolute signal level so they de-ess loud and soft passages consistently — implemented by splitting into buffers and transforming each to the frequency domain (a patented level-independent multi-band approach). ML offers only marginal gains (better S-vs-musical-high discrimination). **Verdict: pure DSP** with smart detection.

### 7. General vocal polish & voice restoration (multi-distortion)

**Generative neural is SOTA.** These models jointly fix noise + reverb + clipping + bandlimiting:
- **VoiceFixer** — two-stage: a ResUNet analysis stage predicts a clean mel-spectrogram from degraded input, then a neural vocoder (TFGAN) synthesizes the 44.1 kHz waveform. Handles denoise, dereverb, declip, and super-resolution simultaneously. **Code MIT; weights need license confirmation** (the LICENSE copyright field is an unfilled `[year] [fullname]` template and there is no separate weights license).
- **Resemble Enhance** — denoiser (UNet on a complex spectrogram) + enhancer (latent CFM/diffusion-style) trained on 44.1 kHz; does denoise + distortion repair + bandwidth extension. **MIT, weights in the same repo — clean for commercial use.**
- **Miipher / Miipher-2** (Google DeepMind, arXiv 2505.04457) — Miipher-2 uses a *frozen* Universal Speech Model (USM) "supporting over 300 languages" as a conditioning-free feature extractor, parallel adapters to clean the features, and a WaveFit vocoder. It hits "a real-time factor of 0.0078, enabling the processing of a million-hour speech dataset in approximately three days using only 100 such accelerators." State of the art for universal restoration but **not open-weight** (USM proprietary); only unofficial reimplementations (Open-Miipher-2, built on Gemma 3's 0.6B USM) exist.

**Bandwidth extension / super-resolution** (restoring HF lost to telephone/MP3/old media):
- **AudioSR** — latent diffusion predicting an HR mel conditioned on LR input + HiFi-GAN vocoder; generalizes to speech/music/SFX. **BUT built on AudioLDM → CC-BY-NC 4.0 weights → commercially BLOCKED.** Do not bundle.
- **NU-Wave / NU-Wave2** — diffusion waveform upsamplers to 48 kHz (Short-Time Fourier Convolution + Bandwidth Spectral Feature Transform, any-to-48 kHz).
- **NVSR** ("neural vocoder is all you need"), **FlowHigh** (flow-matching), **AP-BWE** (explicit HF phase via GAN), and **mdctGAN** are alternatives; flow-matching/bridge models (LatentFlowSR, Latent Bridge Models, 2026) are the frontier — fewer steps and better low-frequency consistency than AudioSR, which can over-boost and add artifacts.

**Verdict:** Generative neural. Default to Resemble Enhance (safe license) for general polish; build a bandwidth-extension module on a NU-Wave2-class or flow-matching model with *verified* licensing (avoid AudioSR's weights).

### 8. Singing-specific restoration

**This is a distinct, under-served problem — most speech models destroy vibrato, sustained notes, and harmonic richness.** The SOTA stack from 2024–2026:
- **Front-end separation with RoFormer-family models.** Hybrid Transformer Demucs (HTDemucs; dual waveform+spectrogram U-Nets with a cross-domain transformer) "achieves a SDR of 9.00 dB on the MUSDB HQ test set… using sparse attention kernels… and per source fine-tuning, [reaches] state-of-the-art 9.20 dB of SDR" (arXiv 2211.08553, **MIT**). The newer **Mel-Band RoFormer / BS-RoFormer** (band-split + rotary-position transformer) are the current SDR leaders for vocal isolation. Code is MIT (lucidrains reference impl; ZFTurbo training repo MIT), but **individual checkpoints have mixed provenance** — e.g., KimberleyJSN's Mel-Band RoFormer "Kim Vocal 2" weights were GPL-3.0 until relicensed MIT in April 2026 — so audit each `.ckpt` before bundling.
- **Singing dereverberation: ReverbFX / Singing-ReverbFX** (Richter, Svajda, Gerkmann, sp-uhh, arXiv 2505.20533). A dataset of RIRs captured from reverb *plugins* (not real rooms), used to train SGMSE+ and a Schrödinger-bridge model specifically for sung vocals: "The Schrödinger bridge also demonstrates strong performance, but is slightly outperformed by SGMSE+… models trained on plugin-derived RIRs significantly outperform those trained on traditional, natural RIRs." Code MIT; **dataset license is contested — see Caveats.**
- **Historical singing restoration: BABE-2** ("A Diffusion-Based Generative Equalizer for Music Restoration," Moliner/Välimäki, Aalto, arXiv 2403.18636). A diffusion prior jointly estimates the degradation filter's magnitude response and hallucinates restored audio; pretrained on studio singing and fine-tuned per target singer, it was demonstrated rejuvenating Enrico Caruso (tenor) and Nellie Melba (soprano) recordings (the pipeline denoises, then separates instrumentals with HT-Demucs first). Verify BABE-2's code license directly before commercial use.
- **SRS — Smule Renaissance Small** (Zang et al., arXiv 2510.21659, 24 Oct 2025). The most directly relevant *and* commercially usable singing-restoration model: "a compact single-stage model… achieving 10.5× real-time inference on iPhone 12 CPU at 48 kHz" (a 10.4M-parameter generative audio model, per its HF card). On the **Extreme Degradation Bench (EDB)** — "87 singing and speech recordings captured under severe acoustic conditions" — "SRS surpasses all open-source baselines on singing and matches commercial systems, while remaining competitive on speech despite no speech-specific training." Baselines included VoiceFixer, Adobe Enhance V2, and Lark V2. **Released under MIT (model + EDB benchmark).** This should be a centerpiece of the singing path.

**Verdict for singing:** pipeline = optional RoFormer/HTDemucs separation → singing-tuned restoration (SRS, MIT) and singing dereverb (ReverbFX SGMSE+). Never reuse a speech-only denoiser on full sung vocals without testing for harmonic/vibrato damage.

### How the pros do it (competitive landscape)
- **iZotope RX (current: RX 12; RX 11 launched May 2024)** — the industry standard. Visual spectrogram editing + per-problem modules, increasingly ML: Dialogue Isolate (neural source separation, now real-time), Dialogue De-Reverb, Music Rebalance/Scene Rebalance/Stems View (neural stem separation), Repair Assistant, ML-powered Breath Control and De-bleed. But De-click, De-hum, and Spectral Repair remain classical DSP. RX 11 launch pricing: "RX 11 Standard - $399 USD… RX 11 Advanced - $1199 USD" (introductory $299/$799).
- **Adobe Podcast Enhance Speech (v2)** — cloud generative speech model; noise fingerprinting + VAD + AGC + generative resynthesis. Cloud-based — a constraint your offline app turns into a selling point.
- **Supertone Clear** ($99) — proprietary NN separating voice / ambience / voice-reverb into three real-time faders; won the 2025 dialogue shootout for transient/complex noise.
- **Acon Digital (Acoustica/Extract:Dialogue), Accentize (dxRevive, SpectralBalance, DeRoom), Waves Clarity Vx, CEDAR** — all moved to neural for denoise/dereverb. dxRevive Pro was a 2025 shootout winner doing generative restoration/bandwidth extension. CEDAR remains strong on classical real-time tools but is being overtaken on hard offline jobs.

The pattern: **classical DSP for surgical/impulsive/spectral-localized problems; neural for separation, denoise, dereverb, and restoration.** Your offline-only, privacy-preserving, no-cloud positioning is a genuine differentiator against Adobe.

### Tech stack recommendations

**Cross-cutting engineering truths.** Phase reconstruction is the deepest hard problem (magnitude-only masks cap quality; Griffin-Lim is poor; complex masks/deep filters and time-domain/vocoder models address it). The artifact triad is musical noise (subtraction), transient smearing (over-aggressive HPF/long windows), and hallucination (generative models inventing wrong detail). Offline removes latency constraints but not compute/quality tradeoffs — diffusion = many NFEs; large STFT windows give better frequency resolution but worse time localization on transients — so expose large-window/high-step "max quality" modes.

**Option A (RECOMMENDED): Rust core + ONNX Runtime + Tauri.**
- *Audio/DSP:* `realfft`/`rustfft` (FFT), `rubato` (sample-rate conversion), `symphonia` (decode), `dasp` (sample/frame primitives), `fundsp` (filter graphs for EQ/reverb/de-ess). Implement STFT/ISTFT, AR declick, WPE, notch de-hum, FDN/convolution reverb natively.
- *Neural inference:* the **`ort`** crate (ONNX Runtime bindings) — execution providers for CUDA, TensorRT, **DirectML** (Windows, vendor-agnostic), **CoreML/Metal** (macOS/Apple Silicon), OpenVINO, with seamless CPU fallback. Reported 3–5× faster than Python with 60–80% less memory, mature and production-proven, and the lingua franca for shipping these exact models. Alternatives: `tract` (pure-Rust, lightweight, what DeepFilterNet uses, but CPU-focused with limited op coverage for diffusion); `candle` (HuggingFace, loads safetensors, Metal/CUDA, good for transformer/diffusion but some GPU paths ~3–4× slower than libtorch); `tch-rs`/libtorch (full PyTorch parity, heavy to bundle); Burn (backend-agnostic pure Rust — CPU/CUDA/Metal/WGPU/WASM — promising but younger).
- *UI:* **Tauri** (native WebView + Rust backend; installers commonly <10 MB vs Electron's 80–150 MB; ~30–50 MB RAM idle vs 150–300 MB). Matches the existing suite and lets the heavy DSP/inference live in Rust behind a TypeScript frontend.
- *Why:* aligns with the user's Rust/Tauri apps; permissive licensing throughout; ONNX covers all target models and all three OSes' GPUs behind one API.
- *Risk:* diffusion models with exotic ops may need ONNX opset care or a `candle`/`tch-rs` fallback; high-resolution spectrogram rendering in a WebView needs a canvas/WebGL strategy.

**Option B: C++ / JUCE core.**
- *Use if* a VST3/AU/AAX plugin is a near-term priority (JUCE is the industry standard and matches your DAW). DSP via the JUCE `dsp` module + FFTW/Intel IPP/pffft; neural via ONNX Runtime C++ or libtorch.
- *Tradeoff:* best path to plugin formats and DAW integration, but no memory safety, slower UI iteration than web frontends, and divergence from the Rust suite. Since DAW integration is explicitly secondary, this is the fallback, not the lead.

**Option C: Python prototype → Rust/C++ port.**
- *R&D only.* PyTorch + librosa + the reference repos (DeepFilterNet, Resemble Enhance, SGMSE, Demucs, VoiceFixer, SRS) to validate quality and tune pipelines, then export to ONNX and port the DSP. Do **not** ship Python to end users for a premium product (bundling/startup/size pain).

**Bundling large models offline:** ship ONNX (optionally FP16 — ~50% size cut, near-identical quality) or quantized weights inside app resources, or a first-run download to app data; load via `ort` with the platform GPU execution provider and CPU fallback. Sizes range from a few MB (DeepFilterNet, RNNoise, SRS at 10.4M params) to hundreds of MB (diffusion + vocoders). Use lazy per-module loading and memory-map where possible.

**Cross-platform acceleration:** macOS → CoreML/Metal EP (Apple Silicon especially); Windows → DirectML (vendor-agnostic) or CUDA/TensorRT (NVIDIA); Linux → CUDA or CPU. ONNX Runtime abstracting these behind one API is the main reason to prefer it over per-framework runtimes.

## Recommendations

**Stage 1 — MVP (highest ROI, lowest license risk):**
1. Stand up Rust + `ort` + Tauri with a spectrogram view.
2. Ship the pure-DSP modules first (your differentiation + zero license risk): AR/LSAR de-click/de-crackle, de-pop (localized linear-phase HPF/dynamic EQ), de-ess (split-band dynamics with level-independent detection), de-hum (adaptive harmonic notch), convolution + FDN reverb, spectral-subtraction/Wiener denoise.
3. Integrate **DeepFilterNet** (ONNX, MIT/Apache-2.0) as the default neural denoiser and **Resemble Enhance** (MIT) for general polish/bandwidth extension.

**Stage 2 — Premium neural features:**
4. Add diffusion **dereverb** (SGMSE+ checkpoints, MIT) + classical WPE for light cases — market this as the flagship.
5. Add the **singing path**: optional HTDemucs/RoFormer separation (verify each checkpoint's license) → **SRS** (MIT) singing restoration → ReverbFX-style singing dereverb.
6. Add a "maximum quality" diffusion denoise mode (StoRM-style) for hard offline jobs.

**Stage 3 — Restoration & bandwidth extension:**
7. Add neural bandwidth extension using a **license-clean** model (NU-Wave2-class or a flow-matching/bridge model with verified license) — **not AudioSR** (CC-BY-NC). Consider training/fine-tuning your own on permissive data for full commercial freedom.
8. Optionally a VoiceFixer-style multi-distortion one-click mode once weights licensing is confirmed in writing.

**Benchmarks/thresholds that change the plan:**
- If you need VST/AU plugin distribution within ~6 months → switch the lead to **C++/JUCE** (Option B).
- If a target model's weights are CC-BY-NC or unverifiable → do not bundle; train your own or substitute a permissive model.
- If diffusion ONNX export proves intractable → use `candle` or `tch-rs` for those specific models while keeping `ort` for the rest.
- If GPU coverage on user machines is poor → lean on DeepFilterNet/RNNoise/classical DSP for the default tier; gate diffusion behind a "GPU recommended" high-quality tier.

## Caveats
- **License verification is mandatory before shipping.** Confirmed commercially-safe: DeepFilterNet (MIT/Apache-2.0), Resemble Enhance (MIT), SGMSE (MIT), Demucs/RoFormer *code* (MIT), SRS (MIT). **Blocked: AudioSR** (AudioLDM lineage; AudioLDM checkpoints are CC-BY-NC 4.0, "not allowed for commercial use"). **Verify before relying on:** VoiceFixer *weights* (template LICENSE, no separate weights license) and individual RoFormer/Demucs *checkpoints* (some historically GPL-3.0). Miipher-2 is not open-weight. Get written confirmation for any ambiguous artifact.
- **Conflicting evidence on the ReverbFX dataset license.** My subagent found the `sp-uhh/reverbfx` repo listed as CC0-1.0 (public domain), but the arXiv 2505.20533 PDF reportedly describes the material as "CC BY-NC 4.0 … for non-commercial research and educational purposes only." **Treat the ReverbFX dataset as potentially non-commercial until you confirm directly with the authors.** The SGMSE *code* itself is MIT.
- **Generative models hallucinate.** For forensic/archival/legal work, prefer interpretable DSP (AR declick, WPE) and label generative output as reconstructive, not faithful.
- **Singing-specific evaluation is immature** — the ReverbFX authors note no singing-specific objective metrics yet exist; you'll need careful subjective listening tests for the singing path.
- **Some proprietary-tool internals are vendor/marketing descriptions** (Supertone, Adobe, iZotope), not published architectures; treat them as informed inference. iZotope's current shipping suite is RX 12; the cited prices are RX 11's 2024 launch prices.
- **Performance numbers** (Miipher-2 RTF 0.0078; HTDemucs 9.2 dB SDR; SRS 10.5× real-time / 10.4M params; Tauri RAM figures) come from the cited papers/benchmarks and may not generalize to your hardware or content.
- The 2026-dated Tauri/Electron and tool-roundup sources are secondary/blog-grade; architectural and licensing claims are corroborated against primary repos and papers where possible.