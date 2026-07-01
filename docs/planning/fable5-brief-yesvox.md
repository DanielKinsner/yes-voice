# Fable 5 Planning Brief — YesVox (v1 → sellable)

> Pair this with [`fable5-planning-constitution.md`](fable5-planning-constitution.md).
> **You have the repo.** Read §A in order before planning. Produce the full output
> contract from Constitution §3. Target: a plan that carries YesVox from where it
> is today all the way to a **sellable, installable, offline** desktop product.
>
> Provenance: baseline 2026-07-01 (Claude Opus 4.8); being refined against an
> ensemble of parallel Claude-4.8 passes — divergences merge in as improvements or
> as new Open Questions.

---

## A. Read first (canonical order; which wins)

1. **`docs/product.md`** — the canonical north-star; **wins every conflict on
   product intent.** §5 = scope fence, §8 = v1-vs-later, §9 = success criteria,
   §11 = decisions ledger, §12 = open questions.
2. **`CLAUDE.md` / `AGENTS.md`** (byte-identical) — guardrails + engineering
   discipline: the one gate, commit style, mechanical-verification rule, the
   goldens-bless rule.
3. **`docs/plans/2026-06-29-feat-yesvox-v1-restoration-workstation-plan.md`** —
   the existing v1 architecture/build plan. Already covers the RT-safe core
   (§1), the ML/denoise pipeline (§2), deterministic DSP repair + interaction
   matrix (§3), restore/finish chain (§4), Standard/Advanced UX + analysis + A/B +
   presets + state (§5), the **singing gate & eval protocol** (§6), **offline /
   packaging / licensing / platforms / CI guarantee tests** (§7), and the **phased
   build sequence + risk register + acceptance matrix** (§8). **Your job is to
   validate, deepen, and extend §7–§8 into an executable, sellable-grade spec —
   not to reinvent it.**
4. **`docs/STATUS.md`** — resume-from-here: what's built & green, what's next,
   decisions on record.
5. **`docs/TESTING.md`** — the gate + the four guarantee gates + goldens-bless.
6. **`docs/research/consolidated-voice-restoration-research.md`** — master
   technical brief (DSP math, model licensing, stack). See also
   `docs/research/5th-deep-research-newest-tech-and-dsp-math.md` (newest tech) and
   `docs/singing-research-decisions.md` (why the singing gate is mechanical).
7. **`docs/ui/yesvox-v1-ui-reference.md`** — the GUI target.
8. **GitHub issues #1–#21** — the tracked build (`afk` = agent-buildable
   end-to-end, `hitl` = needs Dan: a decision or a listening call).

## B. Current state (plan forward, not from zero)

- **Stack decided** — Rust + egui (eframe 0.35) + `ort`/`tract`. Do **not**
  re-litigate the stack.
- **Green today** — the `yesvox-core` rack + the four CI guarantee gates; the
  agent-callable `Session` surface; a custom-painted egui Standard-view shell
  (`yesvox-gui`). ~180 core tests pass. `cargo xtask gate` =
  fmt → clippy `-D warnings` → test → `cargo deny check`.
- **Shipped this phase** — Standard-view punch list G0–G17 and render-queue
  #19a–#19i (see STATUS for the table). The neural engine is behind a `neural`
  cargo feature + `YESVOX_MODEL_PATH`; **without weights it runs the deterministic
  light engine** (Balanced/HQ fall back, reported in status). Weights are **not**
  committed (licensing — plan §7.5).
- **Not yet resolved / not yet built** — see the spine (§D). The to-sellable layer
  (§D.7) is sketched in plan §7 but not yet an executable, staged spec.

## C. Non-negotiable guardrails (violating any is a failed plan)

- **Restoration & finish, NOT production.** No mixing/mastering/multitrack/
  instrument/beat features.
- **No source/stem separator inside YesVox.** De-bleed cleans *imported separated
  stems*; it is not a separator. Separation is **YesStems** (sibling), linked by a
  manual export→open in v1 — designed-for, not built-in.
- **Offline only.** No cloud or network in the processing path, ever.
- **Preserve vocal character above all.** Never trade vibrato/sustain/breath/
  timbre for more cleanup. Generative reconstruction, if any, must be labelled and
  must not change *who* the voice sounds like.
- **Voiceover/Singing = an explicit, user-authoritative toggle**, smart-assisted,
  never auto-only. **Singing is first-class**, validated by the **mechanical
  singing gate** (F0 cents, vibrato rate/depth, HNR, spectral-envelope cosine,
  tail survival on synthetic fixtures) — *not* a blind-listening ship-blocker.
- **Mechanical verification only.** If a property can be checked in code, it must
  be. No "looks/sounds right" gates. Goldens re-bless **only** via explicit
  `YESVOX_BLESS=1`; a plan that "fixes" a failing golden by re-blessing is a bug.
- **No timeline editing.** Breath attenuation + mouth-click cleanup are defect
  fixes and in-scope; cutting/trimming/comping is not. **Same take, same length,
  same sample rate out.**
- **v1 = restoration core only.** Bandwidth extension, creative reverb, pitch/time,
  spectrogram brush-heal, and mobile are explicitly *later* — keep them off the v1
  critical path.

## D. The spine (specific questions I need answered — resolve, don't dodge)

Resolve each into the plan; where one genuinely needs me, escalate it in Open
Questions **with a recommended default.**

1. **The concrete v1 preset set** — the actual Voiceover presets (Podcast /
   Narration / Dialogue-cleanup …) and Singing presets (Lead / Natural …), each
   bundling restoration + a tasteful finish, plus the adaptive "smart-filter"
   tuning behavior per preset. (product.md §12; plan §5.7.)
2. **De-bleed trigger** — auto-detect separation artifacts vs an explicit "this is
   a separated stem" toggle/preset; pure-DSP vs small ML for v1. (plan §4.5.)
3. **Breath control in v1** — confirm it's v1 (vs v1.x) and its default per content
   mode.
4. **Session/project model** — open-process-export each time vs saved sessions
   (settings persistence, reopen). (plan §5.8.)
5. **G18 undo/redo scope** — Revert-only, or a real undo stack? (STATUS flags this
   as explicitly needing my decision — recommend one, with the tradeoff.)
6. **v1 model choices** — confirm DeepFilterNet3 (preview) + MossFormer2_SE_48K
   (batch quality); the ONNX-export discipline (plan §2.3); the footprint budget +
   on-demand model-pack strategy (plan §2.5, §7.1).
7. **The to-sellable layer** — extend plan §7 into an executable, staged spec:
   - Installer + **offline core-model bundling** (fully offline from first launch;
     the larger download *is* the value).
   - Optional heavier model packs — hosting, download, integrity/versioning —
     without breaking the offline guarantee.
   - **Activation / licensing** — one-time online activation *or* offline key, then
     fully offline forever (no always-online checks): the mechanism, key format,
     and the piracy-vs-friction tradeoff.
   - **Cross-platform** — Windows + macOS first (Apple Silicon priority), Linux
     later: code-signing + **notarization**, the execution-provider/runtime matrix
     (plan §7.4).
   - First-run experience, update mechanism, and a crash/telemetry stance that
     stays offline/private.
   - Pricing / suite-bundle relationship (product.md §12) — enough to ship a
     sellable *standalone*.

## E. Cook here — use your own judgment, exceed the brief (named zones)

In these zones I *want* design I didn't ask for. Propose what I haven't thought
of; go deep.

- **The "impressive finish" module.** product.md requires the *optional* finish to
  make the standalone output genuinely *wow* (people buy YesVox on its own).
  Define what "tasteful, character-preserving, impressive" concretely means as
  adaptive presence/air + gentle leveling + optional ambience — and how it stays
  honest (labelled, non-destructive, reblendable).
- **Smart-detection / analysis→apply→override tuning.** The detect-defects-and-
  pre-tune behavior that makes Standard feel magic with no manual (plan §5.1–5.2):
  how confident, how conservative, and how it defers to the toggle on low
  confidence.
- **Packaging + licensing architecture.** Beyond the checklist: the cleanest
  offline-first design a small team can actually ship and maintain across two OSes.
- **Go-to-market framing & pricing** (advisory; flag as non-engineering) — the
  honest positioning and a defensible price/bundle for a private, offline,
  character-preserving tool that does singing *and* voiceover.
- **Performance-budget strategy** — the <60 s batch budget (Criterion): where the
  risk concentrates and the plan to hold it as the neural tier lands.
- **Sequencing to sellable** — if you see a faster or safer path to a shippable,
  sellable v1 than the current issue order, propose it and justify it.

## F. The ship line (concrete "sellable" for YesVox)

**Done =** a private individual can buy it, install it on Windows or macOS, and
from first launch — **fully offline** — take a raw isolated vocal (voiceover *or*
singing) to a **publish-usable, character-intact** result via **Standard** (preset
gallery + intensity, detect→apply→override) with a working **Advanced** rack, plus
A/B + difference monitor + reblend, non-destructive render, stem import + batch.
Ships as a **signed/notarized installer** with bundled core models, activation,
first-run, and an update path. `cargo xtask gate` green; the **four guarantee
gates + the mechanical singing gate** green; the **<60 s** perf budget met; output
publish-usable (not a final loud master — that's yes-master). **Sharpen this into
an acceptance-criteria matrix** (extend plan §8.4) that the staged plan provably
satisfies end-to-end.

## G. Constraints on the plan's shape

- **Dependency-ordered; small green commits straight to `main`; never squash;**
  each commit independently green so `git bisect run cargo test` stays mechanical.
  Conventional-commit prefixes scoped to the ticket (`feat(#3): …`, `test(#1): …`).
- **"Done" = `cargo xtask gate` green.** Every task's *Done-when* is mechanical.
- Respect the **four guarantee gates** and the **goldens-bless** discipline.
- **Tag every task `afk`/`hitl`.** Minimize `hitl`; when a task needs me, say
  exactly what decision or listening/visual call I must make.
- Keep the **v1-vs-later fences** intact (product.md §8; plan §8.3 / §8.3b permanent
  non-goals).
