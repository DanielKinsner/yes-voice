# YesVox — Product Definition (Canonical)

> **YesVox** is a standalone, **offline** desktop app that takes a flawed or raw **isolated vocal** and makes it sound **clean, clear, and — if you want — genuinely polished**, for both voiceover and singing, without making it stop sounding like the performer.
>
> **Status:** canonical north-star (living doc). Last updated 2026-06-29.
> **How to use this doc:** when scope or intent is fuzzy, this file decides. It answers *what* YesVox is and *why*; the *how* (models, DSP math, tech stack, phased build) lives in [`docs/research/consolidated-voice-restoration-research.md`](research/consolidated-voice-restoration-research.md) (the master technical brief). If this doc and an older research note disagree, **this doc wins** on product intent.

---

## 1. What YesVox is (one sentence)

**YesVox restores and polishes an already-isolated vocal — fixing the problems (noise, reverb, clicks/pops, hum, clipping, sibilance, harsh breaths) and, when wanted, applying a tasteful finish that makes it sound produced — entirely offline, with pro-grade quality that preserves the performer's character.**

It is **restoration + polish**, not production. Default behavior is corrective and clean; an optional finish makes the standalone deliverable genuinely *impressive*. It does not mix, master, or produce a full track.

## 2. Positioning — the hook

The wedge is **"studio-clean vocals — voiceover AND singing — that still sound like the performer, completely offline and private."**

- vs **Adobe Podcast / Auphonic / cloud enhancers:** YesVox is **local and private** (no upload) and is **genuinely good at singing**, not speech-only.
- vs **aggressive AI cleaners:** YesVox **preserves character** (vibrato, sustain, breath, timbre) instead of smearing it into a robotic/watery result.
- vs **iZotope RX:** YesVox is **one-click approachable** for a broad audience, with pro depth available — not an intimidating specialist suite.

The Yes suite is **music-first**, so **singing is a first-class citizen, never an afterthought** — even though voiceover is equally supported.

## 3. Who it's for

Primary user is **"pro-grade, broadly accessible"**: as close to a professional tool as possible, but usable by a wider audience. A non-engineer should get a great result fast; a pro should get full control and trust it on paid work. We serve *both* via progressive disclosure (Standard / Advanced), never by weakening the engine.

Content is **equally voiceover and singing** (with singing held first-class given the music-first suite), routed by an **explicit Voiceover/Singing toggle** (user-authoritative, smart-assisted — see §6/§11).

## 4. Operating principles (when unsure, decide this way)

1. **Restoration & polish, NOT production.** Fix/finish one vocal. Default corrective-and-clean; an *optional* tasteful finish (adaptive presence/air, gentle leveling, loudness) can make it impressive. Do **not** add full mixing/mastering/multitrack/instrument/beat features — that's yes-master or a DAW.
2. **The voice is the wanted signal.** YesVox is the opposite of a de-mixer. It takes an *already-isolated* vocal and never splits songs into stems (§5).
3. **Preserve character above all.** Never trade vibrato/sustain/breath/timbre for more cleanup. Prefer the most transparent option; expose intensity so the user owns the tradeoff. Generative reconstruction must be labelled as such and must not change who the voice sounds like.
4. **Singing is first-class.** Speech-tuned processing damages sung vocals; singing presets are conservative by design (e.g., gentle, tail-preserving de-reverb; off/low breath control; split-band, gently-tuned de-ess). Singing quality is validated by a **mechanical gate** — objective character-preservation metrics (F0 cents, vibrato rate/depth, HNR, spectral-envelope cosine, tail survival) run on synthetic fixtures in CI — not a blind-listening release gate.
5. **Standard is simple, Advanced is deep — never force the manual.** Smart/adaptive defaults must produce a great result without a manual; full per-module control is available but optional. Mirrors the **yes-master** Standard/Advanced pattern.
6. **Offline, always; processing never needs internet.** Local/private is the promise. Quality must not suffer for it.
7. **Non-destructive and honest.** Originals preserved; same take/same length out; render non-destructively; let users hear exactly what was removed (A/B + difference monitor + reblend).
8. **No timeline editing.** Breath *attenuation* and mouth-click cleanup are defect-fixes and are in scope; cutting/trimming silences, comping, and rearranging are a DAW's job.
9. **Quality over real-time (for now).** Batch/offline rendering is primary; near-real-time preview is a later, intentionally-simpler convenience.
10. **Suite citizen, standalone-complete.** Consistent UX + clean handoffs with the Yes suite (§10), but fully usable — and sellable — on its own.

## 5. Scope — in and out

**In scope (the YesVox job):**
- **Restore:** denoise, de-reverb / reverb removal (mode-aware — assertive for voiceover, gentle/tail-preserving for singing), de-click / de-pop / de-crackle, **mouth-click / lip-smack cleanup**, de-hum, declip, spectral repair.
- **Finish (corrective default, impressive when wanted):** de-ess, **auto breath control** (attenuate harsh breaths; voiceover-leaning), adaptive presence/air, gentle leveling, **loudness to target**, optional ambience (later). An optional finish pass exists so the standalone output can impress.
- **De-bleed / separation-artifact cleanup:** cleans residual instrumental leakage, musical noise, chirpy-S on imported separated stems. Restoration in purpose; *not* a separator. (Sets expectation: cleans residue/artifacts, not heavy leftover instrumentation.)
- **Later (still identity):** bandwidth extension, deeper creative reverb, pitch/time correction, interactive spectrogram brush-heal editing.

**Out of scope (explicit non-goals):**
- **Source / stem separation** (de-mixing a song). User brings an isolated vocal and owns a separator. *(A neutral "vocal source provider" seam keeps this addable later, but it is not a YesVox feature.)*
- **Recording / capture** — processes already-recorded material.
- **Cloud processing / upload** of any kind.
- **Timeline / arrangement editing** — no cutting, trimming silences, comping, rearranging.
- **Mixing, mastering, multitrack, instrument processing, full production** — that's yes-master / a DAW. (YesVox delivers publish-*usable* level, not a final loud master.)
- **Archival/forensic restoration** of historical/degraded media (tape/vinyl/wax) — tuned for modern voice, not RX-depth archival rescue.

## 6. The experience

Two surfaces over one pro-grade engine (the yes-master pattern):

- **Standard view** — smart and adaptive. On load, YesVox **detects → applies → lets you override**: it analyzes content type (voice/sing), the defects present (noise, reverb, clicks, hum, clipping, band-limit, sibilance) and a tonal/EQ profile, then auto-picks a **content-aware preset** and pre-tunes the chain + intensity, applies it, and **shows what it found**. The user **picks a preset** from a gallery and adjusts **one intensity slider** (Natural ↔ Restoration HQ). **Content mode is an explicit Voiceover/Singing toggle — the user's choice is authoritative — assisted by smart detection** that reads the upload to pre-select the toggle and pre-tune the chain + intensity (always overridable). When detection confidence is low, it defers to the toggle rather than guessing. *(This is also the natural future mobile form.)*
- **Advanced mode** — every module exposed with full parameters, the spectrogram, manual overrides (incl. the detected mode), and (later) brush-heal editing.

Cross-cutting: A/B with loudness matching; **difference monitor + reblend** (hear and partially restore what was removed); non-destructive render queue; stem/file import + **batch**.

Canonical default order-of-operations: **declip → de-click/crackle (+ mouth-clicks) → de-hum → denoise → de-reverb → de-ess → breath control → (finish: presence/air → leveling) → loudness/limiter.** (Bandwidth extension slots before loudness when added.)

## 7. Offline, privacy & delivery

- **Offline guarantee:** core models ship **bundled in the installer** (works fully offline from first launch; larger download is the value). Optional heavier model packs (future max-quality tiers) download on demand. **Processing never needs internet.**
- **Licensing:** a one-time online activation (or offline key), then runs fully offline forever — no always-online checks.
- **Output:** WAV/FLAC (optional MP3) at the input's sample rate; metadata preserved; non-destructive; batch with clear naming.
- **Loudness targets:** a few presets — Podcast/Spoken −16 LUFS, Streaming/Music −14 LUFS, "Match input", Peak-normalize — defaulting sensibly per content type. Deliverable is **publish-usable**, not a final loud master (that's yes-master).

## 8. v1 (MVP) vs later

**v1 (the first lovable version):**
- **Restore:** denoise, de-reverb, de-click/pop (+ mouth-clicks), de-hum, declip.
- **Finish:** de-ess, **breath control**, and a **small set of content-aware broad-strokes presets** (Voiceover vs Singing differ vastly — validating the preset/adaptive approach is part of the MVP), each bundling restoration + a tasteful finish; **loudness targets**; **de-bleed**.
- **UX:** Standard (preset gallery + intensity, detect→apply→override) **and** a working Advanced module rack; both Voiceover/Singing; A/B + difference monitor; non-destructive render; stem import + batch; **desktop, offline**.

**Later (explicitly deferred — keep out of v1):** bandwidth extension; deep manual production controls (manual EQ-to-curve, compressor, saturation/air module, designed reverb); pitch/time correction; interactive spectrogram brush-heal; mobile Standard app; deep singing-specialized / max-quality generative model tiers.

## 9. Success criteria (definition of "working well")

All four matter; #1 is the moat.

1. **Pro-grade output, character intact.** Blind A/B: *professionally clean* **and** *still sounds like the same singer/speaker* — no audible artifacts (no musical noise, robotic/watery sound), no damage to vibrato/sustain/breath/timbre. **Singing held to the same bar as speech.**
2. **Pros trust and control it.** A professional runs it on client work because Advanced + difference monitor + non-destructive render make it transparent and safe.
3. **A non-engineer succeeds fast.** First-time user, Standard view, raw → an accepted result in under ~60 seconds, no manual.
4. **Fits the Yes suite.** stem → YesVox → yes-master feels like one coherent (music-first) suite.

Given (not negotiable): **offline/private with no quality penalty** vs leading cloud tools.

## 10. The Yes suite context

YesVox is one app in the **music-first "Yes" suite**, sharing a design language (**Standard + Advanced**, smart/adaptive processing — e.g., yes-master's EQ-curve detection):
- **yes-master** — mastering (Standard view also ships as a **mobile** app; Advanced for depth).
- **YesStems** — the suite's **stem separator** (demucs-based; already built). Kept as a **fully separate engine** (leaner YesVox MVP, voiceover-only users never carry a de-mixer, plugin/bundle flexibility). **v1 flow is a clean manual export → open** (YesStems writes a vocal WAV, YesVox opens it). A one-button "Send to YesVox" handoff is an **easy *later* add** — cheap precisely because both already speak WAV-in / WAV-out, so keeping the engines separate makes linking *easier*, not harder. **Designed-for, not built-in from day one.**
- **YesVox** — vocal restoration & polish (this product).

**Handoffs (suite-friendly minimum):** the separator can "bounce a stem to YesVox"; YesVox can hand a finished vocal to yes-master. Consistent Standard/Advanced UX. **YesVox remains fully standalone and individually sellable** (hence the impressive-finish requirement).

## 11. Key decisions (the record)

- **Identity = restoration + polish, not production.** Default corrective-clean; *optional impressive finish* so the standalone deliverable can wow (people buy YesVox individually).
- **Input = an already-isolated vocal; no separator *inside* YesVox** — keep a de-bleed module instead (removes the worst model-weights licensing risk). Separation lives in **YesStems** (sibling product, demucs-based, **already built**), a **fully separate engine**. v1 is a **manual export → open** flow; a one-button "Send to YesVox" handoff is a cheap ***later*** add (both speak WAV-in / WAV-out). **Seam designed-for, not built-in from day one** — don't fuse the engines.
- **Pro-grade, broadly accessible** via **Standard + Advanced** (mirrors yes-master).
- **Standard UX = one preset gallery + one intensity slider**, fed by **detect→apply→override** auto-analysis. **Content mode = an explicit Voiceover/Singing toggle (user-authoritative), assisted by smart detection** (pre-selects the toggle + pre-tunes the chain). Chosen over pure auto-detection deliberately: a misclassification (voiceover chain on a sung vocal, or the reverse) is far worse than one deliberate click; the toggle also lets us honestly market **"does both"**; and it keeps **singing a separable mode** — the toggle is the safety net, so a voiceover-only user never carries the singing path.
- **Both voiceover and singing; singing first-class** (music-first suite). Mode-aware processing (e.g., conservative de-reverb on singing, split-band gentle de-ess).
- **Singing quality is validated by a *mechanical gate*** (updated 2026-06-30). Character-preservation on sung vocals is the moat, so it's proven objectively in CI: the `singing` metrics (F0 cents, vibrato rate/depth, HNR, spectral-envelope cosine, tail survival) run on synthetic fixtures with known ground truth. This **replaced the original blind-listening release gate** — singing ships first-class, no human-ears ship-blocker; the toggle keeps it separable if ever needed. (An optional human listen remains available but never blocks.)
- **Input range = modern vocals across the quality spectrum** (studio → noisy Zoom/field), **not** archival/forensic.
- **Editing scope = defect-style processing + auto breath control + mouth-click cleanup; NO timeline editing.** Same take/length out.
- **Offline = bundled core models + one-time activation; processing always local.** Optional advanced model packs download on demand.
- **Output = lossless (WAV/FLAC, +MP3) at input rate + sensible loudness targets**; publish-usable, non-destructive, batch. Final loud master = yes-master.
- **v1 = restoration core + de-ess + breath control + content-aware finish presets + loudness + de-bleed**, Standard + Advanced, desktop. Deep manual production, BWE, pitch, spectrogram-heal, mobile = later.
- **Hook = offline + character-preserving studio quality, great at singing AND voiceover.**
- **Suite-friendly via two cheap seams** (vocal-source-provider, stem-handoff) so separation/handoffs stay additive, not rewrites.

## 12. Open questions (for planning, not blocking)

- **Tech stack** final pick (Rust + egui vs C++ + Qt) — analysis in the master brief; decide at planning.
- **The actual preset set** — concrete v1 presets per content type (e.g., Podcast / Narration / Dialogue-cleanup; Lead Vocal / Natural Vocal / …) and the adaptive "smart filter" tuning behavior.
- **De-bleed trigger** — auto-detect separation artifacts vs a "this is a separated stem" preset/toggle; pure-DSP vs small ML for v1.
- **Breath control in v1** — confirm it's v1 (vs v1.x), and its default per content mode.
- **Session/project model** — open-process-export each time vs saved sessions (settings persistence, reopen).
- **Mobile Standard view** for YesVox someday? (yes-master has one; desktop is primary now.)
- **Pricing / licensing model** and the suite-bundle relationship.
- **Platforms** — Windows + macOS first (per research); Apple Silicon priority; Linux later.
- **v1 model choices** (research leans DeepFilterNet3 for preview + MossFormer2_SE_48K for batch quality — confirm at planning).

---

*Pointers:* technical groundwork → [`docs/research/consolidated-voice-restoration-research.md`](research/consolidated-voice-restoration-research.md) (master brief: DSP math, model licensing, stack, phased plan) and [`docs/research/5th-deep-research-newest-tech-and-dsp-math.md`](research/5th-deep-research-newest-tech-and-dsp-math.md) (newest-tech detail + full citations).
