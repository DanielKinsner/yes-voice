# YesVox Advanced Parameter Inventory

> Issue: #26. Status: approved control contract for the #18 Advanced build.
> Source of truth: current engine code in `crates/yesvox-core/src/modules/`,
> `chain.rs`, `preset.rs`, `settings.rs`, `quality.rs`, and `session.rs`.

This file is the control contract. The generated PNG is allowed to be a layout
and style reference, but #18 should use this inventory for labels and bindings.
Dan approved the draft direction on 2026-07-04 with one implementation caveat:
some areas feel prone to crowding, so #18 should keep the same structure and
style while using grouping, disclosure, and spacing to keep dense Advanced
modules scannable.

## Legend

- **Session wired now:** already exposed through `Session` or the existing GUI
  path.
- **Engine real, needs #18 wiring:** a real field, constructor argument, setter,
  getter, or enum exists in the engine, but the current Standard `Session`
  surface does not expose it as a user control.
- **Fixed/internal:** real implementation detail. Do not show as a primary user
  knob unless #18 deliberately promotes it with tests.

## Global Session Controls

| Area | Parameter | Status | Values / notes |
| --- | --- | --- | --- |
| Content | Content mode | Session wired now | `Voiceover`, `Singing`; user-authoritative toggle. |
| Preset | Active preset | Session wired now | `Podcast`, `Narration`, `Dialogue-cleanup`, `Lead Vocal`, `Natural Vocal`. Selecting a preset resets card overrides and adopts its content mode and quality default. |
| Quality | Quality mode / Restoration Focus | Session wired now | `Natural`, `Balanced`, `RestorationHq`; quantized from the Restoration Focus radial. Owns denoise engine binding and pass budget. |
| Rack | Flow Amount | Session wired now | `0.0..1.0`; global dry/wet master over the whole rack. |
| Rack | Stage/card enabled | Session wired now | Per stage/card enable override; `None` means preset default. |
| Rack | Stage/card amount | Session wired now | `0.0..1.0`; stage wet/dry mix, with independent transient and finish sub-card handling. |
| Polish | Polish Intensity | Session wired now | `0.0..1.0`; currently scales `DeEss` and `Breath` only. Loudness is deliberately excluded. |
| Monitoring | Playback source | Session wired now | `Original`, `Processed`, `Reblend`, `Difference`. For an A/B/C bar, map C to a real monitor source, not a third processed state. |
| Monitoring | Volume Match | Session wired now | Auditions `Original` through `ab_matched_input()`. |
| Monitoring | Reblend | Session wired now | `0.0..1.0`; `0` is processed, `1` is original. |
| Export | Output format | Session wired now | `WAV`, `FLAC`; sample rate is read-only and locked to input. |
| Export | Bit depth override | Session wired now | `None` means source depth; otherwise writer bit depth. |
| Export | Dither | Session wired now | TPDF dither on <=24-bit integer export. |
| Export | FLAC compression | Session wired now | `0..8`, default `5`. |
| Export | File name/location | Session wired now | Output stem and output directory; never overwrite is handled by export. |
| Safety | Memory over-budget override | Session wired now | `allow_over_budget`; render refuses over-budget unless explicitly allowed. |

## Canonical Rack Order

The chain order is fixed:

1. De-bleed
2. Declip
3. De-click / mouth-click
4. De-pop
5. De-hum
6. Denoise
7. De-reverb
8. De-ess
9. Breath control
10. Finish: Presence/Air, Leveling, Loudness/Limiter

Every stage has a common **Enabled** toggle and **Amount** wet/dry mix through
the chain/session layer unless noted.

## Per-Module Controls

### De-bleed

Purpose: residual instrumental leakage and mix-refusal guard for imported
separated stems. It is not a source separator.

| Parameter | Status | Values / notes |
| --- | --- | --- |
| Enabled | Session wired now | Preset default is off for native takes. |
| Amount | Session wired now | Stage wet/dry mix. |
| Source mode | Engine real, needs #18 wiring | `DeBleed::for_source(imported)`; imported stems enable de-bleed by default. |
| Warn threshold | Engine real, needs #18 wiring | Default `0.02`; `set_thresholds(warn, block)`. |
| Block threshold | Engine real, needs #18 wiring | Default `0.15`; clamped to at least warn threshold. |
| Mix-likeness score | Engine real, needs #18 wiring | Readout from `mix_likeness()` / `assess()`: `Clean`, `Warn`, `Block`. |
| Cleaner | Fixed/internal | Uses the deterministic spectral denoiser for gentle leakage cleanup. |

### Declip

Purpose: flat-top clipping repair by constrained peak reconstruction.

| Parameter | Status | Values / notes |
| --- | --- | --- |
| Enabled | Session wired now | Stage enable. |
| Amount | Session wired now | Stage wet/dry mix. |
| Detection threshold | Engine real, needs #18 wiring | Constructor argument; default `0.98`. |
| Repair strength | Engine real, needs #18 wiring | Constructor argument; `0.0..1.0`, default `1.0`. |
| Ceiling ratio | Fixed/internal | Default `1.6`; maximum synthesized overshoot above the clip threshold. |

### De-click / Mouth Clicks

Purpose: short click/crackle repair plus mouth-click/lip-smack repair, using one
shared transient detector. Plosives are owned by De-pop.

| Parameter | Status | Values / notes |
| --- | --- | --- |
| De-click enabled | Session wired now | Independent short click/crackle path. |
| De-click amount | Session wired now | `0.0..1.0`; independent from mouth clicks. |
| Mouth-click enabled | Session wired now | Independent mouth-click/lip-smack path. |
| Mouth-click amount | Session wired now | `0.0..1.0`; independent from short clicks. |
| Detector MAD factor | Engine real, needs #18 wiring | `DetectorConfig.mad_factor`, default `6.0`. |
| Detector local factor | Engine real, needs #18 wiring | `DetectorConfig.local_factor`, default `8.0`. |
| Detector merge gap | Engine real, needs #18 wiring | `DetectorConfig.merge_gap`, default `8` samples. |
| Event kind readout | Engine real, needs #18 wiring | `Click`, `MouthClick`, `Plosive`; events are disjoint and singly classified. |

### De-pop

Purpose: plosive low-cut over detected plosive regions only.

| Parameter | Status | Values / notes |
| --- | --- | --- |
| Enabled | Session wired now | Tied to the Standard De-click / De-pop card for now. |
| Amount | Session wired now | Standard De-click / De-pop amount also sets De-pop stage mix. |
| Low-cut cutoff | Engine real, needs #18 wiring | Field `cutoff_hz`, default `120.0 Hz`. |
| Strength | Engine real, needs #18 wiring | Field `strength`, default `1.0`. |
| Plosive region | Fixed/internal | About `30 ms` with `5 ms` raised-cosine edges. |
| Detector config | Engine real, needs #18 wiring | Same shared transient detector config listed above. |

### De-hum

Purpose: adaptive 50/60 Hz notch cascade with harmonics and drift tracking.

| Parameter | Status | Values / notes |
| --- | --- | --- |
| Enabled | Session wired now | Stage enable. |
| Amount | Session wired now | Stage wet/dry mix. |
| Nominal fundamental | Engine real, needs #18 wiring | `None` for auto-detect, or explicit `50/60 Hz` via `DeHum::new(Some(f0))`. |
| Harmonics | Engine real, needs #18 wiring | Field `harmonics`, default `6`. |
| Notch Q | Engine real, needs #18 wiring | Field `q`, default `30.0`. |
| Drift tracking | Engine real, needs #18 wiring | Field `track`, default `true`. |
| Tracked frequency | Engine real, needs #18 wiring | Readout from `tracked_frequency()`. |
| Tracking window | Fixed/internal | About `100 ms`; refinement scans +/- `1.5 Hz`. |

### Denoise

Purpose: deterministic light engine or neural engine behind the denoiser seam,
with streamed chunks and RAM preflight.

| Parameter | Status | Values / notes |
| --- | --- | --- |
| Enabled | Session wired now | Stage enable. |
| Amount | Session wired now | Stage wet/dry mix. |
| Quality mode | Session wired now | `Natural` = light, 1 pass; `Balanced` = neural, 1 pass; `RestorationHq` = neural, conditional 2 passes. |
| Pass budget | Session wired now via quality | `Denoise::set_passes(1..2)` exists; direct pass override is not a separate Session control. |
| Engine fallback note | Session wired now | Neural-to-light fallback reason surfaces from render. |
| Neural model path | Engine real, needs #18 wiring | Read from `YESVOX_MODEL_PATH`; integrity checked before load. |
| Worker count | Engine real, needs #18 wiring | `Denoise::new(workers)` / `with_engine(workers, ...)`; Session auto-sizes. |
| Chunk length | Fixed/internal | `ChunkPolicy.chunk_s`, default `8.0 s`. |
| Chunk overlap | Fixed/internal | `ChunkPolicy.overlap_s`, default `0.5 s`. |
| Working-set multiplier | Fixed/internal | `ChunkPolicy.working_set_k`, default `16.0`. |
| Light STFT frame/hop | Fixed/internal | `SpectralDenoiser::new(frame, hop)`, default `1024/256`. |
| Light over-subtraction | Fixed/internal | `over_sub`, default `1.5`. |
| Light spectral floor | Fixed/internal | `floor`, default `0.12`. |

### De-reverb

Purpose: light STFT dereverb with direct-path protection, tail preservation,
and analyzer-fed auto-soften.

| Parameter | Status | Values / notes |
| --- | --- | --- |
| Enabled | Session wired now | Voiceover presets enable it; singing presets default it off. |
| Amount | Session wired now | Stage wet/dry mix. |
| Strength | Engine real, needs #18 wiring | Constructor argument; voiceover default `1.2`, singing default `0.8`. |
| Tail preservation | Engine real, needs #18 wiring | Constructor argument; `0.0..1.0`; voiceover default `0.1`, singing default `0.85`. |
| Measured RT60 | Session wired now as analyzer input | `set_rt60(report.reverb_time_s)` drives auto-soften; useful as a readout. |
| Direct-path protect | Fixed/internal | Default `3.0`. |
| Spectral floor | Fixed/internal | Default `0.1`, raised by tail preservation. |
| STFT frame/hop | Fixed/internal | `1024/256`. |

### De-ess

Purpose: split-band or wide sibilance control, publishing HF reduction to breath
control to prevent double-cutting.

| Parameter | Status | Values / notes |
| --- | --- | --- |
| Enabled | Session wired now | Stage enable. |
| Amount | Session wired now | Stage wet/dry mix, also scaled by Polish Intensity. |
| Mode | Engine real, needs #18 wiring | `Split` or `Wide`; presets currently use `Split`. |
| Sensitivity / ess ratio | Engine real, needs #18 wiring | `set_sensitivity(ess_ratio, min_gain)`; default `0.45`, singing preset `0.6`. Higher ducks less. |
| Minimum gain | Engine real, needs #18 wiring | Default `0.1`, singing preset `0.25`. |
| Crossover | Fixed/internal | `5500 Hz`. |
| HF reduction | Engine real, needs #18 wiring | Readout from `hf_reduction_db()`, scaled by de-ess mix before breath sees it. |
| Attack/release | Fixed/internal | About `1 ms` attack, `50 ms` release. |

### Breath Control

Purpose: attenuate harsh breaths without removing time; backs off when de-ess
already reduced the shared high-frequency band.

| Parameter | Status | Values / notes |
| --- | --- | --- |
| Enabled | Session wired now | Voiceover default on; singing default off. |
| Amount | Session wired now | Stage wet/dry mix, also scaled by Polish Intensity. |
| Breath attenuation | Engine real, needs #18 wiring | `set_strength_db(db)`, default `9 dB`; presets use `6 dB`, `3 dB`, or off. |
| Shared HF reduction | Engine real, sidechain | `set_shared_hf_reduction_db(db)` from De-ess; display as readout, not a user knob. |
| Minimum gain | Fixed/internal | `0.25`, never below about `-12 dB`. |
| Detector frame/hop | Fixed/internal | About `20 ms` frames, 50 percent overlap. |

### Finish: Presence / Air

Purpose: preset-driven EQ patch scaled by the Presence/Air amount.

| Parameter | Status | Values / notes |
| --- | --- | --- |
| Enabled | Session wired now | Independent finish sub-card. |
| Amount | Session wired now | `0.0..1.0`; scales the preset EQ patch. |
| Presence boost | Engine real, needs #18 wiring | `set_eq(presence_db, air_db)`; preset values: `0.0` or `1.5 dB`. |
| Air boost | Engine real, needs #18 wiring | `set_eq(presence_db, air_db)`; preset values: `0.0` or `1.0 dB`. |
| Presence frequency/Q | Fixed/internal | Peaking EQ near `5 kHz`, Q `1.0`. |
| Air shelf | Fixed/internal | High shelf near `12 kHz`, Q `0.707`. |

### Finish: Leveling

Purpose: program-level gain rider before loudness normalization.

| Parameter | Status | Values / notes |
| --- | --- | --- |
| Enabled | Session wired now | Independent finish sub-card. |
| Amount | Session wired now | `0.0..1.0`; scales the program rider. |
| Block size | Fixed/internal | About `20 ms`. |
| Max gain range | Fixed/internal | Up to `+/-6 dB * amount`. |
| Rider strength | Fixed/internal | `0.65 * amount`. |
| Smoothing | Fixed/internal | Internal gain smoothing coefficient `0.0025`. |

### Finish: Loudness / Limiter

Purpose: integrated loudness target or peak-normalize plus look-ahead true-peak
limiter.

| Parameter | Status | Values / notes |
| --- | --- | --- |
| Enabled | Session wired now | Independent finish sub-card; bypass skips normalization and limiter. |
| Target | Session wired now | `Podcast` = `-16 LUFS`, `Streaming` = `-14 LUFS`, `MatchInput`, `PeakNormalize`, `Custom(f32)`. |
| True-peak ceiling | Fixed/internal | `-1.0 dBTP`. |
| Look-ahead | Fixed/internal | `0.005 s` minimum. |
| Release | Fixed/internal | About `50 ms`. |
| Detector sidechain cutoff | Fixed/internal | `12 kHz`, low-passed above air band to avoid pumping. |

## Mock Layout Requirements From #26

- Separate Advanced screen, not a Standard card expansion.
- Same visual language as `docs/ui/yesvox-v1-reference.png`: dark shell,
  glass panels, blue/cyan/purple accents, compact cards, circular/slider
  controls, and the same typography feel.
- Full-width spectrogram replaces the Standard waveform. Brush-heal editing is
  not in v1 and should not appear as an active tool.
- Expanded rack should show the module inventory above. Prefer scannable module
  panels grouped by Restore, Polish, Finish, Monitor, and Export; do not squeeze
  every low-level parameter into the first visible row when a disclosure row,
  secondary tab, or compact readout would reduce crowding.
- Include content-mode override and preset/quality controls.
- Include A/B/C compare using real monitor sources: Original, Processed,
  Difference or Reblend.
- Retain Difference Monitor and Reblend controls.
- The mock may show a few small readouts such as tracked hum frequency, RT60,
  HF reduction, mix-likeness verdict, LUFS, true peak, and render memory
  preflight, because those are real engine/session observables.
- Do not show v1-deferred features: source/stem separation, timeline editing,
  pitch/time correction, creative reverb, brush-heal editing, bandwidth
  extension, cloud processing, or multitrack production controls.
