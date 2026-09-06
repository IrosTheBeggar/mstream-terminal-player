# UX contracts

A UX contract is the portable spec for one feature, extracted from the
surface that got it right — the **design of record** — so another surface
can implement the same feature without copying its layout. The contract
carries what the feature *promises*: entry points, states, verbs, wording,
edge cases, and the invisible polish (guards, orderings, failure
taxonomies). It deliberately does not carry layout, widget choices, or
styling — each surface renders the contract in its own idiom (this repo's
is `src/kit/` + docs/ui-kit.md).

The product family builds this way already: the mobile app's screens are
"webapp parity" ports, and this player's TUI tabs are "the webapp's panel,
port for port" with named deviations. A contract is that practice written
down first, so the translation decisions are deliberate and reviewable
instead of rediscovered mid-implementation.

## Format

One file per feature, `<feature>.md`. Sections, in order:

1. **Header** — design of record (repo, files, commit), server API
   endpoints, status of this surface's implementation.
2. **Intent** — one paragraph: the promise to the user.
3. **Entry points** — every way in, including capability gating.
4. **States & flows** — the stages, what persists across them, what
   invalidates what.
5. **Behavior contract** — the numbered rules, grouped by stage. These are
   the testable clauses; an implementation disagreeing with one is a bug
   or a logged deviation.
6. **Wording** — the reference strings (English) with their locale keys.
   All surfaces in the family ship the same ten locales, so translations
   can be carried over, not re-made.
7. **Out of scope here** — parts of the record deliberately not ported to
   this surface, with the consequences named.
8. **Translation notes** — this surface only: idiom mapping, what already
   exists in shared code, open questions to settle during implementation.
9. **Deviations log** — dated, deliberate differences from the record,
   with reasons. Starts at extraction, grows during implementation and
   review.

Rules of the road:

- **One design of record per feature.** When surfaces disagree, the named
  record wins; a better idea from elsewhere goes through the deviations
  log, not silent blending.
- **Pin the record's commit.** The record keeps moving; the contract says
  what was ported. Re-extraction is a new commit touching the header.
- **Contract before code.** The extraction is the review artifact; the
  implementation cites its clauses.
