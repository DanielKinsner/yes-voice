# Assembling the #21 eval corpus (`YESVOX_EVAL_CORPUS`)

> Plain-English recipe for building the real-stem evaluation corpus and running
> it. Written for Dan. The corpus is **yours and license-clean** — it is never
> committed; the harness only reads it when you point an env var at it, so CI and
> fresh clones skip it automatically.

## Bottom line

1. Put your isolated vocal stems in a folder, split into **voiceover** and
   **singing** (at least **12 of each**).
2. **Preflight without rendering** — confirm the harness sees them the way you
   expect: `cargo xtask eval --check`.
3. **Run the real eval**: `cargo xtask eval`. It renders each stem, checks it,
   and writes a report to `target/eval/report.json`.

## Quick start (PowerShell)

```powershell
# Point the harness at your corpus (this session only):
$env:YESVOX_EVAL_CORPUS = "C:\Users\SM - Dan\yesvox-corpus"

# 1. Preflight — lists every stem + the mode/preset it will use, renders nothing:
cargo xtask eval --check

# 2. Real run — renders + evaluates, writes target\eval\report.json:
cargo xtask eval
```

If you **don't** set `YESVOX_EVAL_CORPUS`, both commands print a "skipped" notice
and succeed (exit 0). That is deliberate — it's why the gate can run `eval`
safely on any machine without your private stems.

## Laying out the corpus

The harness needs to know each stem's **content mode** (Voiceover or Singing),
because that picks the preset and the preservation checks. There are two ways to
tell it — pick whichever is easier.

### Option A — folders (simplest)

Drop files into `voiceover\` and `singing\` sub-folders. The mode is read from
the path:

```
yesvox-corpus\
  voiceover\
    interview-01.wav
    narration-take3.flac
    ...            (at least 12 files)
  singing\
    chorus-lead.wav
    verse-double.flac
    ...            (at least 12 files)
```

The mode is inferred from **any** folder or file-name word (split on `-`, `_`,
`.`, or space). Words that mean **voiceover**: `voiceover`, `voiceovers`, `vo`,
`spoken`, `speech`, `dialogue`, `narration`. Words that mean **singing**:
`singing`, `sung`, `song`, `songs`, `vocal`, `vocals`, `lead`. If a path has
**no** mode word, or has **both**, the harness refuses that file and tells you to
sort it into a `voiceover\`/`singing\` folder or add a manifest (Option B).

Only `.wav` and `.flac` files are picked up; sub-folders are searched
recursively.

### Option B — `manifest.tsv` (explicit)

Put a tab-separated `manifest.tsv` at the corpus root. One row per stem,
`path<TAB>mode`, where `path` is **relative to the corpus root** (and must stay
inside it):

```
path	mode
takes/interview-01.wav	voiceover
takes/chorus-lead.flac	singing
```

- A `path<TAB>mode` header row is optional (it's skipped).
- Blank lines and lines starting with `#` are ignored.
- `mode` accepts the same words listed above.
- If `manifest.tsv` exists, it wins; the folder scan (Option A) is not used.

## The minimum

The corpus must contain **at least 12 voiceover and 12 singing** stems. Fewer
than that and both `eval --check` and `eval` stop with:
`corpus must contain at least 12 voiceover and 12 singing stems; found …`.

Twelve-per-mode is the floor for the evaluation to be meaningful — use more when
you have them.

## What `eval --check` tells you

It's a dry run: it resolves the whole corpus and prints what it *would* do,
**without opening or rendering any audio**. Use it to catch a mislabeled file or
a bad manifest row before committing to the (slower) real render pass. Example:

```
eval --check: 24 stems ready (12 voiceover, 12 singing) — nothing rendered:
  singing/chorus-lead.wav — singing — preset Lead Vocal
  voiceover/interview-01.wav — voiceover — preset Podcast
  ...
```

If the corpus is malformed it prints the **exact** problem instead — a bad
extension, an unreadable manifest row, a file the manifest points at that doesn't
exist, an empty folder, or too few stems per mode — and exits non-zero.

## What the real `eval` does, and where the report lands

`cargo xtask eval` opens each stem, applies **Smart Assist** on the mode's
default preset (Voiceover → **Podcast**, Singing → **Lead Vocal**), renders the
full chain, and scores the result. It writes a JSON report to:

```
target\eval\report.json
```

(Override the location with `YESVOX_EVAL_REPORT` if you want.) The report has a
per-stem breakdown plus a summary (total / passed / failed, and the mode counts).
The command exits 0 only if **every** stem passed; any failure exits non-zero and
names the failing stems.

## What "pass" means for one stem

A stem **passes** when the render changed nothing it must preserve and produced a
clean, on-target result — i.e. none of these checks fired:

- **Same shape:** output length, sample rate, and channel count all match the
  input (a restoration must not resample, trim, or change channel count).
- **Loudness on target:** integrated loudness within **±2.0 LU** of the mode
  target (**−16 LUFS** voiceover, **−14 LUFS** singing), and measurable on both
  input and output.
- **No gross clipping:** output true peak stays under the harness ceiling
  (currently **+0.2 dBTP**).
- **Voice preserved (the mechanical singing/voiceover gate):** the sung/spoken
  character survives — voiced support, pitch (F0 cents drift), harmonic-to-noise
  ratio, spectral-envelope similarity (cosine ≥ **0.82** voiceover / **0.88**
  singing), and — for singing — vibrato rate/depth and note-tail survival all
  stay within tolerance.

If any check fires, the stem is marked **failed** and the specific reason(s) are
listed in `report.json` so you know exactly what regressed.

## Troubleshooting

| Message | What it means | Fix |
|---------|---------------|-----|
| `… is not set. … skipped` | No corpus env var. | `$env:YESVOX_EVAL_CORPUS = "…"` before the command. |
| `must point at a directory` | The env var points at a file, not a folder. | Point it at the corpus **folder**. |
| `no eval stems found …` | The folder has no `.wav`/`.flac` (or no `manifest.tsv`). | Add stems, or a manifest. |
| `could not infer content mode for …` | A loose file has no (or a conflicting) mode word in its path. | Move it under `voiceover\`/`singing\`, or list it in `manifest.tsv`. |
| `… mode must be voiceover or singing` | A manifest row's mode column is wrong. | Use one of the accepted mode words. |
| `… missing file …` | A manifest row points at a file that isn't there. | Fix the path (relative to the corpus root). |
| `corpus must contain at least 12 …` | Fewer than 12 of a mode. | Add stems until each mode has ≥ 12. |
