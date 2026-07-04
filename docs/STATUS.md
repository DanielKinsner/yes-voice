# YesVox — build status & pick-up guide

> Resume-from-anywhere snapshot. Last updated **2026-07-04**. For product intent
> read [`product.md`](product.md) first; for the build plan see
> [`plans/2026-06-29-feat-yesvox-v1-restoration-workstation-plan.md`](plans/2026-06-29-feat-yesvox-v1-restoration-workstation-plan.md);
> for the GUI target see [`ui/yesvox-v1-ui-reference.md`](ui/yesvox-v1-ui-reference.md).

## Where we are

**#17 — the Standard view is closed.** The engine (`yesvox-core` rack) already
existed; this phase built the agent-callable, mechanically-tested `Session`
surface the UI needs, then a custom-painted **egui shell** over it. The current
pick-up item is **#35 playback/worker polish**, then **#18 Advanced build**.

- **Workspace:** `yesvox-core` (lib), `yesvox-app` (headless CLI, bin `yesvox`),
  **`yesvox-gui`** (the egui app, bin `yesvox-gui`, **new**), `xtask`.
- **The gate is green.** `cargo xtask gate` = fmt → clippy `-D warnings` → test →
  `cargo deny check`. 180 core tests + the GUI compile/tests pass.

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
| **G6** | **Independent finish-card enables** — Presence/Air, Leveling, and Loudness no longer toggle the whole `FinishStage`. |
| **G7** | **Smart Assist** — deterministic analysis-report → Standard-card amounts/enables + Restoration Focus/Polish Intensity, with singing-conservative de-reverb/breath behavior and no hidden content-toggle flip. |
| **G8** | **Vocal Isolation Check** — re-analyzes the rendered output and reports deterministic residual noise/reverb/bleed verdicts for the Standard bottom strip. |
| **G9** | **Program Leveler** — the Leveling card now drives a deterministic finish-stage gain rider, with amount/enabled tests proving rendered dynamics change while length and loudness guarantees stay intact. |
| **G10** | **Presence/Air amount** — Session + `FinishStage` expose the EQ amount, and the GUI card slider drives it without changing loudness/leveling. |
| **G11** | **Independent transient-card controls** — De-click / De-pop now drives short click/crackle + DePop, while Mouth Clicks independently drives the mouth-click/lip-smack sub-path, with state and render tests proving the split. |
| **G12/13** | **OUTPUT + FILE** panels — `OutputFormat`, bit-depth/dither, name/location, `export_target()`/`export_to_target()`. |
| **G15** | **Volume Match** — `ab_matched_input()`. |
| **G17** | **Waveform peaks + channel meters** — `monitor::waveform_peaks / channel_peak_db / channel_rms_db`. |
| **#19a** | **Render queue controls** — `RenderQueue` now supports append, pause/resume, reorder, pending-only resume, failure skip/summary. |
| **#19b** | **Crash-recoverable queue state** — `RenderQueue` serializes versioned `queue.json`, keeps `queue.json.bak`, recovers from a corrupt primary, and always restores paused. |
| **#19c** | **Per-file render logs + failure summary** — `RenderQueue::run_with_log()` appends JSONL entries per processed file and returns failed-item details for end-of-batch retry/summary UI. |
| **#19d** | **Retry selected failures** — `RenderQueue::retry_failed()` marks selected failed queue items pending so the next run retries only those items. |
| **#19e** | **Batch preset mode** — `RenderQueue` now supports force-preset mode and per-file detection mode, with low-confidence fallback and crash-recovery JSON compatibility. |
| **#19f** | **GUI queue affordances** — the Standard shell now exposes queue-current, force-vs-detect batch mode, pause/resume, pending-batch export through `RenderQueue`, and tested queue summaries. |
| **#19g** | **Batch progress/cancel surface** — `RenderQueue::run_with_progress()` reports file-boundary progress and can cancel before the next file, leaving unprocessed rows pending; the GUI batch export path uses that observer and exposes a cancel affordance. |
| **#19h** | **FLAC level persistence + encoder wiring** — `ExportSettings` now persists/clamps the planned 0–8 FLAC compression level, `ExportOptions` carries it to the FLAC writer, and tests prove level 0 vs 8 reach the encoder without changing decoded audio. |
| **#19i** | **Original-file metadata round-trip coverage** — decoded buffers now carry source container metadata through render/export; WAV `bext`/`iXML`/`LIST` plus unknown RIFF chunks and FLAC Vorbis/application blocks round-trip mechanically in `io_export` tests. |
| **GUI** | **Custom-painted egui Standard-view shell** (`crates/yesvox-gui`) now launches as a 1920×1080 borderless canvas and tracks the reference layout: left order/type/assist stack, waveform/transport, two horizontal rack rows, meters, monitor/reblend/isolation strip, and the right preset/output/file/export column. Live capture: `target/visual/yesvox-gui-client.png`. |
| **#17 close-out** | **Zero-dead-chrome shell pass** — verified the drawn controls against the current shell, fixed the read-only Sample Rate row so it no longer paints as a selector, and removed static help/favorite icons that looked actionable but had no wired behavior. |

## Next up (re-planned 2026-07-03 — grill session)

A full GUI wiring audit + adversarial gate/core audits produced a re-cut plan.
**All remaining v1 work is tracked as issues #18, #20–#21, #22–#34**; build in
this order (each TDD'd, each its own green commit):

- ✅ **#27 H0 — gate integrity repairs** (done 2026-07-03). All 8 findings
  landed as green commits: OFFLINE now enforced by a source-level net-symbol
  scan; `YESVOX_BLESS` + `cargo xtask bless` implemented with a `golden`
  harness (reference numbers in `tests/goldens/*.golden`); NULL/SAME-LENGTH
  proven around a mock (non-`SpectralDenoiser`) engine; batch-queue guarantee
  coverage (`tests/guarantee_batch.rs`); singing gate rejects a no-op;
  same-path export lock; `LoudnessTarget::Custom` swept; `cargo-deny` now a
  required gate step; docs realigned. Also cleared a pre-existing red
  (quick-xml RUSTSEC-2026-0194/-0195 scope-ignore).

- ✅ **#22 background threading + real progress** (done 2026-07-03). Added
  `yesvox_core::worker` (spawn → monotonic progress → Done/Failed, panic
  caught as a friendly error, cooperative cancel); proved a worker render is
  byte-identical to sync and batch cancel leaves rows pending
  (`tests/worker_jobs.rs`); added `Session::prepare_export` (detached `Send`
  export unit). The GUI runs render/export/batch off the UI thread, polls
  events each frame, and the 0/0.55/1.0 progress heuristic is gone.

- ✅ **#23 playback trust loop** (done 2026-07-03). `yesvox_core::playback`
  (device-independent Transport: play/pause/seek/fill, shared playhead across
  A/B sources; `PlaybackSource` + `Session::playback_buffer`), all tested
  without a device. GUI-side cpal `Player` (`crates/yesvox-gui/src/audio.rs`)
  opens an f32 output stream, surfaces device failures as a status line, and
  wires the play button, transport-bar + waveform seek, real position readout,
  reblend-strip play, and instant A/B/Volume-Match/reblend source switching.
  cpal added to CI (Linux ALSA); its licenses were already covered.

- ✅ **#24 Standard wiring sweep + chrome triage** (done 2026-07-03). Wired:
  preset dropdown (`select_preset`), all 9 card amount knobs
  (`set_card_amount`, `visual_card_amount` stub deleted), Loudness Target +
  Bit Depth selectors, editable output name (TextEdit) + rfd folder picker.
  Removed dead chrome: Gear/Help/Back-to-Suite, Auto Polish, Save as New
  Preset, Add to Suite Library, Preview LUFS, duplicate Loudness "Target",
  per-card "Advanced", transport "Loop". Persistence-lite (last dir/format
  via eframe storage). Only Undo/Redo remain unwired — they are #25.

- ✅ **#25 undo/redo** (done 2026-07-03). `yesvox_core::history::History<T>`
  (bounded, coalescing, redo-truncating; proptest + cap/eviction tests) +
  `Session::snapshot`/`restore_snapshot` (settings-only; restore marks the
  render stale like a direct edit). GUI holds `History<SessionSnapshot>` cap
  100; every settings edit records with a per-control coalesce key; the
  toolbar arrows (real enabled state) + Ctrl/Cmd+Z/Shift+Z/Y drive it. This
  also wired the last dead controls from #24 → zero dead chrome.

- ✅ **#28–#34 hardening H1–H7 all done** (2026-07-03) — numerics, panic-safety,
   decoder fuzz-lite, filesystem/atomic export, resource guards, model-load
   validation, determinism. Details below.
   - ✅ **#28 H1 numeric hygiene** (done 2026-07-03): decode-time
     `AudioBuffer::sanitize` (NaN/±Inf→0, clamp ±4, counted →
     `AnalysisReport::non_finite_samples`), biquad denormal flush-to-zero,
     and a hostile-sample proptest (analyze+render+export panic-free/finite)
     + NaN-WAV round-trip.
   - ✅ **#29 H2 panic-safety** (done 2026-07-03): partial_cmp().unwrap →
     total_cmp; worker-pool poison recovery + dead-worker fallback (kill-a-
     worker test); io/render_queue/session unwrap/expect removed;
     `deny(clippy::unwrap_used, expect_used)` crate-wide (tests exempt).
     `indexing_slicing` deliberately NOT denied (180 bounds-safe DSP sites);
     ort_model expect chain waived → #33.
   - ✅ **#30 H3 decoder fuzz-lite** (done 2026-07-03): malformed WAV/FLAC
     corpus + two 512-case byte-mutation proptests — decode always returns a
     clean IoError or a sane buffer, never a panic; fixed an `int_scale(0)`
     shift-overflow on a bit-depth lie.
   - ✅ **#31 H4 filesystem/export hazards** (done 2026-07-03): `atomic_write`
     (temp drop-guard cleanup + transient-lock rename retry with backoff) for
     both writers; hazard tests (missing dir, dir-target, injected write
     failure) + real Windows share-mode lock tests (locked in/out →
     friendly error, temp cleaned).
   - ✅ **#32 H5 resource guards** (done 2026-07-03): ingest caps (zero-length,
     >192 kHz, >2 h) with friendly rejects; enforcing RAM preflight in render
     (GlobalMemoryStatusEx budget on Windows, override + allow-over-budget);
     a counting-allocator test measured the real render peak and CORRECTED
     `working_set_k` 8→16 (8 under-estimated); docs honesty (whole-buffer
     model, denoise-only streaming).
   - ✅ **#33 H6 model-load validation** (done 2026-07-03): `OrtMaskModel::run`
     inference expects → graceful passthrough (waiver removed, deny lint now
     covers the neural path); neural→light fallback carries a reason to
     `Session::engine_note()`/CLI; dependency-free SHA-256 + shipped-weight
     manifest integrity gate. (`ort`'s dylib probe *hangs* without
     ORT_DYLIB_PATH — the corrupt-model test is #[ignore]d; the fallback
     mechanism is tested without the feature.)
   - ✅ **#34 H7 whole-chain determinism** (done 2026-07-03): hard
     same-machine gate — full rack rendered twice, and with 1 vs 8 worker
     threads, is byte-identical (pool reassembly is order-independent) — runs
     on every CI OS; a `determinism-fingerprint` CI job records each
     platform's SHA-256 for the informational cross-platform comparison
     (TESTING.md).

**All AFK issues #22–#25 and #27–#34 are done and adversarially reviewed.**
A four-reviewer pass (2026-07-03, after the AFK build) verified the work and
landed two fix commits (`fix(#22)`, `fix(#23)`): honest A/B (Processed = pure
output via the tested `Session::playback_buffer`; the reblend mix auditions
only via the strip button), glitch-free source swap (old buffer freed outside
the callback lock), render-panic session recovery (source path + pre-render
snapshot), non-blocking `JobHandle::drop` (cancel + 200 ms grace → detach;
safe because exports are atomic), undoable content-mode switches, progress
coalescing, workspace-wide offline scan, and dead-code cleanup. Deferred
polish is tracked in **#35** (render cancel polling, device-SR resample
fallback, monitor-strip per-frame perf, long-file K validation).

> **CI note:** GitHub Actions minutes are exhausted on this account — pushes
> do NOT get a CI run right now. `cargo xtask gate` locally **is the merge
> bar**; run it before every push and never push red.

## Next steps (in order)

1. ✅ **#26 Advanced screen mock (hitl gate for #18):** approved by Dan on
   2026-07-04 and committed as
   `docs/ui/yesvox-v1-advanced-reference.png`. #18 should follow
   `docs/ui/advanced-parameter-inventory.md` for real controls and preserve the
   approved structure/style while avoiding crowded dense-card layouts.
2. ✅ **#17 Standard view close-out:** shell chrome verified; every remaining
   drawn control either mutates state, starts a real job/playback action, seeks,
   imports/exports, or is static display text/chip.
3. **#35 playback/worker polish (afk):** small, well-scoped — good filler.
4. **#18 Advanced build:** only after the #26 mock is approved.
5. **#20 v1.0 lane:** weights license audit (redistribution rights for every
   bundled weight, recorded in docs) → unsigned Windows installer (Inno/NSIS,
   bundled weights + ORT, ≤2 GB, clean-VM smoke test).
6. **#21 eval lanes:** CI-safe perf ceiling + local Criterion; `cargo xtask
   eval` real-stem no-regression harness (corpus dir via env var, never
   committed; Dan supplies stems); optional non-blocking listen.

## Decisions on record

- **Two intensity dials** (Restoration Focus + Polish Intensity) kept over the
  older "one slider" plan language — the axes genuinely differ (`QualityMode`
  only binds the denoiser; polish has no engine binding).
- **Sample rate = read-only** (export SR locked to input — hard guardrail + the
  same-length CI gate). No SR-override API; the OUTPUT panel shows it as a chip.
- **2026-07-03 (grill session):** v1 target = Dan-usable/demo-able **and** on
  the sellable trajectory (commerce plumbing itself is v1.1). **Playback is a
  v1 ship-blocker** (#23). **Undo = bounded settings snapshot stack** (#25,
  resolves G18). **The reference PNG is visual guidance, not a feature
  contract** — product.md decides features; dead chrome is wire-or-remove
  (#24). **Advanced = a separate screen** mirroring yes-master's pattern, in
  the Standard reference's visual language, mock-first (#26 → #18). **#16
  closed (GO)** — mechanical singing gate stands; real-stem no-regression
  corpus moved to #21. **#20 split** — v1.0 unsigned Windows installer +
  weights-license audit now; signing + merchant-of-record activation at v1.1.
  Declip/de-bleed stay card-less/automatic (by design, answers the product.md
  de-bleed-trigger question).
- **2026-07-04:** #26 Advanced reference mock approved. The PNG is a
  structure/style reference; `advanced-parameter-inventory.md` remains the
  control contract. #18 must keep dense Advanced capability scannable and avoid
  over-crowded module cards.
- **Known-good / known-weak (2026-07-03 audits):** DSP core verdict = real,
  competent work (BS.1770-4 loudness, RBJ biquads, adaptive notch cascade,
  split-band de-ess with de-ess→breath HF handoff, torch-golden-pinned
  MossFormer2 host chain). Weak spots now tracked: gate integrity (#27),
  NaN/panic hygiene (#28/#29), no resource caps (#32), neural expect-crashes
  (#33), streaming is denoise-only (docs honesty in #32).
- **Deferred but not in the Standard shell:** "Save as New Preset" + "Add to
  Suite Library" (no user-preset store; suite handoff is a designed-for seam,
  not built-in). They remain future product seams, not v1 dead chrome.

## GUI dependency note (read before touching the shell)

`yesvox-gui` is on **eframe 0.35**, which is a **redesigned egui API** vs older
versions: implement `App::ui(&mut self, ui, frame)` (not `update(ctx, frame)`);
the visual shell now paints a fixed 1920×1080 coordinate system directly from
`Ui::max_rect()` instead of relying on stock widgets for layout; theming still
uses `ctx.set_visuals` + `ctx.all_styles_mut`. `deny.toml`
carries four scoped, permissive license exceptions for the egui tree
(hexf-parse CC0, clipboard-win/error-code BSL-1.0, epaint_default_fonts
OFL-1.1/Ubuntu-font). Visual fidelity is verified by eye; widget logic is tested
in `yesvox-core`.
