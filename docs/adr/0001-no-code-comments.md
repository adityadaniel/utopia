# 0001. No code comments; a ratchet enforces it

Status: Accepted (2026-11-24)

## Context

On 2026-11-24 the tracked `.rs`, `.swift` and `.ts` files held 11,471 comment lines in about
5,800 blocks (about 10% of the code). When we read them, we found four kinds:

- **Tautology**: the comment says again what the name, the type or the next line says.
- **Excuse**: the comment explains away a shortcut ("invariants are checked elsewhere",
  "resync the cache"), so the next agent copies the shortcut instead of fixing it.
- **Stale**: the comment describes code that changed (a doc block that moved onto the wrong
  function, a crate doc that names one command of fifteen).
- **Decision record**: the comment records a real, non-obvious decision.

Only the fourth kind is worth keeping, and a comment is a poor place for it: nothing checks
it, it cannot be found from outside the file, and it decays silently.

## Decision

- Code carries no comments: no `//`, `///`, `//!` or `/* */` in Rust, and no `//` or
  `/* */` in Swift and TypeScript.
- The one exception is a `// SAFETY:` comment (and the `//` lines straight after it) on an
  `unsafe` block, which Rust convention and review need.
- The reason for a decision goes into a name, a type, a test (named after the PRD id, as
  AGENTS.md asks) or an ADR in this folder. A shortcut goes into an issue, not a comment.
- Shell, YAML, TOML and Makefile comments are out of scope for now: they are configuration,
  and the lexer does not read those languages.

## Enforcement: the ratchet

`xtask/src/comments.rs` lexes every tracked `.rs`, `.swift` and `.ts` file (strings, raw
strings, char literals, lifetimes, Swift interpolation and TypeScript templates are skipped,
so `"http://"` is not a comment) and counts the lines that hold a comment.
`xtask/comment-baseline.txt` records the count each file may still hold.

- `the_repository_holds_the_comment_ratchet` runs in `cargo test --workspace`, so
  `make check` and CI fail when a file holds more comment lines than its baseline, or when a
  file that is not in the baseline holds any.
- A file that holds fewer also fails, until `cargo run -p xtask -- comments --update`
  writes the lower count. So the ratchet only moves down, and every PR that deletes comments
  records it.
- `--update` refuses to raise a count; `--allow-growth` exists only for a file that moved.
- `comments --list <path>` prints the comments in a path, `comments --strip <path>` deletes
  them (not the SAFETY ones), and `comments --total` prints what is left.

The `/delete-comment` skill (`.agents/skills/delete-comment/SKILL.md`) is the procedure for
one sweep: list, sort into the four kinds, move decision records here, collect excuses in
the tracking issue (#106, label `comment-excuse`), strip, check, tighten the ratchet.

## Consequences

- rustdoc output for the crates becomes empty. The crates are not published, so nothing
  outside the repository reads it.
- Reference from code to PRD ids (`FR-2.5` in a doc comment) is lost; test names keep it.
- An ADR can go stale too, but it is one place to look and to review, and it is not copied
  next to every use.
