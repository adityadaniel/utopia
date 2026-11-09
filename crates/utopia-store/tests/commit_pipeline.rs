//! Commit pipeline acceptance scripts (§13.3, AGENTS.md rule 2): a writer failure is a
//! state the store reports and repairs, never a silent frontier advance or lost keystroke.
//!
//! Failures are injected through SQLite itself (an `outbox` trigger that aborts the
//! commit) rather than a test-only hook, so the writer path runs unchanged.

use chrono::NaiveDate;
use utopia_core::{BlockId, PageId};
use utopia_store::Store;

fn date(y: i32, m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, d).unwrap()
}

fn fail_commits(store: &Store) {
    store
        .conn()
        .execute_batch(
            "CREATE TRIGGER fail_commit BEFORE INSERT ON outbox
             BEGIN SELECT RAISE(ABORT, 'injected commit failure'); END",
        )
        .unwrap();
}

/// Fail the commit before its outbox row: block projections are written for every edit,
/// including the imported ones that carry no outbox payload.
fn fail_block_writes(store: &Store) {
    store
        .conn()
        .execute_batch(
            "CREATE TRIGGER fail_commit BEFORE INSERT ON block
             BEGIN SELECT RAISE(ABORT, 'injected commit failure'); END",
        )
        .unwrap();
}

fn allow_commits(store: &Store) {
    store
        .conn()
        .execute_batch("DROP TRIGGER fail_commit")
        .unwrap();
}

fn text(store: &mut Store, page: &PageId, block: &BlockId) -> String {
    store
        .page_state(page)
        .unwrap()
        .block(block)
        .unwrap()
        .text
        .clone()
}

fn count(store: &Store, sql: &str, page: &PageId) -> i64 {
    store
        .conn()
        .query_row(sql, [page.as_str()], |r| r.get(0))
        .unwrap()
}

/// A durable page with one block, a writer that fails on the next commit, and a text edit
/// that reaches the writer before the failure is noticed. Returns `(path, page, block)`
/// with the store left alive and failed.
fn failed_commit(dir: &tempfile::TempDir) -> (std::path::PathBuf, Store, PageId, BlockId) {
    let path = dir.path().join("g.utopia");
    let mut s = Store::open(&path).unwrap();
    s.set_today(Some(date(2026, 11, 2)));
    let (page, block) = s.open_daily(date(2026, 11, 2)).unwrap();
    s.set_block_text(&page, &block, "a", None, None).unwrap();
    s.sync_index().unwrap();
    s.set_debounced(true).unwrap();
    fail_commits(&s);
    s.set_block_text(&page, &block, "ab", None, None).unwrap();
    s.flush().unwrap();
    // The UI's background task resolves on the error; it must not report success.
    let _ = s.write_ticket().wait();
    (path, s, page, block)
}

/// Issue 1: `flush` advanced the shard's persisted frontier before the writer ran, so a
/// failed job left the next update based on a version the database never received.
#[test]
fn ac_13_3_a_failed_commit_does_not_advance_the_persisted_frontier() {
    let dir = tempfile::tempdir().unwrap();
    let (path, mut s, page, block) = failed_commit(&dir);
    allow_commits(&s);

    // A plain text edit after the failure: its update must not be based on the missing
    // snapshot. The store keeps the shard (rule 5) and reports the failure instead.
    s.set_block_text(&page, &block, "abc", None, None).unwrap();
    let flushed = s.flush();
    assert!(
        flushed.is_err() || s.sync_index().is_err(),
        "a flush over a failed writer must not succeed"
    );
    assert!(
        s.page_state(&page).is_ok(),
        "a failed commit keeps the loaded shard"
    );
    assert_eq!(text(&mut s, &page, &block), "abc");

    let _ = s.sync_index();
    s.sync_index().unwrap();
    assert!(s.doctor().unwrap().is_ok());
    drop(s);

    // The page still opens: `page_doc` covers every character that was typed.
    let mut s = Store::open(&path).unwrap();
    assert_eq!(text(&mut s, &page, &block), "abc");
    assert!(s.doctor().unwrap().is_ok());
}

/// Issue 8: the writer kept going after a failed job, so a later job for a page whose
/// first snapshot never landed inserted an orphan `page_update` row.
#[test]
fn ac_13_3_the_writer_stops_after_a_failed_job() {
    let dir = tempfile::tempdir().unwrap();
    let (path, mut s, page, block) = failed_commit(&dir);
    let before = count(
        &s,
        "SELECT count(*) FROM page_update WHERE page_id=?1",
        &page,
    );
    allow_commits(&s);

    s.set_block_text(&page, &block, "abc", None, None).unwrap();
    let flushed = s.flush();
    assert!(
        flushed.is_err(),
        "a failed writer refuses more jobs until it is repaired"
    );
    let _ = s.write_ticket().wait();

    let shards = count(&s, "SELECT count(*) FROM page_doc WHERE page_id=?1", &page);
    let updates = count(
        &s,
        "SELECT count(*) FROM page_update WHERE page_id=?1",
        &page,
    );
    assert_eq!(
        updates, before,
        "no update row without the snapshot it builds on"
    );
    assert_eq!(shards, 1, "the durable snapshot is still there");

    let _ = s.sync_index();
    s.sync_index().unwrap();
    assert_eq!(text(&mut s, &page, &block), "abc");
    assert!(s.doctor().unwrap().is_ok());
    drop(s);

    let mut s = Store::open(&path).unwrap();
    assert_eq!(text(&mut s, &page, &block), "abc");
    assert!(s.doctor().unwrap().is_ok());
}

/// Issue 2: the failure was taken by whoever called `wait` next, so a second barrier saw a
/// healthy store while the disk still lagged the in-memory shards.
#[test]
fn ac_13_3_a_writer_failure_stays_until_repaired() {
    let dir = tempfile::tempdir().unwrap();
    let (_path, mut s, page, block) = failed_commit(&dir);

    assert!(s.sync_index().is_err(), "the failed commit is reported");
    assert!(
        s.sync_index().is_err(),
        "a failed writer is a state, not a one-shot value"
    );
    // The shard was retained rather than dropped, so the repair can write every character.
    assert!(s.page_state(&page).is_ok());

    allow_commits(&s);
    let _ = s.sync_index();
    s.sync_index().unwrap();
    assert_eq!(text(&mut s, &page, &block), "ab");
}

/// Issue 3: repairing a failure cleared `drafts`, while the open `PageView` kept the draft
/// page id; the next `page_state` call then reported `NotFound`.
#[test]
fn ac_13_3_a_failed_commit_keeps_draft_pages() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("g.utopia");
    let mut s = Store::open(&path).unwrap();
    s.set_today(Some(date(2026, 11, 2)));
    s.set_debounced(true).unwrap();
    let page = s.draft_daily(date(2026, 11, 2)).unwrap();
    let block = s.page_state(&page).unwrap().live_roots()[0].clone();

    fail_commits(&s);
    // The first edit promotes the draft and its whole snapshot; that commit fails.
    s.set_block_text(&page, &block, "draft text", None, None)
        .unwrap();
    let _ = s.write_ticket().wait();
    assert!(s.sync_index().is_err());

    // The daily feed still holds this id: it must keep loading (rule 5).
    assert!(
        s.page_state(&page).is_ok(),
        "the draft page survives a failed commit"
    );
    assert_eq!(text(&mut s, &page, &block), "draft text");

    allow_commits(&s);
    let _ = s.sync_index();
    s.sync_index().unwrap();
    assert_eq!(text(&mut s, &page, &block), "draft text");
    assert!(s.doctor().unwrap().is_ok());
    drop(s);

    let mut s = Store::open(&path).unwrap();
    assert_eq!(text(&mut s, &page, &block), "draft text");
    assert!(s.doctor().unwrap().is_ok());
}

/// Issue 9: `flush` took `pending` before doing fallible work, so a failed commit stranded
/// the queue; `has_pending` then reported false and the UI stopped scheduling flushes.
#[test]
fn ac_13_3_a_failed_inline_commit_keeps_its_pending_edit() {
    let mut s = Store::open_in_memory().unwrap();
    s.set_today(Some(date(2026, 11, 2)));
    let (page, block) = s.open_daily(date(2026, 11, 2)).unwrap();
    s.set_block_text(&page, &block, "a", None, None).unwrap();
    s.sync_index().unwrap();

    fail_commits(&s);
    let r = s.set_block_text(&page, &block, "ab", None, None);
    assert!(r.is_err(), "the inline commit fails at once");
    assert!(
        s.has_pending(),
        "the edit stays queued instead of being stranded"
    );
    assert_eq!(text(&mut s, &page, &block), "ab");

    allow_commits(&s);
    let _ = s.sync_index();
    s.sync_index().unwrap();
    assert_eq!(text(&mut s, &page, &block), "ab");
}

/// Issue 10: a strict-mode index read through `conn()` took the writer error and dropped
/// it, so the next `sync_index` saw a healthy store while the disk lagged.
#[test]
fn ac_13_3_a_strict_index_read_does_not_consume_the_writer_failure() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("g.utopia");
    let mut s = Store::open(&path).unwrap();
    s.set_today(Some(date(2026, 11, 2)));
    let (page, block) = s.open_daily(date(2026, 11, 2)).unwrap();
    s.set_block_text(&page, &block, "a", None, None).unwrap();
    s.sync_index().unwrap();

    fail_commits(&s);
    s.set_block_text(&page, &block, "ab", None, None).unwrap();
    // Strict-mode index reads go through `conn()`; it cannot return the error, but the
    // failure must remain the store's state for the next barrier to report.
    let _ = s.page_exists(&page);
    let _ = s.conn();
    assert!(
        s.sync_index().is_err(),
        "the read must not consume the failed commit"
    );
}

/// Review P1: a visit must not write a day (FR-1.2). An unedited draft is in `docs` only
/// so the feed can render it; a repair must not turn it into a page.
#[test]
fn fr_1_2_a_writer_failure_does_not_promote_unedited_drafts() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("g.utopia");
    let mut s = Store::open(&path).unwrap();
    s.set_today(Some(date(2026, 11, 2)));
    s.set_debounced(true).unwrap();
    // Both days were visited (their drafts are in memory); only one is edited.
    let untouched = s.draft_daily(date(2026, 11, 2)).unwrap();
    let edited = s.draft_daily(date(2026, 11, 1)).unwrap();
    let block = s.page_state(&edited).unwrap().live_roots()[0].clone();

    fail_commits(&s);
    s.set_block_text(&edited, &block, "typed", None, None)
        .unwrap();
    let _ = s.write_ticket().wait();
    allow_commits(&s);
    let _ = s.sync_index();
    s.sync_index().unwrap();

    assert_eq!(
        count(
            &s,
            "SELECT count(*) FROM page_doc WHERE page_id=?1",
            &edited
        ),
        1,
        "the edited draft became a page"
    );
    assert_eq!(
        count(
            &s,
            "SELECT count(*) FROM page_doc WHERE page_id=?1",
            &untouched
        ),
        0,
        "an unedited draft must not be written by a repair"
    );
    assert_eq!(
        count(&s, "SELECT count(*) FROM page WHERE id=?1", &untouched),
        0
    );
    assert!(s.is_draft(&untouched), "the day is still only visited");
    assert!(s.page_state(&untouched).is_ok(), "the draft stays loaded");
}

/// Review P2: `import_batch` queues a remote page with `enqueue = false` so another
/// device's ops are not echoed to the relay. A repair must keep that flag.
#[test]
fn ac_13_3_repair_does_not_echo_remote_updates() {
    use utopia_core::BlockKind;
    use utopia_core::Command;
    use utopia_doc::Frontier;
    use utopia_store::{EditOpts, Store as _Store};

    let dir_a = tempfile::tempdir().unwrap();
    let dir_b = tempfile::tempdir().unwrap();
    let mut a = Store::open(dir_a.path().join("g.utopia")).unwrap();
    a.set_today(Some(date(2026, 11, 2)));
    let page = a.create_page("Shared").unwrap();
    let block = BlockId::new();
    a.execute(
        &page,
        vec![Command::InsertBlock {
            id: block.clone(),
            parent: None,
            after: None,
            text: "one".into(),
            kind: BlockKind::Bullet,
        }],
        EditOpts::default(),
    )
    .unwrap();
    a.sync_index().unwrap();

    // B receives the page from the relay, then a later remote edit.
    let full = a.export_since(&page, &Frontier::default()).unwrap();
    let mut b = _Store::open(dir_b.path().join("g.utopia")).unwrap();
    b.set_today(Some(date(2026, 11, 2)));
    b.import(&page, &full).unwrap();
    b.sync_index().unwrap();
    assert!(b.outbox_pending(100).unwrap().is_empty());

    let base = a.frontier(&page).unwrap();
    a.set_block_text(&page, &block, "two", None, None).unwrap();
    a.sync_index().unwrap();
    let update = a.export_since(&page, &base).unwrap();

    fail_block_writes(&b);
    let _ = b.import(&page, &update);
    allow_commits(&b);
    let _ = b.sync_index();
    b.sync_index().unwrap();

    assert_eq!(
        text(&mut b, &page, &block),
        "two",
        "the remote edit is applied"
    );
    let echoed = b
        .outbox_pending(100)
        .unwrap()
        .into_iter()
        .filter(|(_, p, _, _)| p == &page)
        .count();
    assert_eq!(echoed, 0, "a repair must not enqueue imported ops");
}

/// SB-2 with issue 2 of this machine: a failed commit publishes nothing, because the set it
/// would publish describes an index the failed job never reached; the repair that rewrites
/// the shards then publishes the set that index implies.
#[test]
fn sb_2_a_failed_commit_keeps_the_set_and_the_repair_republishes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("g.utopia");
    let mut s = Store::open(&path).unwrap();
    s.set_today(Some(date(2026, 11, 2)));
    let notes = s.create_page("Notes").unwrap();
    let block = s.ensure_first_block(&notes).unwrap();
    s.set_block_text(&notes, &block, "about #alpha", None, None)
        .unwrap();
    s.commit_block(&notes, &block).unwrap();
    s.sync_index().unwrap();
    let alpha = s
        .find_page("alpha")
        .unwrap()
        .expect("the tag made the page");
    assert!(s.tag_pages().unwrap().contains(&alpha));

    // The edit turns the hashtag into a plain link, so the page stops being a tag page.
    fail_commits(&s);
    s.set_block_text(&notes, &block, "about [[alpha]]", None, None)
        .unwrap();
    let _ = s.flush();
    assert!(s.write_ticket().wait().is_err(), "the commit failed");
    assert!(
        s.tag_pages().unwrap().contains(&alpha),
        "a failed commit leaves the set that describes the index on disk: {:?}",
        s.tag_pages()
    );

    allow_commits(&s);
    // The repair reports the failure it recovered from, then answers normally.
    let _ = s.sync_index();
    s.sync_index().unwrap();
    assert!(
        !s.tag_pages().unwrap().contains(&alpha),
        "the repair republishes: {:?}",
        s.tag_pages()
    );
    assert_eq!(s.tag_pages().unwrap(), s.hashtag_only_pages().unwrap());
}
