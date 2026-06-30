# YesVox — repo guidance for agents

**YesVox** is a standalone, **offline** desktop app that takes an already-isolated vocal (voiceover or singing) and makes it sound **clean, clear, and finished** — restoration + essential polish.

## Read this first

**Before any substantive work, read [`docs/product.md`](docs/product.md).** It is the canonical product north-star; when scope or intent is fuzzy, **it decides**. Technical groundwork (architecture, DSP math, model licensing, tech stack, phased plan) is in [`docs/research/consolidated-voice-restoration-research.md`](docs/research/consolidated-voice-restoration-research.md).

## Non-negotiable guardrails (don't drift, even before reading product.md)

- **Restoration & finish, NOT production.** Fix/finish one vocal (denoise, de-reverb, de-click/pop, de-hum, declip, de-ess, loudness, de-bleed). Do **not** add mixing/mastering/multitrack/instrument/full-production features — that's yes-master or a DAW.
- **No source/stem separator.** YesVox takes an *already-isolated* vocal; the voice is the wanted signal, not something to de-mix. (A small **de-bleed** module cleans imported separated stems — that's restoration, not separation.)
- **Offline only.** No cloud, no upload, no network dependency for processing.
- **Preserve vocal character above all** — never trade vibrato/sustain/breath/timbre for more noise removal.
- **Standard + Advanced** progressive disclosure (mirrors yes-master): smart/adaptive defaults that don't require a manual, with full pro depth available. Non-destructive; show what was removed (A/B + difference monitor).
- **v1 = restoration core only.** Bandwidth extension, creative reverb, pitch/time, spectrogram brush-heal editing, and mobile are explicitly **later** — don't let them creep into v1.

## Status

Planning/research stage — no application code yet. Product definition done via `/workflows:brainstorm`; deep research via `/deep-research`. Part of the **"Yes" suite** (yes-master, a separate stemmer, YesVox).
