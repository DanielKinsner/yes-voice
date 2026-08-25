# YesVox hand-test runbook

> Plain-English, click-by-click test script for hand-testing a YesVox build.
> Written for Dan (not a coder): every step is **do X → you should see Y**, with
> an explicit **PASS / FAIL** line. If a step FAILs, stop and write it down using
> the bug template in [§7](#7-how-to-report-a-bug) — don't push past a failure,
> because later steps assume the earlier ones worked.
>
> This runbook covers the two things that gate v1: the **#20 clean-Windows-VM
> installer smoke** (§1) and a **full GUI tour** (§2–§6). The corpus eval (#21)
> has its own recipe in [`eval-corpus.md`](eval-corpus.md).

## How to use this document

- Work top to bottom the first time. After that you can jump to a section.
- "The app" means the **GUI** (`yesvox-gui.exe`, the window). "The CLI" means the
  headless `yesvox.exe` you run from a terminal.
- Every control this runbook names **exists in the shipped build** — if you can't
  find one on screen, that's a FAIL worth reporting (it means the build you're
  testing doesn't match this doc).

---

## 0. What am I running? (read this first, every time)

**Bottom line:** before you test *anything*, confirm the build in front of you is
the build you think it is. A stale build is the #1 waster of hand-testing time —
you chase a "bug" that was already fixed, or miss one that's really there.

**The version stamp** is how you check. Every YesVox build shows the same stamp
in two places:

- **In the GUI:** a small, dim line in the **bottom corner** of the window,
  always visible on both the Standard and Advanced screens. It reads something
  like:

  > `v1.0.0-rc.3 · a1b2c3d · Neural`

  - `v1.0.0-rc.3` — the version.
  - `a1b2c3d` — the exact code build (a short git hash). If it ends in `-dirty`
    the build was made from uncommitted changes; if it says `unknown` the build
    machine had no git — both are fine for a local test, just note it.
  - `Neural` or `Light` — **which engine actually ran your last render.** Neural
    is the full-quality MossFormer2 model; Light is the always-available fallback.
    If a render couldn't load the neural model it falls back to Light and the
    stamp says so (hover it for the reason).

- **In the CLI:** run `yesvox --version` (or `yesvox -V`). You should see:

  > `yesvox 1.0.0-rc.3 (a1b2c3d)`

**The rule:** *if the stamp doesn't match the build you meant to test, STOP and
rebuild/reinstall. You are chasing a ghost.* For example, if you just built a new
installer but the app still shows the old hash, you installed the wrong file.

**PASS:** the GUI shows a stamp in the corner, and `yesvox --version` prints a
matching version. **FAIL:** no stamp visible, or the two disagree, or the hash
isn't the build you expected.

> _(This stamp is added by wave slice **W1**. Until W1 lands, the app has no
> stamp yet — that's expected, and §1's "confirm the stamp shows Neural" steps
> land together with W1.)_

---

## 1. #20 — clean-Windows-VM installer smoke

**Goal:** prove the unsigned installer works end-to-end on a *clean* Windows
machine (a fresh VM with no Rust, no Visual Studio, nothing YesVox) — install,
render with the real neural engine, export, play, uninstall. This is the #20
ship gate.

**You need:** a clean Windows 10/11 VM and the installer file
`yesvox-v1-unsigned.exe` (Dan carries it in; it's built by wave slice W2 and its
path + size + SHA-256 are reported with that build).

Do each step in order. Every step has a PASS/FAIL.

**1.1 — Copy the installer in.**
Copy `yesvox-v1-unsigned.exe` onto the VM (shared folder, drag-drop, or download).
→ You should see the file on the VM's desktop or Downloads.
**PASS:** file is there. **FAIL:** can't get it onto the VM.

**1.2 — Launch the installer; expect the SmartScreen warning.**
Double-click `yesvox-v1-unsigned.exe`.
→ Because v1.0 is **unsigned**, Windows SmartScreen will likely pop
"**Windows protected your PC**". This is expected, not a bug.
→ Click **More info**, then **Run anyway**.
**PASS:** the SmartScreen prompt appears and "Run anyway" starts the installer.
**FAIL:** installer refuses to run, or a different error (not SmartScreen) appears.
> _(If you built the installer locally on the same machine, there's no
> Mark-of-the-Web and SmartScreen may not appear at all — that's also fine.)_

**1.3 — Install (per-user, no admin prompt).**
Follow the installer through.
→ It installs **per-user** under `%LOCALAPPDATA%\Programs\YesVox` and should
**not** ask for administrator/UAC elevation.
**PASS:** install completes with no UAC prompt. **FAIL:** it demands admin
rights, or install fails.

**1.4 — Launch the GUI.**
Open YesVox from the Start menu (or the installed `yesvox-gui.exe`).
→ The window opens to the Standard view.
**PASS:** window opens, no crash. **FAIL:** nothing opens, or it crashes/errors.

**1.5 — Confirm the build stamp.**
Look at the bottom-corner stamp (see §0).
→ It should show the version you expect and a git hash.
**PASS:** stamp is present and matches the build Dan handed you. **FAIL:** no
stamp, or wrong version/hash → stop, you have the wrong installer.

**1.6 — Import a WAV.**
Import a vocal `.wav` (any short isolated voiceover or vocal clip).
→ The waveform draws and the file's info appears.
**PASS:** the clip loads and you see its waveform. **FAIL:** import errors or
nothing draws.

**1.7 — Render with the default preset.**
Leave the default preset selected and render.
→ Processing runs to completion.
**PASS:** render finishes without error. **FAIL:** render errors or hangs.

**1.8 — Confirm the engine stamp shows Neural.**
After the render, check the engine word in the stamp (§0).
→ On the installed build (which bundles the model + runtime), it should say
**Neural**.
**PASS:** stamp reads **Neural**. **FAIL:** it reads **Light** — that means the
bundled neural model or ONNX runtime didn't load. Hover the stamp for the reason
and report it (this is a packaging problem, exactly what this smoke is for).

**1.9 — Export the result.**
Export to a `.wav`.
→ A file is written where you chose.
**PASS:** the export file exists. **FAIL:** export errors, or no file appears.

**1.10 — Play the exported file.**
Open the exported `.wav` in any player (or YesVox's own transport).
→ It plays and sounds like a cleaned version of your input (not silence, not
garbage).
**PASS:** it plays and sounds like processed audio. **FAIL:** silence, noise, or
won't open.

**1.11 — Uninstall.**
Go to **Settings → Apps**, find **YesVox**, and uninstall (or run
`unins000.exe` from the install folder).
→ It removes cleanly.
**PASS:** YesVox is gone from the app list and the install folder is removed.
**FAIL:** uninstall errors or leaves the app behind.

**Smoke verdict:** all eleven PASS = #20 clean-VM smoke is GO. Any FAIL = stop and
report it with the step number.

---

## 2. Standard view tour

**Goal:** touch every control on the Standard screen and confirm it does
something sensible. You don't need a VM for this — run the GUI on any dev machine
(`cargo run -p yesvox-gui`). Load a real vocal clip first (see 1.6).

Control names below are exactly what's painted on screen.

**2.1 — Content mode toggle (Voiceover / Singing).**
Click **Voiceover**, then **Singing** (top-left of the canvas).
→ The selected mode highlights; this is the user-authoritative content mode (not
auto-detected). Switching it is undoable.
**PASS:** the toggle switches and the highlight follows your click. **FAIL:** it
doesn't switch, or both look selected.

**2.2 — SMART ASSIST.**
Click **SMART ASSIST**.
→ It reads the analysis of your loaded clip and sets the cards' amounts/enables
and the Restoration Focus / Polish Intensity dials to sensible starting points.
You should see several card knobs and the two dials jump to new positions.
**PASS:** controls visibly move to assisted values. **FAIL:** nothing changes, or
it errors. (No clip loaded → it should do nothing or say so, not crash.)

**2.3 — Preset gallery.**
Pick a different **Preset** from the gallery.
→ The selected preset highlights; the render will use that preset's character.
**PASS:** selection changes. **FAIL:** can't change it.

**2.4 — The 10 cards.**
Each card has an **on/off toggle** and (except Loudness) an **amount knob**. The
cards are:

| Card | What it does |
|------|--------------|
| **Denoise** | Reduces background noise while preserving detail. |
| **De-reverb** | Removes reverb while preserving natural space. |
| **De-click / De-pop** | Removes clicks, pops and crackle. |
| **Mouth Clicks** | Reduces lip smacks and mouth noises. |
| **De-hum** | Removes electrical hum and low-frequency noise. |
| **De-ess** | Controls harsh sibilance without dulling. |
| **Breath Control** | Attenuates harsh breaths (voiceover-leaning). |
| **Presence / Air** | Adds clarity and air, adaptively. |
| **Leveling** | Smooths vocal level consistently. |
| **Loudness** | Sets loudness to the target level (target chosen in the OUTPUT panel). |

For each card: **toggle it off then on**, and **drag its knob to an extreme**,
then render and A/B (2.7).
→ Toggling a card off should remove that effect from the result; turning a knob
up should make that effect stronger (more obvious noise reduction, more
de-essing, etc.). Loudness's knob shows the current target.
**PASS:** each card audibly/visibly changes the result in the expected
direction. **FAIL:** a card does nothing, or does the opposite, or crashes.

**2.5 — FLOW AMOUNT (the master dry→wet).**
Drag **FLOW AMOUNT** to **0**, then render and A/B.
→ At 0 the whole rack is nulled: the output should equal the input (no
processing at all). Drag it back up and the processing returns.
**PASS:** at 0 the processed output sounds identical to the input; above 0 it
processes. **FAIL:** at 0 the output still differs from the input.

**2.6 — Restoration Focus / Polish Intensity dials.**
Turn **Restoration Focus** and **Polish Intensity** across their range and render.
→ Restoration Focus biases toward more/less aggressive restoration (quality
mode); Polish Intensity scales the finishing polish (de-ess + breath). Both
should shift the sound accordingly.
**PASS:** each dial changes the result. **FAIL:** either does nothing.

**2.7 — A/B and Volume Match.**
Use the **A/B** compare and **Volume Match**.
→ A/B flips between the original (A) and processed (B). Volume Match
loudness-matches the two so you judge the *change*, not just "louder = better".
**PASS:** A/B flips instantly and Volume Match evens the loudness. **FAIL:** A/B
doesn't switch, or one side is silent.

**2.8 — Reblend strip + difference monitor.**
Use the **Reblend** strip and the **Difference view (What was removed)** monitor.
→ Reblend lets you audition a mix of dry+processed; the difference monitor shows
(and its play button auditions) *only what was removed* — noise, hum, reverb
tails. On a clean render this should be mostly the junk you wanted gone, not the
voice.
**PASS:** reblend auditions a blend; the difference view shows removed content.
**FAIL:** difference view is empty on an obviously-noisy input, or plays the
whole voice.

**2.9 — Vocal Isolation Check.**
Read the **Vocal Isolation Check** verdicts (bottom strip) after a render.
→ It re-analyzes the *output* and reports residual noise / reverb / bleed
verdicts. On a good render these should read clean/low.
**PASS:** verdicts appear and match what you hear. **FAIL:** no verdicts, or they
contradict an obviously-clean or obviously-bad result.

**2.10 — Undo / Redo.**
Change a few controls, then press **Undo** (toolbar arrow or **Ctrl+Z**) and
**Redo** (**Ctrl+Shift+Z** or **Ctrl+Y**, or the toolbar arrow).
→ Undo steps back through your setting changes; Redo re-applies them. The arrows
are greyed when there's nothing to undo/redo.
**PASS:** undo/redo walk your changes correctly. **FAIL:** they do nothing, skip
steps, or restore the wrong values.

**2.11 — Transport / seek.**
Press **Play**, then click on the waveform / transport bar to **seek**.
→ Playback starts; clicking moves the playhead to that spot; the position
readout tracks it.
**PASS:** plays and seeks. **FAIL:** won't play (note any device-error status
line), or seeking doesn't move the playhead.

---

## 3. Advanced view tour

**Goal:** confirm the Advanced screen exposes the full module rack and the
deeper compare/spectrogram tools. Advanced is the same shell as Standard with
per-module depth — the sound engine is identical, so nothing here changes what a
render produces vs Standard at the same settings.

**3.1 — Switch to Advanced.**
Click the **Advanced** tab (top of the canvas; **Standard** is next to it).
→ The view switches to the Advanced layout.
**PASS:** Advanced view opens. **FAIL:** tab doesn't switch.

**3.2 — Module rack in canonical order.**
Read the module rack. It should list, in this fixed order:

1. **De-bleed** · 2. **Declip** · 3. **De-click** · 4. **De-pop** ·
5. **De-hum** · 6. **Denoise** · 7. **De-reverb** · 8. **De-ess** ·
9. **Breath** · 10. **Finish** (Presence + leveling + loudness limiter).

→ De-bleed and Declip are automatic/card-less by design (they have no Standard
card) but appear here as modules 1–2.
**PASS:** all ten modules present, in this order. **FAIL:** a module missing or
out of order.

**3.3 — A/B/C pinned snapshots (loudness-matched).**
Render, then **Pin C** to capture a snapshot. Change some settings, render again,
and flip between **A** (input), **B** (current processed) and **C** (your pinned
snapshot).
→ A/B/C are loudness-matched so you compare the *processing*, not level. C holds
until you re-pin. Trying to Pin C before any render should say "Render before
pinning C", not crash.
**PASS:** all three sources audition and C stays pinned. **FAIL:** C is empty
after a render, or the three aren't level-matched.

**3.4 — Spectrogram.**
Look at the **SPECTROGRAM** (full-width in Advanced).
→ It shows the frequency content of the current audio (a real CPU STFT). Use it
to eyeball residual noise/hum lines/reverb smear.
**PASS:** spectrogram draws and updates with the audio. **FAIL:** blank or frozen.

**3.5 — Per-parameter spot checks.**
Open each module and exercise its parameters, using
[`advanced-param-coverage.md`](advanced-param-coverage.md) as the checklist of
what each module exposes (e.g. Denoise engine Natural/Balanced/HQ, De-ess
Split/Wide, De-hum 50/60 Hz + tracking, De-reverb RT60/tail preserve, Breath
attenuation dB, Finish presence/leveling/loudness).
→ Each parameter should change the module's readout and, where audible, the
render.
**PASS:** parameters move and take effect. **FAIL:** a listed parameter is
missing or inert.

---

## 4. Batch / queue

**Goal:** confirm the **BATCH QUEUE** panel (right column) processes multiple
files and survives interruption. The v1 queue exposes these buttons: **Force
preset** / **Detect each**, a **pause/resume** button (shows `||` while running,
`>` when paused), a **cancel** button (`x`), **+ Queue Current**, and
**Export Batch**.

> Note: reorder and retry-failed exist in the engine but are **not** buttons in
> the v1 Standard queue — there's nothing to click for those, so they're not
> tested here.

**4.1 — Build a queue.**
Import a vocal, click **+ Queue Current**, then load another vocal and queue it
too.
→ The queue summary near the panel title updates (e.g. "2 queued - 0 done -
0 failed"). Queuing the same file twice should say "Already queued", not add a
duplicate.
**PASS:** the count grows; duplicates are refused. **FAIL:** count doesn't
update, or duplicates stack.

**4.2 — Force preset vs Detect each.**
Toggle **Force preset** and **Detect each**.
→ Force preset applies your selected preset to every file; Detect each picks a
mode per file (falling back when detection is low-confidence). The highlighted
segment shows the active mode.
**PASS:** the toggle switches. **FAIL:** neither highlights, or both do.

**4.3 — Export Batch, then pause / resume.**
Click **Export Batch**. While it runs, click the **pause** button (`||`).
→ Processing pauses at the next file boundary (the glyph flips to `>`); clicking
`>` resumes.
**PASS:** pause halts progress and resume continues. **FAIL:** pause does
nothing.

**4.4 — Cancel mid-batch.**
Start a batch and click **cancel** (`x`) partway through.
→ You should see "Batch cancel requested"; the batch stops at the next file and
the not-yet-processed rows stay **pending** (not lost, not marked done).
**PASS:** cancel stops it and unprocessed rows remain pending. **FAIL:** it
finishes anyway, or loses the pending rows.

**4.5 — Crash recovery.**
Start a batch, then **force-kill the app** (Task Manager → End task) mid-batch.
Relaunch YesVox.
→ The queue should restore from disk and come back **paused**, with the
done/failed/pending counts intact.
**PASS:** the queue reappears paused after relaunch. **FAIL:** the queue is empty
or its state is wrong.

---

## 5. Export matrix

**Goal:** confirm the **OUTPUT** and **FILE** panels write correct files. Render
first, then use **EXPORT VOCAL** (or **Export Batch** for queues).

**5.1 — Format WAV vs FLAC.**
In **OUTPUT**, switch **Format** between **WAV** and **FLAC**, export each, and
open both.
→ Both should open and play identically (FLAC is lossless).
**PASS:** both export and play. **FAIL:** either won't write or won't open.

**5.2 — Bit depth.**
Change **Bit Depth** and export.
→ The exported file's bit depth matches your choice.
**PASS:** bit depth follows the selector. **FAIL:** it's ignored.

**5.3 — Dither.**
Toggle **Dither** on/off (under OUTPUT) and export at a lower bit depth.
→ Dither adds tiny shaped noise to reduce quantization artifacts; on/off should
both produce a valid file.
**PASS:** both export cleanly. **FAIL:** either errors.

**5.4 — Sample Rate is read-only.**
Look at **Sample Rate** in OUTPUT.
→ It's a **read-only chip** locked to the input's rate — there's deliberately no
way to change it (hard guardrail). It should not behave like a dropdown.
**PASS:** Sample Rate shows the input rate and isn't editable. **FAIL:** it lets
you change it.

**5.5 — Output name + location.**
Edit **Output File Name**, and click the **Location** field to pick a folder.
→ The name is editable text; Location opens a folder picker and shows the chosen
path.
**PASS:** name edits and folder picks stick. **FAIL:** either won't change.

**5.6 — Same-path export lock.**
Try to export on top of the **input file** (same path).
→ It should refuse with a friendly message rather than overwrite your source.
**PASS:** same-path export is blocked. **FAIL:** it overwrites the input.

**5.7 — Locked-file error.**
Open the intended export file in another app (so it's locked), then export.
→ You should get a friendly error, not a crash, and your source is untouched.
**PASS:** friendly error, no crash. **FAIL:** it crashes or corrupts something.

> _(FLAC compression level and source-metadata round-trip are handled
> automatically — there's no knob for them in v1 — and are covered by engine
> tests. Nothing to hand-test beyond "the FLAC export opens and plays".)_

---

## 6. Error paths (friendly messages, never a crash)

**Goal:** feed YesVox bad input and confirm it refuses with a clear message
instead of crashing. The exact message the user should see is quoted for each.

**6.1 — Corrupt / truncated file.**
Import a file that's not really audio (rename a `.txt` to `.wav`) or a
half-downloaded clip.
→ You should get a clean "couldn't read this file"-style error (a decode error),
**not** a crash.
**PASS:** friendly error, app still usable. **FAIL:** crash or hang.

**6.2 — Zero-length / empty file.**
Import an empty audio file.
→ Message: **"this file has no audio to process"**.
**PASS:** that message (or close), no crash. **FAIL:** crash, or it pretends to
process.

**6.3 — Over 2 hours.**
Import a file longer than 2 hours.
→ Message: **"this vocal is N min, over the 2 h limit — split it into shorter
takes"**.
**PASS:** that message. **FAIL:** it tries to load it and struggles/crashes.

**6.4 — Above 192 kHz.**
Import a file with a sample rate over 192 kHz.
→ Message: **"… Hz is above the 192 kHz limit — resample the vocal first"**.
**PASS:** that message. **FAIL:** it loads it anyway.

**6.5 — Looks like a mix (de-bleed refusal).**
Import a full mix (music + vocal), not an isolated vocal.
→ Message: **"input looks like a mix — run YesStems first"** (YesVox restores
*isolated* vocals; separation is YesStems' job).
**PASS:** it refuses with the YesStems pointer. **FAIL:** it processes the mix.

**6.6 — Missing / corrupt neural model (light-engine fallback).**
Test a build where the neural model is absent or corrupt (e.g. a dev build with
no bundled model, or a deliberately damaged model file).
→ The app should **not** crash: it falls back to the **Light** engine, and the
build stamp's engine word reads **Light** with the reason available on hover.
**PASS:** it renders on Light and the stamp explains why. **FAIL:** it crashes or
silently claims Neural.

**6.7 — Locked output file.**
(Same as 5.7 — listed here too because it's an error path.) Export to a file
another app has open → friendly error, no crash.
**PASS:** friendly error. **FAIL:** crash.

---

## 7. How to report a bug

When any step FAILs, capture this so it can be reproduced without you in the
room. Copy the template and fill it in:

```
YesVox bug report
------------------
Build stamp:   (the full stamp — version · hash · engine, e.g. v1.0.0-rc.3 · a1b2c3d · Neural)
Where:         GUI (Standard / Advanced) or CLI
Content mode:  Voiceover / Singing
Preset:        (which preset was selected)
Settings touched: (cards/knobs/dials you changed from default)
Source file:   (format, sample rate, bit depth, length, mono/stereo; where it came from)
Runbook step:  (e.g. 2.5 FLOW AMOUNT null-at-0)
What I did:    (the exact clicks)
What I expected: (the "you should see Y" from the step)
What I got:    (what actually happened — copy any on-screen message verbatim)
Timestamp:     (roughly when, so logs can be matched)
```

**The single most important field is the build stamp** — without it, a fix can't
be confirmed against the build you tested (§0). If the stamp said **Light** when
you expected **Neural**, say so; that alone explains a lot of "it sounds worse
than I remember" reports.
