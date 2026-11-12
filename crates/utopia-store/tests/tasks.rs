//! Tasks view reads (PRD §9.2.2): every live to-do, grouped by page.

use chrono::NaiveDate;
use utopia_core::outline;
use utopia_core::{BlockId, BlockKind, PageId};
use utopia_store::{Store, TaskGroup};

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

fn todo(s: &mut Store, page: &PageId, parent: Option<&BlockId>, text: &str) -> BlockId {
    let id = write_under(s, page, parent, text);
    s.set_block_kind(page, &id, BlockKind::Todo, false).unwrap();
    id
}

fn titles(groups: &[TaskGroup]) -> Vec<&str> {
    groups.iter().map(|g| g.title.as_str()).collect()
}

fn texts(g: &TaskGroup) -> Vec<&str> {
    g.tasks.iter().map(|t| t.text.as_str()).collect()
}

/// TK-2: one group per page, most recently changed first; outline order inside a page;
/// the nearest heading, a plain parent's text and the to-do above each task.
#[test]
fn ac_tk_2_tasks_grouped_by_page() {
    let (mut s, _d) = store();
    s.set_clock(Some(1_000));
    let atlas = s.create_page("Atlas Launch").unwrap();
    let plan = write_under(&mut s, &atlas, None, "Launch plan");
    s.set_block_kind(&atlas, &plan, BlockKind::Heading(2), false)
        .unwrap();
    todo(&mut s, &atlas, Some(&plan), "Write the notes");
    let review = todo(&mut s, &atlas, Some(&plan), "Book the review");
    let nested = todo(&mut s, &atlas, Some(&review), "Ask [[Ana]] for a slot");
    let done = todo(&mut s, &atlas, None, "Pick the testers");
    s.set_block_kind(&atlas, &done, BlockKind::Todo, true)
        .unwrap();

    s.set_clock(Some(2_000));
    let (day, _) = s.open_daily(today()).unwrap();
    let met = write_under(&mut s, &day, None, "Met [[Ana]] about the **timeline**");
    todo(&mut s, &day, Some(&met), "Send her the notes");

    let groups = s.tasks().unwrap();
    assert_eq!(titles(&groups), vec!["November 2nd, 2026", "Atlas Launch"]);
    assert_eq!(groups[0].daily, Some(today()));
    assert_eq!(groups[1].daily, None);

    let day_task = &groups[0].tasks[0];
    assert_eq!(
        day_task.context.as_deref(),
        Some("Met Ana about the timeline")
    );
    assert_eq!(day_task.heading, None);

    let a = &groups[1];
    assert_eq!(
        texts(a),
        vec![
            "Write the notes",
            "Book the review",
            "Ask [[Ana]] for a slot",
            "Pick the testers"
        ]
    );
    assert_eq!(a.tasks[0].heading.as_deref(), Some("Launch plan"));
    // A heading parent is the sub-heading, not a context line.
    assert_eq!(a.tasks[0].context, None);
    assert_eq!(a.tasks[2].heading.as_deref(), Some("Launch plan"));
    assert_eq!(a.tasks[2].parent_task, Some(review));
    assert_eq!(a.tasks[2].block, nested);
    assert_eq!(a.tasks[3].heading, None);
    assert!(a.tasks[3].checked);
    assert_eq!(a.open(), 3);

    // TK-1: each to-do counts once, children or not.
    assert_eq!(s.open_task_count().unwrap(), 4);
}

/// TK-5: the earliest date link is the due date; an ambiguous numeric date never is.
#[test]
fn tk_5_due_is_the_earliest_date_link() {
    let (mut s, _d) = store();
    let p = s.create_page("Home").unwrap();
    todo(
        &mut s,
        &p,
        None,
        "Pay rent [[December 1st, 2026]] or [[November 16th, 2026]]",
    );
    todo(&mut s, &p, None, "Call the venue 11/6/26");
    let groups = s.tasks().unwrap();
    let tasks = &groups[0].tasks;
    assert_eq!(tasks[0].due, NaiveDate::from_ymd_opt(2026, 11, 16));
    assert_eq!(tasks[1].due, None);
}

/// TK-2: deleted blocks and blocks on deleted pages are not tasks, and do not count.
#[test]
fn tk_2_deleted_tasks_are_gone() {
    let (mut s, _d) = store();
    let keep = s.create_page("Keep").unwrap();
    todo(&mut s, &keep, None, "stays");
    let gone = todo(&mut s, &keep, None, "deleted block");
    let now = s.now();
    let plan = outline::delete_subtree(s.page_state(&keep).unwrap(), &gone, now).unwrap();
    s.execute_plan(&keep, plan, None).unwrap();
    let trash = s.create_page("Trash me").unwrap();
    todo(&mut s, &trash, None, "on a deleted page");
    s.delete_page(&trash).unwrap();

    let groups = s.tasks().unwrap();
    assert_eq!(titles(&groups), vec!["Keep"]);
    assert_eq!(texts(&groups[0]), vec!["stays"]);
    assert_eq!(s.open_task_count().unwrap(), 1);
}

/// TK-4: ticking a box is the page's own command, so it undoes in one step.
#[test]
fn tk_4_tick_is_one_undo_step() {
    let (mut s, _d) = store();
    let p = s.create_page("Home").unwrap();
    let t = todo(&mut s, &p, None, "water the plants");
    s.set_block_kind(&p, &t, BlockKind::Todo, true).unwrap();
    assert!(s.tasks().unwrap()[0].tasks[0].checked);
    assert_eq!(s.open_task_count().unwrap(), 0);
    s.undo().unwrap();
    assert!(!s.tasks().unwrap()[0].tasks[0].checked);
    assert_eq!(s.open_task_count().unwrap(), 1);
}
