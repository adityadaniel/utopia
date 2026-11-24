# 0002. Identity, title normalization and clocks

Status: Accepted. Source: `utopia-core` `title.rs`, `ids.rs`, `lib.rs` (PRD §10.3, §10.7, §14.4).

## Context

Two devices that create a page with the same title while offline must end up with one page,
and the rules that decide that must give the same answer on every platform for ever.

## Decision

- `utopia-core` is pure Rust (no CRDT, no SQL, no UI) and deterministic, because title
  normalization, date parsing and reference parsing are part of the frozen cross-platform
  contract (§14.4).
- `normalize_title` gives the `title_key`: NFKC, Unicode default case fold, NFKC again
  (the fold can denormalize), trim, and every internal whitespace run collapsed to one
  ASCII space. So `Café` (precomposed) and `Cafe` + combining acute, full-width `ＡＢＣ` and
  `abc`, `Straße` and `STRASSE` are one key.
- The function is **frozen and versioned**: its output feeds the uuidv5 page namespace, so a
  change duplicates pages across devices. A change bumps `NORMALIZATION_VERSION` and needs a
  migration.
- `clean_title` is the display form: NFC, trimmed, whitespace collapsed, case kept.
- A page made from a title has `uuidv5(NS_PAGE, "title:" + title_key)`; a daily page has
  `uuidv5(NS_DAILY, "daily:" + YYYY-MM-DD)`. The deterministic id is a duplicate-prevention
  heuristic, not an invariant: when that id is tombstoned, the caller uses
  `PageId::fresh` and never resurrects it.
- Blocks get a ULID at creation (FR-2.1). `((uid))` targets accept ULIDs and Roam-style
  short uids (1 to 64 characters).
- `now_ms` (Unix ms, UTC) is for display only. Ordering never uses a timestamp (§10.7).

## Consequences

The block creation time is the ULID's timestamp, so a fixture cannot fake the day a block
was written (issue #106, item E4).
