# YesVox — build status & pick-up guide

> Resume-from-anywhere snapshot. Last updated **2026-11-23** (the Aug 25 RC.3 /
> brand work and the Nov 14 Import / Standard view, built on two machines, are
> merged on `main`; see the newest entry). Before that, **2026-08-25** (RC.3 packaging,
> neural-boundary hardening, and YES-suite brand unification executed; installer
> build and local install/render/uninstall smoke are now one fail-closed command).
> For product intent
> read [`product.md`](product.md) first; for the build plan see
> [`plans/2026-06-29-feat-yesvox-v1-restoration-workstation-plan.md`](plans/2026-06-29-feat-yesvox-v1-restoration-workstation-plan.md);
> for the GUI target see [`ui/yesvox-v1-ui-reference.md`](ui/yesvox-v1-ui-reference.md).

## 2026-11-23 — Two machines' work merged

- **What happened:** this repo split at 2026-07-15. One PC made the Aug 25 RC.3 hardening
  wave and brand unification (below) but never pushed it; another machine built the Nov 14
  Import / Standard view from the July state. Both are now merged on `main`. The unmerged
  Aug 25 side is kept on branch `backup/this-pc-2026-08-25`.
- **Conflict calls (6 spots, all small):** checksum loop takes Nov 14's version; the Standard
  switch keeps Aug 25's centered position with Nov 14's "Back to Standard" label; Nov 14's
  top-bar section labels ("PROCESSING AND OUTPUT", "ADVANCED") are dropped because Aug 25 put
  the centered view switch in that space; Presets hides during a render *and* on the compact
  Standard screen; the batch queue uses Aug 25's quiet section rail at Nov 14's height.
- **Guard:** the Standard smoke now checks that both view-switch labels fit their segments
  and that no other text overlaps the switch. Re-adding the old section label fails it.
- **Dependency:** `url` 2.5.8 → 2.5.9 for RUSTSEC-2026-0592 (published after both sides).
- **Verified:** `cargo xtask gate` green on the merged tree (fmt, clippy, all tests, deny).
- **Not yet checked (Dan):** the Nov 14 Standard screen was designed before the brand pass,
  so it inherits the new palette but may still use older filled panels. Look at Import,
  Standard, More Controls and Advanced side by side and say whether Standard needs a
  brand-matching pass.

## 2026-11-14 — Import and Standard view

- The app opens to an Import Vocal screen that accepts a WAV or FLAC drop or opens a file picker.
- Standard puts Original / Processed next to playback and Export Vocal in its own panel. Voice type, preset, and effect strength stay visible.
- More Controls opens the restoration and polish rack, difference monitor, and batch queue. Output Settings opens the same screen with the file and format options.
- A headless GUI smoke covers empty and failed imports, successful drag and drop, preview switching, stale-render export protection, output decoding, and source-file integrity.
- The dependency gate now uses updated browser, clipboard, and executor packages. Three maintenance notices for the Linux accessibility stack stay documented while AccessKit requires zbus 5.

## Where we are

**The v1.0 unsigned Windows installer is now RC.3 and has passed a local silent
install → exact `--version` → bundled neural render (`denoiser Neural`) →
uninstall-residue verification smoke.** `packaging/windows/build-rc.ps1` starts
from a clean worktree, audits the exact model and ONNX Runtime payloads, runs
the canonical gate, stamps the source revision, builds the installer, and
writes a green JSON report only after uninstall has been attested. The bundled
MossFormer2 payload remains hash-pinned at `af292f14…`; ONNX Runtime 1.26.0's
two DLLs, license, and third-party notices are pinned by
`packaging/windows/onnxruntime-1.26.0-sha256.tsv`. What remains on #20 is the
clean-Windows-VM smoke (Dan); on #21, Dan's private real-stem corpus
(`docs/eval-corpus.md`;
`cargo xtask eval --check` previews it) and the suite round-trip on a clean
machine. The engine (`yesvox-core` rack) already
existed; #17 built the agent-callable, mechanically-tested `Session` surface the
UI needs, then a custom-painted **egui shell** over it. #26 supplied the
approved Advanced reference, #35 closed the deferred playback/worker polish,
#18 built the Advanced screen (CPU spectrogram fallback, A/B/C compare, signed
parameter coverage), and #36 closed the adversarial-review findings on it
(mid-render settings lock, view-independent render cancel + Esc, audible-cache
trim, preview-rate badge).

- **Workspace:** `yesvox-core` (lib), `yesvox-app` (headless CLI, bin `yesvox`),
  **`yesvox-gui`** (the egui app, bin `yesvox-gui`, **new**), `xtask`.
- **The gate is green on RC.3.** `cargo xtask gate` = fmt → clippy `-D warnings`
  → test → `cargo deny check`. Core unit tests, integration tests, and all 54
  GUI tests pass.

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
| **GUI** | **Custom-painted egui Standard-view shell** (`crates/yesvox-gui`) launches as a 1920×1080 borderless canvas and tracks the reference layout: left order/type/assist stack, waveform/transport, two horizontal rack rows, meters, monitor/reblend/isolation strip, and the right preset/output/file/export column. |
| **#17 close-out** | **Zero-dead-chrome shell pass** — verified the drawn controls against the current shell, fixed the read-only Sample Rate row so it no longer paints as a selector, and removed static help/favorite icons that looked actionable but had no wired behavior. |
| **#35** | **Playback/worker polish close-out** — render jobs now poll cancel between DSP stages and expose a render-cancel affordance; playback resamples preview buffers to the output device rate without touching export; the monitor strip caches the audible buffer/metrics and avoids resetting an unchanged playback source; memory-budget coverage now includes an opt-in two-minute render measurement (`YESVOX_LONG_MEMORY_BUDGET=1`). |
| **#18** | **Advanced screen** — separate Advanced view over the approved #26 structure/style, full module rack in canonical order, A/B/C with loudness-matched pinned snapshots, real CPU STFT spectrogram fallback with cache, monitor/reblend strip, and signed parameter coverage in `docs/advanced-param-coverage.md`. |

## 2026-08-25 RC.3 hardening wave

- **Gate/toolchain restored:** current Rust Clippy issues are fixed and
  `webbrowser` is updated to 1.2.2, clearing RUSTSEC-2026-0257.
- **Exact native payload contract:** package audit now verifies the staged model
  and every ONNX Runtime 1.26.0 file against source-controlled SHA-256
  manifests, including the upstream license and third-party notices.
- **Repeatable package/smoke:** `packaging/windows/build-rc.ps1` and
  `smoke-installer.ps1` fail closed on a dirty tree, wrong version, light-engine
  fallback, render-shape mismatch, uninstall failure, or installed residue.
- **Neural output hardening:** malformed/non-finite ONNX mask outputs, missing
  session inputs/outputs, and wrong `[1,T,961]` shapes degrade safely to
  passthrough instead of crashing or corrupting a render. The real model parity
  test and corrupt-model fallback test pass in isolated processes, and the
  installed RC.3 smoke proves the bundled neural path end to end.
- **UI/UX polish:** the borderless desktop shell now has visible, tested
  minimize/maximize/close controls plus a draggable/double-clickable title
  region. Render progress keeps its own clear toolbar lane. The Advanced Finish
  card uses the available panel space and mechanically prevents its ten labels
  and nine control regions from overlapping. Captured Standard and Advanced
  surfaces match the approved dark visual language; final taste approval stays
  with Dan.
- **YES-suite brand unification:** current YES Master source and the shipped
  reference were treated as the brand authority while YesVox kept its own
  cyan/violet identity and restoration workflow. Standard and Advanced now use
  the shared graphite material depth, quiet rails and active states, compact
  marked section labels, centered primary view switch, unified listening deck,
  dimensional 270-degree/28-tick hardware knobs, metallic slider thumbs, and
  top-lit primary actions. The dead Standard Restore/Polish title-bar switch was
  removed because both racks remain simultaneously visible. Palette, navigation,
  and knob geometry are pinned by GUI tests; both views were built and captured
  at the reference canvas for visual inspection.

## Next up (re-planned 2026-07-03 — grill session)

A full GUI wiring audit + adversarial gate/core audits produced a re-cut plan.
The done AFK issues (#22–#25, #27–#35) are listed below; remaining v1 work is
#18, #20, and #21. Build in this order (each TDD'd, each its own green commit):

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

**All AFK issues #22–#25 and #27–#35 are done and adversarially reviewed.**
A four-reviewer pass (2026-07-03, after the AFK build) verified the work and
landed two fix commits (`fix(#22)`, `fix(#23)`): honest A/B (Processed = pure
output via the tested `Session::playback_buffer`; the reblend mix auditions
only via the strip button), glitch-free source swap (old buffer freed outside
the callback lock), render-panic session recovery (source path + pre-render
snapshot), non-blocking `JobHandle::drop` (cancel + 200 ms grace → detach;
safe because exports are atomic), undoable content-mode switches, progress
coalescing, workspace-wide offline scan, and dead-code cleanup. Deferred
polish from that review is closed in **#35**: render cancel polling,
device-SR preview resampling, monitor-strip cache/source reuse, and opt-in
long-file memory-budget validation.

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
3. ✅ **#35 playback/worker polish (afk):** done 2026-07-04. Render cancel,
   device-rate preview resampling, monitor cache/source reuse, and opt-in
   long-file memory-budget validation are landed and locally gated.
4. ✅ **#18 Advanced build:** done 2026-07-04. Advanced follows
   `docs/ui/advanced-parameter-inventory.md` and
   `docs/ui/yesvox-v1-advanced-reference.png`, with the final coverage/sign-off
   recorded in `docs/advanced-param-coverage.md`.
5. **#20 v1.0 lane — RC.3 installer built + locally smoked (2026-08-25).** The
   MossFormer2 payload is pinned end-to-end: the golden-validated fp32 ONNX
   export is the `bundled` row in `licenses/model-weights.tsv` (shipped
   SHA-256 recorded), mirrored into the load-time integrity manifest
   (cross-checked by `xtask/tests/weights_license_audit.rs`), with the full
   Apache-2.0 text in `THIRD-PARTY-NOTICES.txt`. The package also pins the exact
   ONNX Runtime 1.26.0 DLL/license/notice set. Both binaries have a
   `neural` passthrough feature; the app now **discovers** the bundled model
   (`{app}\models`, manifest names only) and the bundled ONNX Runtime dylib
   next to the exe (no env vars needed; `YESVOX_MODEL_PATH`/`ORT_DYLIB_PATH`
   still override). The recipe compiles from the repo root (`SourceDir` fix)
   → `target/package/yesvox-v1-unsigned.exe` (~215 MB). The clean-tree build
   command audits both payloads, runs the canonical gate, builds, then performs
   a local silent install → exact RC.3 version → installed CLI neural render →
   silent uninstall with residue verification. See `packaging/windows/README.md`
   for the verified steps +
   SmartScreen notes. **Remaining:** clean-Windows-VM smoke (Dan). DFN3 is
   explicitly not in the v1.0 bundle (unwired).
6. **#21 eval lanes:** perf tripwire (`tests/perf_budget.rs`), local Criterion
   benches, and the skip-safe `cargo xtask eval` real-stem harness are in.
   **Lane 3 (2026-07-04):** `tests/eval_lane3.rs` pins silence/hard-clipping
   refusal as clean typed errors, a recoverably-clipped full-chain render
   inside the −1 dBTP ceiling, and the YesStems→YesVox→yes-master round-trip
   (SR/bit-depth/channels/length/metadata). Measured on this reference
   machine (release, 30 s clip, full CLI): neural 11.8 s, light 0.9 s — well
   under the 60 s drop-to-export budget. Optional non-blocking listen doc:
   `docs/listening-checklist.md`. **Remaining:** Dan's corpus for
   `YESVOX_EVAL_CORPUS`, and the clean-machine round-trip once the VM smoke
   happens.
7. ✅ **Test-readiness wave (W1–W4, executed 2026-07-07, afk):** all four
   slices done, green, pushed (plan:
   [`plans/2026-07-07-test-readiness-wave-execution-plan.md`](plans/2026-07-07-test-readiness-wave-execution-plan.md)).
   - **W3** hand-test runbook: [`docs/hand-test-runbook.md`](hand-test-runbook.md).
   - **W1** build stamp: `yesvox --version` → `yesvox 1.0.0-rc.2 (<hash>)`; a
     dim always-visible GUI status-strip stamp shows version · hash · engine
     (Neural/Light + a `· fallback` marker), sourced from `Session`'s
     last-render engine. Crates bumped 0.0.0 → **1.0.0-rc.2** (xtask stays 0.0.0).
   - **W4** corpus kit: `cargo xtask eval --check` preflight (skip-safe,
     no-render) + [`docs/eval-corpus.md`](eval-corpus.md).
   - **W2** installer rebuilt with the stamp on the office PC and locally
     smoked (see "Where we are"). Superseded by the automated RC.3 lane above.
   - **Open (Dan's lane only):** the #20 clean-Windows-VM smoke of the rebuilt
     installer, and the #21 corpus. `gh` is still unauthenticated on the office
     PC (no new issues created this wave). **Follow-up worth doing:** the ONNX
     payload is regenerated, not stored, so every rebuild risks a re-bless —
     This risk is now mechanically contained by the clean-tree payload audit; a
     regenerated artifact cannot package unless it matches the approved hash.

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
OFL-1.1/Ubuntu-font). Layout, hit regions, and collision boundaries are tested;
captured surfaces support the separate owner visual/taste review.
