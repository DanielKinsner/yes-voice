# YesVox — build status & pick-up guide

> Resume-from-anywhere snapshot. Last updated **2026-06-30**. For product intent
> read [`product.md`](product.md) first; for the build plan see
> [`plans/2026-06-29-feat-yesvox-v1-restoration-workstation-plan.md`](plans/2026-06-29-feat-yesvox-v1-restoration-workstation-plan.md);
> for the GUI target see [`ui/yesvox-v1-ui-reference.md`](ui/yesvox-v1-ui-reference.md).

## Where we are

Building **#17 — the Standard view**. The engine (`yesvox-core` rack) already
existed; this phase builds the agent-callable, mechanically-tested `Session`
surface the UI needs, then a thin **egui shell** over it. Both are underway and
**green**.

- **Workspace:** `yesvox-core` (lib), `yesvox-app` (headless CLI, bin `yesvox`),
  **`yesvox-gui`** (the egui app, bin `yesvox-gui`, **new**), `xtask`.
- **The gate is green.** `cargo xtask gate` = fmt → clippy `-D warnings` → test →
  `cargo deny check`. 165 core tests + the GUI compile/tests pass.

## Build / run / verify (any machine)

```bash
cargo xtask gate            # the one canonical gate (must be green to call it done)
cargo run -p yesvox-gui     # launch the GUI (opens a window)
cargo run -p yesvox -- in.wav out.wav   # headless CLI
```

- **`Cargo.lock` is committed** — another machine resolves the identical
  dependency tree (the one whose licenses were audited for `deny.toml`).
- **Neural weights are not committed** (gitignored; plan §7.5). The neural engine
  is behind the `neural` cargo feature + a `YESVOX_MODEL_PATH` env var; **without
  it the app runs fine on the deterministic light engine** (Balanced/HQ fall back
  to light, reported in the status/CLI output). No model needed to build or demo.
- Requires a normal desktop GPU/GL for the GUI (eframe/glow). Headless machines
  can still run `cargo xtask gate` and the CLI.

## Done this phase (each TDD'd, its own green commit)

A multi-agent audit of the UI reference vs the core produced a punch list
(G0–G18). Shipped:

| ID | What |
|----|------|
| **G0** | 🐛 **Bug fix:** `render()` ignored the gallery-selected preset (only the content-mode default rendered — loudness/EQ/de-reverb character silently dropped). `build.rs` now takes the selected preset. |
| **G1** | **Card model** (`CardId` → Stage(s)) — the UI's 10 cards are *not* 1:1 with engine stages; one card can drive two stages, three share one. `resolve_stage_enabled` folds the preset default for the toggle. |
| **G2** | **Flow Amount** global dry→wet master (nulls the whole rack at 0). |
| **G3** | **Restoration Focus** radial → quantized onto `QualityMode`. |
| **G4** | **Polish Intensity** radial → scales the polish group (de-ess+breath; default = nominal). |
| **G5** | **Loudness target** settable on the Session (+`LoudnessTarget::Custom(f32)`). |
| **G12/13** | **OUTPUT + FILE** panels — `OutputFormat`, bit-depth/dither, name/location, `export_target()`/`export_to_target()`. |
| **G15** | **Volume Match** — `ab_matched_input()`. |
| **G17** | **Waveform peaks + channel meters** — `monitor::waveform_peaks / channel_peak_db / channel_rms_db`. |
| **GUI** | **egui Standard-view shell** (`crates/yesvox-gui`) wired to all of the above. |

## Next up (remaining mechanical gaps)

Build order, mechanical-first, each TDD'd:

1. **G6** — FinishStage sub-enables so Presence/Air, Leveling, Loudness are
   *independent* cards (today they share one Finish stage/knob).
2. **G10** — Presence/Air amount reachable from the Session.
3. **G11** — per-kind DeClick: split Click vs MouthClick so the two transient
   cards drive independently (today Mouth-Clicks shares De-click's knob).
4. **G7** — Smart Assist: auto-configure card amounts/enables from the analysis
   report (noisier → more denoise, etc.).
5. **G8** — Vocal Isolation Check: re-analyze the rendered output → residual
   noise/reverb/bleed verdict.
6. **G9** — **real new DSP**: the program leveler behind the "Leveling" card
   (FinishStage has none today; the doc comment was aspirational). Largest item.
7. **G18** — undo/redo. **Needs a scope decision from Dan** (unspec'd in
   product.md/plan; ship Revert-only, or a real undo stack?).

Then GUI polish: native file picker (`rfd`), theme/spacing fidelity vs the mock,
and (deferred) the playback transport + spectral-monitor/reblend (#18).

## Decisions on record

- **Two intensity dials** (Restoration Focus + Polish Intensity) kept over the
  older "one slider" plan language — the axes genuinely differ (`QualityMode`
  only binds the denoiser; polish has no engine binding).
- **Sample rate = read-only** (export SR locked to input — hard guardrail + the
  same-length CI gate). No SR-override API; the OUTPUT panel shows it as a chip.
- **Deferred:** playback transport, spectral monitor + reblend slider (#18
  Advanced), background-thread progress, native file picker, "Save as New Preset"
  + "Add to Suite Library" (stubbed — no user-preset store; suite handoff is a
  designed-for seam, not built-in).

## GUI dependency note (read before touching the shell)

`yesvox-gui` is on **eframe 0.35**, which is a **redesigned egui API** vs older
versions: implement `App::ui(&mut self, ui, frame)` (not `update(ctx, frame)`);
panels are the unified `egui::Panel::left/right/top/bottom(id).exact_size(n)
.show(ui, …)` (the old `SidePanel`/`TopBottomPanel` are gone; only `CentralPanel`
remains); theming via `ctx.set_visuals` + `ctx.all_styles_mut`. `deny.toml`
carries four scoped, permissive license exceptions for the egui tree
(hexf-parse CC0, clipboard-win/error-code BSL-1.0, epaint_default_fonts
OFL-1.1/Ubuntu-font). Visual fidelity is verified by eye; widget logic is tested
in `yesvox-core`.
