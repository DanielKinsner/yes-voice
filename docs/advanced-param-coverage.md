# YesVox Advanced Parameter Coverage

> Issue: #18. Last updated 2026-07-04.
> Scope source: `docs/product.md`; control inventory source:
> `docs/ui/advanced-parameter-inventory.md`; implementation source:
> `yesvox-core::Session` plus `crates/yesvox-gui`.

## Sign-off

#18 Advanced parameter coverage is **signed off for v1**.

This means every v1 user-facing restoration/finish parameter exposed by the
Session surface is reachable from the Advanced screen, round-trips through
focused GUI tests, and has core tests proving the setting reaches render output
where that is observable. Low-level implementation constants that are not
product controls remain fixed/internal and are listed below.

The Advanced mock was approved by Dan on 2026-07-04. The final Advanced build
keeps that structure/style while bounding dense areas with compact controls,
overlap tests, and a capped spectrogram grid.

## Global And Monitor Coverage

| Area | Coverage | Evidence |
| --- | --- | --- |
| View shape | Separate Advanced screen over the same Standard shell. | `advanced_layout_keeps_dense_modules_scannable` |
| Content mode | Voiceover/Singing remains explicit and user-authoritative. | Standard top-bar Session wiring; product scope |
| Preset | Current preset remains selectable and drives defaults. | Standard preset wiring from #24 |
| Quality / denoise engine | Natural/Balanced/HQ reachable in Advanced Denoise. | `AdvancedDenoisePanel`; `advanced_denoise_panel_round_trips_session_controls` |
| Rack amount | Per-stage/card amount reachable from module cards. | Per-module panel tests; `set_stage_mix` |
| Rack bypass | Stage/card bypass remains reachable through Standard cards; sub-path bypasses are exposed where Advanced splits them. | `card_enabled`, `set_card_enabled`, transient/finish panel tests |
| A/B/C | A=input, B=current processed, C=pinnable snapshot. | `pinned_compare_snapshot_is_independent_playback_source_c`, GUI compare controls |
| Loudness match | A and C are loudness-matched to B when enabled. | `volume_match_brings_the_dry_to_the_processed_loudness`, `volume_match_brings_compare_snapshot_to_current_processed_loudness` |
| Difference monitor | Deterministic residual and generative wet/dry semantics are tested; reblend is audible through transport. | `difference_monitors_compute_as_specified`, `reblend_spans_processed_to_original`, #23 playback tests |
| Spectrogram | CPU fallback STFT grid renders in Advanced and is cached by audible source/grid. | `monitor::tests::spectrogram_*`, `advanced_spectrogram_grid_stays_dense_but_bounded`, `spectrogram_cache_key_tracks_audible_source_and_grid` |
| Export and safety | Output format/bit depth/dither/name/location/memory override remain reachable from the shell. | #24/#32/#35 tests and `cargo xtask gate` |

## Module Coverage

| Module | Advanced controls/readouts | Session/API evidence | GUI evidence |
| --- | --- | --- | --- |
| De-bleed | Source mode, warn threshold, block threshold, amount. | `debleed_source_imported`, `set_debleed_thresholds`, `advanced_debleed_thresholds_reach_render_refusal` | `AdvancedDeBleedPanel`, `advanced_debleed_panel_round_trips_session_controls`, overlap test |
| Declip | Detection threshold, repair strength, amount. | `declip_settings`, `set_declip_settings`, `advanced_declip_settings_reach_rendered_repair` | `AdvancedDeClipPanel`, round-trip and overlap tests |
| De-click / Mouth Clicks | Short-click enable/amount and mouth-click enable/amount split. | `set_click_repair_enabled`, `set_mouth_click_enabled`, `set_click_repair_amount`, `set_mouth_click_amount`, `transient_card_enables_reach_the_rendered_declick_paths` | `AdvancedDeClickPanel`, round-trip and overlap tests |
| De-pop | Low-cut cutoff, strength, amount. | `depop_settings`, `set_depop_settings`, `advanced_depop_settings_reach_rendered_plosive_filter` | `AdvancedDePopPanel`, round-trip and overlap tests |
| De-hum | Auto/50/60 fundamental, harmonics, notch Q, tracking, tracked-frequency readout, amount. | `dehum_settings`, `set_dehum_settings`, `dehum_tracked_frequency`, `advanced_dehum_settings_reach_rendered_notches_and_readout` | `AdvancedDeHumPanel`, round-trip and overlap tests |
| Denoise | Quality/engine mode, worker count, engine/fallback readout path, amount. | `set_quality_mode`, `set_worker_count`, `engine_note`, `advanced_worker_count_clamps_and_restores` | `AdvancedDenoisePanel`, round-trip and overlap tests |
| De-reverb | Strength, tail preservation, RT60 readout, amount. | `dereverb_settings`, `set_dereverb_settings`, `dereverb_rt60`, `advanced_dereverb_settings_reach_rendered_tail_and_rt60_readout` | `AdvancedDeReverbPanel`, round-trip and overlap tests |
| De-ess | Split/Wide mode, sensitivity ratio, minimum gain, HF reduction readout, amount. | `deess_settings`, `set_deess_settings`, `deess_hf_reduction_db`, `advanced_deess_settings_reach_rendered_reduction_and_readout` | `AdvancedDeEssPanel`, round-trip and overlap tests |
| Breath | Attenuation dB, de-ess sidechain readout, amount. | `breath_strength_db`, `set_breath_strength_db`, `breath_shared_hf_reduction_db`, `advanced_breath_settings_reach_rendered_attenuation_and_sidechain_readout` | `AdvancedBreathPanel`, round-trip and overlap tests |
| Finish | Presence/Air enable, presence dB, air dB, presence amount, Leveling enable/amount, Loudness enable/target. | `presence_air_eq`, `set_presence_air_eq`, `set_leveling_amount`, `set_loudness_target`, finish render tests | `AdvancedFinishPanel`, round-trip and overlap tests |

## Fixed/Internal Exclusions

These are real implementation values, but they are not v1 Advanced user
controls. They stay fixed because they protect the product scope: restoration
and finish, character preservation, and a pro-deep interface that is still
broadly accessible.

| Area | Exclusion | Reason |
| --- | --- | --- |
| Shared transient detector | `DetectorConfig` MAD factor, local factor, merge gap. | These are classifier safety constants shared by De-click, Mouth Clicks, and De-pop. Exposing them would turn Advanced into detector engineering and could undermine the no-double-repair guarantee. |
| Transient events | Raw event list and manual transient editing. | Manual event editing is timeline/spectrogram repair depth; the plan flags manual De-pop transient lists as deferred after false-positive measurement. |
| Declip | Ceiling ratio and solver internals. | The v1 engine exposes threshold/strength/amount; ceiling ratio remains a voice-preserving bound. |
| De-pop | Plosive region/ramp length. | Region shape is part of the plosive safety envelope, not a musical control. |
| De-hum | Tracking window and refinement span. | Advanced exposes F0, harmonics, Q, and tracking; detector windowing stays internal. |
| Denoise | Neural model path, chunk length/overlap, light-STFT frame/hop, over-subtraction/floor. | Model path is installer/environment plumbing for #20, not a rack knob. Chunk and light-engine constants protect offline determinism and memory gates. |
| De-reverb | Direct-path protect, spectral floor, STFT frame/hop. | The user-facing controls are strength and tail preservation; the lower-level constants preserve attack/sustain safety. |
| De-ess | Crossover and attack/release. | Advanced exposes mode, sensitivity, minimum gain, and amount; timing/crossover stay bounded to avoid production-EQ drift. |
| Breath | Detector frame/hop and minimum gain. | Advanced exposes attenuation and sidechain readout while preserving same-length, never-silence behavior. |
| Finish loudness | True-peak ceiling, look-ahead, release, limiter sidechain cutoff. | v1 exposes target and enable; limiter safety remains fixed so exports stay under the guarantee ceiling. |

## Acceptance Mapping

| #18 criterion | Status |
| --- | --- |
| Every engine parameter is reachable. | PASS for v1 user-facing Session parameters; fixed/internal exclusions above are deliberate. |
| Spectrogram renders smoothly; STFT math tested in core. | PASS with CPU fallback and cached GUI grid. |
| A=input, B=current, C=pinnable snapshot, all loudness-matched. | PASS. |
| Difference monitor and reblend. | PASS: monitor semantics and reblend transport path are mechanically covered. |
| Visual fidelity vs approved #26 mock. | PASS for structure/style: separate Advanced screen, full spectrogram, grouped module rack, monitor/export bands, and dense-card scannability tests. |

## Gate

Local gate is the merge bar while GitHub Actions minutes are exhausted.
`cargo xtask gate` was green before pushing the #18 spectrogram slices, and
must remain green for the docs close-out commit.
