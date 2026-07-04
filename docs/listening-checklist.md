# Optional listening checklist (#21 Lane 3 — never gates the ship)

The mechanical gates (null/same-length/loudness/singing metrics, the #21 perf
tripwire and real-stem harness) are what gate the release. This checklist is
the **optional, non-blocking** human listen on the reference laptops — a
structured way to catch what metrics can't, not a sign-off requirement.

Setup: install the current `yesvox-v1-unsigned.exe`, use decent headphones,
pick 2–3 stems per content mode from the local eval corpus
(`YESVOX_EVAL_CORPUS`), render with the mode's default preset at default
settings, and A/B at matched loudness (Volume Match on).

For each stem, listen for and note (pass / concern + timestamp):

## Both modes
- [ ] **Voice character** — does the processed voice still sound like the same
      person? (timbre, weight, breathiness)
- [ ] **Denoise artifacts** — musical noise, underwater/phasey texture,
      chirps in pauses, or noise pumping around phrases.
- [ ] **Transitions** — word onsets/offsets clipped or softened; breaths
      chopped into gasps.
- [ ] **De-ess** — lisping or dulled consonants (s/t/k) vs. remaining harsh
      sibilance.
- [ ] **Tails** — room/reverb tails cut abruptly vs. natural decay.
- [ ] **Difference monitor** (reblend strip) — does what was *removed* sound
      like only noise/room, or is there voice in it?

## Voiceover
- [ ] Plosives (p/b) controlled without a thinned low end.
- [ ] Mouth clicks/lip smacks gone in pauses without pause "dropouts".
- [ ] Leveling: consistent presence across quiet/loud sentences, no pumping.

## Singing
- [ ] Vibrato intact (rate and depth — no flattening or wobble).
- [ ] Sustained notes: steady, no level breathing or tremolo introduced.
- [ ] Held-note tails and falls survive de-reverb.
- [ ] Breaths between phrases still sound human (not gated or ticking).

## Recording a result
Note stem name, mode, preset, and any concern with a rough timestamp in the
issue (or a `listening-notes.md` next to the local corpus — never committed).
A concern here does **not** block a release by itself; it becomes a normal
issue, ideally with a synthetic-fixture reproduction so the fix gets a
mechanical gate.
