//! Remote changes that wait for another device's earlier changes (§12.3 step 3, §2.3.7).
//!
//! The incident behind these scripts: a phone pushed its first changes to a mistyped graph
//! id and its later ones to the real one. The Mac imported the later ones, Loro held them
//! pending, the store reported a clean import, and the changes were gone. A store must keep
//! them durable until the missing changes arrive, and say that it holds them.

use chrono::NaiveDate;
use utopia_core::{BlockId, BlockKind, Command, PageId};
use utopia_doc::Frontier;
use utopia_store::{EditOpts, Store};

fn date(y: i32, m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, d).unwrap()
}

fn open(path: &std::path::Path) -> Store {
    let mut s = Store::open(path).unwrap();
    s.set_today(Some(date(2026, 11, 23)));
    s
}

fn insert(store: &mut Store, page: &PageId, after: Option<&BlockId>, text: &str) -> BlockId {
    let id = BlockId::new();
    store
        .execute(
            page,
            vec![Command::InsertBlock {
                id: id.clone(),
                parent: None,
                after: after.cloned(),
                text: text.into(),
                kind: BlockKind::Bullet,
            }],
            EditOpts::default(),
        )
        .unwrap();
    store.sync_index().unwrap();
    id
}

fn texts(store: &mut Store, page: &PageId) -> Vec<String> {
    let mut t: Vec<String> = store
        .page_state(page)
        .unwrap()
        .blocks
        .values()
        .filter(|b| b.deleted_at.is_none())
        .map(|b| b.text.clone())
        .collect();
    t.sort();
    t
}

/// The sender's page in three parts: what the receiver already has, the change that never
/// reaches it, and the later change that depends on the missing one.
struct Sender {
    page: PageId,
    base: Vec<u8>,
    missing: Vec<u8>,
    later: Vec<u8>,
}

fn sender(dir: &std::path::Path) -> Sender {
    let mut a = open(&dir.join("phone.utopia"));
    let page = a.create_page("Shared").unwrap();
    let first = insert(&mut a, &page, None, "from the Mac's base");
    let base = a.export_since(&page, &Frontier::default()).unwrap();
    let f0 = a.frontier(&page).unwrap();
    let mid = insert(&mut a, &page, Some(&first), "pushed to the wrong graph id");
    let missing = a.export_since(&page, &f0).unwrap();
    let f1 = a.frontier(&page).unwrap();
    insert(&mut a, &page, Some(&mid), "pushed to the right graph id");
    let later = a.export_since(&page, &f1).unwrap();
    Sender {
        page,
        base,
        missing,
        later,
    }
}

/// §12.3 step 3, §2.3.7: changes that wait are reported, survive local compaction and a
/// restart, and apply when the missing change arrives; `doctor` stays clean throughout.
#[test]
fn ac_12_3_waiting_changes_survive_compaction_and_reopen_then_apply() {
    let dir = tempfile::tempdir().unwrap();
    let s = sender(dir.path());
    let path = dir.path().join("mac.utopia");
    {
        let mut b = open(&path);
        b.import(&s.page, &s.base).unwrap();
        assert_eq!(b.waiting_count().unwrap(), 0);
        b.import(&s.page, &s.later).unwrap();
        assert!(b.has_waiting(&s.page).unwrap(), "the later change waits");
        let waiting = b.waiting_pages().unwrap();
        assert_eq!(waiting.len(), 1);
        assert!(
            !waiting[0].ranges.is_empty(),
            "it names the peer it waits on"
        );
        assert_eq!(texts(&mut b, &s.page), vec!["from the Mac's base"]);

        // Enough local edits to fold the shard into a fresh snapshot, which never holds
        // pending changes.
        let block = b
            .page_state(&s.page)
            .unwrap()
            .blocks
            .keys()
            .next()
            .cloned()
            .unwrap();
        for i in 0..140 {
            b.set_block_text(&s.page, &block, &format!("edit {i}"), None, None)
                .unwrap();
        }
        b.sync_index().unwrap();
        b.compact().unwrap();
        // §12.6: `doctor` reports the page, and the report is the only problem.
        let report = b.doctor().unwrap();
        assert_eq!(report.problems.len(), 1, "{:?}", report.problems);
        assert!(
            report.problems[0].starts_with("sync: page"),
            "{:?}",
            report.problems
        );
    }
    {
        let mut b = open(&path);
        assert!(
            b.has_waiting(&s.page).unwrap(),
            "still waiting after a restart"
        );
        b.import(&s.page, &s.missing).unwrap();
        assert_eq!(b.waiting_count().unwrap(), 0, "nothing waits any more");
        let t = texts(&mut b, &s.page);
        assert!(
            t.contains(&"pushed to the wrong graph id".to_string()),
            "{t:?}"
        );
        assert!(
            t.contains(&"pushed to the right graph id".to_string()),
            "{t:?}"
        );
        assert!(b.doctor().unwrap().is_ok());
    }
    let mut b = open(&path);
    assert_eq!(b.waiting_count().unwrap(), 0);
    assert_eq!(texts(&mut b, &s.page).len(), 3, "and they are durable");
}

/// A page this device has never seen, known only through a waiting change, has no shard
/// row; the change must still survive a restart.
#[test]
fn ac_12_3_a_page_known_only_by_waiting_changes_survives_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let s = sender(dir.path());
    let path = dir.path().join("mac.utopia");
    {
        let mut b = open(&path);
        b.import(&s.page, &s.later).unwrap();
        assert!(b.has_waiting(&s.page).unwrap());
    }
    let mut b = open(&path);
    assert!(b.has_waiting(&s.page).unwrap());
    b.import(&s.page, &s.missing).unwrap();
    assert!(b.has_waiting(&s.page).unwrap(), "the base is still missing");
    b.import(&s.page, &s.base).unwrap();
    assert_eq!(b.waiting_count().unwrap(), 0);
    assert_eq!(texts(&mut b, &s.page).len(), 3);
}
