# Singing research — decisions & flags

> Distilled from `docs/research/new singing only research/` (3 deep-research docs)
> on 2026-06-30, when the blind-listening singing gate was replaced by the
> mechanical `singing` metric gate and singing was unlocked as a first-class
> toggle. The research is a **tool, not a task list** — most of it confirms what
> we already do; this file records what we *applied*, and the few items that need
> a human (Dan) decision before building.

## Applied this pass (settings relayed onto our existing knobs)

The research strongly validated the existing singing path. Almost every
recommendation was already our default:

| Recommendation | Status |
|---|---|
| Breath OFF for singing (never gate breaths to zero) | already our default ✓ |
| De-reverb OFF for both singing presets (don't fight plugin reverb) | already ✓ (Lead/Natural both off) |
| De-reverb `tail_preservation` ≈ 0.85 when on | already `DeReverb::singing()` ✓ |
| Conservative light/Natural denoise; HQ only when noise demands | already ✓ (Natural Vocal = light) |
| Loudness: Lead −14 LUFS, Natural match-input; avoid heavy limiting | already ✓ |
| Narrow (high-Q) de-hum notches at 50/60 + harmonics | already ✓ |
| Gentle air (+1 dB) / modest presence; no formant-shifting | already ✓ |
| Declip first ("deepest damage first") | already our canonical order ✓ |
| **De-ess Split (not Wide) for singing + gentler ratio** | **applied** — `feat(#16)`: both singing presets → Split; `DeEss::set_sensitivity(0.6, 0.25)` for singing |

**The mechanical singing gate** (the listening-gate replacement) was built from
the research's "must-pass preservation metrics": F0 deviation (cents), vibrato
rate+depth, HNR change, spectral-envelope cosine, tail survival — all computed
on synthetic fixtures with known ground truth (`src/singing.rs`,
`tests/singing_gate.rs`).

## Flag for Dan — do NOT build unprompted (run it by Dan first)

The user's rule: *if the research has a wildly different stack, or remarkable
new tech that would change the product, run it by me first.* These qualify.

### Remarkable-new-tech (buildable in our stack, but a scope decision)
- **IMCRA continuous noise-PSD estimator** — *no weights, Rust-implementable.*
  The single "key swap" that makes a speech denoiser singing-safe: replaces the
  VAD/silence-driven noise estimate (which mistakes a held note's decay for
  noise and gates its upper harmonics) with a continuous minima-tracked PSD.
  Lowest-friction flag; directly fixes the main sung-denoise failure mode. Would
  be a new front-end component on the denoise path, not just a knob.
- **Vocal-fry / creak guard for de-click** — *no weights.* A generic de-clicker
  misclassifies glottal pulses of vocal fry (F0 ≈ 20–70 Hz, high jitter, low
  HNR) as a click train → "metallic chirp." Add a creak detector (residual peak
  prominence + H2–H1 ratio) that **bypasses** de-click interpolation in creak
  regions. New detector path; the `singing` HNR metric already gives us the
  building blocks.
- **Dynamic resonance suppressor** (Soothe2/RESO-style, per-ERB-bin de-resonance)
  — *no weights.* Tames chaotic vowel/mic "honk" without a static notch
  hollowing the voice. Distinct new module.
- **NMF denoise** — *no weights, algorithmic.* Avoids spectral-hole "musical
  noise" on harmonic material; an alternative engine behind our `Denoiser` trait.
- **Singing-native shallow-diffusion restorer** (generative, reconstructs
  harmonics) — research-tier; **breaks determinism + the <60 s budget** and risks
  hallucinating harmonics. Always-A/B, never default. Out of v1 spirit.

### Needs weights / licensing review (mostly out of scope)
- **Separation weights** (Mel-Band / BS-RoFormer, Kim Vocal, etc.) — licensing
  minefield **and** separation is out of scope (that's YesStems). Excluded.
- **`pedalboard` (Spotify) is GPL-3.0** — hard landmine; must never be linked
  into the closed-source app.
- **AudioSR (MIT)** bandwidth-extension / air-restoration — BWE is explicitly
  v1-later.
- **ReverbFX-trained singing dereverb**, **ReverbMiipher** — data-driven baked-in
  reverb removal; need datasets + weights-license review; inspiration only.

### Wildly-different-stack (intentionally not our path)
- **JUCE/C++ + RTNeural/ARA2 real-time plugin** — the docs' framework rec;
  conflicts with our Rust + egui + ort/tract offline app. (One confirmation: our
  ort-ONNX path matches their "ONNX Runtime for stateless transformer/U-Net" rec.)
- **Two-graph (causal real-time + offline) architecture** — adds a live mode;
  cuts against our offline-finish-only stance. Roadmap decision.
- **Note-aware pitch/time editing UI** (Melodyne/RePitch) — pitch/time editing is
  out of v1 scope; we already harvested its best by-product (the vibrato/
  modulation-deviation metric).

## Caveat carried from the research
The code metrics are **necessary but not sufficient** — the research recommends a
human MUSHRA panel as a *secondary, non-blocking* signal ("no single objective
metric captures 'still sung by the same person, cleaned up and in tune'"). Per
Dan's call, that listen is **optional and never a ship-blocker**; the mechanical
gate + the toggle are what gate the ship.
