# Architecture decision records

An ADR records one decision about the code that is not obvious from the code: what was
chosen, why, and what it costs. Since ADR 0001 the code carries no comments, so this folder
is where the "why" lives. Cite an ADR from a test name, a commit or a PR, not from the code.

| ADR | Decision |
|---|---|
| [0001](0001-no-code-comments.md) | No code comments; a ratchet enforces it |
| [0002](0002-identity-and-titles.md) | Identity, title normalization and clocks |
| [0003](0003-daily-dates.md) | Daily-page dates refuse to guess |
| [0004](0004-commands-and-undo.md) | Invertible commands, one global undo stack |
| [0005](0005-reference-page-tree.md) | `PageState` is the CRDT-free reference semantics |
| [0006](0006-inline-grammar-and-markup.md) | Inline grammar, attachments and queries are plain text |
| [0007](0007-outline-planners.md) | Outline edits are planners; depth is refused, not clamped |
| [0008](0008-graph-layout.md) | Deterministic force-directed graph layout |
| [0009](0009-crdt-boundary.md) | The Loro boundary: one shard per page, waiting changes kept |
| [0010](0010-wire-format-v1.md) | Sync wire format v1 |
| [0011](0011-plugin-sandbox.md) | WASM plugin sandbox and its run result |
| [0012](0012-dev-tooling.md) | Benchmarks, screenshots, the icon and the mobile check |
| [0013](0013-typed-dates-on-the-phone.md) | Typed dates on the phone mean the day the chip was tapped |

To add one: copy the shape of an existing file (Status, Context, Decision, Consequences),
take the next number, and add a row here.
