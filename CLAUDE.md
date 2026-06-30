> `AGENTS.md` and `CLAUDE.md` are kept **byte-identical** — edit both together.

# YesVox — repo guidance for agents

**YesVox** is a standalone, **offline** desktop app that takes an already-isolated vocal (voiceover or singing) and makes it sound **clean, clear, and finished** — restoration + essential polish.

## Read this first

**Before any substantive work, read [`docs/product.md`](docs/product.md).** It is the canonical product north-star; when scope or intent is fuzzy, **it decides**. Technical groundwork (architecture, DSP math, model licensing, tech stack, phased plan) is in [`docs/research/consolidated-voice-restoration-research.md`](docs/research/consolidated-voice-restoration-research.md). The concrete v1 architecture/build plan is [`docs/plans/2026-06-29-feat-yesvox-v1-restoration-workstation-plan.md`](docs/plans/2026-06-29-feat-yesvox-v1-restoration-workstation-plan.md); the build is tracked as **GitHub issues #1–#21** (build in dependency order; `afk` = agent-buildable end-to-end, `hitl` = needs Dan — a decision or a listening call).

## Non-negotiable guardrails (don't drift, even before reading product.md)

- **Restoration & finish, NOT production.** Fix/finish one vocal (denoise, de-reverb, de-click/pop, de-hum, declip, de-ess, loudness, de-bleed). Do **not** add mixing/mastering/multitrack/instrument/full-production features — that's yes-master or a DAW.
- **No source/stem separator inside YesVox.** YesVox takes an *already-isolated* vocal; the voice is the wanted signal, not something to de-mix. (A small **de-bleed** module cleans imported separated stems — that's restoration, not separation.) Separation is a **separate engine — YesStems** (suite sibling); v1 links via a manual export → open, **not** a built-in handoff. Don't fuse them.
- **Offline only.** No cloud, no upload, no network dependency for processing.
- **Preserve vocal character above all** — never trade vibrato/sustain/breath/timbre for more noise removal.
- **Standard + Advanced** progressive disclosure (mirrors yes-master): smart/adaptive defaults that don't require a manual, with full pro depth available. Non-destructive; show what was removed (A/B + difference monitor).
- **Content mode = an explicit Voiceover/Singing toggle** (user-authoritative), assisted by smart detection — *not* auto-detected-only. Keeps singing a **separable mode** that can ship later if it can't clear the quality bar; **validate singing quality early, as a gate.**
- **v1 = restoration core only.** Bandwidth extension, creative reverb, pitch/time, spectrogram brush-heal editing, and mobile are explicitly **later** — don't let them creep into v1.

## Engineering discipline (how we build)

Mirrors the proven discipline of the Yes suite (yes-master, yes-daw). Not optional.

- **Mechanical verification only.** Dan does not read diffs and will not hand-verify by ear or eye. Every check must be **mechanical** — a self-asserting test that exits 0/1, a CI gate, or a golden compare. If a property *can* be checked in code (output==input null, no network during processing, byte-hash non-destructive, same-length, loudness within tolerance, no double-repair), it **must** be. The *only* place human ears are the gate is explicit blind-listening sign-off — the singing gate (#16) and final eval (#21) — and those are marked `hitl`. Never call a slice done because it "looks right"; verify the behavior.
- **One canonical gate.** `cargo xtask gate` runs the whole gate: `cargo fmt --check` → `cargo clippy --all-targets -- -D warnings` → `cargo test` → `cargo deny check`. CI runs the **identical** command on every push. "Done" = the gate is green. Fix red before stopping.
- **Commit in small green chunks, straight to `main`.** Every green slice is its own commit — high commit count, **never squash**, each commit independently green so `git bisect run cargo test` stays mechanical. Conventional-commit prefixes scoped to the ticket: `feat(#3): de-hum notch cascade`, `test(#1): null gate`, `chore(ci): …`, `docs: …`. CI on every push is the safety net; no PR required (solo + agents). Commit message footer: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
- **Goldens are blessed deliberately, never silently.** DSP byte-identity buffers and analysis-report goldens regenerate **only** via an explicit `YESVOX_BLESS=1` path (`cargo xtask bless`) that an autonomous loop cannot trigger as a side-effect. A loop that "fixes" a failing golden by re-blessing it is a bug, not a fix.
- **Warnings are errors on our code** (`-D warnings`); vendored deps are exempt (`SYSTEM`-equivalent). A pre-commit hook (`.githooks/pre-commit`, wired via `core.hooksPath`) runs `cargo fmt --check` + `cargo clippy` on staged Rust so red never reaches `main`.
- **Tooling we add that the siblings lacked:** `proptest` for value types, `cargo-deny` + `cargo-audit` for code-license + advisory hygiene (the model-**weights** license audit is separate — see plan §7.5), and Criterion for the <60 s perf budget. Full lane reference: [`docs/TESTING.md`](docs/TESTING.md).

## Status

**Build kickoff (2026-06-29).** Product definition + v1 architecture/build plan complete; implementation underway starting at **#1** (core boundary + WAV/FLAC I/O skeleton + the four CI guarantee gates). Stack: **Rust + egui + `ort`/`tract`**. Part of the music-first **"Yes" suite** (yes-master, yes-daw, YesStems, YesVox).
