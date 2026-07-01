# YesVox status

The canonical current build snapshot is [`docs/STATUS.md`](docs/STATUS.md).

This root file is kept as a compatibility pointer for older handoffs that still
open `STATUS.md` first. Do not duplicate the detailed status here; it drifts
too easily. Update `docs/STATUS.md` whenever a slice lands, a decision is made,
or a blocker clears.

Current boundary, as of 2026-07-01:

- `main` is green through the canonical `cargo xtask gate` lane and CI runs the
  identical command on Windows, macOS, and Ubuntu.
- #16 singing is first-class and gated by the mechanical `singing` preservation
  test, not a blind-listening or Voiceover-only fallback flow.
- #17 Standard view and #19 batch/export work are implemented through the
  current `Session` and egui shell surface described in `docs/STATUS.md`.
- The remaining named mechanical gap is G18 undo/redo, which needs Dan's scope
  decision: Revert-only or a real undo stack.
- #18, #20, and #21 remain HITL/deferred boundaries.
