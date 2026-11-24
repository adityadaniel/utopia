---
name: delete-comment
description: Sweep code comments out of a part of Utopia and tighten the comment ratchet (ADR 0001). Sort each comment into Tautology, Excuse, Stale or Decision record; delete the first three, move decision records into docs/adr/, add excuses to the comment-sweep tracking issue. Use when the user runs /delete-comment [path…], asks to delete or sweep comments, or a ratchet failure says "fewer comments than the ratchet records".
---

# /delete-comment [path…]

Utopia's code carries no comments (`docs/adr/0001-no-code-comments.md`). The ratchet in
`xtask/comment-baseline.txt` records the comment lines each file still holds;
`cargo test --workspace` fails when a file holds more, and also when it holds fewer than
recorded until the baseline is tightened. This skill is one sweep: it takes a path (a crate,
a folder or files) and leaves it with no comments, the decisions written down, and the
shortcuts on record.

No path given: pick the smallest crate that still has comments
(`sort -n xtask/comment-baseline.txt`), say which, and go on.

## 0. Set up

- Work in a new worktree (AGENTS.md): `git worktree add ../u-topia-<slug> -b chore/delete-comments-<slug> main`,
  then `cp -cR <main>/target <worktree>/target` so the first build is seconds, not minutes.
- `source ~/.cargo/env; export DEVELOPER_DIR="$(xcode-select -p)" CARGO_INCREMENTAL=0`.

## 1. List

```bash
cargo run -q -p xtask -- comments --list <path…>
```

This prints every comment's first line with `file:line`. Read each one **with the code
around it** (open the file at that line). A comment's category depends on the code, not on
its wording.

## 2. Sort every comment into one category

| Category | Test | Action |
|---|---|---|
| **Tautology** | The name, the signature, the type or the next lines already say it. Also: a test comment that says what the assertion asserts, and a doc that only adds a PRD id. | Delete. |
| **Stale** | It does not match the code now: wrong function, wrong count, a type or file that is gone, a doc block attached to the wrong item. Check it (grep, read). | Delete. If the code is what is wrong, it is an Excuse instead. |
| **Excuse** | It explains away a shortcut: a silent fallback, a swallowed error, a sentinel value, a magic coordinate, "checked elsewhere", "for now", "should never happen", a workaround for our own tool. | Delete, and add one item to the tracking issue (step 4). Do not fix it in this sweep. |
| **Decision record** | It records a choice that is not obvious from the code and would be easy to undo by mistake: a frozen format, an ordering, a "refused, not clamped", why a simpler way does not work, a cost accepted on purpose. | Move it into an ADR (step 3), then delete it. |

Rules of thumb:

- When unsure between Tautology and Decision record, ask: "would a careful engineer who
  deletes this line later make the wrong change?" Yes means Decision record.
- When unsure between Decision record and Excuse, ask: "is this the design, or a gap in
  it?" A gap is an Excuse.
- `// SAFETY:` comments on `unsafe` blocks are exempt. Leave them; the lexer skips them.
- Comments in shell, YAML, TOML and Makefiles are out of scope (ADR 0001).
- PRD ids from deleted comments belong in test names (`ac_4_6_…`, `mc_8_…`), not in a
  new place in the code. Rename a test only when the id is not in its name and the test
  clearly covers that id.

Keep a ledger as you go: one row per comment block, `file:line`, category, destination,
first line. Save it as `docs/verification/<branch>/ledger.md`
(`docs/verification/chore-comment-ratchet/ledger.md` is the worked example).

## 3. Write the decision records

- Read `docs/adr/README.md`. If an ADR already covers the topic, add to it; otherwise copy
  the shape of an existing one (Status, Context, Decision, Consequences), take the next
  number, and add a row to the README table.
- Group by topic, not by file: one ADR per design area, with the decisions as short bullets.
- Write what was decided and why, in plain words, with the PRD ids. Do not paste the
  comments; rewrite them so they read as one record.
- Name the source files in the ADR's Status line so the next reader can find the code.

## 4. Record the excuses

All excuses go into the one tracking issue, one checklist item each (the user chose one
tracking issue over one issue per hack):

```bash
gh issue list --label comment-excuse --state open          # find the tracking issue
gh issue view <n> --json body -q .body > /tmp/body.md        # add the new items, then
gh issue edit <n> --body-file /tmp/body.md
```

Each item: the file and function, what the shortcut is, why it is wrong (the concrete
failure), and what a fix looks like. Number items on from the last one (`E6`, `E7`, …).
If there is no open tracking issue, create one with the label `comment-excuse`.

## 5. Strip, check, tighten

```bash
cargo run -q -p xtask -- comments --strip <path…>   # deletes every comment except SAFETY ones
cargo fmt --all
cargo run -q -p xtask -- comments --update          # records the lower counts
make check                                          # fmt, build, test (holds the ratchet), clippy
```

- `--strip` removes whole comment lines and trailing comments, and keeps strings intact.
  Read the diff anyway: `git diff -U0 | grep '^+'` must show only rustfmt reflows.
- `--update` refuses to raise a count. `--allow-growth` is only for a file that moved.
- If clippy asks for a doc (for example `missing_safety_doc` on a `pub unsafe fn`), the
  decision goes into an ADR and the `# Safety` section stays: say so in the PR.

## 6. Report

In the PR description and in the reply to the user:

- the ledger counts per category and the link to the ledger;
- the ADRs added or extended, and the tracking issue items added;
- the ratchet before and after (`cargo run -q -p xtask -- comments --total`);
- the `make check` output (test count), quoted, not described.

The sweep changes no behaviour, so no screenshots are needed. Say that in the PR.
