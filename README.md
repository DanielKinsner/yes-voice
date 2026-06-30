# YesVox

**Offline vocal restoration & polish for an already-isolated vocal — voiceover *and* singing.**

YesVox takes a flawed or raw isolated vocal and makes it sound **clean, clear, and — when you want — genuinely polished**, entirely on-device, without making it stop sounding like the performer. It's part of the music-first **"Yes" suite** (alongside yes-master and a stem separator).

## At a glance

- **Offline & private** — local processing, no cloud, no upload. Processing never needs internet.
- **Isolated vocal in** — restoration & polish, *not* source separation (you bring the vocal; YesVox doesn't de-mix songs).
- **Restoration + essential polish** — denoise, de-reverb, de-click/pop, de-hum, declip, de-ess, breath control, de-bleed, loudness — *not* a full mixing/mastering/production suite.
- **Voiceover and singing, equal** — with singing held first-class (a content-aware mode picks the right chain).
- **Pro-grade, broadly accessible** — a smart **Standard** view (pick a preset + intensity; it auto-detects and pre-tunes) over a deep **Advanced** mode.
- **Standalone & premium** — quality over real-time; batch/offline rendering first.

## Documentation

> **Start here:** [`docs/product.md`](docs/product.md) is the **canonical product north-star** — when intent or scope is fuzzy, it decides. [`CLAUDE.md`](CLAUDE.md) auto-loads it for AI agents.

| Doc | What it is |
|---|---|
| [`docs/product.md`](docs/product.md) | **Canonical product definition** — identity, positioning, scope, Standard/Advanced UX, v1 line, offline/delivery, success criteria. |
| [`CLAUDE.md`](CLAUDE.md) | Repo guidance + non-negotiable guardrails for AI agents; points to the docs above. |
| [`docs/brainstorms/`](docs/brainstorms/) | Dated product-definition decision records. |
| [`docs/research/consolidated-voice-restoration-research.md`](docs/research/consolidated-voice-restoration-research.md) | **Master technical brief** — architecture, DSP math, model licensing, tech stack, phased plan. |
| [`docs/research/5th-deep-research-newest-tech-and-dsp-math.md`](docs/research/5th-deep-research-newest-tech-and-dsp-math.md) | Newest-tech (mid-2026) research + DSP math, with full citations. |
| `docs/research/*` (earlier passes) | Prior deep-research passes kept for source/citation depth. |

## Status

Planning / research stage — **no application code yet.** The product definition is complete (see `docs/product.md`); the next step is turning the v1 scope into an architecture/build plan.
