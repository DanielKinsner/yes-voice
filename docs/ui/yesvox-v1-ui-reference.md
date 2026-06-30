# YesVox v1 — UI reference

> **The canonical visual target for the v1 shell.** When the egui UI is built
> (#17 Standard view, #18 Advanced view), it should realize this layout. The
> backend (the `yesvox-core` rack) already exists; this is the surface that will
> sit on top of it.

## The reference image

The annotated mock lives next to this file:

```
docs/ui/yesvox-v1-reference.png
```

**⚠️ Action needed (one-time):** the binary could not be written automatically
(it was pasted into chat, not available on disk to the agent). Drop the PNG Dan
shared into `docs/ui/yesvox-v1-reference.png` and commit it, so it syncs to all
machines. Until then, the written spec below is the source of truth.

---

## Full layout transcription

Dark theme — near-black navy background, glassy rounded panels, blue→purple/cyan
accent gradients, subtle glow on active controls.

### Top bar

- **Left:** `YES Vox` wordmark with a waveform glyph.
- **Center:** segmented tabs — **Restore** (active, blue fill) | **Polish**.
- **Right:** undo / redo, **A / B** compare, **Presets ▾**, settings gear,
  help `?`, **‹ Back to Suite**.

### Left sidebar

- **VOCAL ORDER** — "1 vocal • 3:28". Selected item `01 Lead Vocal.wav 3:28`
  with a mini waveform. **+ Add Vocal**.
- **VOICE TYPE** — two big toggles: **Voiceover** (mic) | **Singing** (note,
  selected here). *(Backend: content mode, #2; Singing only selectable once the
  #16 singing gate passes.)*
- **SMART ASSIST** card (AI badge): "Analyzes your vocal and builds the best
  settings for a clean, natural result." → **Analyze Vocal** button.
- **Bottom:** "Analyzing audio 14%" progress; **Import Vocal** (upload).

### Center — header + transport

- Title `LEAD VOCAL.WAV` (edit pencil). Chips: **WAV · 48 kHz · 24-bit · Mono ·
  3:28**.
- **FLOW AMOUNT** master slider → `x1.00` (global dry→wet macro — *not yet in
  the backend; a v1 UI concept to design*).
- **Original / Processed** toggle; **IN −7.8 LUFS / OUT −10.1 LUFS** with stereo
  L/R meters (0 → −48 dB). *(Backend: BS.1770 loudness, #2/#14.)*
- Waveform (0:00–3:28), blue→purple gradient, playhead.
- Transport: play, `0:00 / 3:28`, loop/reset; center "Analyzing audio 14%";
  **Volume Match** + **Preview LUFS** checkboxes.

### Center — RESTORE rack (5 cards: toggle · blurb · circular % · Advanced)

| Card | Blurb | Knob | Backend module |
|---|---|---|---|
| **Denoise** | Reduces background noise while preserving detail | 48% | `Denoise` (#8); HQ engine = MossFormer2 (#9/#10) |
| **De-reverb** | Removes reverb while preserving natural space | 42% | `DeReverb` (#12) |
| **De-click / De-pop** | Removes clicks, pops and crackle | 55% | `DeClick`+`DePop` via shared detector (#5/#6) |
| **Mouth Clicks** | Reduces lip smacks and mouth noises | 60% | `TransientKind::MouthClick` path in `DeClick` (#5) — UI splits it out |
| **De-hum** | Removes electrical hum and low frequency noise | 35% | `DeHum` (#3) |

- Right: **Restoration Focus** radial — *More Transparent ↔ More Aggressive*
  (global macro; maps to quality-mode/intensity, #11/#17).

### Center — POLISH rack (5 cards)

| Card | Blurb | Knob | Backend module |
|---|---|---|---|
| **De-ess** | Controls harsh sibilance without dulling | 40% | `DeEss` (#7) |
| **Breath Control** | Attenuates harsh breaths (voiceover-leaning) | 30% | `BreathControl` (#13) |
| **Presence / Air** | Adds clarity and air in an adaptive way | 35% | `FinishStage` EQ (#14) |
| **Leveling** | Smooths vocal level consistently | 45% | `FinishStage` leveler (#14) |
| **Loudness** | Sets loudness to target level | −14.0 LUFS · **Target** | `FinishStage` loudness/limiter (#14) |

- Right: **Polish Intensity** radial — *Gentle ↔ Strong*, 50%; **Auto Polish**.

### Center — bottom strip

- **SPECTRAL MONITOR** — "Difference view (What was removed)", 20 Hz–20 kHz
  spectrum. *(Backend: difference monitor, #18.)*
- **REBLEND** — "Blend removed signal back in", slider 25%, preview play.
  *(Generative wet/dry reblend, #18.)*
- **Vocal Isolation Check** — "Checks for residual noise, reverb, or bleed." →
  **Check Now**. *(Backend: analyzer residual + de-bleed guard, #2/#15.)*

### Right sidebar

- **PRESET** — `Singing – Clean & Natural` (star). Desc: "Balanced restoration
  for singing vocals. Preserves character and natural ambience." → **Save as New
  Preset**. *(Presets, #17.)*
- **OUTPUT** — Format **WAV**, Sample Rate **48.0 kHz**, Bit Depth **24-bit**,
  Loudness Target **−14.0 LUFS** (gear), **Dither** ON. *(Backend: export
  settings, #1.)*
- **FILE** — Output File Name `Lead Vocal_YesVox`, Location `/Users/You/Music/
  YesVox`, **Add to Yes Suite Library** (checked). *(Never-overwrite export, #1;
  suite handoff seam.)*
- **TOOLS** (collapsed); **EXPORT VOCAL** (primary, upload).

### Bottom status bar

`Analyzing · Quality —` · `LIVE PEAK —` · `LIVE LUFS —` · `PROCESSING /
ANALYZING AUDIO`.

---

## Notes for whoever builds the shell

- **Restore/Polish split** mirrors the backend's two rack groups — wire each card
  to its module's `set_enabled` + a single intensity/strength param; the circular
  knobs are those params.
- **Singing UI is conditional.** This mock shows the singing-enabled state. Per
  the product guardrails, the Singing toggle + singing presets appear **only if
  the #16 quality gate passes**; otherwise ship voiceover-only with the slot
  reserved (schema already supports this — `SingingSettings`).
- **"Mouth Clicks" is a sub-control**, not a separate engine — the shared
  transient detector already classifies `MouthClick` distinctly from `Click`.
- **FLOW AMOUNT** and the two radial macros are global controls with no 1:1
  backend field yet — design them as macros over the per-module params.
- Every visible action must have an agent-callable equivalent (agent-native
  parity) — the core already exposes all of these as plain methods.
