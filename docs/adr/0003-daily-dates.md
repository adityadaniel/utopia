# 0003. Daily-page dates refuse to guess

Status: Accepted. Source: `utopia-core/src/date.rs` (PRD §5, §5.5, §10.3.1, AC-1.8, DN-2).

## Decision

- A day's identity is always ISO `YYYY-MM-DD`. A `DateFormat` is only a rendering, and the
  weekday caption (`weekday_name_of`, DN-2, English, Monday first) is display only.
- Parsing refuses to guess. A format is unambiguous only when it has year, month and day and
  the month is a name or the numeric order is year first. `11/6/26` never resolves (§5.5).
- `parse_unambiguous_date` (§10.3.1) accepts ISO-8601, the built-in month-name forms
  (accepted whatever the graph setting is), and the graph's format when it is unambiguous.
- When the graph changes its format, the old pattern is kept in `previous`, so links typed
  under it still resolve (AC-1.8: "no link breaks").
- Month names match longest first, so `May` and `Mar` do not collide. A whitespace literal
  in a pattern matches any run of whitespace.
- Relative phrases (`today`, `next sunday`, `in 3 days`, `2 weeks ago`) are never seen by
  the resolver: what "tomorrow" means changes every day, so a stored link to it could not be
  rebuilt. The editor rewrites the phrase to the day's title instead:
  - while the block is being typed (idle commit), a link the caret is inside is left alone;
  - when the block is finished (blur, Enter, Escape), that link is expanded too and the
    caret goes just after it;
  - a phrase that names an existing page or alias is left alone, so a page called "Today"
    keeps its links.
- `next <weekday>` is the first such day after today, `last <weekday>` the most recent one
  before it. A bare weekday (`Monday`) is not a date: it is too likely to be a page title.
