# 0006. Inline grammar, attachments and queries are plain text

Status: Accepted. Source: `utopia-core` `inline.rs`, `attach.rs`, `text.rs`, `markdown.rs`,
`query.rs` (PRD Appendix B, §7.2, §6.5, FR-2.15, FR-2.16, FR-3.9, FR-4.14, AC-2.8 to AC-2.10).

## Inline grammar

- A block body is plain source text. Ranges are byte offsets into it.
- An unclosed delimiter is literal text (rule 3, AC-2.9). Code is opaque (rule 4).
- Atoms (code, page refs, tags, block refs, links) never contain emphasis and are skipped
  whole when searching for a closing delimiter. Refs inside emphasis are still refs.
- Intraword underscores (`snake_case`) are not emphasis.
- A bare `http(s)://` URL (Appendix B `bare_url`, after GFM extended autolinks) ends at
  whitespace, `<` or `>`, without trailing punctuation and without a `)` or `]` that closes
  nothing inside it. A URL is opaque like code: its `#fragment` is no tag, its underscores
  no italics.
- `#tag`, `#[[Title]]` and `[[Title]]` are the same edge (FR-2.15); `Tag` stays distinct
  only for styling. Autocomplete writes `#Title` for one token and `#[[Title]]` otherwise.
- Hashtag longest match happens at commit only (§6.5): a bare `#Word` followed by words
  that together are a live page title becomes `#[[Word more words]]`, up to six words. Only
  brackets are added; the typed characters stay.
- `{{embed: ((uid))}}` is the same edge as a block ref. `Key:: value` at the start of a block
  is an attribute.
- Unlinked mentions (FR-3.9) are case-insensitive, whole-word, and never inside a ref, code
  or link.
- A link is external unless it is `blob:`, `pdfpage:` or `utopia:` (FR-4.10).

## Typing helpers

- Only brackets auto-pair or wrap a selection. `**`, `_` and backtick have Cmd+B/I/E, and a
  quote, `*` or `_` is as often an apostrophe or a bullet as a delimiter.
- An opener before a word character is a lone opener (`(foo`); a closer that is already the
  next character is stepped over; Backspace in an empty pair deletes both.
- Typing `[` over a selection wraps it and keeps it selected, so `[` twice gives `[[word]]`.
- Ctrl+T swaps the graphemes around the caret; at a paragraph end it swaps the two before
  the caret, as TextEdit does. It never swaps across a line break and never splits a
  grapheme.

## Attachments

- Files live outside the graph, named by SHA-256 (§10.3, §11.6). Blocks point at them with
  ordinary Markdown: `![name](blob:H)`, `[name](blob:H)`, a PDF by its name; Roam's
  `{{pdf: blob:H}}` is read too. Plain text syncs, exports and survives Markdown untouched.
- Brackets in a file name are replaced, because they would end the link text early.
- A PDF highlight is `{{highlight: blob:H <page> x,y,w,h[;…]}}`: page 1-based in the text,
  rectangles page-relative (`0..1`, origin top left), one per selected line or one area, at
  most 64.

## Markdown and queries

- Paste: one block per non-empty line, list markers stripped, relative indentation kept,
  a tab counts as 4 spaces, and an indent that opened no level closes back to the nearest
  shallower one.
- Query blocks use Roam's syntax so imported graphs keep working (`{and:}`, `{or:}`,
  `{not:}`, `{between:}`, `{attr:}`, `"text"`, `[[TODO]]`/`[[DONE]]`; several top-level
  terms mean `and`). A query block with a syntax error shows the error instead of results.
