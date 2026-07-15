# YesVox status

The canonical current build snapshot is [`docs/STATUS.md`](docs/STATUS.md).

This root file is kept as a compatibility pointer for older handoffs that still
open `STATUS.md` first. Do not duplicate the detailed status here; it drifts
too easily. Update `docs/STATUS.md` whenever a slice lands, a decision is made,
or a blocker clears.

Snapshot (see `docs/STATUS.md` for the authoritative, always-current detail):

- The build is `1.0.0-rc.2`. `main` is green through the canonical
  `cargo xtask gate` lane, and CI runs the identical command on Windows, macOS,
  and Ubuntu.
- #16 singing is first-class and gated by the mechanical `singing` preservation
  test, not a blind-listening or Voiceover-only fallback flow.
- #17 Standard view and #19 batch/export work are implemented through the
  current `Session` and egui shell surface described in `docs/STATUS.md`.
- Undo/redo (formerly the open G18 gap) is DONE — #25 landed a bounded,
  coalescing `History<T>` wired into the shell. See `docs/STATUS.md`.
- Remaining boundaries are the HITL/deferred items tracked in `docs/STATUS.md`.
