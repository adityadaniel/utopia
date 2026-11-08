//! Acceptance scripts from the PRD (§5.4, §6.4, §7.4, §8.4, §8.6, §4.3) run against a
//! real SQLite store.

use chrono::NaiveDate;
use utopia_core::outline::{self, Plan};
use utopia_core::{BlockId, BlockKind, Command, PageId, PageKind};
use utopia_doc::Frontier;
use utopia_store::{BlockRefStatus, EditOpts, PageFate, RefQuery, RefTarget, Store, StoreError};

fn date(y: i32, m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, d).unwrap()
}

/// A file-backed store, so every acceptance script runs through the writer thread. The
/// store is dropped (flushing and joining the writer) before its directory is removed.
struct TestStore {
    store: Store,
    _dir: tempfile::TempDir,
}

impl std::ops::Deref for TestStore {
    type Target = Store;
    fn deref(&self) -> &Store {
        &self.store
    }
}

impl std::ops::DerefMut for TestStore {
    fn deref_mut(&mut self) -> &mut Store {
        &mut self.store
    }
}

fn store() -> TestStore {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path().join("graph.utopia")).unwrap();
    store.set_today(Some(date(2026, 11, 2)));
    TestStore { store, _dir: dir }
}

fn run(s: &mut Store, page: &PageId, plan: Plan) -> Option<BlockId> {
    s.execute_plan(page, plan, None).unwrap().map(|f| f.block)
}

/// Append a block to the end of a page, type `text`, and commit it.
fn write(s: &mut Store, page: &PageId, text: &str) -> BlockId {
    let plan = outline::append_block(s.page_state(page).unwrap(), None);
    let id = run(s, page, plan).unwrap();
    s.set_block_text(page, &id, text, None, None).unwrap();
    s.commit_block(page, &id).unwrap();
    id
}

/// Type without committing (no page auto-creation).
fn type_only(s: &mut Store, page: &PageId, text: &str) -> BlockId {
    let plan = outline::append_block(s.page_state(page).unwrap(), None);
    let id = run(s, page, plan).unwrap();
    s.set_block_text(page, &id, text, None, None).unwrap();
    id
}

fn text(s: &mut Store, page: &PageId, b: &BlockId) -> String {
    s.page_state(page).unwrap().block(b).unwrap().text.clone()
}

fn live_texts(s: &mut Store, page: &PageId) -> Vec<(String, usize)> {
    let st = s.page_state(page).unwrap().clone();
    st.live_preorder()
        .into_iter()
        .map(|(id, d)| (st.block(&id).unwrap().text.clone(), d))
        .collect()
}

fn assert_doctor(s: &mut Store) {
    let r = s.doctor().unwrap();
    assert!(r.is_ok(), "doctor: {:#?}", r.problems);
}

/// The local trash row for `page`, read straight from the graph. `list_trash` hides a stale
/// row on purpose (it joins the page row), which is how the hidden state below stayed
/// un-modelled (docs/state-machines/page-lifecycle-trash.md).
fn trash_rows(s: &Store, page: &PageId) -> i64 {
    s.conn()
        .query_row(
            "SELECT count(*) FROM trash WHERE id=?1",
            [page.as_str()],
            |r| r.get(0),
        )
        .unwrap()
}

/// The tombstone column of the projected `page` row: what the index believes.
fn deleted_at(s: &Store, page: &PageId) -> Option<i64> {
    s.conn()
        .query_row(
            "SELECT deleted_at FROM page WHERE id=?1",
            [page.as_str()],
            |r| r.get(0),
        )
        .unwrap()
}

#[test]
fn ac_1_1_lazy_daily_and_relaunch() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("g.utopia");
    let page;
    {
        let mut s = Store::open(&path).unwrap();
        s.set_today(Some(date(2026, 11, 2)));
        assert!(
            s.list_pages(true).unwrap().is_empty(),
            "untouched graph stays empty"
        );
        let (p, b) = s.open_daily(date(2026, 11, 2)).unwrap();
        s.set_block_text(&p, &b, "hello", None, None).unwrap();
        page = p;
    }
    let mut s = Store::open(&path).unwrap();
    s.set_today(Some(date(2026, 11, 2)));
    let (p, b) = s.open_daily(date(2026, 11, 2)).unwrap();
    assert_eq!(p, page);
    assert_eq!(text(&mut s, &p, &b), "hello");
    assert_eq!(s.page_row(&p).unwrap().unwrap().title, "November 2nd, 2026");
    assert_doctor(&mut s);
}

#[test]
fn ac_1_4_backfill_thirty_days() {
    let mut s = store();
    for i in 0..30 {
        let d = date(2026, 10, 1) + chrono::Days::new(i);
        let (p, b) = s.open_daily(d).unwrap();
        assert_eq!(s.page_state(&p).unwrap().live_count(), 1);
        // The feed and sidebar list days with entries (FR-1.2).
        s.set_block_text(&p, &b, "x", None, None).unwrap();
    }
    assert_eq!(s.list_pages(true).unwrap().len(), 30);
    assert_eq!(s.journal(14).unwrap().len(), 14);
}

/// FR-1.2: the Daily Notes feed stops loading older days at the oldest daily page. A day
/// that was only visited is a draft, not a page, so it does not extend the feed; a
/// trashed day does not hold it open either.
#[test]
fn fr_1_2_oldest_daily_page_bounds_the_feed() {
    let mut s = store();
    assert_eq!(s.oldest_daily().unwrap(), None, "a fresh graph has no day");
    // Visiting an old day creates a draft, not a page (FR-1.2).
    let drafted = s.draft_daily(date(2026, 10, 20)).unwrap();
    assert_eq!(
        s.oldest_daily().unwrap(),
        None,
        "a visited day is a draft, not a page"
    );
    write(&mut s, &drafted, "older");
    assert_eq!(s.oldest_daily().unwrap(), Some(date(2026, 10, 20)));
    // A later visit to an even older date must not move the feed's floor.
    s.draft_daily(date(2019, 1, 1)).unwrap();
    assert_eq!(
        s.oldest_daily().unwrap(),
        Some(date(2026, 10, 20)),
        "drafts do not bound the feed"
    );
    let (trashed, _) = s.open_daily(date(2026, 10, 10)).unwrap();
    assert_eq!(
        s.oldest_daily().unwrap(),
        Some(date(2026, 10, 10)),
        "a live page older than every other"
    );
    s.delete_page(&trashed).unwrap();
    assert_eq!(
        s.oldest_daily().unwrap(),
        Some(date(2026, 10, 20)),
        "trashed days do not hold the feed open"
    );
    assert_doctor(&mut s);
}

#[test]
fn fr_1_2_visited_day_is_a_draft_until_written() {
    let mut s = store();
    let d = date(2026, 11, 2);
    let p = s.draft_daily(d).unwrap();
    assert_eq!(p, PageId::for_date(d));
    assert_eq!(s.draft_daily(d).unwrap(), p, "one draft per day");
    assert!(s.is_draft(&p));
    assert_eq!(s.page_state(&p).unwrap().live_count(), 1);
    assert!(s.find_page("2026-11-02").unwrap().is_none());
    assert!(s.list_pages(true).unwrap().is_empty());
    assert!(s.outbox_pending(10).unwrap().is_empty(), "nothing to sync");
    assert_doctor(&mut s);
    // The first edit writes the whole page: meta, first block and the edit.
    let b = s.page_state(&p).unwrap().live_roots()[0].clone();
    s.set_block_text(&p, &b, "hello", None, None).unwrap();
    assert!(!s.is_draft(&p));
    assert_eq!(s.find_page("2026-11-02").unwrap(), Some(p.clone()));
    assert!(!s.outbox_pending(10).unwrap().is_empty());
    assert_eq!(live_texts(&mut s, &p), [("hello".to_string(), 0)]);
    assert_doctor(&mut s);
    // A link to a drafted day writes that draft instead of a second page (FR-1.10).
    let next = s.draft_daily(date(2026, 11, 3)).unwrap();
    write(&mut s, &p, "see [[2026-11-03]]");
    assert_eq!(s.find_page("2026-11-03").unwrap(), Some(next.clone()));
    assert!(!s.is_draft(&next));
    assert_doctor(&mut s);
}

/// `docs/state-machines/draft-daily-page.md` issue 1: a command that fails on a draft used
/// to remove its shard but keep the `drafts` entry, so `is_draft` said "draft" while
/// `page_state` said `NotFound`. A draft and its shard go together (FR-1.2).
#[test]
fn fr_1_2_failed_command_leaves_no_ghost_draft() {
    let mut s = store();
    let d = date(2026, 11, 2);
    let draft = s.draft_daily(d).unwrap();
    assert!(s.is_draft(&draft));
    // A block id that is already in the shard fails the whole command group (the same
    // NOOP a racing insert produces), after the earlier commands applied in memory.
    let taken = s.page_state(&draft).unwrap().live_roots()[0].clone();
    let failed = s.execute(
        &draft,
        vec![Command::InsertBlock {
            id: taken,
            parent: None,
            after: None,
            text: String::new(),
            kind: BlockKind::Bullet,
        }],
        EditOpts::default(),
    );
    assert!(failed.is_err(), "the duplicate block id must fail");
    assert!(
        !s.is_draft(&draft),
        "a draft whose shard is gone must not be reported as a draft"
    );
    assert!(
        s.page_state(&draft).is_err(),
        "the shard went with the draft entry"
    );
    assert!(
        matches!(s.draft_append_block(&draft), Err(StoreError::Invalid(_))),
        "typing on it is refused before `load_doc` looks for a draft shard"
    );
    // The day is simply unwritten again: visiting it builds a fresh draft under the same
    // deterministic id.
    let again = s.draft_daily(d).unwrap();
    assert_eq!(again, draft, "the date keeps its identity");
    assert!(s.is_draft(&again));
    assert_eq!(s.page_state(&again).unwrap().live_count(), 1);
    assert_doctor(&mut s);
}

/// `docs/state-machines/draft-daily-page.md` issue 2: a writer failure clears the in-memory
/// shards so they reload from disk. A draft has no disk state to disagree with, so clearing
/// it too threw away the day the view was still showing (FR-1.2).
#[test]
fn fr_1_2_writer_failure_keeps_the_draft() {
    let mut s = store();
    let live = s.create_page("Ana").unwrap();
    let d = date(2026, 11, 2);
    let draft = s.draft_daily(d).unwrap();
    // Fault injection: the next commit cannot write its outbox row, so the writer thread
    // records an error and `sync_index` drops the shards that have a durable counterpart.
    s.conn().execute_batch("DROP TABLE outbox").unwrap();
    let plan = outline::append_block(s.page_state(&live).unwrap(), None);
    run(&mut s, &live, plan);
    assert!(s.sync_index().is_err(), "the writer reported the failure");
    assert!(
        s.is_draft(&draft),
        "a draft has no durable state to reload, so it survives"
    );
    assert_eq!(s.page_state(&draft).unwrap().live_count(), 1);
    let appended = s.draft_append_block(&draft).unwrap();
    assert!(s.page_state(&draft).unwrap().block(&appended).is_some());
}

/// `docs/state-machines/draft-daily-page.md` issue 3: a draft is promoted for a date that a
/// remote device already wrote under another id (its uuidv5 was tombstoned there, rule 4).
/// Both would be live daily pages for the same date, `page_daily_uq` rejects the second row
/// and the commit fails with the typed text still only in memory (FR-1.2, §8.6).
#[test]
fn fr_1_2_promoted_draft_merges_into_the_days_page() {
    let d = date(2026, 11, 2);
    // Another device: the day's deterministic id was tombstoned there, so the live page for
    // the date is a fresh ULID.
    let mut remote = store();
    let gone = remote.ensure_daily_page(d).unwrap();
    write(&mut remote, &gone, "remote one");
    remote.delete_page(&gone).unwrap();
    let live = remote.ensure_daily_page(d).unwrap();
    assert_ne!(live, gone, "a tombstoned id is never resurrected");
    write(&mut remote, &live, "remote two");
    let shard = remote.export_since(&live, &Frontier::default()).unwrap();

    // This device already shows the day as a draft; only the live page's update arrives
    // (the tombstoned id may come in a later round).
    let mut s = store();
    let draft = s.draft_daily(d).unwrap();
    s.import_batch(&[(live.clone(), shard)]).unwrap();
    assert!(s.is_draft(&draft), "the draft is still open");
    let block = s.page_state(&draft).unwrap().live_roots()[0].clone();
    s.set_block_text(&draft, &block, "typed here", None, None)
        .unwrap();
    s.sync_index().expect("promoting a draft must not fail");
    assert!(!s.is_draft(&draft));
    let days = s.days_between(d, d).unwrap();
    assert_eq!(days.get(&d).cloned(), Some(live.clone()), "one live day");
    let texts = live_texts(&mut s, &live);
    assert!(
        texts.iter().any(|(t, _)| t == "typed here"),
        "the draft's text moved into the day's page: {texts:?}"
    );
    assert!(
        texts.iter().any(|(t, _)| t == "remote two"),
        "the remote text stays: {texts:?}"
    );
    assert_eq!(s.find_page("2026-11-02").unwrap(), Some(live));
    assert_doctor(&mut s);
}

/// Continuing issue 3: the only way a draft's date is owned by another page is that the
/// remote *deleted* the deterministic id (rule 4). Writing the draft's shard as a
/// `merged_into` tombstone then lets `reconcile_merged` move the trashed blocks into the
/// live day, on both devices. A draft has no durable identity, so its shard must never be
/// written when the date is owned (rule 5).
#[test]
fn fr_1_2_promoted_draft_does_not_resurrect_a_deleted_page() {
    let d = date(2026, 11, 2);
    // Remote: "remote one" was written on uuidv5(D), then trashed; the live page for the
    // date is a fresh ULID.
    let mut remote = store();
    let gone = remote.ensure_daily_page(d).unwrap();
    write(&mut remote, &gone, "remote one");
    remote.delete_page(&gone).unwrap();
    let live = remote.ensure_daily_page(d).unwrap();
    write(&mut remote, &live, "remote two");
    let gone_shard = remote.export_since(&gone, &Frontier::default()).unwrap();
    let live_shard = remote.export_since(&live, &Frontier::default()).unwrap();

    // This device drafts the day and receives only the live page (batches are per round).
    let mut local = store();
    let draft = local.draft_daily(d).unwrap();
    assert_eq!(draft, gone);
    local.import_batch(&[(live.clone(), live_shard)]).unwrap();
    let block = local.page_state(&draft).unwrap().live_roots()[0].clone();
    local
        .set_block_text(&draft, &block, "typed here", None, None)
        .unwrap();
    local.sync_index().unwrap();

    // The trashed uuidv5(D) shard arrives in the next round: it stays deleted.
    local.import_batch(&[(gone.clone(), gone_shard)]).unwrap();
    let texts = live_texts(&mut local, &live);
    assert!(texts.iter().any(|(t, _)| t == "typed here"), "{texts:?}");
    assert!(
        !texts.iter().any(|(t, _)| t == "remote one"),
        "a trashed page's blocks came back on this device: {texts:?}"
    );
    // Imported tombstones do not create a local trash row, but the shard stays deleted.
    assert!(
        local.page_row(&gone).unwrap().is_some_and(|r| !r.is_live()),
        "the trashed page stays out of the index"
    );

    // And our shards must not bring them back on the other device either.
    push(&mut local, &mut remote);
    let texts = live_texts(&mut remote, &live);
    assert!(texts.iter().any(|(t, _)| t == "typed here"), "{texts:?}");
    assert!(
        !texts.iter().any(|(t, _)| t == "remote one"),
        "a trashed page's blocks came back on the other device: {texts:?}"
    );
    assert!(
        remote
            .list_trash()
            .unwrap()
            .iter()
            .any(|(id, ..)| *id == gone),
        "the trashed page stays trashed"
    );
}

/// Continuing issue 3 / P2: after a merged promotion the editor still targets the draft's
/// id. The id was never durable, so it is a session alias for the day (`Store::redirect`):
/// every later keystroke must reach the page that owns the date (rule 5).
#[test]
fn fr_1_2_typing_after_a_merged_promotion_reaches_the_day() {
    let d = date(2026, 11, 2);
    let mut remote = store();
    let gone = remote.ensure_daily_page(d).unwrap();
    remote.delete_page(&gone).unwrap();
    let live = remote.ensure_daily_page(d).unwrap();
    write(&mut remote, &live, "remote two");
    let shard = remote.export_since(&live, &Frontier::default()).unwrap();

    let mut s = store();
    let draft = s.draft_daily(d).unwrap();
    s.import_batch(&[(live.clone(), shard)]).unwrap();
    let block = s.page_state(&draft).unwrap().live_roots()[0].clone();
    s.set_block_text(&draft, &block, "t", None, None).unwrap();
    // The editor has not been told yet: same page id, same block id.
    assert_eq!(s.redirect(&draft), Some(live.clone()));
    s.set_block_text(&draft, &block, "ty", None, None).unwrap();
    s.sync_index().unwrap();
    let texts = live_texts(&mut s, &live);
    assert!(texts.iter().any(|(t, _)| t == "ty"), "{texts:?}");
    assert!(!s.is_draft(&draft));
}

/// P3 of the draft-day review: undo after a merged promotion must reach the day's shard.
/// The undo step used to keep the draft's id, whose shard no longer exists, so undo changed
/// only the in-memory survivor and the index kept the typed text (rule 2).
#[test]
fn fr_1_2_undo_after_a_merged_promotion_reaches_the_day() {
    let d = date(2026, 11, 2);
    let mut remote = store();
    let gone = remote.ensure_daily_page(d).unwrap();
    remote.delete_page(&gone).unwrap();
    let live = remote.ensure_daily_page(d).unwrap();
    write(&mut remote, &live, "remote two");
    let shard = remote.export_since(&live, &Frontier::default()).unwrap();

    let mut s = store();
    let draft = s.draft_daily(d).unwrap();
    s.import_batch(&[(live.clone(), shard)]).unwrap();
    let block = s.page_state(&draft).unwrap().live_roots()[0].clone();
    s.set_block_text(&draft, &block, "t", None, None).unwrap();
    s.undo().unwrap();
    s.sync_index().unwrap();
    let texts = live_texts(&mut s, &live);
    assert!(
        !texts.iter().any(|(t, _)| t == "t"),
        "undo reached the day in memory: {texts:?}"
    );
    let stored: String = s
        .conn()
        .query_row(
            "SELECT text FROM block WHERE id=?1",
            [block.as_str()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(stored, "", "undo reached the day's shard, not a dead id");
    assert_doctor(&mut s);
}

/// An id that is still a session alias must stay reserved. Once the merged promotion's
/// survivor is deleted, reusing the discarded id for a recreated day would shadow the new
/// draft behind the deleted page (`Store::redirect`, rule 4).
#[test]
fn fr_1_2_a_recreated_day_does_not_reuse_a_live_alias() {
    let d = date(2026, 11, 2);
    let mut remote = store();
    let gone = remote.ensure_daily_page(d).unwrap();
    remote.delete_page(&gone).unwrap();
    let live = remote.ensure_daily_page(d).unwrap();
    write(&mut remote, &live, "remote two");
    let shard = remote.export_since(&live, &Frontier::default()).unwrap();

    let mut s = store();
    let draft = s.draft_daily(d).unwrap();
    s.import_batch(&[(live.clone(), shard)]).unwrap();
    let block = s.page_state(&draft).unwrap().live_roots()[0].clone();
    s.set_block_text(&draft, &block, "t", None, None).unwrap();
    s.sync_index().unwrap();
    assert_eq!(
        s.redirect(&draft),
        Some(live.clone()),
        "the draft's id is an alias for the day"
    );

    // The survivor is deleted before the remote tombstone for the draft's id arrives.
    s.delete_page(&live).unwrap();
    // Revisiting the date must not reuse an id that redirects to the deleted page.
    let again = s.draft_daily(d).unwrap();
    assert_ne!(again, draft, "an id that is still an alias is reserved");
    let (deleted, live_count) = {
        let st = s.page_state(&again).unwrap();
        (st.meta.deleted_at.is_some(), st.live_count())
    };
    assert!(
        !deleted,
        "the recreated day is a usable draft, not a deleted page"
    );
    assert!(s.is_draft(&again));
    assert_eq!(live_count, 1);
    assert_doctor(&mut s);
}

#[test]
fn ac_1_5_deleted_daily_is_recreated_and_recoverable() {
    let mut s = store();
    let (old, b) = s.open_daily(date(2026, 11, 2)).unwrap();
    s.set_block_text(&old, &b, "kept", None, None).unwrap();
    s.delete_page(&old).unwrap();
    let (new, _) = s.open_daily(date(2026, 11, 2)).unwrap();
    assert_ne!(
        old, new,
        "a tombstoned deterministic id is never resurrected"
    );
    assert_eq!(s.list_trash().unwrap().len(), 1);
    // Restoring while the date is taken merges instead of duplicating.
    let survivor = s.restore_page(&old).unwrap();
    assert_eq!(survivor, new);
    let texts: Vec<String> = live_texts(&mut s, &new).into_iter().map(|t| t.0).collect();
    assert!(texts.contains(&"kept".to_string()));
    assert_doctor(&mut s);
}

/// Issue 1: the trash row was written outside the undo group, so Cmd+Z brought the page
/// back and left the row behind: a live page the trash still owned a row for. The row must
/// follow the tombstone it belongs to (docs/state-machines/page-lifecycle-trash.md).
#[test]
fn ac_1_5_undo_of_delete_clears_the_trash_row() {
    let mut s = store();
    let (page, block) = s.open_daily(date(2026, 11, 2)).unwrap();
    s.set_block_text(&page, &block, "kept", None, None).unwrap();
    s.delete_page(&page).unwrap();
    assert_eq!(s.list_trash().unwrap().len(), 1);
    assert_eq!(trash_rows(&s, &page), 1);

    assert!(s.can_undo(), "delete is one undo step");
    s.undo().unwrap();
    s.sync_index().unwrap();

    assert_eq!(deleted_at(&s, &page), None, "undo brings the page back");
    assert_eq!(
        trash_rows(&s, &page),
        0,
        "a live page has no trash row (the row is the tombstone's)"
    );
    assert!(s.list_trash().unwrap().is_empty());
    assert_doctor(&mut s);
}

/// Issue 2: the tombstone went through the writer and the trash row was autocommitted on
/// the read connection, so a failed write could tombstone a page that no trash row would
/// ever list or restore. The two must land in one transaction or not at all.
#[test]
fn ac_1_5_the_trash_row_lands_with_the_tombstone() {
    // An in-memory graph writes on the caller's thread, so a failed commit is reported by
    // the call that caused it.
    let mut s = Store::open_in_memory().unwrap();
    let page = s.create_page("Doomed").unwrap();
    write(&mut s, &page, "gone soon");
    // Any failure of the local bookkeeping, e.g. a disk error on the trash table.
    s.conn()
        .execute_batch("ALTER TABLE trash RENAME TO trash_off")
        .unwrap();
    assert!(
        s.delete_page(&page).is_err(),
        "a delete that cannot write its trash row must fail as a whole"
    );
    assert_eq!(
        deleted_at(&s, &page),
        None,
        "nothing was committed: the page is still live"
    );

    s.conn()
        .execute_batch("ALTER TABLE trash_off RENAME TO trash")
        .unwrap();
    // The repair rewrites the whole shard and still reports the failure once.
    assert!(s.sync_index().is_err());
    s.sync_index().unwrap();
    assert_eq!(
        deleted_at(&s, &page).is_some(),
        trash_rows(&s, &page) == 1,
        "the retry writes the tombstone and its row together"
    );
    assert_eq!(s.list_trash().unwrap().len(), 1);
    assert_doctor(&mut s);
}

/// Issue 3: Empty Trash deletes the page row and the shard, which used to be the only
/// record that the id had been tombstoned. The next `create_page` then reissued the same
/// uuidv5 (rule 4) and a peer that still held the tombstoned shard merged the new page's
/// ops into it, bringing `deleted_at` back.
#[test]
fn ac_1_5_empty_trash_never_reissues_a_purged_id() {
    let mut s = store();
    let id = s.create_page("Foo").unwrap();
    write(&mut s, &id, "gone soon");
    let shard = s.export_since(&id, &Frontier::default()).unwrap();
    s.delete_page(&id).unwrap();
    assert_eq!(s.empty_trash(None).unwrap(), 1);
    assert!(
        s.page_row(&id).unwrap().is_none(),
        "Empty Trash is a hard delete (rule 5)"
    );
    assert!(
        matches!(s.page_fate(&id).unwrap(), Some(PageFate::Purged { .. })),
        "only the local tombstone memory is left"
    );

    let again = s.create_page("Foo").unwrap();
    assert_ne!(again, id, "a purged id is never issued again (rule 4)");

    // A peer that still holds the tombstoned shard must not adopt the new page.
    s.import_batch(&[(id.clone(), shard)]).unwrap();
    assert_eq!(
        s.find_page("Foo").unwrap(),
        Some(again.clone()),
        "the re-created page stays live under its own id"
    );
    assert_eq!(
        trash_rows(&s, &id),
        0,
        "a tombstone from another device is not in this device's trash"
    );

    // The memory is local bookkeeping, not a projection: a rebuild keeps it.
    s.rebuild_index().unwrap();
    s.delete_page(&again).unwrap();
    s.empty_trash(None).unwrap();
    assert_ne!(
        s.create_page("Foo").unwrap(),
        again,
        "a rebuild does not forget a purged id"
    );
    assert_doctor(&mut s);
}

#[test]
fn ac_1_6_scheduled_for_this_day() {
    let mut s = store();
    let (nov2, _) = s.open_daily(date(2026, 11, 2)).unwrap();
    let blk = write(&mut s, &nov2, "Pay rent [[2026-12-01]]");
    let dec1 = s
        .find_page("2026-12-01")
        .unwrap()
        .expect("pre-created by reference (FR-1.10)");
    assert_eq!(s.page_row(&dec1).unwrap().unwrap().kind, PageKind::Daily);
    assert_eq!(
        s.page_state(&dec1).unwrap().live_count(),
        0,
        "created empty, unvisited"
    );
    let refs = s.linked_references(&dec1, &RefQuery::default()).unwrap();
    assert_eq!(refs.groups.len(), 1);
    assert_eq!(refs.groups[0].page.id, nov2, "navigates to the source page");
    assert_eq!(refs.groups[0].hits[0].block, blk);
    assert!(
        s.journal(14).unwrap().iter().all(|j| j.page.id != dec1),
        "future days not in journal"
    );
}

#[test]
fn ac_1_7_and_1_8_date_forms_and_format_change() {
    let mut s = store();
    s.set_date_format("yyyy/MM/dd").unwrap();
    let a = s.create_page("A").unwrap();
    let b = s.create_page("B").unwrap();
    let c = s.create_page("C").unwrap();
    write(&mut s, &a, "due [[2026-12-01]]");
    write(&mut s, &b, "due [[December 1st, 2026]]");
    write(&mut s, &c, "due [[2026/12/01]]");
    let day = s.find_page("2026-12-01").unwrap().unwrap();
    let dailies: Vec<_> = s
        .list_pages(true)
        .unwrap()
        .into_iter()
        .filter(|p| p.is_daily())
        .collect();
    assert_eq!(dailies.len(), 1, "one page, not three");
    assert_eq!(s.backlink_count(&day).unwrap(), 3);
    assert_eq!(s.page_row(&day).unwrap().unwrap().title, "2026/12/01");
    s.set_date_format("yyyy-MM-dd").unwrap();
    assert_eq!(s.page_row(&day).unwrap().unwrap().title, "2026-12-01");
    assert_eq!(
        s.find_page("2026-12-01").unwrap(),
        Some(day.clone()),
        "ids unchanged"
    );
    assert_eq!(s.backlink_count(&day).unwrap(), 3, "backlinks identical");
    assert_doctor(&mut s);
}

#[test]
fn ambiguous_numeric_date_is_a_plain_page() {
    let mut s = store();
    let p = s.create_page("Notes").unwrap();
    write(&mut s, &p, "see [[11/6/26]]");
    let target = s.find_page("11/6/26").unwrap().unwrap();
    assert_eq!(s.page_row(&target).unwrap().unwrap().kind, PageKind::Page);
}

#[test]
fn ac_2_3_two_hundred_undos_restore_projection() {
    let mut s = store();
    let (page, first) = s.open_daily(date(2026, 11, 2)).unwrap();
    s.seal_undo();
    let projection = |s: &mut Store| -> Vec<String> {
        let mut stmt = s
            .conn()
            .prepare(
                "SELECT id||'|'||ifnull(parent_id,'')||'|'||depth||'|'||text||'|'||block_type||'|'||ifnull(deleted_at,'')
                 FROM block ORDER BY id",
            )
            .unwrap();
        let rows = stmt.query_map([], |r| r.get::<_, String>(0)).unwrap();
        rows.map(Result::unwrap).collect()
    };
    let start = projection(&mut s);
    let mut focus = first.clone();
    let mut done = 0;
    let mut i = 0u64;
    while done < 200 {
        i += 1;
        let st = s.page_state(&page).unwrap().clone();
        let plan = match i % 6 {
            0 | 1 => outline::split_block(&st, &focus, st.block(&focus).unwrap().text.len(), false),
            2 => outline::indent(&st, &focus),
            3 => outline::outdent(&st, &focus),
            4 => outline::move_up(&st, &focus),
            _ => {
                let t = format!("{} w{i}", st.block(&focus).unwrap().text);
                s.set_block_text(&page, &focus, &t, None, None).unwrap();
                s.seal_undo();
                done += 1;
                continue;
            }
        };
        if let Ok(plan) = plan {
            if let Some(f) = run(&mut s, &page, plan) {
                focus = f;
            }
            done += 1;
        }
    }
    let end = projection(&mut s);
    for _ in 0..200 {
        s.undo().unwrap();
    }
    assert_eq!(projection(&mut s), start, "byte-identical after 200 undos");
    while s.can_redo() {
        s.redo().unwrap();
    }
    assert_eq!(projection(&mut s), end);
    assert_doctor(&mut s);
}

#[test]
fn ac_2_5_paste_five_hundred_lines_one_undo() {
    let mut s = store();
    let (page, b) = s.open_daily(date(2026, 11, 2)).unwrap();
    let md: String = (0..500)
        .map(|i| format!("{}- line {i}\n", "  ".repeat(i % 4)))
        .collect();
    let st = s.page_state(&page).unwrap();
    let plan = outline::paste(st, &b, 0..0, &md).unwrap();
    run(&mut s, &page, plan);
    let rows = live_texts(&mut s, &page);
    assert_eq!(rows.len(), 500);
    assert_eq!(rows[3], ("line 3".into(), 3));
    assert_eq!(rows[4], ("line 4".into(), 0));
    s.undo().unwrap();
    assert_eq!(live_texts(&mut s, &page), vec![(String::new(), 0)]);
}

#[test]
fn ac_2_7_hashtag_equals_link() {
    let mut s = store();
    let a = s.create_page("A").unwrap();
    let b = s.create_page("B").unwrap();
    write(&mut s, &a, "practice #mindfulness");
    write(&mut s, &b, "more [[mindfulness]]");
    let m: Vec<_> = s
        .list_pages(false)
        .unwrap()
        .into_iter()
        .filter(|p| p.title_key == "mindfulness")
        .collect();
    assert_eq!(m.len(), 1);
    let refs = s.linked_references(&m[0].id, &RefQuery::default()).unwrap();
    assert_eq!(refs.shown(), 2);
}

#[test]
fn ac_2_8_multi_word_tags() {
    let mut s = store();
    s.create_page("Modern Religion").unwrap();
    let p = s.create_page("Notes").unwrap();
    let b = write(&mut s, &p, "#Modern Religion is interesting");
    assert_eq!(text(&mut s, &p, &b), "#[[Modern Religion]] is interesting");
    let b2 = write(&mut s, &p, "#modern society");
    assert_eq!(
        text(&mut s, &p, &b2),
        "#modern society",
        "tag ends at the space"
    );
    assert!(s.find_page("modern").unwrap().is_some());
    assert!(s.find_page("modern society").unwrap().is_none());
}

#[test]
fn ac_2_10_deleted_page_becomes_unresolved_intent() {
    let mut s = store();
    let notes = s.create_page("Notes").unwrap();
    write(&mut s, &notes, "#[[Modern Religion]]");
    let mr = s.find_page("Modern Religion").unwrap().unwrap();
    s.delete_page(&mr).unwrap();
    let unresolved = s.unresolved_refs(&notes).unwrap();
    assert_eq!(unresolved.len(), 1);
    assert!(
        matches!(&unresolved[0].target, RefTarget::UnresolvedPage { key } if key == "modern religion")
    );
    let again = s.create_page("Modern Religion").unwrap();
    assert_ne!(again, mr);
    assert!(s.unresolved_refs(&notes).unwrap().is_empty());
    assert_eq!(s.backlink_count(&again).unwrap(), 1);
}

#[test]
fn ac_3_2_unclosed_link_creates_nothing() {
    let mut s = store();
    let p = s.create_page("P").unwrap();
    let b = write(&mut s, &p, "typing [[Alpha");
    assert!(s.find_page("Alpha").unwrap().is_none());
    s.set_block_text(&p, &b, "typing [[Alpha]]", None, None)
        .unwrap();
    s.commit_block(&p, &b).unwrap();
    let alpha = s.find_page("Alpha").unwrap().unwrap();
    assert_eq!(s.backlink_count(&alpha).unwrap(), 1);
}

#[test]
fn ac_3_3_rename_keeps_references_via_alias() {
    let mut s = store();
    let src = s.create_page("Log").unwrap();
    for i in 0..5 {
        write(&mut s, &src, &format!("note {i} about [[Alpha]]"));
    }
    let alpha = s.find_page("Alpha").unwrap().unwrap();
    assert_eq!(s.rename_page(&alpha, "Alpha Prime").unwrap(), alpha);
    assert_eq!(s.backlink_count(&alpha).unwrap(), 5);
    assert_eq!(s.find_page("alpha").unwrap(), Some(alpha.clone()));
    assert_eq!(s.aliases(&alpha).unwrap(), vec!["Alpha".to_string()]);
    assert_eq!(s.rewrite_references(&alpha).unwrap(), 5);
    let first = &live_texts(&mut s, &src)[0].0;
    assert_eq!(first, "note 0 about [[Alpha Prime]]");
    assert_eq!(s.backlink_count(&alpha).unwrap(), 5);
    s.undo().unwrap();
    assert_eq!(live_texts(&mut s, &src)[0].0, "note 0 about [[Alpha]]");
    assert_doctor(&mut s);
}

#[test]
fn ac_3_6_co_reference_filter() {
    let mut s = store();
    let src = s.create_page("Reading").unwrap();
    write(&mut s, &src, "[[Mindfulness]] alone");
    write(&mut s, &src, "[[Mindfulness]] and [[Books]]");
    let parent = write(&mut s, &src, "about [[Books]]");
    let st = s.page_state(&src).unwrap();
    let plan = outline::split_block(st, &parent, "about [[Books]]".len(), false).unwrap();
    let child = run(&mut s, &src, plan).unwrap();
    let st = s.page_state(&src).unwrap();
    let plan = outline::indent(st, &child).unwrap();
    run(&mut s, &src, plan);
    s.set_block_text(&src, &child, "child of books: [[Mindfulness]]", None, None)
        .unwrap();
    s.commit_block(&src, &child).unwrap();
    let m = s.find_page("Mindfulness").unwrap().unwrap();
    let books = s.find_page("Books").unwrap().unwrap();
    let all = s.linked_references(&m, &RefQuery::default()).unwrap();
    assert_eq!(all.shown(), 3);
    let q = RefQuery {
        also: vec![books],
        ..Default::default()
    };
    let filtered = s.linked_references(&m, &q).unwrap();
    assert_eq!(filtered.shown(), 2, "direct co-reference and via ancestor");
    assert_eq!(filtered.total, 3);
    let q = RefQuery {
        text: Some("alone".into()),
        ..Default::default()
    };
    assert_eq!(s.linked_references(&m, &q).unwrap().shown(), 1);
    let crumbs = &filtered.groups[0]
        .hits
        .iter()
        .find(|h| h.block == child)
        .unwrap()
        .breadcrumb;
    assert_eq!(crumbs.len(), 1);
    assert_eq!(crumbs[0].1, "about Books");
}

#[test]
fn ac_3_8_creating_a_page_resolves_pending_intents() {
    let mut s = store();
    let p = s.create_page("Inbox").unwrap();
    for i in 0..6 {
        type_only(&mut s, &p, &format!("{i} [[Zettelkasten]]"));
    }
    assert_eq!(s.unresolved_refs(&p).unwrap()[0].count, 6);
    let z = s.create_page("Zettelkasten").unwrap();
    assert_eq!(s.backlink_count(&z).unwrap(), 6);
    assert!(s.unresolved_refs(&p).unwrap().is_empty());
    assert_doctor(&mut s);
}

#[test]
fn fr_3_4_and_3_7_live_updates_and_self_references() {
    let mut s = store();
    let x = s.create_page("X").unwrap();
    let b = write(&mut s, &x, "I refer to [[X]] myself");
    assert_eq!(s.backlink_count(&x).unwrap(), 1, "self-references count");
    let st = s.page_state(&x).unwrap();
    let plan = outline::delete_subtree(st, &b, 1).unwrap();
    run(&mut s, &x, plan);
    assert_eq!(
        s.backlink_count(&x).unwrap(),
        0,
        "deleting the source removes its edge"
    );
    assert!(
        s.find_page("X").unwrap().is_some(),
        "no cleanup debt: page stays"
    );
}

#[test]
fn ac_4_2_edit_through_reference_and_undo() {
    let mut s = store();
    let src = s.create_page("Source").unwrap();
    let target = write(&mut s, &src, "a thesis");
    let other = s.create_page("Other").unwrap();
    write(&mut s, &other, &format!("see (({target}))"));
    // An embed edits the original block, on its own page.
    let page = s.block_page(&target).unwrap();
    assert_eq!(page, src);
    s.set_block_text(&page, &target, "a better thesis", None, None)
        .unwrap();
    assert!(matches!(
        s.block_ref_status(target.as_str()).unwrap(),
        BlockRefStatus::Live { ref text, .. } if text == "a better thesis"
    ));
    assert_eq!(s.block_backlinks(&target).unwrap().len(), 1);
    s.undo().unwrap();
    assert_eq!(text(&mut s, &src, &target), "a thesis");
}

#[test]
fn ac_4_5_deleted_and_missing_block_refs() {
    let mut s = store();
    let src = s.create_page("Source").unwrap();
    let target = write(&mut s, &src, "doomed");
    let other = s.create_page("Other").unwrap();
    for _ in 0..3 {
        write(&mut s, &other, &format!("ref (({target}))"));
    }
    write(&mut s, &other, "ghost ((01NOTAREALBLOCK00000000000))");
    let st = s.page_state(&src).unwrap();
    let plan = outline::delete_subtree(st, &target, 5).unwrap();
    run(&mut s, &src, plan);
    assert!(matches!(
        s.block_ref_status(target.as_str()).unwrap(),
        BlockRefStatus::Deleted { .. }
    ));
    assert_eq!(
        s.block_ref_status("01NOTAREALBLOCK00000000000").unwrap(),
        BlockRefStatus::Missing
    );
    let unresolved = s.unresolved_refs(&other).unwrap();
    assert_eq!(
        unresolved.len(),
        1,
        "only the never-existing uid is unresolved"
    );
}

#[test]
fn ac_4_6_and_4_7_merge_on_rename_and_undo() {
    let mut s = store();
    let a = s.create_page("Apple Watch").unwrap();
    for i in 0..3 {
        write(&mut s, &a, &format!("a{i}"));
    }
    let b = s.create_page("applewatch").unwrap();
    let mut b_ids = Vec::new();
    for i in 0..5 {
        b_ids.push(write(&mut s, &b, &format!("b{i}")));
    }
    // Give b0 a two-level subtree.
    for (depth, label) in [(1, "b0.1"), (2, "b0.1.1")] {
        let st = s.page_state(&b).unwrap();
        let anchor = st.live_preorder()[depth - 1].0.clone();
        let plan = outline::split_block(st, &anchor, st.block(&anchor).unwrap().text.len(), false)
            .unwrap();
        let n = run(&mut s, &b, plan).unwrap();
        s.set_block_text(&b, &n, label, None, None).unwrap();
        let st = s.page_state(&b).unwrap();
        if st.depth(&n) < depth {
            let plan = outline::indent(st, &n).unwrap();
            run(&mut s, &b, plan);
        }
    }
    assert_eq!(
        live_texts(&mut s, &b)[..3],
        [("b0".into(), 0), ("b0.1".into(), 1), ("b0.1.1".into(), 2)]
    );
    let refs = s.create_page("Refs").unwrap();
    for _ in 0..4 {
        write(&mut s, &refs, "[[applewatch]]");
    }
    let before_a = live_texts(&mut s, &a);
    let before_b = live_texts(&mut s, &b);
    let survivor = s.rename_page(&b, "apple watch").unwrap();
    assert_eq!(survivor, a);
    let merged = live_texts(&mut s, &a);
    let roots: Vec<_> = merged.iter().filter(|(_, d)| *d == 0).collect();
    assert_eq!(roots.len(), 8);
    assert_eq!(
        &merged[3..6],
        &[("b0".into(), 0), ("b0.1".into(), 1), ("b0.1.1".into(), 2)]
    );
    for id in &b_ids {
        assert_eq!(
            s.block_page(id).unwrap(),
            a,
            "ids preserved, moved not copied"
        );
    }
    assert!(s.page_row(&b).unwrap().unwrap().deleted_at.is_some());
    assert_eq!(s.backlink_count(&a).unwrap(), 4);
    assert_eq!(s.find_page("applewatch").unwrap(), Some(a.clone()));
    assert_doctor(&mut s);
    // One Cmd+Z reverses the whole merge.
    s.undo().unwrap();
    assert_eq!(live_texts(&mut s, &a), before_a);
    assert_eq!(live_texts(&mut s, &b), before_b);
    assert!(s.page_row(&b).unwrap().unwrap().deleted_at.is_none());
    assert_eq!(s.backlink_count(&b).unwrap(), 4);
    assert_doctor(&mut s);
}

#[test]
fn ac_4_9_unicode_equivalent_titles_merge() {
    let mut s = store();
    let a = s.create_page("Caf\u{e9}  Notes").unwrap();
    let b = s.create_page("Other").unwrap();
    assert_eq!(s.rename_page(&b, "Cafe\u{301} Notes").unwrap(), a);
}

#[test]
fn rename_to_date_merges_into_daily_page() {
    let mut s = store();
    let p = s.create_page("Meeting").unwrap();
    write(&mut s, &p, "agenda");
    let daily = s.rename_page(&p, "2026-11-06").unwrap();
    let row = s.page_row(&daily).unwrap().unwrap();
    assert_eq!(row.daily_date, Some(date(2026, 11, 6)));
    assert_eq!(live_texts(&mut s, &daily), vec![("agenda".into(), 0)]);
    assert_eq!(
        s.find_page("Meeting").unwrap(),
        Some(daily),
        "old title still resolves"
    );
    let d = s.find_page("2026-11-06").unwrap().unwrap();
    assert!(
        s.rename_page(&d, "x").is_err(),
        "daily titles are display-only"
    );
}

#[test]
fn w3_placeholder_pages_exist_without_content() {
    let mut s = store();
    let (today, _) = s.open_daily(date(2026, 11, 2)).unwrap();
    write(
        &mut s,
        &today,
        "Thinking about #[[Modern Religion]] and [[Dana]]",
    );
    let titles: Vec<String> = s
        .list_pages(false)
        .unwrap()
        .into_iter()
        .map(|p| p.title)
        .collect();
    assert_eq!(titles, vec!["Dana", "Modern Religion"]);
    assert_doctor(&mut s);
}

#[test]
fn quick_jump_and_block_search() {
    let mut s = store();
    s.create_page("Alpha Prime").unwrap();
    s.create_page("Reading log").unwrap();
    let p = s.create_page("Notes").unwrap();
    write(&mut s, &p, "the central thesis of everything");
    let hits = s.search_titles("alp", 5).unwrap();
    assert_eq!(hits[0].title, "Alpha Prime");
    let hits = s.search_titles("rdlg", 5).unwrap();
    assert_eq!(hits[0].title, "Reading log", "subsequence match");
    let blocks = s.search_blocks("thes", 5).unwrap();
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].page_title, "Notes");
}

#[test]
fn collapse_state_is_local() {
    let mut s = store();
    let (p, b) = s.open_daily(date(2026, 11, 2)).unwrap();
    s.set_collapsed(&b, true).unwrap();
    assert!(s.is_collapsed(&b));
    // Not part of the shard: the page's outbox payloads never mention it.
    let before = s.outbox_pending(100).unwrap().len();
    s.set_collapsed(&b, false).unwrap();
    assert_eq!(s.outbox_pending(100).unwrap().len(), before);
    let _ = p;
}

#[test]
fn export_markdown_and_json() {
    let mut s = store();
    let (today, b) = s.open_daily(date(2026, 11, 2)).unwrap();
    s.set_block_text(&today, &b, "Woke up thinking about [[Alpha]]", None, None)
        .unwrap();
    s.commit_block(&today, &b).unwrap();
    let st = s.page_state(&today).unwrap();
    let plan = outline::split_block(st, &b, st.block(&b).unwrap().text.len(), false).unwrap();
    let child = run(&mut s, &today, plan).unwrap();
    let st = s.page_state(&today).unwrap();
    let plan = outline::indent(st, &child).unwrap();
    run(&mut s, &today, plan);
    s.set_block_text(&today, &child, "nested", None, None)
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(s.export_markdown(dir.path()).unwrap(), 2);
    let md = std::fs::read_to_string(dir.path().join("2026-11-02.md")).unwrap();
    assert_eq!(
        md,
        "# November 2nd, 2026\n\n- Woke up thinking about [[Alpha]]\n  - nested\n"
    );
    let json = s.export_json_value().unwrap();
    let pages = json["pages"].as_array().unwrap();
    assert_eq!(pages.len(), 2);
}

fn push(from: &mut Store, to: &mut Store) {
    let batch: Vec<(PageId, Vec<u8>)> = from
        .all_page_ids()
        .unwrap()
        .into_iter()
        .map(|p| {
            let bytes = from.export_since(&p, &Frontier::default()).unwrap();
            (p, bytes)
        })
        .collect();
    to.import_batch(&batch).unwrap();
}

fn exchange(a: &mut Store, b: &mut Store) {
    push(a, b);
    push(b, a);
}

fn snapshot(s: &mut Store) -> Vec<(String, Vec<(String, usize)>)> {
    let pages = s.list_pages(true).unwrap();
    pages
        .into_iter()
        .map(|p| (p.title.clone(), live_texts(s, &p.id)))
        .collect()
}

#[test]
fn both_devices_create_same_title_offline_converge() {
    let mut a = store();
    let mut b = store();
    let pa = a.create_page("Alpha").unwrap();
    let pb = b.create_page("Alpha").unwrap();
    assert_eq!(pa, pb, "deterministic uuidv5");
    write(&mut a, &pa, "from a");
    write(&mut b, &pb, "from b");
    exchange(&mut a, &mut b);
    exchange(&mut a, &mut b);
    assert_eq!(snapshot(&mut a), snapshot(&mut b));
    assert_eq!(live_texts(&mut a, &pa).len(), 2);
    assert_doctor(&mut a);
    assert_doctor(&mut b);
}

#[test]
fn import_time_dedupe_merges_colliding_pages() {
    let mut a = store();
    let mut b = store();
    // Device A's "Beta" got a fresh id (its uuidv5 was tombstoned); B's did not.
    let old = a.create_page("Beta").unwrap();
    a.delete_page(&old).unwrap();
    let pa = a.create_page("Beta").unwrap();
    let pb = b.create_page("Beta").unwrap();
    assert_ne!(pa, pb);
    write(&mut a, &pa, "a's note");
    write(&mut b, &pb, "b's note");
    exchange(&mut a, &mut b);
    exchange(&mut a, &mut b);
    let live: Vec<_> = a
        .list_pages(false)
        .unwrap()
        .into_iter()
        .filter(|p| p.title == "Beta")
        .collect();
    assert_eq!(live.len(), 1);
    assert_eq!(snapshot(&mut a), snapshot(&mut b));
    let texts: Vec<String> = live_texts(&mut a, &live[0].id)
        .into_iter()
        .map(|t| t.0)
        .collect();
    assert!(texts.contains(&"a's note".to_string()) && texts.contains(&"b's note".to_string()));
    assert_doctor(&mut a);
    assert_doctor(&mut b);
}

#[test]
fn ac_4_8_offline_blocks_follow_merge() {
    let mut d1 = store();
    let mut d2 = store();
    let a = d1.create_page("Alpha").unwrap();
    write(&mut d1, &a, "a0");
    let bpage = d1.create_page("Bravo").unwrap();
    let b0 = write(&mut d1, &bpage, "b0");
    exchange(&mut d1, &mut d2);
    // Device 1 merges Bravo into Alpha while device 2, offline, adds three blocks.
    d1.merge_pages(&bpage, &a).unwrap();
    for i in 0..3 {
        write(&mut d2, &bpage, &format!("offline {i}"));
    }
    let st = d2.page_state(&bpage).unwrap();
    let plan = outline::split_block(st, &b0, 2, false).unwrap();
    let child = run(&mut d2, &bpage, plan).unwrap();
    let st = d2.page_state(&bpage).unwrap();
    let plan = outline::indent(st, &child).unwrap();
    run(&mut d2, &bpage, plan);
    d2.set_block_text(&bpage, &child, "child of b0", None, None)
        .unwrap();
    exchange(&mut d1, &mut d2);
    exchange(&mut d1, &mut d2);
    assert_eq!(snapshot(&mut d1), snapshot(&mut d2));
    let merged: Vec<String> = live_texts(&mut d1, &a).into_iter().map(|t| t.0).collect();
    for want in [
        "a0",
        "b0",
        "offline 0",
        "offline 1",
        "offline 2",
        "child of b0",
    ] {
        assert!(
            merged.contains(&want.to_string()),
            "{want} missing from {merged:?}"
        );
    }
    assert_eq!(merged.len(), 6, "no duplicates: {merged:?}");
    for s in [&mut d1, &mut d2] {
        assert!(s.page_row(&bpage).unwrap().unwrap().deleted_at.is_some());
        assert!(live_texts(s, &bpage).is_empty());
        assert_doctor(s);
    }
}

#[test]
fn rolling_backups_keep_six_generations() {
    let mut s = store();
    let (p, b) = s.open_daily(date(2026, 11, 2)).unwrap();
    s.set_block_text(&p, &b, "backed up", None, None).unwrap();
    let dir = tempfile::tempdir().unwrap();
    for i in 0..8 {
        s.set_clock(Some(1_000 + i));
        s.backup(dir.path(), 6).unwrap();
    }
    let files: Vec<_> = std::fs::read_dir(dir.path()).unwrap().collect();
    assert_eq!(files.len(), 6);
    // A backup is a complete, openable graph.
    let newest = dir.path().join("graph-1007.db");
    let mut restored = Store::open(&newest).unwrap();
    let st = restored.page_state(&p).unwrap();
    assert_eq!(st.block(&b).unwrap().text, "backed up");
}

#[test]
fn debounced_commits_reach_disk_on_flush() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("g.utopia");
    let page;
    {
        let mut s = Store::open(&path).unwrap();
        s.set_today(Some(date(2026, 11, 2)));
        s.set_debounced(true).unwrap();
        let (p, b) = s.open_daily(date(2026, 11, 2)).unwrap();
        for i in 0..50 {
            s.set_block_text(&p, &b, &format!("draft {i} [[Later]]"), None, None)
                .unwrap();
        }
        assert!(s.has_pending(), "keystrokes stay in memory until a flush");
        s.flush().unwrap();
        s.write_ticket()
            .wait()
            .expect("the debounced commit reaches disk");
        assert!(!s.has_pending());
        page = p;
        // Dropping the store flushes, stops the writer and compacts.
        s.set_block_text(&page, &b, "final [[Later]]", None, None)
            .unwrap();
    }
    let mut s = Store::open(&path).unwrap();
    let st = s.page_state(&page).unwrap();
    let text = st.block(&st.live_roots()[0]).unwrap().text.clone();
    assert_eq!(text, "final [[Later]]");
    assert!(s.doctor().unwrap().is_ok());
}

#[test]
fn reopening_mid_session_sees_writer_commits() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("g.utopia");
    let mut a = Store::open(&path).unwrap();
    let p = a.create_page("Shared").unwrap();
    write(&mut a, &p, "from the writer thread");
    a.sync_index().unwrap();
    // A second connection (e.g. `utopia doctor` while the app runs) sees the commit.
    let mut b = Store::open(&path).unwrap();
    assert_eq!(
        live_texts(&mut b, &p),
        vec![("from the writer thread".to_string(), 0)]
    );
}

/// SR-1: each hit carries the byte ranges of the matched terms in the plain text the
/// palette draws, so the view never has to scan for them (and never marks a bracket the
/// renderer hides). Prefixes match at a word start, as the FTS tokenizer does.
#[test]
fn sr_1_search_marks_the_matched_terms() {
    let mut s = store();
    let p = s.create_page("Notes").unwrap();
    write(
        &mut s,
        &p,
        "The **zebra** crossing and [[Zebra Crossing]] again",
    );
    write(&mut s, &p, "bathes in the sun");
    let hits = s.search_blocks("zeb", 5).unwrap();
    assert_eq!(hits.len(), 1);
    let plain = utopia_core::inline::plain_text(&hits[0].text);
    let marked: Vec<&str> = hits[0].anchors.iter().map(|&(a, b)| &plain[a..b]).collect();
    assert_eq!(marked, vec!["zebra", "Zebra"], "{plain:?}");
    // The ranges are the ones gpui's StyledText requires: sorted and non-overlapping.
    assert!(hits[0].anchors.windows(2).all(|w| w[0].1 <= w[1].0));
    assert!(
        s.search_blocks("thes", 5).unwrap().is_empty(),
        "a match inside a word is not the index's match either"
    );
    // Two terms merge into one painted run rather than a nested highlight.
    let both = utopia_store::match_anchors("zebra crossing", "zeb cross");
    assert_eq!(both, vec![(0, 5), (6, 14)], "the whole word is washed");
}

/// SR-2: results sit under a header per page, with that page's blocks in one group and
/// the page's title/daily flag from the index.
#[test]
fn sr_2_search_groups_results_by_page() {
    let mut s = store();
    let a = s.create_page("Atlas").unwrap();
    write(&mut s, &a, "atlas launch plan");
    write(&mut s, &a, "atlas risks");
    let b = s.create_page("Notes").unwrap();
    write(&mut s, &b, "the atlas is late");
    let groups = s.search_groups("atlas", 30).unwrap();
    assert_eq!(groups.len(), 2, "{groups:?}");
    assert!(!groups[0].daily && !groups[1].daily);
    let atlas = groups.iter().find(|g| g.title == "Atlas").unwrap();
    assert_eq!(atlas.page, a);
    assert_eq!(atlas.hits.len(), 2);
    assert!(atlas.hits.iter().all(|h| h.page == a));
    let notes = groups.iter().find(|g| g.title == "Notes").unwrap();
    assert_eq!(notes.page, b);
    assert_eq!(notes.hits.len(), 1);
}

/// SR-3: the scope chips narrow to daily pages, ordinary pages or to-dos, and the age
/// menu drops hits older than the window. A daily hit is dated by its day, other blocks
/// by the local date of their last edit.
#[test]
fn sr_3_search_scopes_and_age() {
    use utopia_core::BlockKind;
    use utopia_store::{SearchAge, SearchScope};

    // Seed at local noon so "the block was edited today" holds in every timezone.
    let ts = |y, m, d, h| {
        date(y, m, d)
            .and_hms_opt(h, 0, 0)
            .unwrap()
            .and_local_timezone(chrono::Local)
            .single()
            .unwrap()
            .timestamp_millis()
    };
    let mut s = store();
    s.set_clock(Some(ts(2026, 11, 2, 12)));
    let atlas = s.create_page("Atlas").unwrap();
    write(&mut s, &atlas, "atlas launch");
    let (day, db) = s.open_daily(date(2026, 11, 2)).unwrap();
    s.set_block_text(&day, &db, "atlas daily note", None, None)
        .unwrap();
    s.commit_block(&day, &db).unwrap();
    let work = s.create_page("Work").unwrap();
    let todo = write(&mut s, &work, "atlas todo");
    s.set_block_kind(&work, &todo, BlockKind::Todo, false)
        .unwrap();

    // An older block, written before the window.
    s.set_clock(Some(ts(2026, 10, 1, 12)));
    let old = s.create_page("Old").unwrap();
    write(&mut s, &old, "atlas old");
    s.set_clock(Some(ts(2026, 11, 2, 12)));
    s.sync_index().unwrap();

    let pages = |s: &Store, scope, age| -> Vec<String> {
        let mut v: Vec<String> = s
            .search_blocks_filtered("atlas", scope, age, 50)
            .unwrap()
            .into_iter()
            .map(|h| h.page_title)
            .collect();
        v.sort();
        v
    };
    assert_eq!(
        pages(&s, SearchScope::All, SearchAge::AnyTime),
        vec!["Atlas", "November 2nd, 2026", "Old", "Work"]
    );
    assert_eq!(
        pages(&s, SearchScope::Journal, SearchAge::AnyTime),
        vec!["November 2nd, 2026"]
    );
    assert_eq!(
        pages(&s, SearchScope::Pages, SearchAge::Today),
        vec!["Atlas", "Work"],
        "the October note is outside Today"
    );
    assert_eq!(
        pages(&s, SearchScope::Todos, SearchAge::AnyTime),
        vec!["Work"]
    );
    let daily = s
        .search_groups_filtered("atlas", SearchScope::Journal, SearchAge::Today, 30)
        .unwrap();
    assert_eq!(daily.len(), 1);
    assert!(daily[0].daily && daily[0].hits.len() == 1);
    assert_eq!(
        s.search_blocks_filtered("atlas", SearchScope::All, SearchAge::Year, 50)
            .unwrap()
            .len(),
        4
    );
}

/// SR-4: the preview of a search hit carries its path from the root, its state and its
/// first children (with the total count), read from the index rather than at paint time.
#[test]
fn sr_4_preview_has_path_children_and_state() {
    use utopia_core::BlockKind;

    let mut s = store();
    let p = s.create_page("Atlas").unwrap();
    let parent = write(&mut s, &p, "Risks");
    let plan = outline::append_block(s.page_state(&p).unwrap(), Some(&parent));
    let hit = run(&mut s, &p, plan).unwrap();
    s.set_block_text(&p, &hit, "zebra migration is late", None, None)
        .unwrap();
    s.set_block_kind(&p, &hit, BlockKind::Todo, true).unwrap();
    // A couple of children under the hit, one more than the preview lists.
    for i in 0..(utopia_store::PREVIEW_CHILDREN + 1) {
        let plan = outline::append_block(s.page_state(&p).unwrap(), Some(&hit));
        let c = run(&mut s, &p, plan).unwrap();
        s.set_block_text(&p, &c, &format!("child {i}"), None, None)
            .unwrap();
    }
    s.sync_index().unwrap();
    let preview = s.preview_block(&hit).unwrap().expect("a live block");
    assert_eq!(preview.page, p);
    assert_eq!(preview.page_title, "Atlas");
    assert!(!preview.daily);
    assert_eq!(preview.path.len(), 1, "the parent above the hit");
    assert_eq!(preview.path[0].1, "Risks");
    assert_eq!((preview.kind, preview.checked), (BlockKind::Todo, true));
    assert_eq!(
        preview.child_count,
        utopia_store::PREVIEW_CHILDREN + 1,
        "the count is the whole list"
    );
    assert_eq!(
        preview.children.len(),
        utopia_store::PREVIEW_CHILDREN,
        "the pane lists the first few"
    );
    assert_eq!(preview.children[0].text, "child 0");
    // No panic on user data: a missing block is `None`, never an error.
    assert!(s.preview_block(&BlockId("nope".into())).unwrap().is_none());
}

/// SR-3: the age window is part of the query. A pile of older matches must not starve the
/// candidate list: 305 October blocks used to fill the 10x over-fetch before one Today
/// block was ever looked at, so Today came back empty.
#[test]
fn sr_3_age_window_is_applied_before_the_limit() {
    use utopia_store::{SearchAge, SearchScope};

    let ts = |m, d, h| {
        date(2026, m, d)
            .and_hms_opt(h, 0, 0)
            .unwrap()
            .and_local_timezone(chrono::Local)
            .single()
            .unwrap()
            .timestamp_millis()
    };
    let mut s = store();
    s.set_clock(Some(ts(11, 2, 12)));
    let (day, b) = s.open_daily(date(2026, 11, 2)).unwrap();
    s.set_block_text(&day, &b, "zebra newest", None, None)
        .unwrap();
    s.commit_block(&day, &b).unwrap();

    s.set_clock(Some(ts(10, 1, 12)));
    let old = s.create_page("Archive").unwrap();
    for i in 0..305 {
        write(&mut s, &old, &format!("zebra old {i}"));
    }
    s.set_clock(Some(ts(11, 2, 12)));
    s.sync_index().unwrap();

    let hits = s
        .search_blocks_filtered("zebra", SearchScope::All, SearchAge::Today, 30)
        .unwrap();
    assert!(
        hits.iter().any(|h| h.page == day),
        "Today must still find the day's block (got {} hits)",
        hits.len()
    );
}

/// SR-3: Today and the past windows end today. A future daily page (reachable through
/// day navigation) must not pass Today or Past week/month/year; Any time still has it.
#[test]
fn sr_3_age_window_excludes_future_days() {
    use utopia_store::{SearchAge, SearchScope};

    let mut s = store();
    let (today, b) = s.open_daily(date(2026, 11, 2)).unwrap();
    s.set_block_text(&today, &b, "zebra today", None, None)
        .unwrap();
    s.commit_block(&today, &b).unwrap();
    let (tomorrow, b) = s.open_daily(date(2026, 11, 3)).unwrap();
    s.set_block_text(&tomorrow, &b, "zebra tomorrow", None, None)
        .unwrap();
    s.commit_block(&tomorrow, &b).unwrap();
    s.sync_index().unwrap();

    let days = |age| -> Vec<Option<NaiveDate>> {
        let mut d: Vec<_> = s
            .search_blocks_filtered("zebra", SearchScope::All, age, 30)
            .unwrap()
            .into_iter()
            .map(|h| s.page_row(&h.page).unwrap().and_then(|p| p.daily_date))
            .collect();
        d.sort();
        d
    };
    assert_eq!(
        days(SearchAge::Today),
        vec![Some(date(2026, 11, 2))],
        "tomorrow is not Today"
    );
    assert_eq!(
        days(SearchAge::Week),
        vec![Some(date(2026, 11, 2))],
        "nor in the past week"
    );
    assert_eq!(
        days(SearchAge::AnyTime),
        vec![Some(date(2026, 11, 2)), Some(date(2026, 11, 3))],
        "Any time is unrestricted"
    );
}

/// SB-1: the sidebar's muted count says how much was written on a day, so an empty day
/// (kept only for a link into it) counts zero and a blank row never counts.
#[test]
fn sb_1_journal_counts_written_blocks() {
    let mut s = store();
    let (p, first) = s.open_daily(date(2026, 11, 2)).unwrap();
    s.set_block_text(&p, &first, "one", None, None).unwrap();
    write(&mut s, &p, "two");
    let blank = write(&mut s, &p, "");
    assert!(
        s.page_state(&p).unwrap().block(&blank).unwrap().live,
        "the blank block exists but is not written on"
    );
    assert_eq!(s.page_block_count(&p).unwrap(), 2);
    let entry = s.journal(14).unwrap().remove(0);
    assert_eq!(entry.page.id, p);
    assert_eq!(entry.blocks, 2);
    assert_eq!(
        entry.preview.len(),
        2,
        "the preview still has one line per block"
    );
}

/// SB-2: a page with no blocks of its own that only hashtags point at is a tag page;
/// `[[link]]` references (and a page with a block of its own) are ordinary pages.
#[test]
fn sb_2_hashtag_only_pages_are_marked() {
    let mut s = store();
    let (day, b) = s.open_daily(date(2026, 11, 2)).unwrap();
    s.set_block_text(
        &day,
        &b,
        "#atlas and [[Atlas Launch]] and #[[deep work]]",
        None,
        None,
    )
    .unwrap();
    s.commit_block(&day, &b).unwrap();
    s.sync_index().unwrap();
    let tags = s.hashtag_only_pages().unwrap();
    let atlas = s.find_page("atlas").unwrap().expect("the tag made a page");
    let deep = s.find_page("deep work").unwrap().expect("a multi-word tag");
    let linked = s.find_page("Atlas Launch").unwrap().unwrap();
    assert!(tags.contains(&atlas), "{tags:?}");
    assert!(tags.contains(&deep), "{tags:?}");
    assert!(!tags.contains(&linked), "a [[link]] page is not a tag page");
    // Writing on a tag page makes it an ordinary page; the list is display-only.
    write(&mut s, &atlas, "notes about the atlas");
    assert!(!s.hashtag_only_pages().unwrap().contains(&atlas));
    assert_doctor(&mut s);
}
