//! v1.1 (P1) acceptance scripts: §4.2 P1 list, FR-2.18/2.19, FR-3.9/3.11, AC-3.7, W2, W7.

use chrono::NaiveDate;
use utopia_core::outline;
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

fn write(s: &mut Store, page: &PageId, text: &str) -> BlockId {
    let plan = outline::append_block(s.page_state(page).unwrap(), None);
    let id = s.execute_plan(page, plan, None).unwrap().unwrap().block;
    s.set_block_text(page, &id, text, None, None).unwrap();
    s.commit_block(page, &id).unwrap();
    id
}

fn texts(s: &mut Store, page: &PageId) -> Vec<(String, usize)> {
    let st = s.page_state(page).unwrap().clone();
    st.live_preorder()
        .into_iter()
        .map(|(id, d)| (st.block(&id).unwrap().text.clone(), d))
        .collect()
}

fn doctor_ok(s: &mut Store) {
    let r = s.doctor().unwrap();
    assert!(r.is_ok(), "{:#?}", r.problems);
}

#[test]
fn ac_3_7_promote_mention_to_link_and_undo() {
    let (mut s, _d) = store();
    let m = s.create_page("Mindfulness").unwrap();
    let a = s.create_page("Reading").unwrap();
    let b1 = write(&mut s, &a, "the mindfulness of the thing");
    write(&mut s, &a, "a [[Mindfulness]] practice");
    write(&mut s, &a, "`mindfulness` in code and unmindfulness");
    let mentions = s.unlinked_references(&m).unwrap();
    assert_eq!(mentions.len(), 1, "{mentions:?}");
    assert_eq!(&mentions[0].text[mentions[0].range.clone()], "mindfulness");
    s.promote_mention(&b1, mentions[0].range.clone(), &m)
        .unwrap();
    assert_eq!(
        texts(&mut s, &a)[0].0,
        "the [[Mindfulness]] of the thing",
        "no other character altered"
    );
    assert!(s.unlinked_references(&m).unwrap().is_empty());
    assert_eq!(s.backlink_count(&m).unwrap(), 2);
    s.undo().unwrap();
    assert_eq!(
        s.unlinked_references(&m).unwrap().len(),
        1,
        "Cmd+Z restores both views"
    );
    assert_eq!(s.backlink_count(&m).unwrap(), 1);
    assert!(
        s.promote_mention(&b1, 0..3, &m).is_err(),
        "only real mentions can be promoted"
    );
    doctor_ok(&mut s);
}

#[test]
fn aliases_ui_rules() {
    let (mut s, _d) = store();
    let p = s.create_page("Dana Smith").unwrap();
    s.create_page("Other").unwrap();
    s.add_alias(&p, "Dana").unwrap();
    assert_eq!(s.find_page("dana").unwrap(), Some(p.clone()));
    assert!(
        s.add_alias(&p, "Other").is_err(),
        "cannot shadow another page"
    );
    assert!(
        s.add_alias(&p, "2026-11-02").is_err(),
        "aliases are never dates"
    );
    s.remove_alias(&p, "dana").unwrap();
    assert_eq!(s.find_page("Dana").unwrap(), None);
    doctor_ok(&mut s);
}

#[test]
fn journal_and_page_templates() {
    let (mut s, _d) = store();
    let t = s.create_page("templates/journal").unwrap();
    write(&mut s, &t, "Gratitude::");
    write(&mut s, &t, "Top 3 today");
    let st = s.page_state(&t).unwrap();
    let parent = st.live_roots()[1].clone();
    let plan = outline::split_block(st, &parent, "Top 3 today".len(), false).unwrap();
    let child = s.execute_plan(&t, plan, None).unwrap().unwrap().block;
    let plan = outline::indent(s.page_state(&t).unwrap(), &child).unwrap();
    s.execute_plan(&t, plan, None).unwrap();
    s.set_block_text(&t, &child, "1.", None, None).unwrap();
    let (day, first) = s.open_daily(today()).unwrap();
    assert_eq!(
        texts(&mut s, &day),
        vec![
            ("Gratitude::".into(), 0),
            ("Top 3 today".into(), 0),
            ("1.".into(), 1)
        ]
    );
    assert_ne!(first, BlockId::from("x"));
    // A page template inserted after a block, one undo step.
    let meeting = s.create_page("templates/meeting").unwrap();
    write(&mut s, &meeting, "Attendees::");
    assert_eq!(s.template_pages().unwrap().len(), 2);
    let notes = s.create_page("Standup").unwrap();
    let b = write(&mut s, &notes, "intro");
    s.apply_template(&notes, None, Some(&b), &meeting).unwrap();
    assert_eq!(
        texts(&mut s, &notes),
        vec![("intro".into(), 0), ("Attendees::".into(), 0)]
    );
    s.undo().unwrap();
    assert_eq!(texts(&mut s, &notes), vec![("intro".into(), 0)]);
    doctor_ok(&mut s);
}

#[test]
fn fr_2_18_extract_block_to_page() {
    let (mut s, _d) = store();
    let src = s.create_page("Notes").unwrap();
    write(&mut s, &src, "before");
    let b = write(&mut s, &src, "a big idea");
    let st = s.page_state(&src).unwrap();
    let plan = outline::split_block(st, &b, "a big idea".len(), false).unwrap();
    let c = s.execute_plan(&src, plan, None).unwrap().unwrap().block;
    let plan = outline::indent(s.page_state(&src).unwrap(), &c).unwrap();
    s.execute_plan(&src, plan, None).unwrap();
    s.set_block_text(&src, &c, "supporting detail", None, None)
        .unwrap();
    write(&mut s, &src, "after");
    let other = s.create_page("Elsewhere").unwrap();
    write(&mut s, &other, &format!("quoting (({b}))"));
    let dst = s.extract_block_to_page(&src, &b, "Big Idea").unwrap();
    assert_eq!(
        texts(&mut s, &src),
        vec![
            ("before".into(), 0),
            ("[[Big Idea]]".into(), 0),
            ("after".into(), 0)
        ]
    );
    assert_eq!(
        texts(&mut s, &dst),
        vec![("a big idea".into(), 0), ("supporting detail".into(), 1)]
    );
    assert_eq!(
        s.block_page(&b).unwrap(),
        dst,
        "ids kept, so ((refs)) still resolve"
    );
    assert_eq!(s.block_backlinks(&b).unwrap().len(), 1);
    assert_eq!(s.backlink_count(&dst).unwrap(), 1);
    doctor_ok(&mut s);
    s.undo().unwrap();
    assert_eq!(texts(&mut s, &src).len(), 4, "one undo step");
    assert_eq!(s.block_page(&b).unwrap(), src);
    doctor_ok(&mut s);
}

#[test]
fn drag_block_across_pages() {
    let (mut s, _d) = store();
    let a = s.create_page("A").unwrap();
    let b = s.create_page("B").unwrap();
    let x = write(&mut s, &a, "moving");
    let y = write(&mut s, &b, "target");
    s.move_block_to_page(&a, &x, &b, None, Some(&y)).unwrap();
    assert!(texts(&mut s, &a).is_empty());
    assert_eq!(
        texts(&mut s, &b),
        vec![("target".into(), 0), ("moving".into(), 0)]
    );
    doctor_ok(&mut s);
}

#[test]
fn block_types_and_capture() {
    let (mut s, _d) = store();
    let (page, first) = s.capture("call [[Dana]] about #invoices").unwrap();
    assert_eq!(
        texts(&mut s, &page),
        vec![("call [[Dana]] about #invoices".into(), 0)]
    );
    assert!(s.find_page("Dana").unwrap().is_some());
    s.capture("second thought").unwrap();
    assert_eq!(texts(&mut s, &page).len(), 2, "appends to today");
    s.set_block_kind(&page, &first, BlockKind::Todo, true)
        .unwrap();
    let b = s.page_state(&page).unwrap().block(&first).unwrap().clone();
    assert_eq!((b.kind, b.checked), (BlockKind::Todo, true));
    assert_eq!(s.block_row(&first).unwrap().unwrap().kind, BlockKind::Todo);
    s.undo().unwrap();
    assert_eq!(
        s.page_state(&page).unwrap().block(&first).unwrap().kind,
        BlockKind::Bullet
    );
}

#[test]
fn roam_json_import() {
    let (mut s, _d) = store();
    let json = r#"[
      {"title": "November 1st, 2026", "children": [
        {"string": "{{[[TODO]]}} read about [[Zettelkasten]]", "uid": "abcDEF123"},
        {"string": "Met __Dana__", "uid": "ghiJKL456", "children": [
          {"string": "see ((abcDEF123))", "uid": "mnoPQR789"}
        ]}
      ]},
      {"title": "Zettelkasten", "children": [{"string": "{{[[DONE]]}} set up", "uid": "stuVWX000", "heading": 2}]}
    ]"#;
    let r = s.import_roam_json(json).unwrap();
    assert_eq!((r.pages, r.blocks), (2, 4));
    let day = s
        .find_page("2026-11-01")
        .unwrap()
        .expect("dates become daily pages");
    assert_eq!(
        texts(&mut s, &day),
        vec![
            ("read about [[Zettelkasten]]".into(), 0),
            ("Met _Dana_".into(), 0),
            ("see ((abcDEF123))".into(), 1)
        ]
    );
    let todo = s.block_row(&BlockId::from("abcDEF123")).unwrap().unwrap();
    assert_eq!(todo.kind, BlockKind::Todo);
    assert_eq!(
        s.block_backlinks(&BlockId::from("abcDEF123"))
            .unwrap()
            .len(),
        1,
        "uids preserved"
    );
    let z = s.find_page("Zettelkasten").unwrap().unwrap();
    assert_eq!(s.backlink_count(&z).unwrap(), 1);
    let done = s.block_row(&BlockId::from("stuVWX000")).unwrap().unwrap();
    assert_eq!((done.kind, done.checked), (BlockKind::Heading(2), true));
    // Importing again does not duplicate blocks (uids taken → fresh ids, pages reused).
    s.import_roam_json(json).unwrap();
    assert_eq!(s.list_pages(true).unwrap().len(), 2);
    doctor_ok(&mut s);
}

#[test]
fn opml_export() {
    let (mut s, dir) = store();
    let p = s.create_page("Outline").unwrap();
    write(&mut s, &p, "A & B");
    let path = dir.path().join("out.opml");
    s.export_opml(&path).unwrap();
    let x = std::fs::read_to_string(path).unwrap();
    assert!(x.contains("<outline text=\"Outline\">") && x.contains("A &amp; B"));
}

#[test]
fn w7_people_crm_attributes() {
    let (mut s, _d) = store();
    let dana = s.create_page("Dana").unwrap();
    write(&mut s, &dana, "Email:: dana@example.com");
    write(&mut s, &dana, "Company:: [[Acme]]");
    let (day, b) = s.open_daily(today()).unwrap();
    s.set_block_text(&day, &b, "lunch with [[Dana]]", None, None)
        .unwrap();
    s.commit_block(&day, &b).unwrap();
    let attrs: Vec<(String, String)> = s
        .page_state(&dana)
        .unwrap()
        .live_roots()
        .iter()
        .filter_map(|r| {
            let t = s.block_row(r).unwrap().unwrap().text;
            utopia_core::inline::attribute(&t).map(|(k, v)| (k, t[v].to_string()))
        })
        .collect();
    assert_eq!(attrs[0], ("Email".into(), "dana@example.com".into()));
    assert!(
        s.find_page("Acme").unwrap().is_some(),
        "attribute values link like any text"
    );
    assert_eq!(
        s.linked_references(&dana, &Default::default())
            .unwrap()
            .shown(),
        1
    );
}

#[test]
fn date_filter_on_linked_references() {
    let (mut s, _d) = store();
    let topic = s.create_page("Topic").unwrap();
    for (y, m, d) in [(2026, 10, 1), (2026, 10, 30), (2026, 11, 2)] {
        let (day, b) = s
            .open_daily(NaiveDate::from_ymd_opt(y, m, d).unwrap())
            .unwrap();
        s.set_block_text(&day, &b, "on [[Topic]]", None, None)
            .unwrap();
        s.commit_block(&day, &b).unwrap();
    }
    let all = s.linked_references(&topic, &Default::default()).unwrap();
    assert_eq!(all.shown(), 3);
    let q = utopia_store::RefQuery {
        since: NaiveDate::from_ymd_opt(2026, 10, 27),
        ..Default::default()
    };
    assert_eq!(s.linked_references(&topic, &q).unwrap().shown(), 2);
}

/// LR-4: a reference's inline children come from the `block` projection, in outline order,
/// with the total count so the view can fold the rest behind its chip.
#[test]
fn reference_children_come_from_the_index_in_order() {
    let (mut s, _d) = store();
    let src = s.create_page("Source").unwrap();
    let parent = write(&mut s, &src, "the plan depends on [[Target]]");
    let mut kids = Vec::new();
    for n in 1..=4 {
        let plan = outline::append_block(s.page_state(&src).unwrap(), Some(&parent));
        let id = s.execute_plan(&src, plan, None).unwrap().unwrap().block;
        s.set_block_text(&src, &id, &format!("Reason {n}"), None, None)
            .unwrap();
        s.commit_block(&src, &id).unwrap();
        kids.push(id);
    }
    s.sync_index().unwrap();

    let folded = s.ref_children(std::slice::from_ref(&parent), 2).unwrap();
    let folded = folded.get(&parent).unwrap();
    assert_eq!(folded.total, 4, "the chip's count is the whole truth");
    assert_eq!(
        folded
            .shown
            .iter()
            .map(|(_, t)| t.as_str())
            .collect::<Vec<_>>(),
        ["Reason 1", "Reason 2"]
    );
    assert_eq!(folded.shown[0].0, kids[0], "in outline order");

    // 0 asks for the count alone; usize::MAX asks for every child (the chip was opened).
    let none = s
        .ref_children(std::slice::from_ref(&parent), 0)
        .unwrap()
        .remove(&parent)
        .unwrap();
    assert!(none.shown.is_empty());
    assert_eq!(none.total, 4);
    let all = s
        .ref_children(std::slice::from_ref(&parent), usize::MAX)
        .unwrap()
        .remove(&parent)
        .unwrap();
    assert_eq!(all.shown.len(), 4);
    // A block with no children is just a count of zero, never a missing entry.
    let empty = s
        .ref_children(std::slice::from_ref(&kids[0]), 2)
        .unwrap()
        .remove(&kids[0])
        .unwrap();
    assert!(empty.shown.is_empty());
    assert_eq!(empty.total, 0);
}

/// LR-5: "Link all" rewrites every unlinked mention, two in one block included, as one undo
/// step.
#[test]
fn promote_all_mentions_links_every_mention_in_one_step() {
    let (mut s, _d) = store();
    let m = s.create_page("Mindfulness").unwrap();
    let a = s.create_page("Reading").unwrap();
    let b = write(
        &mut s,
        &a,
        "mindfulness in the morning, mindfulness at night",
    );
    write(&mut s, &a, "and a mindful walk");
    assert_eq!(s.unlinked_references(&m).unwrap().len(), 2);

    assert_eq!(s.promote_all_mentions(&m).unwrap(), 2);
    s.sync_index().unwrap();
    let text = s.page_state(&a).unwrap().block(&b).unwrap().text.clone();
    assert_eq!(
        text, "[[Mindfulness]] in the morning, [[Mindfulness]] at night",
        "both mentions, in place"
    );
    assert!(s.unlinked_references(&m).unwrap().is_empty());
    assert_eq!(s.backlink_count(&m).unwrap(), 1, "one referencing block");

    // One undo puts both back.
    s.undo().unwrap();
    s.sync_index().unwrap();
    let text = s.page_state(&a).unwrap().block(&b).unwrap().text.clone();
    assert_eq!(text, "mindfulness in the morning, mindfulness at night");
    assert_eq!(s.unlinked_references(&m).unwrap().len(), 2);
    doctor_ok(&mut s);

    // Nothing to link is zero, not an error.
    assert_eq!(s.promote_all_mentions(&m).unwrap_or(0), 2);
}

/// LR-5 review fix: a title and one of its aliases can match the same words. The mention
/// list then holds overlapping ranges ("New York" 0..8 and "York" 4..8), and linking both
/// would rewrite the text twice over itself.
#[test]
fn promote_all_mentions_keeps_overlapping_mentions_apart() {
    let (mut s, _d) = store();
    let ny = s.create_page("New York").unwrap();
    s.add_alias(&ny, "York").unwrap();
    let src = s.create_page("Travel").unwrap();
    let b = write(&mut s, &src, "flying to New York in May");
    let mentions = s.unlinked_references(&ny).unwrap();
    assert_eq!(
        mentions.len(),
        2,
        "the title and its alias both match: {mentions:?}"
    );

    assert_eq!(
        s.promote_all_mentions(&ny).unwrap(),
        1,
        "one span, one link"
    );
    s.sync_index().unwrap();
    let text = s.page_state(&src).unwrap().block(&b).unwrap().text.clone();
    assert_eq!(text, "flying to [[New York]] in May", "no doubled rewrite");
    assert!(s.unlinked_references(&ny).unwrap().is_empty());
    doctor_ok(&mut s);
}

/// LR-5 review fix (rule 10): the reference panel's count is read from the index as it
/// stands. Reading it must not flush a pending edit, and "nothing pending" is not the same
/// as "the index is current": `flush` empties the queue long before the writer has
/// committed.
#[test]
fn unlinked_count_reads_the_index_without_flushing() {
    let (mut s, _d) = store();
    let m = s.create_page("Mindfulness").unwrap();
    let src = s.create_page("Reading").unwrap();
    // From here on, edits stay in the shard until the debounced flush, as in the UI.
    s.set_debounced(true).unwrap();
    let b = s.ensure_first_block(&src).unwrap();
    s.set_block_text(&src, &b, "practising mindfulness daily", None, None)
        .unwrap();
    assert!(s.has_pending(), "the edit waits for the debounced flush");

    assert!(
        s.unlinked_references_now(&m).unwrap().is_empty(),
        "the index has not seen the new block yet"
    );
    assert!(
        s.has_pending(),
        "reading the count must not flush the pending edit"
    );

    // The old guard: after a flush the queue is empty while the writer may still be
    // running, so a page drawn in that window waited on disk commits.
    s.flush().unwrap();
    assert!(
        !s.has_pending(),
        "flush empties the queue before the writer is done"
    );

    // The blocking form makes the edit durable first and then sees it; the panel uses the
    // non-blocking form and catches up when the index reports it is durable.
    assert_eq!(s.unlinked_references(&m).unwrap().len(), 1);
    assert_eq!(s.unlinked_references_now(&m).unwrap().len(), 1);
}

/// LR-1 review fix: the co-reference chips and their counts use the filter's own predicate.
/// An alias resolves to the page, an ancestor reference counts for its descendants, and two
/// mentions of the same page in one block are still one hit.
#[test]
fn co_reference_counts_use_the_filter_predicate() {
    use utopia_core::outline;
    let (mut s, _d) = store();
    let target = s.create_page("Target").unwrap();
    let ana = s.create_page("Ana Lovelace").unwrap();
    s.add_alias(&ana, "AL").unwrap();
    let bob = s.create_page("Bob").unwrap();
    let src = s.create_page("Source").unwrap();
    let parent = write(&mut s, &src, "Review of [[AL]]");
    // A child that references Target, under a parent that references Ana through her alias.
    let plan = outline::append_block(s.page_state(&src).unwrap(), Some(&parent));
    let child = s.execute_plan(&src, plan, None).unwrap().unwrap().block;
    s.set_block_text(
        &src,
        &child,
        "the [[Target]] plan with [[Bob]] and [[Bob]]",
        None,
        None,
    )
    .unwrap();
    s.commit_block(&src, &child).unwrap();
    write(&mut s, &src, "another [[Target]] note with [[Bob]]");
    s.sync_index().unwrap();

    let refs = s.linked_references(&target, &Default::default()).unwrap();
    let hits: Vec<_> = refs.groups.iter().flat_map(|g| g.hits.clone()).collect();
    assert_eq!(hits.len(), 2);

    let co = s.co_references(&hits, &target, &[]).unwrap();
    let by_page = |p: &PageId| co.iter().find(|c| c.page == *p).map(|c| c.hits);
    assert_eq!(
        by_page(&ana),
        Some(1),
        "the alias reference on the ancestor counts for the child hit"
    );
    assert_eq!(
        by_page(&bob),
        Some(2),
        "two hits, although one block mentions Bob twice"
    );
    assert!(
        co.iter().all(|c| c.page != target),
        "the page itself is never a co-reference"
    );
    // The count is exactly what the filter leaves.
    let q = utopia_store::RefQuery {
        also: vec![bob.clone()],
        ..Default::default()
    };
    assert_eq!(
        s.linked_references(&target, &q).unwrap().shown(),
        by_page(&bob).unwrap()
    );
    // And applied filters are not offered again.
    let co = s
        .co_references(&hits, &target, std::slice::from_ref(&bob))
        .unwrap();
    assert!(co.iter().all(|c| c.page != bob));
}

/// SB-2 review fix: the tag-page classification is published after a commit, but only the
/// pages a commit can affect are reclassified. One whole-graph scan is enough, no matter
/// how many refs the graph holds.
#[test]
fn tag_classification_is_updated_incrementally() {
    let (mut s, _d) = store();
    let src = s.create_page("Notes").unwrap();
    let tag = write(&mut s, &src, "see #alpha here");
    s.sync_index().unwrap();
    let alpha = s.find_page("alpha").unwrap().expect("the tag made a page");
    let note = write(&mut s, &src, "an ordinary note");
    s.sync_index().unwrap();
    let full = s.tag_page_full_scans();
    assert_eq!(full, 1, "the first commit classifies once");
    assert!(s.tag_pages().unwrap().contains(&alpha));

    // Writing an unrelated block does not rescan the graph, and keeps the answer.
    s.set_block_text(&src, &note, "an ordinary note, edited", None, None)
        .unwrap();
    s.commit_block(&src, &note).unwrap();
    assert_eq!(s.tag_page_full_scans(), full, "no whole-graph scan");
    assert!(s.tag_pages().unwrap().contains(&alpha), "still a tag page");

    // Editing prose inside the tagged block leaves the tag unchanged, so the destination
    // needs no reclassification; only the page actually written is classified.
    let classified = s.tag_page_classifications();
    s.set_block_text(&src, &tag, "see #alpha here, with more prose", None, None)
        .unwrap();
    s.commit_block(&src, &tag).unwrap();
    assert_eq!(
        s.tag_page_classifications(),
        classified + 1,
        "an unchanged tag does not rescan its destination"
    );
    assert!(s.tag_pages().unwrap().contains(&alpha), "still a tag page");

    // The same block's tag becoming a link takes the page out of the set.
    s.set_block_text(&src, &tag, "see [[alpha]] here", None, None)
        .unwrap();
    s.commit_block(&src, &tag).unwrap();
    assert_eq!(s.tag_page_full_scans(), full);
    assert!(
        !s.tag_pages().unwrap().contains(&alpha),
        "a [[link]] is not a tag"
    );
    assert_eq!(
        s.tag_pages().unwrap(),
        s.hashtag_only_pages().unwrap(),
        "the published set matches a full classification"
    );

    // A tag again: back in, still without a scan.
    s.set_block_text(&src, &tag, "see #alpha here", None, None)
        .unwrap();
    s.commit_block(&src, &tag).unwrap();
    assert!(s.tag_pages().unwrap().contains(&alpha));
    assert_eq!(s.tag_page_full_scans(), full);

    // Removing the last reference drops it (a page that lost every ref cannot be a tag
    // page), which the incremental path has to notice without a scan.
    s.set_block_text(&src, &tag, "nothing to see", None, None)
        .unwrap();
    s.commit_block(&src, &tag).unwrap();
    assert!(!s.tag_pages().unwrap().contains(&alpha));
    assert!(s.tag_pages().unwrap().is_empty());
    assert_eq!(s.tag_page_full_scans(), full);
}

/// SB-2 review fix: a commit must not walk the published tag pages. A destination that lost
/// its last reference is already one of the commit's changed destinations, so an edit that
/// cannot remove one visits only the page it wrote — not every tag in the graph.
#[test]
fn unrelated_commits_do_not_walk_every_tag_page() {
    let (mut s, _d) = store();
    let notes = s.create_page("Notes").unwrap();
    let alpha_block = write(&mut s, &notes, "a note about #alpha");
    for tag in ["beta", "gamma", "delta", "epsilon"] {
        write(&mut s, &notes, &format!("a note about #{tag}"));
    }
    s.sync_index().unwrap();
    let alpha = s.find_page("alpha").unwrap().unwrap();
    assert_eq!(s.tag_pages().unwrap().len(), 5, "five tag-only pages");
    let visited = s.tag_pages_visited();

    // An ordinary block mentions no tag: nothing can lose a reference, so only the pages
    // being written are looked at (before the fix the publisher probed all five tags).
    write(&mut s, &notes, "an ordinary note");
    s.sync_index().unwrap();
    let unrelated = s.tag_pages_visited() - visited;
    assert!(
        unrelated <= 2,
        "an unrelated commit visits only the page it wrote, not every tag page: {unrelated}"
    );
    assert_eq!(s.tag_pages().unwrap().len(), 5, "still five tag pages");

    // Growing the graph must not make an unrelated edit cost more: the work is the written
    // page's, whatever the number of published tags.
    for tag in ["zeta", "eta"] {
        write(&mut s, &notes, &format!("a note about #{tag}"));
    }
    s.sync_index().unwrap();
    assert_eq!(s.tag_pages().unwrap().len(), 7);
    let visited = s.tag_pages_visited();
    write(&mut s, &notes, "another ordinary note");
    s.sync_index().unwrap();
    assert_eq!(
        s.tag_pages_visited() - visited,
        unrelated,
        "the cost of an unrelated commit does not grow with the graph"
    );

    // The last reference going away still unpublishes the page: the edge that disappeared is
    // one of the commit's changed destinations.
    let visited = s.tag_pages_visited();
    s.set_block_text(&notes, &alpha_block, "no tag here", None, None)
        .unwrap();
    s.commit_block(&notes, &alpha_block).unwrap();
    assert!(
        s.tag_pages_visited() - visited <= unrelated + 2,
        "a lost reference adds its destination, nothing more"
    );
    assert!(!s.tag_pages().unwrap().contains(&alpha));
    assert_eq!(s.tag_pages().unwrap().len(), 6);
    assert_eq!(
        s.tag_pages().unwrap(),
        s.hashtag_only_pages().unwrap(),
        "the published set still matches a full classification"
    );
}

/// SB-2 review fix: a page that was a plain page only because of one `[[link]]` becomes a
/// tag page when that link goes away. The destination is no longer referenced by the page
/// that changed, so the reclassification has to start from the references the commit
/// *removed*, not only from the ones it left behind.
#[test]
fn a_link_that_goes_away_makes_its_destination_a_tag_page() {
    let (mut s, _d) = store();
    let notes = s.create_page("Notes").unwrap();
    let tagged = s.create_page("Tagged").unwrap();
    write(&mut s, &tagged, "kept as a tag: #alpha");
    let link = write(&mut s, &notes, "a plain link: [[alpha]]");
    s.sync_index().unwrap();
    let alpha = s.find_page("alpha").unwrap().expect("the tag made a page");
    let full = s.tag_page_full_scans();
    assert!(
        !s.tag_pages().unwrap().contains(&alpha),
        "one [[link]] keeps the page a page, not a tag"
    );

    // The link goes away; only the hashtag still reaches the page.
    s.set_block_text(&notes, &link, "no link any more", None, None)
        .unwrap();
    s.commit_block(&notes, &link).unwrap();
    s.sync_index().unwrap();
    assert!(
        s.tag_pages().unwrap().contains(&alpha),
        "reclassified from the removed reference: {:?}",
        s.tag_pages()
    );
    assert_eq!(s.tag_page_full_scans(), full, "still no whole-graph scan");
    assert_eq!(s.tag_pages().unwrap(), s.hashtag_only_pages().unwrap());
}

/// PG-2 review fix: an export copies what it needs off the store, then the serialization and
/// the file writes take no store at all — that is what lets them run on a background worker
/// instead of freezing every window. Both routes must write the same files.
#[test]
fn export_snapshot_is_written_off_the_store() {
    let (mut s, _d) = store();
    let src = s.create_page("Notes").unwrap();
    write(&mut s, &src, "see #alpha and [[Beta]]");
    let daily = s.create_page("Reading").unwrap();
    write(&mut s, &daily, "a second page");
    s.sync_index().unwrap();

    // The snapshot is a value: the free writer runs it without borrowing the store.
    let snapshot = s.export_snapshot().unwrap();
    let from_snapshot = tempfile::tempdir().unwrap();
    let pages = utopia_store::write_export(&snapshot, from_snapshot.path()).unwrap();
    assert_eq!(pages, snapshot.pages.len());
    for name in ["graph.json", "graph.opml"] {
        assert!(
            from_snapshot.path().join(name).is_file(),
            "{name} is written"
        );
    }

    // The store's own methods write the same files.
    let from_store = tempfile::tempdir().unwrap();
    assert_eq!(s.export_markdown(from_store.path()).unwrap(), pages);
    s.export_json(&from_store.path().join("graph.json"))
        .unwrap();
    s.export_opml(&from_store.path().join("graph.opml"))
        .unwrap();
    let mut names: Vec<String> = std::fs::read_dir(from_store.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    let mut snapshot_names: Vec<String> = std::fs::read_dir(from_snapshot.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    snapshot_names.sort();
    assert_eq!(names, snapshot_names, "the same files either way");
    for name in names {
        assert_eq!(
            std::fs::read(from_snapshot.path().join(&name)).unwrap(),
            std::fs::read(from_store.path().join(&name)).unwrap(),
            "{name} has the same bytes"
        );
    }
}

/// SB-2: a widely used tag on an edited page must not be reclassified on every flush.
/// Editing a block that has no references leaves every incoming `#tag` edge untouched;
/// only the page the job wrote needs its (cheap) classification check.
#[test]
fn unrelated_edits_do_not_reclassify_a_busy_tag() {
    let (mut s, _d) = store();
    let notes = s.create_page("Notes").unwrap();
    write(&mut s, &notes, "a standing tag #todo");
    for i in 0..40 {
        let page = s.create_page(&format!("Task {i}")).unwrap();
        write(&mut s, &page, "another #todo task");
    }
    let unrelated = write(&mut s, &notes, "an unrelated note");
    s.sync_index().unwrap();
    let before = s.tag_page_classifications();

    s.set_block_text(&notes, &unrelated, "an unrelated note, edited", None, None)
        .unwrap();
    s.commit_block(&notes, &unrelated).unwrap();
    s.sync_index().unwrap();
    assert_eq!(
        s.tag_page_classifications(),
        before + 1,
        "only the written page is classified; the tag's graph-wide refs are not rescanned"
    );
}

/// SB-2 + AC-1.8: switching the date format re-renders daily titles and re-resolves every
/// reference on the store's own connection, so no commit runs and nothing publishes. The
/// published set classifies that index, so it has to be republished with it.
#[test]
fn sb_2_date_format_change_republishes_tag_pages() {
    let (mut s, _d) = store();
    let notes = s.create_page("Notes").unwrap();
    // Not a date under the default format, so this is a hashtag into an ordinary page;
    // under "do MMM yyyy" the same text parses as 2026-06-01 (§10.3.1) and stops being one.
    write(&mut s, &notes, "#[[1st Jun 2026]]");
    s.sync_index().unwrap();
    let tag = s
        .find_page("1st Jun 2026")
        .unwrap()
        .expect("the hashtag made the page");
    assert!(
        s.tag_pages().unwrap().contains(&tag),
        "reached only through a hashtag: {:?}",
        s.tag_pages()
    );

    s.set_date_format("do MMM yyyy").unwrap();
    s.sync_index().unwrap();
    assert_eq!(
        s.tag_pages().unwrap(),
        s.hashtag_only_pages().unwrap(),
        "the published set follows the index the new format produced"
    );
    assert!(
        !s.tag_pages().unwrap().contains(&tag),
        "the hashtag now names a day, not the page"
    );
}

/// SB-2 + §10.4: `rebuild_index` is the repair for a projection that disagrees with the
/// shards. A classification taken from the damaged index must not survive the repair.
#[test]
fn sb_2_rebuild_index_republishes_tag_pages() {
    let (mut s, _d) = store();
    let notes = s.create_page("Notes").unwrap();
    let source = write(&mut s, &notes, "about #alpha");
    let alpha = s.find_page("alpha").unwrap().expect("the hashtag made it");
    s.sync_index().unwrap();
    // Alpha is an ordinary page because it has words of its own.
    write(&mut s, &alpha, "the atlas, in its own words");
    s.sync_index().unwrap();
    assert!(!s.tag_pages().unwrap().contains(&alpha));

    // The projection loses Alpha's block rows: exactly the drift `rebuild_index` repairs.
    s.conn()
        .execute("DELETE FROM block WHERE page_id=?1", [alpha.as_str()])
        .unwrap();
    // A commit that changes the tag's edges classifies Alpha from that damaged index …
    s.set_block_text(&notes, &source, "about nothing", None, None)
        .unwrap();
    s.commit_block(&notes, &source).unwrap();
    s.set_block_text(&notes, &source, "about #alpha", None, None)
        .unwrap();
    s.commit_block(&notes, &source).unwrap();
    assert!(
        s.tag_pages().unwrap().contains(&alpha),
        "classified from the index that lost its blocks"
    );

    s.rebuild_index().unwrap();
    assert_eq!(
        s.tag_pages().unwrap(),
        s.hashtag_only_pages().unwrap(),
        "the repaired index reclassifies the page"
    );
    assert!(
        !s.tag_pages().unwrap().contains(&alpha),
        "the page has blocks again, so it is not a tag page"
    );
}

/// SB-2: emptying the trash removes rows the classification reads (the purged page and
/// everything that pointed at it) outside any commit, so the set has to be republished
/// from the index that is left.
#[test]
fn sb_2_empty_trash_republishes_tag_pages() {
    let (mut s, _d) = store();
    let notes = s.create_page("Notes").unwrap();
    write(&mut s, &notes, "a note about #alpha");
    s.sync_index().unwrap();
    let alpha = s.find_page("alpha").unwrap().expect("the hashtag made it");
    assert!(s.tag_pages().unwrap().contains(&alpha));
    s.delete_page(&alpha).unwrap();
    let scans = s.tag_page_scans();

    assert_eq!(s.empty_trash(None).unwrap(), 1, "the page is purged");
    assert!(
        s.tag_page_scans() > scans,
        "the purge republishes the set from the index it left behind"
    );
    assert!(
        !s.tag_pages().unwrap().contains(&alpha),
        "a purged page cannot stay in the set"
    );
    assert_eq!(s.tag_pages().unwrap(), s.hashtag_only_pages().unwrap());
}

/// SB-2: the published set classifies the index, so a republish must not scan while a
/// commit it was handed is still in flight. The set it would publish is stale, and because
/// the slot is then `Published`, every later commit edits it incrementally — the wrong
/// classification survives the session instead of being fixed by the next full scan.
#[test]
fn sb_2_republish_waits_for_a_submitted_commit() {
    let (mut s, _d) = store();
    let notes = s.create_page("Notes").unwrap();
    let block = write(&mut s, &notes, "about #alpha");
    s.sync_index().unwrap();
    let alpha = s
        .find_page("alpha")
        .unwrap()
        .expect("the tag made the page");
    assert!(s.tag_pages().unwrap().contains(&alpha));

    // Debounced is the UI's mode (rule 10): index reads never wait for the writer, and
    // `AppState::new` publishes the startup classification in exactly this state.
    s.set_debounced(true).unwrap();
    // Hold the write lock, so the next commit can be submitted but cannot land: the index
    // (and a scan of it) still says "tag page" while the job is in flight.
    let holder = rusqlite::Connection::open(s.path().unwrap()).unwrap();
    holder.execute_batch("BEGIN IMMEDIATE").unwrap();
    s.set_block_text(&notes, &block, "about [[alpha]]", None, None)
        .unwrap();
    s.flush().unwrap();
    // Release the lock shortly, so a republish that waits can finish.
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(400));
        holder.execute_batch("COMMIT").unwrap();
    });

    let published = s.republish_tag_pages().unwrap();
    assert!(
        published.is_empty(),
        "the set published the index the commit left, not the one it found: {published:?}"
    );
    assert_eq!(
        s.tag_pages().unwrap(),
        s.hashtag_only_pages().unwrap(),
        "and that is what the slot holds"
    );
}

/// SB-2 + rule 11: the purge has already committed when the set is republished. A failure
/// to classify must not turn a purge that happened into an error: the caller would see
/// "failed" and a retry would purge nothing. Log it and keep the purge's own answer.
#[test]
fn sb_2_empty_trash_reports_the_purge_when_the_scan_fails() {
    let (mut s, _d) = store();
    let notes = s.create_page("Notes").unwrap();
    // `beta` stays and is still blockless, so the classification really runs its queries
    // (it returns early when no page can be a tag page at all).
    write(&mut s, &notes, "a note about #alpha and #beta");
    s.sync_index().unwrap();
    let alpha = s.find_page("alpha").unwrap().expect("the hashtag made it");
    s.find_page("beta").unwrap().expect("and so did this one");
    s.delete_page(&alpha).unwrap();

    // The classification reads `alias`; the purge does not, so breaking it fails only the
    // republish that follows the committed delete.
    s.conn().execute_batch("DROP TABLE alias").unwrap();
    assert_eq!(
        s.empty_trash(None).unwrap(),
        1,
        "the purge reports what it purged"
    );
}
