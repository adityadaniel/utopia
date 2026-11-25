# 0013. Typed dates on the phone mean the day the chip was tapped

Status: Accepted (2026-11-25). Source: `crates/utopia-core/src/date.rs`,
`crates/utopia-ffi/src/lib.rs`, `mobile/ios/UtopiaMobile` (ios-prd §4.6, AC-4.12, AC-4.13,
C1, C3; existing PRD §5.5, §10.3.1; ADR 0003).

## Context

The owner's to-do "Need to have a way linking date in Utopia mobile" asks for the desktop
habit without brackets: the words of a compose line that name a day offer a chip, and a tap
links that day. The phone may not parse dates itself (C1), so the core has to say which
words are a date and which day they mean.

The desktop already resolves relative phrases (`[[tomorrow]]`, `[[next sunday]]`,
`[[2 weeks ago]]`) through `utopia_core::date::relative_date`, and writes the day's title
from the graph's date format. It does not resolve `[[Nov 26]]`; only titles in a graph's
date format resolve, and `parse_unambiguous_date` needs a year. A month and day typed on
the phone still need an answer.

There are three ways to choose the year for `Nov 26`: the current year always, the next
such day (roll to next year once it has passed), or ask. Only two can be a chip.

## Decision

- The accepted phrases are the relative phrases `relative_date` already knows, a weekday
  name on its own (its next occurrence, today included), and an absolute
  month-and-day with an optional four-digit year (`Nov 26`, `26 November`,
  `November 26th, 2027`). `26th of November` is not accepted. ISO `YYYY-MM-DD` is accepted
  through `parse_iso`. A month name may be abbreviated to three or more letters.
- **A month-and-day with no year is that day in the current year, even if it has already
  passed.** This is a choice, not a guess: `Nov 26` on December 1st links the November
  26th just gone, which is the date the reader wrote. The year is displayed in the chip, so
  a wrong one is visible before the tap. Typing the year is how a reader asks for another
  one; the date is still editable after the tap (AC-4.4).
- A bare number (`26`), a bare month (`may`, `march`), a bare weekday abbreviated to three
  letters (`sun`, `sat`) and a numeric date (`11/6/26`, existing PRD §5.5) are no date.
  Full weekday names (`friday`) are, because they are unambiguous.
- The day is resolved when the chip is offered and stored on the accepted link as
  `YYYY-MM-DD`, together with the words that were tapped. The chip shows the day's title,
  and the Send writes `[[title of the accepted day]]`. The link is kept while its range
  still holds those same words (in any letter case) and the range sits on word boundaries,
  so the character before and after it is not a letter or digit. `'` and `-` are not
  letters or digits, so `tomorrow's` keeps its link and the Send writes `[[…]]'s`; that is
  deliberate. The check applies only to a date chip's link, never to a title link or a
  picker insert, which keep the rule they had before. The check never consults the clock.
- A title link with no page yet writes the typed words as the title; the Send makes the
  page.
- One FFI call, `date_suggestions(text, accepted, limit)`, returns the title, the day, the
  day's page id, the words and the UTF-16 range. It shares the accepted-range skip and the
  limit with `title_suggestions`. Today comes from the store's clock (`set_today` pins it for
  tests and screenshots), so the phone does not keep a second one.

## Consequences

- `Nov 26` near the end of the year links backwards. A reader who means next year types it.
- A relative phrase kept in a draft for days (`tomorrow`, sent a week later) still writes
  the day the chip promised: the link never expires with time. It is dropped only when the
  reader edits its words, so the range no longer holds them or no longer sits on word
  boundaries. A launch and a refresh never drop it.
- The phone and the Mac agree because both resolve through `utopia_core::date`; only the
  trigger differs (a chip on the phone, `[[…]]` completion on the Mac).
- A yearless `Nov 26` chip can be offered for a day the graph has no page for. Send makes
  the day's page through the normal link path (FR-1.2), like any other accepted link.
