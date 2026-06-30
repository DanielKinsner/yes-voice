---
date: 2026-06-30
topic: content-mode-toggle-and-yesstems-handoff
---

# YesVox — Content-Mode Toggle, YesStems Handoff & Singing Gate

**Outcome:** three refinements folded into the canonical [`docs/product.md`](../product.md) (§6, §10, §11). This file is the dated record of how they were reached.

## Context

Review of the v1 plan surfaced three things to pin down: how content mode (voiceover vs singing) is chosen, how separation relates to YesVox now that **YesStems already exists**, and how to handle the singing-quality risk.

## Decisions

### 1. Content mode = explicit Voiceover/Singing TOGGLE, smart-assisted (not pure auto-detection)
- The **toggle is authoritative**; **smart detection** reads the upload to pre-select it and pre-tune the chain + intensity (always overridable). When detection confidence is low, it **defers to the toggle rather than guessing.**
- **Why an explicit toggle over auto-detection alone:**
  - **Misclassification is the worst failure.** Applying a voiceover chain to a sung vocal (or vice-versa) ruins the result. One deliberate click removes the single thing most damaging to "get wrong."
  - **Marketing:** lets us honestly say **"does both"** — a real differentiator ("wait, it does singing AND voiceover?").
  - **De-risks the singing bet:** keeps **singing a separable mode we can pull from the engine** if it can't clear the quality bar — ship voiceover first, add singing once proven, without re-architecting.
- Friction is acknowledged and accepted (one click) as a deliberate trade for correctness + the above.

### 2. Separation stays separate (YesStems), with a seamless handoff
- **YesStems** (sibling product, **demucs-based, already built**) owns separation. YesVox does **not** weld a de-mixer in.
- **Why separate:** leaner YesVox MVP; voiceover-only users (a big share) never carry a de-mixer; genuinely different tech; plugin/bundle flexibility.
- **But kill the step-zero friction with an invisible handoff:** a singer drops a full song, one button routes it through YesStems → YesVox already-separated — **"song in → clean vocal out"** — two engines, one experience. (Or a suite bundle wires the path.) This resolves the "bring your own isolated vocal" gap for singers without bloating YesVox.

### 3. Singing quality is validated EARLY, as a gate
- Character-preservation on **sung** vocals (vibrato/breath/sustain intact through denoise/de-reverb) is **both the moat and the hardest technical bet.** Prove it *before* building the rack around it. Speech-trained SE models can smear held notes — validate (SingVERSE/EDB) up front. The toggle (Decision 1) is the safety valve if it can't clear the bar.

## What did NOT change
- Identity (restoration + polish, not production), offline-only, preserve-character-above-all, Standard/Advanced, v1 restoration core — all hold.
- The "Yes" suite is **further along than a single-app effort**: yes-master ready to ship (has a landing page), **YesDAW ~70%**, YesStems built. This is a **modular ecosystem**, which reframes "focus" — each app is focused and they compose via clean handoffs.
