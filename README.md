# YesVox

**Offline vocal restoration & polish for an already-isolated vocal — voiceover *and* singing.**

YesVox takes a flawed or raw isolated vocal and makes it sound **clean, clear, and — when you want — genuinely polished**, entirely on-device, without making it stop sounding like the performer. It's part of the music-first **"Yes" suite** (alongside **yes-master**, **YesDAW**, and **YesStems** — the demucs-based stem separator that hands an isolated vocal to YesVox).

## At a glance

- **Offline & private** — local processing, no cloud, no upload. Processing never needs internet.
- **Isolated vocal in** — restoration & polish, *not* source separation (you bring the vocal; YesVox doesn't de-mix songs).
- **Restoration + essential polish** — denoise, de-reverb, de-click/pop, de-hum, declip, de-ess, breath control, de-bleed, loudness — *not* a full mixing/mastering/production suite.
- **Voiceover and singing, equal** — with singing held first-class (an explicit **Voiceover/Singing toggle**, smart-assisted, picks the right chain).
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

**Release candidate — `1.0.0-rc.2`.** This is a working app, not a plan. Rust workspace (`yesvox-core` lib · `yesvox-app` headless CLI `yesvox` · `yesvox-gui` egui shell · `xtask`); neural denoise via MossFormer2 (ONNX Runtime) plus the full restoration/polish rack; Standard **and** Advanced views; batch/queue rendering; offline-enforced at the source level. An **unsigned Windows installer** was rebuilt and locally smoke-tested (install → `--version` → neural render → uninstall) on 2026-07-07.

What's left before a public v1.0 is **verification and signing, not feature work**: a clean-Windows-VM installer smoke ([#20](https://github.com/DanielKinsner/yes-voice/issues/20)), a real vocal-stem eval corpus + full-suite round-trip ([#21](https://github.com/DanielKinsner/yes-voice/issues/21)), and code-signing (shared with the rest of the "Yes" suite) to clear the SmartScreen wall. Canonical build snapshot: [`docs/STATUS.md`](docs/STATUS.md).
