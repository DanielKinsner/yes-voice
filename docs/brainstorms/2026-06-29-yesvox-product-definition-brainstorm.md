---
date: 2026-06-29
topic: yesvox-product-definition
---

# YesVox — Product Definition Brainstorm

**Outcome:** the canonical product north-star lives in [`docs/product.md`](../product.md). This file is the dated session record of how those decisions were reached; `product.md` is the living source of truth.

## What We're Building

**YesVox** — a standalone, offline desktop app that takes an already-isolated vocal (voiceover or singing) and makes it sound clean, clear, and finished: **restoration + essential polish, not production.** Pro-grade engine under an approachable surface (Standard + Advanced views, mirroring yes-master). Part of the "Yes" suite.

## Why This Approach

Explored three identity options (restoration-purist / restoration+polish / full finishing chain) → chose **restoration + essential polish** to keep scope tight and avoid competing with DAWs and yes-master. Explored three interaction models → chose **smart auto-chain + full pro depth (Standard/Advanced)** because it matches the "pro but broadly accessible" target and the existing yes-master pattern. Explored three v1 boundaries → chose the **focused restoration MVP** (YAGNI: defer BWE, creative, pitch, spectrogram-heal, mobile).

## Key Decisions

- **Identity:** restoration + essential polish ("clean, clear, finished"), not mixing/mastering/production.
- **Input:** already-isolated vocal; **no separator** (de-bleed module instead); suite-friendly seams.
- **Audience:** pro-grade, broadly accessible → **Standard + Advanced** progressive disclosure.
- **Content:** voiceover + singing, equal, auto-detected mode toggle (chains differ).
- **Constraints:** offline-only; quality > real-time; non-destructive; preserve vocal character above all.
- **v1:** restoration core (denoise, de-reverb, de-click/pop, de-hum, declip, de-ess, loudness, de-bleed), Standard + Advanced, desktop. Everything else = later.
- **Success (ranked):** pro-grade output character-intact → pros trust & control → non-engineer succeeds fast → fits the Yes suite. (Offline-no-quality-penalty is a given.)
- **Suite:** Yes suite (yes-master, stemmer, YesVox); shared Standard/Advanced UX + clean handoffs; YesVox standalone-complete.

## Open Questions

- Tech stack final pick (Rust+egui vs C+++Qt — see master brief).
- Mobile Standard view for YesVox someday?
- De-bleed: pure-DSP vs small ML for v1.
- Standard preset taxonomy + adaptive "smart filter" behavior.
- Pricing/licensing model and suite relationship.
- v1 model choices (research leans DeepFilterNet3 + MossFormer2_SE_48K — confirm at planning).

## Next Steps

→ `/workflows:plan` to turn the v1 scope (§6 of `product.md`) into a concrete architecture/build plan.
