# 0009. The Loro boundary: one shard per page, waiting changes kept

Status: Accepted. Source: `utopia-doc/src/lib.rs`, `tests.rs` (PRD §13.2, §11.4, §12.3, §12.4,
§12.6, §8.6, §2.3.7).

## Decision

- One Loro document per page (a "shard"). Loro stays inside `utopia-doc` so it can be
  replaced (§11.4); `PageDoc` is the narrow seam, and commands are its only mutation path.
- Shard layout: map `page` (`title`, `kind`, `daily_date`, `created_at`, `deleted_at`,
  `merged_into`, and a mergeable map `titles` of title_key → display that holds the current
  title and every alias, so concurrent renames keep the losing title as an alias, §12.4) and
  tree `blocks` with fractional index, one node per block with `id`, `text` (LoroText),
  `kind`, `level`, `checked`, `deleted_at`, `moved_to`, `created_at`, `updated_at`.
- Two live nodes can carry one block id after two devices merge the same block concurrently
  (§8.6). The node with the smallest `TreeID` wins; the loser is hidden and its children go
  to the winner.
- Positions are `"<fractional index>!<block id>"`: `!` sorts below every fractional-index
  character, so index order is kept and positions are unique among siblings. Loro may
  rearrange sibling indexes when a new one does not fit, so after an insert or move the whole
  sibling list is read again if a neighbour moved.
- A structural command is mirrored on the cached `PageState` (ADR 0005) instead of a full
  rebuild, so batches stay linear (AC-4.7, AC-2.5).
- `import` is idempotent. Remote changes whose causal predecessors are missing are kept, not
  applied: `waiting()` exports them so the caller stores them durably, because snapshots and
  `export_since` never contain them (§12.3 step 3, §2.3.7). They apply on the import that
  brings the missing changes.
- `version_of` gives the version a destination's copy adds up to without building the page;
  changes that wait do not count, because no device can apply them (§12.3 step 4).
  `has_more_than` is what a sender asks before it sends more. `copy_state` and
  `waiting_ranges` (header only, no import) are for diagnostics (§12.6): a non-empty
  `waiting` on a destination means some device marked changes sent that never arrived.
