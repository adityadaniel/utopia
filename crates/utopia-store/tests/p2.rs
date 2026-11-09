//! P2 (v1.2+) acceptance scripts: query blocks and queryable attributes.

use chrono::NaiveDate;
use utopia_core::outline;
use utopia_core::query::parse;
use utopia_core::{BlockId, BlockKind, PageId};
use utopia_store::Store;

fn today() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 11, 2).unwrap()
}

fn store() -> (Store, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Store::open(dir.path().join("g.utopia")).unwrap();
    s.set_today(Some(today()));
    (s, dir)
}

fn write_under(s: &mut Store, page: &PageId, parent: Option<&BlockId>, text: &str) -> BlockId {
    let plan = outline::append_block(s.page_state(page).unwrap(), parent);
    let id = s.execute_plan(page, plan, None).unwrap().unwrap().block;
    s.set_block_text(page, &id, text, None, None).unwrap();
    s.commit_block(page, &id).unwrap();
    id
}

fn write(s: &mut Store, page: &PageId, text: &str) -> BlockId {
    write_under(s, page, None, text)
}

/// When the row was last confirmed missing; the lost-row reconciliation rotates on it.
fn lost_at(s: &Store, hash: &str) -> i64 {
    s.conn()
        .query_row(
            "SELECT lost_at FROM attachment WHERE hash = ?1",
            [hash],
            |r| r.get(0),
        )
        .unwrap()
}

fn hits(s: &Store, q: &str) -> Vec<String> {
    s.run_query(&parse(q).unwrap(), 100)
        .unwrap()
        .hits
        .into_iter()
        .map(|h| h.text)
        .collect()
}

#[test]
fn query_terms_combine_and_inherit() {
    let (mut s, _d) = store();
    let p = s.create_page("Reading").unwrap();
    let books = write(&mut s, &p, "[[Books]] to read");
    write_under(&mut s, &p, Some(&books), "Thinking in Systems #toread");
    write_under(&mut s, &p, Some(&books), "Antifragile");
    write(&mut s, &p, "a #toread article");
    write(&mut s, &p, "{{query: {and: [[Books]] #toread}}}");
    // The child inherits [[Books]] from its parent.
    assert_eq!(
        hits(&s, "{and: [[Books]] #toread}"),
        vec!["Thinking in Systems #toread"]
    );
    // A parent and its matching children list once, as the parent.
    assert_eq!(hits(&s, "[[Books]]"), vec!["[[Books]] to read"]);
    assert_eq!(
        hits(&s, "{or: [[Books]] #toread}"),
        vec!["[[Books]] to read", "a #toread article"],
        "query blocks never match themselves"
    );
    assert_eq!(
        hits(&s, "{and: #toread {not: [[Books]]}}"),
        vec!["a #toread article"]
    );
    assert_eq!(hits(&s, "\"antifragile\""), vec!["Antifragile"]);
    // A page term matches the blocks on that page.
    let b = s.find_page("Books").unwrap().unwrap();
    write(&mut s, &b, "on the Books page");
    assert!(hits(&s, "[[Books]]").contains(&"on the Books page".to_string()));
    // Unresolved and aliased titles resolve like references do.
    s.add_alias(&b, "Library").unwrap();
    assert_eq!(hits(&s, "[[Library]]"), hits(&s, "[[Books]]"));
}

#[test]
fn todo_between_and_attributes() {
    let (mut s, _d) = store();
    let (day1, _) = s.open_daily(today()).unwrap();
    let t = write(&mut s, &day1, "call the bank");
    s.set_block_kind(&day1, &t, BlockKind::Todo, false).unwrap();
    let done = write(&mut s, &day1, "pay rent");
    s.set_block_kind(&day1, &done, BlockKind::Todo, true)
        .unwrap();
    let (day0, _) = s.open_daily(today().pred_opt().unwrap()).unwrap();
    write(&mut s, &day0, "yesterday's note");
    assert_eq!(hits(&s, "[[TODO]]"), vec!["call the bank"]);
    assert_eq!(hits(&s, "[[DONE]]"), vec!["pay rent"]);
    let between = hits(
        &s,
        "{between: [[November 1st, 2026]] [[November 2nd, 2026]]}",
    );
    // Newest journal page first, outline order within a page; the empty first blocks of
    // each daily page are blocks too.
    assert_eq!(
        between
            .into_iter()
            .filter(|t| !t.is_empty())
            .collect::<Vec<_>>(),
        vec!["call the bank", "pay rent", "yesterday's note"]
    );

    let book = s.create_page("Thinking in Systems").unwrap();
    write(&mut s, &book, "Type:: [[Book]]");
    write(&mut s, &book, "Status:: reading");
    let other = s.create_page("Dune").unwrap();
    write(&mut s, &other, "Type:: book");
    write(&mut s, &other, "Status:: done");
    assert_eq!(
        hits(&s, "{attr: type book}").len(),
        2,
        "plain and linked values"
    );
    assert_eq!(
        hits(&s, "{and: {attr: Type [[Book]]} {attr: status reading}}"),
        Vec::<String>::new(),
        "different blocks: attributes are per block"
    );
    assert_eq!(hits(&s, "{attr: Status}").len(), 2);
    assert_eq!(hits(&s, "{attr: status Reading}"), vec!["Status:: reading"]);
    let r = s.run_query(&parse("{attr: status}").unwrap(), 1).unwrap();
    assert_eq!((r.hits.len(), r.total), (1, 2), "limit keeps the total");
    // Attribute rows follow edits and survive a full rebuild.
    let st = s.page_state(&other).unwrap().clone();
    let status = st.live_roots()[1].clone();
    s.set_block_text(&other, &status, "Status:: reading", None, None)
        .unwrap();
    s.commit_block(&other, &status).unwrap();
    assert_eq!(hits(&s, "{attr: status reading}").len(), 2);
    let r = s.doctor().unwrap();
    assert!(r.is_ok(), "{:#?}", r.problems);
    s.rebuild_index().unwrap();
    assert_eq!(hits(&s, "{attr: status reading}").len(), 2);
}

#[test]
fn v6_migration_backfills_attributes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("g.utopia");
    {
        let mut s = Store::open(&path).unwrap();
        let p = s.create_page("Dune").unwrap();
        write(&mut s, &p, "Author:: Frank Herbert");
        s.flush().unwrap();
    }
    // Simulate a v5 file: drop the table and roll the version back.
    {
        let c = rusqlite::Connection::open(&path).unwrap();
        c.execute_batch(
            "DROP TABLE attr; DROP TABLE attachment; DROP TABLE tombstone;
             PRAGMA user_version = 5;",
        )
        .unwrap();
    }
    let s = Store::open(&path).unwrap();
    assert_eq!(
        hits(&s, "{attr: author [[Frank Herbert]]}"),
        vec!["Author:: Frank Herbert"]
    );
}

/// The `attachment` table as v7 and v8 had it, so a migration test can rewind a finished
/// file to the shape those versions really wrote (rule 14: migrations are tested against
/// the old schema, not against today's).
const V8_ATTACHMENT: &str = "
CREATE TABLE attachment (
  hash       TEXT PRIMARY KEY,
  name       TEXT NOT NULL,
  mime       TEXT NOT NULL,
  size       INTEGER NOT NULL,
  created_at INTEGER NOT NULL,
  uploaded   INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX attachment_upload ON attachment(uploaded) WHERE uploaded = 0;";

/// v8 (docs/state-machines/page-lifecycle-trash.md issue 3): an older graph keeps opening
/// and gains the tombstone memory, so a title trashed before the upgrade is not issued the
/// same uuidv5 again after Empty Trash. The same migration drops a trash row that outlived
/// its tombstone (the state an undo of a delete used to leave, issue 1).
#[test]
fn v8_migration_remembers_tombstones_and_repairs_the_trash() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("g.utopia");
    let (gone, live) = {
        let mut s = Store::open(&path).unwrap();
        let gone = s.create_page("Gone").unwrap();
        write(&mut s, &gone, "kept in the trash");
        s.delete_page(&gone).unwrap();
        let live = s.create_page("Live").unwrap();
        s.sync_index().unwrap();
        (gone, live)
    };
    // Simulate a v7 file: no tombstone memory, the v7 attachment table, and a trash row for
    // a live page.
    {
        let c = rusqlite::Connection::open(&path).unwrap();
        c.execute_batch(&format!(
            "DROP TABLE tombstone;
             DROP TABLE attachment;
             {V8_ATTACHMENT}
             INSERT OR REPLACE INTO trash(id, kind, payload, deleted_at)
               VALUES ('{live}', 'page', X'7b7d', 1);
             PRAGMA user_version = 7;"
        ))
        .unwrap();
    }

    let mut s = Store::open(&path).unwrap();
    assert_eq!(
        s.list_trash().unwrap().len(),
        1,
        "the trashed page is still listed"
    );
    let stale: i64 = s
        .conn()
        .query_row(
            "SELECT count(*) FROM trash WHERE id=?1",
            [live.as_str()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(stale, 0, "a row that outlived its tombstone is dropped");
    let trashed_at = s.list_trash().unwrap()[0].2;
    assert_eq!(
        s.page_fate(&gone).unwrap(),
        Some(utopia_store::PageFate::Trashed { at: trashed_at }),
        "the migration does not change a page's fate"
    );
    assert_eq!(s.empty_trash(None).unwrap(), 1);
    assert_eq!(
        s.page_fate(&gone).unwrap(),
        Some(utopia_store::PageFate::Purged { at: trashed_at }),
        "the memory outlives the shard and the page row"
    );
    assert_ne!(
        s.create_page("Gone").unwrap(),
        gone,
        "a purged id is not issued again (rule 4)"
    );
}

/// v9 (docs/state-machines/attachment-blob-state.md): the upload flag becomes an explicit
/// `state` column. A v8 file keeps opening (§14.4) and nothing is uploaded twice or
/// forgotten by the upgrade: `uploaded = 1` is still the queue's complement.
#[test]
fn v9_migration_backfills_the_attachment_state() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("g.utopia");
    let (sent, pending) = {
        let mut s = Store::open(&path).unwrap();
        let sent = s.add_blob("sent.pdf", b"%PDF-1.4 sent").unwrap();
        let pending = s.add_blob("pending.pdf", b"%PDF-1.4 pending").unwrap();
        s.mark_blobs_uploaded(std::slice::from_ref(&sent.hash))
            .unwrap();
        (sent, pending)
    };
    // Simulate a v8 file: the v7 attachment table with its `uploaded` flag.
    {
        let c = rusqlite::Connection::open(&path).unwrap();
        c.execute_batch(&format!(
            "DROP TABLE attachment;
             {V8_ATTACHMENT}
             INSERT INTO attachment(hash, name, mime, size, created_at, uploaded)
               VALUES ('{sent}', 'sent.pdf', 'application/pdf', 14, 1, 1);
             INSERT INTO attachment(hash, name, mime, size, created_at, uploaded)
               VALUES ('{pending}', 'pending.pdf', 'application/pdf', 17, 2, 0);
             PRAGMA user_version = 8;",
            sent = sent.hash,
            pending = pending.hash
        ))
        .unwrap();
    }

    let s = Store::open(&path).unwrap();
    assert_eq!(
        s.attachment_state(&sent.hash).unwrap(),
        Some(utopia_store::BlobState::Uploaded),
        "a sent blob is not queued again"
    );
    assert_eq!(
        s.attachment_state(&pending.hash).unwrap(),
        Some(utopia_store::BlobState::Pending)
    );
    let queue = s.blobs_to_upload(10).unwrap();
    assert_eq!(
        queue.iter().map(|(h, _)| h.as_str()).collect::<Vec<_>>(),
        vec![pending.hash.as_str()]
    );
}

#[test]
fn page_graph_global_and_local() {
    use utopia_store::GraphOptions;
    let (mut s, _d) = store();
    let a = s.create_page("A").unwrap();
    write(&mut s, &a, "[[B]] and [[C]]");
    write(&mut s, &a, "[[B]] again");
    let b = s.find_page("B").unwrap().unwrap();
    write(&mut s, &b, "[[D]] and back to [[A]]");
    s.create_page("Orphan").unwrap();
    let (day, _) = s.open_daily(today()).unwrap();
    write(&mut s, &day, "[[A]] today");
    let g = s.page_graph(&GraphOptions::default()).unwrap();
    let titles: Vec<&str> = g.nodes.iter().map(|n| n.title.as_str()).collect();
    assert_eq!(titles[0], "A", "best connected first: {titles:?}");
    assert!(!titles.contains(&"Orphan"), "orphans are left out");
    assert!(!g.nodes.iter().any(|n| n.daily), "journal pages are opt-in");
    let ix = |t: &str| titles.iter().position(|x| *x == t).unwrap();
    let ab = g
        .edges
        .iter()
        .find(|e| (e.0, e.1) == (ix("A").min(ix("B")), ix("A").max(ix("B"))))
        .unwrap();
    assert_eq!(ab.2, 3, "both directions add up, one edge");
    assert_eq!(g.edges.len(), 3);
    // GR-4: the graph keeps the direction and count of every stored reference, so the
    // view can draw an arrowhead without resolving anything at paint time (rule 3).
    let dir = |from: &str, to: &str| {
        g.directed
            .iter()
            .find(|e| e.0 == ix(from) && e.1 == ix(to))
            .map(|e| e.2)
    };
    assert_eq!(dir("A", "B"), Some(2));
    assert_eq!(dir("B", "A"), Some(1));
    assert_eq!(dir("A", "C"), Some(1));
    assert_eq!(dir("B", "D"), Some(1));
    assert_eq!(dir("D", "B"), None, "direction is not invented");
    assert_eq!(g.directed.len(), 4, "{:?}", g.directed);
    // Local graph of D at depth 1: D and B only.
    let d = s.find_page("D").unwrap().unwrap();
    let local = s
        .page_graph(&GraphOptions {
            center: Some(d.clone()),
            depth: 1,
            ..Default::default()
        })
        .unwrap();
    let mut t: Vec<&str> = local.nodes.iter().map(|n| n.title.as_str()).collect();
    assert_eq!(t[0], "D", "center first");
    t.sort();
    assert_eq!(t, vec!["B", "D"]);
    let with_daily = s
        .page_graph(&GraphOptions {
            include_daily: true,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(with_daily.nodes.len(), g.nodes.len() + 1);
    let limited = s
        .page_graph(&GraphOptions {
            limit: 2,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(limited.nodes.len(), 2);
    assert!(limited.edges.iter().all(|e| e.0 < 2 && e.1 < 2));
}

#[test]
fn attachments_are_content_addressed_files() {
    let (mut s, dir) = store();
    let src = dir.path().join("Scan 1.pdf");
    std::fs::write(&src, b"%PDF-1.7 hello").unwrap();
    let a = s.attach_file(&src).unwrap();
    assert_eq!(a.mime, "application/pdf");
    assert_eq!(a.size, 14);
    assert_eq!(a.hash, utopia_store::sha256_hex(b"%PDF-1.7 hello"));
    let path = s.blob_path(&a.hash).unwrap();
    assert_eq!(path, dir.path().join("g.blobs").join(&a.hash));
    assert_eq!(std::fs::read(&path).unwrap(), b"%PDF-1.7 hello");
    // Same content, same blob; nothing to download, one upload pending.
    let again = s.add_blob("copy.pdf", b"%PDF-1.7 hello").unwrap();
    assert_eq!(again.hash, a.hash);
    assert_eq!(s.blobs_to_upload(10).unwrap().len(), 1);
    s.mark_blobs_uploaded(std::slice::from_ref(&a.hash))
        .unwrap();
    assert!(s.blobs_to_upload(10).unwrap().is_empty());
    // References from blocks drive what sync fetches.
    let p = s.create_page("Inbox").unwrap();
    let other = "ab".repeat(32);
    write(
        &mut s,
        &p,
        &format!(
            "{} and ![x.png](blob:{other})",
            utopia_core::attach::markup("Scan 1.pdf", &a.hash, &a.mime)
        ),
    );
    assert_eq!(s.missing_blobs().unwrap(), vec![other.clone()]);
    assert!(s.put_synced_blob(&other, b"not matching").is_err());
    assert!(
        s.blob("../../etc/passwd").unwrap().is_none(),
        "only hashes name blobs"
    );
    // Size cap.
    let big = vec![0u8; utopia_store::MAX_BLOB_BYTES + 1];
    assert!(s.add_blob("big.bin", &big).is_err());
}

/// docs/state-machines/attachment-blob-state.md issue 1 (§11.6, §10.3): a row whose file is
/// gone must not pin the upload queue. `blobs_to_upload` applied `LIMIT` in SQL, before
/// checking that the file exists, and silently skipped dead rows, so the oldest dead rows
/// made every newer attachment wait forever.
#[test]
fn a_missing_file_does_not_pin_the_upload_queue() {
    let (mut s, dir) = store();
    let lost = s.add_blob("gone.pdf", b"%PDF-1.4 gone").unwrap();
    let live = s.add_blob("here.pdf", b"%PDF-1.4 here").unwrap();
    // The dead row is the oldest, so the SQL `LIMIT` selected it on every round.
    s.conn()
        .execute(
            "UPDATE attachment SET created_at = 1 WHERE hash = ?1",
            [&lost.hash],
        )
        .unwrap();
    s.conn()
        .execute(
            "UPDATE attachment SET created_at = 2 WHERE hash = ?1",
            [&live.hash],
        )
        .unwrap();
    std::fs::remove_file(s.blob_path(&lost.hash).unwrap()).unwrap();

    let upload = s.blobs_to_upload(1).unwrap();
    assert_eq!(
        upload.iter().map(|(h, _)| h.as_str()).collect::<Vec<_>>(),
        vec![live.hash.as_str()],
        "the blob this device still has uploads even though the oldest row is dead"
    );
    assert_eq!(upload[0].1, b"%PDF-1.4 here");
    assert!(
        matches!(
            s.attachment_state(&lost.hash).unwrap(),
            Some(utopia_store::BlobState::Lost { .. })
        ),
        "the dead row is noticed, not silently skipped"
    );
    s.mark_blobs_uploaded(std::slice::from_ref(&live.hash))
        .unwrap();
    assert!(
        s.blobs_to_upload(10).unwrap().is_empty(),
        "the dead row is never selected again"
    );
    // The bytes coming back (a restored `.blobs` directory) revives the row on the next
    // round instead of leaving it lost forever.
    std::fs::write(
        dir.path().join("g.blobs").join(&lost.hash),
        b"%PDF-1.4 gone",
    )
    .unwrap();
    assert_eq!(
        s.blobs_to_upload(10).unwrap(),
        vec![(lost.hash.clone(), b"%PDF-1.4 gone".to_vec())]
    );
    assert_eq!(
        s.attachment_state(&lost.hash).unwrap(),
        Some(utopia_store::BlobState::Pending)
    );
}

/// docs/state-machines/attachment-blob-state.md issue 1 and rule 10: filling the upload
/// batch must stop as soon as it has `limit` blobs. `blobs_to_upload` runs inside
/// `SyncEngine::prepare` on the UI thread (`Workspace::start_sync`), and the full-queue scan
/// inspected, and marked, every row behind a batch that was already full.
#[test]
fn filling_the_upload_batch_stops_at_the_limit() {
    let (mut s, _dir) = store();
    let live = s.add_blob("here.pdf", b"%PDF-1.4 here").unwrap();
    let behind = s.add_blob("gone.pdf", b"%PDF-1.4 gone").unwrap();
    // The live blob is the oldest, so it fills the single slot.
    s.conn()
        .execute(
            "UPDATE attachment SET created_at = 1 WHERE hash = ?1",
            [&live.hash],
        )
        .unwrap();
    s.conn()
        .execute(
            "UPDATE attachment SET created_at = 2 WHERE hash = ?1",
            [&behind.hash],
        )
        .unwrap();
    std::fs::remove_file(s.blob_path(&behind.hash).unwrap()).unwrap();

    let upload = s.blobs_to_upload(1).unwrap();
    assert_eq!(
        upload.iter().map(|(h, _)| h.as_str()).collect::<Vec<_>>(),
        vec![live.hash.as_str()]
    );
    assert_eq!(
        s.attachment_state(&behind.hash).unwrap(),
        Some(utopia_store::BlobState::Pending),
        "the batch was full, so the row behind it was not looked at yet"
    );
}

/// docs/state-machines/attachment-blob-state.md issue 1 and rule 10: lost rows are not
/// re-checked by the queue scan every round (that was unbounded filesystem work on the UI
/// thread). They get a bounded reconciliation pass instead, over the batch's spare slots,
/// oldest check first, refreshing `lost_at` so the pass rotates and every row is reached.
#[test]
fn lost_rows_are_rechecked_in_a_bounded_rotation() {
    let (mut s, dir) = store();
    let blobs = ["one", "two", "three"].map(|n| s.add_blob(n, n.as_bytes()).unwrap());
    for b in &blobs {
        std::fs::remove_file(s.blob_path(&b.hash).unwrap()).unwrap();
    }
    // Marking is not the bounded part: a row is met while pending once, marked, and then
    // leaves that scan for good.
    assert!(s.blobs_to_upload(3).unwrap().is_empty());
    let mut order: Vec<String> = blobs.iter().map(|b| b.hash.clone()).collect();
    order.sort();
    for (i, h) in order.iter().enumerate() {
        s.conn()
            .execute(
                "UPDATE attachment SET lost_at = ?2 WHERE hash = ?1",
                rusqlite::params![h, i as i64 + 1],
            )
            .unwrap();
    }

    // A round with two spare upload slots re-checks only the two oldest checks.
    assert!(s.blobs_to_upload(2).unwrap().is_empty());
    assert!(
        lost_at(&s, &order[0]) > 3 && lost_at(&s, &order[1]) > 3,
        "checked rows move to the back of the rotation"
    );
    assert_eq!(lost_at(&s, &order[2]), 3, "its turn has not come yet");
    assert!(blobs.iter().all(|b| matches!(
        s.attachment_state(&b.hash).unwrap(),
        Some(utopia_store::BlobState::Lost { .. })
    )));

    // The bytes come back for the row whose turn it is: it is revived and uploaded in the
    // same round, in one of the spare slots.
    let content = |hash: &str| {
        blobs
            .iter()
            .find(|b| b.hash == hash)
            .map(|b| b.name.clone())
            .unwrap()
    };
    std::fs::write(
        dir.path().join("g.blobs").join(&order[2]),
        content(&order[2]).as_bytes(),
    )
    .unwrap();
    let upload = s.blobs_to_upload(2).unwrap();
    assert_eq!(
        upload.iter().map(|(h, _)| h.as_str()).collect::<Vec<_>>(),
        vec![order[2].as_str()]
    );
    assert_eq!(
        s.attachment_state(&order[2]).unwrap(),
        Some(utopia_store::BlobState::Pending)
    );
}

#[test]
fn markdown_export_carries_attachments() {
    let (mut s, dir) = store();
    let a = s.add_blob("cat.png", b"\x89PNG\r\n\x1a\nmeow").unwrap();
    let p = s.create_page("Pets").unwrap();
    write(
        &mut s,
        &p,
        &utopia_core::attach::markup(&a.name, &a.hash, &a.mime),
    );
    let out = dir.path().join("export");
    s.export_markdown(&out).unwrap();
    let md = std::fs::read_to_string(out.join("Pets.md")).unwrap();
    assert!(
        md.contains(&format!("![cat.png](blobs/{})", a.hash)),
        "{md}"
    );
    assert_eq!(
        std::fs::read(out.join("blobs").join(&a.hash)).unwrap(),
        b"\x89PNG\r\n\x1a\nmeow"
    );
}

#[test]
fn pdf_highlights_are_quote_blocks_under_the_pdf() {
    let (mut s, _d) = store();
    let a = s.add_blob("paper.pdf", b"%PDF-1.4 fake").unwrap();
    let p = s.create_page("Reading").unwrap();
    let host = write(
        &mut s,
        &p,
        &utopia_core::attach::markup(&a.name, &a.hash, &a.mime),
    );
    let (hp, hb) = s
        .add_pdf_highlight(
            &a.hash,
            None,
            1,
            &[[0.1, 0.5, 0.6, 0.05]],
            "  feedback\\nloops   dominate ",
        )
        .unwrap();
    assert_eq!(hp, p, "lands on the page that shows the PDF");
    let st = s.page_state(&p).unwrap().clone();
    assert_eq!(st.parent_of(&hb), Some(host.clone()), "under the PDF block");
    assert_eq!(st.block(&hb).unwrap().kind, BlockKind::Quote);
    s.add_pdf_highlight(&a.hash, None, 0, &[[0.0, 0.2, 1.0, 0.1]], "first page")
        .unwrap();
    let hs = s.pdf_highlights(&a.hash).unwrap();
    assert_eq!(hs.len(), 2);
    assert_eq!((hs[0].pdf_page, hs[0].text.as_str()), (0, "first page"));
    assert_eq!(hs[1].text, "feedback\\nloops dominate");
    assert_eq!(hs[1].rects, vec![[0.1, 0.5, 0.6, 0.05]]);
    // Searchable like any block; the markup is not the text people search for.
    let hits = s.search_blocks("dominate", 10).unwrap();
    assert_eq!(hits.len(), 1);
    // Opened from a second block that attaches the same PDF: highlights go there.
    let other = s.create_page("Seminar").unwrap();
    let host2 = write(
        &mut s,
        &other,
        &utopia_core::attach::markup(&a.name, &a.hash, &a.mime),
    );
    let (hp2, hb2) = s
        .add_pdf_highlight(
            &a.hash,
            Some(&host2),
            0,
            &[[0.1, 0.1, 0.2, 0.02]],
            "seminar",
        )
        .unwrap();
    assert_eq!(hp2, other);
    assert_eq!(s.page_state(&other).unwrap().parent_of(&hb2), Some(host2));
    // A highlight block is never a host, even when named as one.
    let (hp3, _) = s
        .add_pdf_highlight(&a.hash, Some(&hb2), 0, &[[0.1, 0.2, 0.2, 0.02]], "x")
        .unwrap();
    assert_eq!(hp3, p, "falls back to the oldest attaching block");
    // Highlights of an unembedded PDF go to today's page.
    let lone = s.add_blob("lone.pdf", b"%PDF-1.4 other").unwrap();
    let (tp, _) = s
        .add_pdf_highlight(&lone.hash, None, 0, &[[0., 0., 1., 1.]], "x")
        .unwrap();
    assert_eq!(s.page_state(&tp).unwrap().meta.daily_date, Some(today()));
}

#[test]
fn encryption_at_rest_round_trip() {
    if !utopia_store::encryption_available() {
        eprintln!("SQLCipher not linked on this platform; skipping");
        return;
    }
    use utopia_store::{is_encrypted, set_encryption, StoreError};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("g.utopia");
    let page = {
        let mut s = Store::open(&path).unwrap();
        let p = s.create_page("Diary").unwrap();
        write(&mut s, &p, "my very private thought");
        s.flush().unwrap();
        p
    };
    let raw = |p: &std::path::Path| {
        let mut b = std::fs::read(p).unwrap();
        if let Ok(w) = std::fs::read(p.with_extension("utopia-wal")) {
            b.extend(w);
        }
        b
    };
    let has = |hay: &[u8], n: &[u8]| hay.windows(n.len()).any(|w| w == n);
    assert!(has(&raw(&path), b"private thought"), "plain before");
    assert!(!is_encrypted(&path));
    set_encryption(&path, None, Some("hunter2 horse")).unwrap();
    assert!(is_encrypted(&path));
    assert!(!has(&raw(&path), b"private thought"), "ciphertext only");
    assert!(!has(&raw(&path), b"SQLite format 3"), "not even the header");
    // Without the key, or with a wrong one: a clear error, never a corrupt open.
    assert!(matches!(Store::open(&path), Err(StoreError::Encrypted)));
    for bad in ["wrong", ""] {
        assert!(matches!(
            Store::open_with_key(&path, Some(bad)),
            Err(StoreError::Encrypted)
        ));
    }
    {
        let mut s = Store::open_with_key(&path, Some("hunter2 horse")).unwrap();
        let st = s.page_state(&page).unwrap().clone();
        let texts: Vec<String> = st
            .live_preorder()
            .into_iter()
            .map(|(id, _)| st.block(&id).unwrap().text.clone())
            .collect();
        assert!(texts.contains(&"my very private thought".to_string()));
        // Writes go through the (keyed) writer thread.
        write(&mut s, &page, "another secret line");
        s.flush().unwrap();
        assert_eq!(s.search_blocks("another", 5).unwrap().len(), 1);
        let r = s.doctor().unwrap();
        assert!(r.is_ok(), "{:#?}", r.problems);
        // Backups of an encrypted graph stay encrypted.
        let b = s.backup(&dir.path().join("backup"), 3).unwrap();
        assert!(is_encrypted(&b));
        assert!(Store::open_with_key(&b, Some("hunter2 horse")).is_ok());
    }
    assert!(!has(&raw(&path), b"another secret"));
    // Re-key, then decrypt.
    set_encryption(&path, Some("hunter2 horse"), Some("new pass")).unwrap();
    assert!(Store::open_with_key(&path, Some("new pass")).is_ok());
    set_encryption(&path, Some("new pass"), None).unwrap();
    assert!(!is_encrypted(&path));
    let s = Store::open(&path).unwrap();
    assert_eq!(s.search_blocks("secret", 5).unwrap().len(), 1);
    assert!(
        set_encryption(&path, None, Some("")).is_err(),
        "empty passphrase"
    );
}

/// Issue 1 (docs/state-machines/encryption-at-rest-state.md): `SQLITE_NOTADB` is not
/// always encryption. A plain graph whose SQLite header survives but whose pages are
/// damaged is corrupt, not locked: a passphrase cannot open it, so the UI must show the
/// error instead of the unlock screen.
#[test]
fn ac_15_6_a_damaged_plain_graph_is_not_a_lock() {
    use utopia_store::{is_encrypted, FileKind, StoreError};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("g.utopia");
    {
        let mut s = Store::open(&path).unwrap();
        let p = s.create_page("Damaged").unwrap();
        write(&mut s, &p, "still not ciphertext");
        s.flush().unwrap();
    }
    for ext in ["utopia-wal", "utopia-shm"] {
        let _ = std::fs::remove_file(path.with_extension(ext));
    }
    // Keep SQLite's 16-byte header, destroy the first page: `SQLITE_NOTADB`, exactly what
    // a missing passphrase also reports.
    let mut bytes = std::fs::read(&path).unwrap();
    bytes.truncate(16);
    bytes.resize(4096, 0);
    std::fs::write(&path, &bytes).unwrap();

    assert!(
        !is_encrypted(&path),
        "a file that still starts with SQLite's header is not ciphertext"
    );
    assert_eq!(utopia_store::file_kind(&path), FileKind::Corrupt);
    assert!(
        matches!(Store::open(&path), Err(StoreError::Corrupt)),
        "opening a damaged plain graph reports damage, not a missing passphrase"
    );
}
