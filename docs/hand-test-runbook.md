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

  > `v1.0.0-rc.2 · a1b2c3d · Neural`

  - `v1.0.0-rc.2` — the version.
  - `a1b2c3d` — the exact code build (a short git hash). If it ends in `-dirty`
    the build was made from uncommitted changes; if it says `unknown` the build
    machine had no git — both are fine for a local test, just note it.
  - `Neural` or `Light` — **which engine actually ran your last render.** Neural
    is the full-quality MossFormer2 model; Light is the always-available fallback.
    If a render couldn't load the neural model it falls back to Light and the
    stamp says so (hover it for the reason).

- **In the CLI:** run `yesvox --version` (or `yesvox -V`). You should see:

  > `yesvox 1.0.0-rc.2 (a1b2c3d)`

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
