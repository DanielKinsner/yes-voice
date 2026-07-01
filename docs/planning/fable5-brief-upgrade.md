# yes-voice / YesVox — Fable session cook card

> Add-on to [`fable5-brief-yesvox.md`](fable5-brief-yesvox.md) +
> [`fable5-planning-constitution.md`](fable5-planning-constitution.md) + the shared **Suite
> Constitution** (`…/yes-master/docs/fable-planning/00-FABLE-SUITE-CONSTITUTION.md`). Your
> three-bucket forcing function and "questions you should've asked" section are best-in-class —
> they're in the shared Constitution now. This card is only the stuff **Fable can't derive from
> the code**: your intent, one trust instruction, and where to cook.

## What only you know (feed this; it's not in the code)

- The product is **offline vocal restoration + finish, NOT production.** Preserve vocal character
  above all — never trade vibrato/breath/timbre for more cleanup. Singing is first-class,
  validated by the **mechanical singing gate**, not blind listening. Same take, same length, same
  sample rate out — no timeline editing.
- The optional **finish must genuinely "wow"** — people buy YesVox on its own, so the standalone
  output has to be impressive, not just clean.
- One app in a family: de-bleed cleans *imported* separated stems (separation is yes-stems);
  YesVox is publish-usable, **not** the final loud master (that's yes-master).

## The one instruction that matters most here

**Treat my existing v1 plan as *intent*, not as built reality.** Parts of it describe the product
I want, not the code that exists — before you inherit or deepen any section, reconcile it against
the actual source at HEAD and tell me plainly where my plan out-ran the implementation. Don't
plan on top of anything the code doesn't actually contain. (This overrides "inherit §1–§6, don't
reinvent" — verify first.)

## Cook here — think past my framing

- Define what "tasteful, character-preserving, impressive finish" actually *is* — adaptive
  presence/air + gentle leveling + optional ambience — and how it stays honest (labelled,
  non-destructive, reblendable). Go well beyond what I asked.
- If this were yours: the fastest *honest* path to a sellable v1 — what's the true critical path,
  and what in my current plan is theatre?
- The neural quality tier is what people actually pay for — think hard about what it takes to ship
  working, licensed neural restoration on a stranger's machine, and what you'd cut to de-risk it.

## Cross-repo note (not a dependency)
YesVox's feature-gated `ort` + deterministic-fallback approach is the pattern the sibling
yes-stems will likely borrow when *it* chooses native inference — nothing for YesVox to do here,
just don't spend budget re-deriving it; it's already yours.
