## What and why

<!-- What changed, and the PRD ids it implements (FR-2.5, AC-4.6, §10.3.1). -->

## Proof of work

<!--
Screenshots from a real run against seeded mock data, per "Verifying a change" in AGENTS.md.
Commit them under docs/verification/<branch>/ and inline them here, each captioned with what
it proves. "The tests pass" is not proof that a feature works.
-->

| What it proves | Screenshot |
| --- | --- |
|  |  |

## Checks

```
<!-- Paste the output: make check, and the test count. -->
```

- [ ] `make check` passes (fmt, build, test, clippy)
- [ ] Seeded a scratch graph with mock data and drove the real app (`UTOPIA_SHOT_SCRIPT`)
- [ ] **Looked at** every screenshot captured, not just checked the run succeeded
- [ ] Screenshots committed under `docs/verification/<branch>/` and inlined above
- [ ] `cargo run -p xtask --release -- bench --ui` (only if performance-sensitive)
- [ ] `make sync-fuzz` (only if the sync engine, the store's import/outbox path or `utopia-doc` changed)
- [ ] Settings → Help screenshots regenerated if the UI they show changed
      (`cargo run -p xtask -- help-shots`)

## Bugs found on the way

<!-- Anything that broke while building this, and how it was fixed. The useful part of a review. -->

## Not done / known limits

<!-- What you left out, worked around, or could not verify, and why. Say it here, not nowhere. -->
