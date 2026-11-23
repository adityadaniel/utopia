use super::*;
use utopia_core::command::TextEdit;

/// Small deterministic PRNG so the fuzz tests are reproducible without extra deps.
pub(crate) struct Rng(u64);

impl Rng {
    pub(crate) fn new(seed: u64) -> Self {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }
    pub(crate) fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    pub(crate) fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
    pub(crate) fn pick<'a, T>(&mut self, v: &'a [T]) -> Option<&'a T> {
        if v.is_empty() {
            None
        } else {
            Some(&v[self.below(v.len())])
        }
    }
}

const WORDS: &[&str] = &["alpha ", "β ", "日本 ", "🎉", "[[Link]] ", "#tag ", "x"];

/// A random command that is valid against `st`.
pub(crate) fn random_command(st: &PageState, rng: &mut Rng, n: &mut u64) -> Command {
    let all: Vec<BlockId> = st
        .blocks
        .values()
        .filter(|b| b.present)
        .map(|b| b.id.clone())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    loop {
        match rng.below(8) {
            0 | 1 => {
                *n += 1;
                let parent = if rng.below(3) == 0 {
                    None
                } else {
                    rng.pick(&all).cloned()
                };
                let sibs = st.children_of(parent.as_ref()).to_vec();
                let after = if rng.below(3) == 0 {
                    None
                } else {
                    rng.pick(&sibs).cloned()
                };
                return Command::InsertBlock {
                    id: BlockId(format!("B{n:05}")),
                    parent,
                    after,
                    text: WORDS[rng.below(WORDS.len())].to_string(),
                    kind: BlockKind::Bullet,
                };
            }
            2 | 3 => {
                let Some(id) = rng.pick(&all).cloned() else {
                    continue;
                };
                let text = &st.block(&id).unwrap().text;
                let bounds: Vec<usize> = (0..=text.len())
                    .filter(|i| text.is_char_boundary(*i))
                    .collect();
                let a = *rng.pick(&bounds).unwrap();
                let b = *rng.pick(&bounds).unwrap();
                let (s, e) = (a.min(b), a.max(b));
                return Command::SetBlockText {
                    id,
                    edit: TextEdit {
                        range: s..e,
                        insert: WORDS[rng.below(WORDS.len())].to_string(),
                    },
                };
            }
            4 => {
                let Some(id) = rng.pick(&all).cloned() else {
                    continue;
                };
                let candidates: Vec<BlockId> = all
                    .iter()
                    .filter(|p| **p != id && !st.is_ancestor(&id, p))
                    .cloned()
                    .collect();
                let parent = if rng.below(3) == 0 {
                    None
                } else {
                    rng.pick(&candidates).cloned()
                };
                let sibs: Vec<BlockId> = st
                    .children_of(parent.as_ref())
                    .iter()
                    .filter(|s| **s != id)
                    .cloned()
                    .collect();
                let after = if rng.below(3) == 0 {
                    None
                } else {
                    rng.pick(&sibs).cloned()
                };
                return Command::MoveBlock { id, parent, after };
            }
            5 => {
                let Some(id) = rng.pick(&all).cloned() else {
                    continue;
                };
                let del = st.block(&id).unwrap().deleted_at.is_none();
                return Command::SetDeleted {
                    ids: vec![id],
                    deleted_at: del.then_some(42),
                };
            }
            6 => {
                let Some(id) = rng.pick(&all).cloned() else {
                    continue;
                };
                return Command::SetBlockKind {
                    id,
                    kind: [BlockKind::Bullet, BlockKind::Todo, BlockKind::Heading(2)][rng.below(3)],
                    checked: rng.below(2) == 0,
                };
            }
            _ => {
                let Some(id) = rng.pick(&all).cloned() else {
                    continue;
                };
                if rng.below(2) == 0 {
                    return Command::PurgeBlock { id };
                }
                return Command::SetPageMeta(PageMetaPatch {
                    title: Some(format!("Title {}", rng.below(5))),
                    aliases: Some(vec![format!("alias {}", rng.below(5))]),
                    ..Default::default()
                });
            }
        }
    }
}

fn page() -> PageId {
    PageId::from("page-1")
}

#[test]
fn loro_matches_reference_semantics() {
    for seed in 1..=20 {
        let mut rng = Rng::new(seed);
        let mut doc = LoroPageDoc::new(page(), 1);
        let mut model = PageState::new(page(), PageMeta::default());
        let mut n = 0;
        for step in 0..150 {
            let cmd = random_command(&model, &mut rng, &mut n);
            let m = model.apply(&cmd, step);
            let d = doc.apply(&cmd, step);
            assert_eq!(
                m.is_ok(),
                d.is_ok(),
                "seed {seed} step {step}: {cmd:?} {m:?} {d:?}"
            );
            assert_eq!(
                doc.state().canonical(),
                model.canonical(),
                "seed {seed} step {step}: {cmd:?}"
            );
            // The incrementally maintained cache equals a full rebuild from the CRDT,
            // including order, positions and liveness.
            let rebuilt = doc.rebuilt_state();
            assert_eq!(
                doc.state().canonical(),
                rebuilt.canonical(),
                "seed {seed} step {step}"
            );
            for (id, b) in &rebuilt.blocks {
                let c = doc.state().block(id).expect("same blocks");
                assert_eq!(
                    (&c.position, c.live, c.present, &c.parent),
                    (&b.position, b.live, b.present, &b.parent),
                    "seed {seed} step {step} block {id}"
                );
            }
        }
        // Liveness must agree too.
        for (id, b) in &model.blocks {
            assert_eq!(doc.state().block(id).map(|x| x.live), Some(b.live));
        }
    }
}

#[test]
fn inverses_restore_state_in_loro() {
    let mut rng = Rng::new(7);
    let mut doc = LoroPageDoc::new(page(), 1);
    let mut n = 0;
    let mut history = Vec::new();
    for step in 0..120 {
        let cmd = random_command(doc.state(), &mut rng, &mut n);
        let before = doc.state().canonical();
        let Ok(inv) = cmd.invert(doc.state()) else {
            continue;
        };
        if doc.apply(&cmd, step).is_ok() {
            history.push((before, inv));
        }
    }
    // AC-2.3 in miniature: undo everything and land on the empty page, byte-identical
    // canonical projection at every step.
    while let Some((before, inv)) = history.pop() {
        for c in &inv {
            doc.apply(c, 1000).unwrap();
        }
        assert_eq!(doc.state().canonical(), before);
    }
}

#[test]
fn snapshot_round_trip_and_positions_unique() {
    let mut rng = Rng::new(3);
    let mut doc = LoroPageDoc::new(page(), 9);
    let mut n = 0;
    for step in 0..100 {
        let cmd = random_command(doc.state(), &mut rng, &mut n);
        let _ = doc.apply(&cmd, step);
    }
    let snap = doc.snapshot().unwrap();
    let back = LoroPageDoc::from_snapshot(page(), &snap, 10).unwrap();
    assert_eq!(back.state().canonical(), doc.state().canonical());
    for b in doc.state().blocks.values() {
        let sibs = doc.state().children_of(b.parent.as_ref());
        let positions: Vec<&str> = sibs
            .iter()
            .map(|s| doc.state().block(s).unwrap().position.as_str())
            .collect();
        let mut sorted = positions.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            positions.len(),
            "positions unique among siblings"
        );
    }
}

fn sync(a: &mut LoroPageDoc, b: &mut LoroPageDoc) {
    let a_to_b = a.export_since(&b.frontier()).unwrap();
    let b_to_a = b.export_since(&a.frontier()).unwrap();
    b.import(&a_to_b).unwrap();
    a.import(&b_to_a).unwrap();
}

#[test]
fn two_peer_convergence_fuzz() {
    for seed in 1..=15 {
        let mut rng = Rng::new(seed * 31);
        let mut a = LoroPageDoc::new(page(), 1);
        let mut n = 0;
        for step in 0..20 {
            let cmd = random_command(a.state(), &mut rng, &mut n);
            let _ = a.apply(&cmd, step);
        }
        let mut b = LoroPageDoc::from_snapshot(page(), &a.snapshot().unwrap(), 2).unwrap();
        for round in 0..5 {
            for step in 0..15 {
                let ca = random_command(a.state(), &mut rng, &mut n);
                let _ = a.apply(&ca, step);
                let cb = random_command(b.state(), &mut rng, &mut n);
                let _ = b.apply(&cb, step);
            }
            sync(&mut a, &mut b);
            assert_eq!(
                a.state().canonical(),
                b.state().canonical(),
                "seed {seed} round {round}"
            );
            // Importing the same updates again is a no-op (idempotent import).
            let again = a.export_since(&Frontier::default()).unwrap();
            assert!(!b.import(&again).unwrap());
        }
    }
}

#[test]
fn concurrent_same_block_inserts_both_survive() {
    let mut a = LoroPageDoc::new(page(), 1);
    let id = BlockId::from("blk");
    a.apply(
        &Command::InsertBlock {
            id: id.clone(),
            parent: None,
            after: None,
            text: "hello world".into(),
            kind: BlockKind::Bullet,
        },
        0,
    )
    .unwrap();
    let mut b = LoroPageDoc::from_snapshot(page(), &a.snapshot().unwrap(), 2).unwrap();
    a.apply(
        &Command::SetBlockText {
            id: id.clone(),
            edit: TextEdit {
                range: 5..5,
                insert: " big".into(),
            },
        },
        1,
    )
    .unwrap();
    b.apply(
        &Command::SetBlockText {
            id: id.clone(),
            edit: TextEdit {
                range: 11..11,
                insert: "!".into(),
            },
        },
        1,
    )
    .unwrap();
    sync(&mut a, &mut b);
    assert_eq!(a.state().block(&id).unwrap().text, "hello big world!");
    assert_eq!(b.state().block(&id).unwrap().text, "hello big world!");
}

#[test]
fn duplicate_block_ids_resolve_to_one_node() {
    let mut a = LoroPageDoc::new(page(), 1);
    let mut b = LoroPageDoc::new(page(), 2);
    let insert = |parent: Option<&str>, id: &str, text: &str| Command::InsertBlock {
        id: id.into(),
        parent: parent.map(Into::into),
        after: None,
        text: text.into(),
        kind: BlockKind::Bullet,
    };
    a.apply(&insert(None, "dup", "same"), 0).unwrap();
    a.apply(&insert(Some("dup"), "ca", "child a"), 0).unwrap();
    b.apply(&insert(None, "dup", "same"), 0).unwrap();
    b.apply(&insert(Some("dup"), "cb", "child b"), 0).unwrap();
    sync(&mut a, &mut b);
    assert_eq!(a.state().canonical(), b.state().canonical());
    let roots = a.state().live_roots();
    assert_eq!(roots, vec![BlockId::from("dup")]);
    let kids = a.state().live_children(Some(&"dup".into()));
    assert_eq!(kids.len(), 2, "loser's children adopted by the winner");
}

#[test]
fn concurrent_renames_keep_loser_as_alias() {
    let mut a = LoroPageDoc::new(page(), 1);
    a.apply(
        &Command::SetPageMeta(PageMetaPatch {
            title: Some("Alpha".into()),
            ..Default::default()
        }),
        0,
    )
    .unwrap();
    let mut b = LoroPageDoc::from_snapshot(page(), &a.snapshot().unwrap(), 2).unwrap();
    let rename = |t: &str| {
        Command::SetPageMeta(PageMetaPatch {
            title: Some(t.into()),
            aliases: Some(vec!["Alpha".into()]),
            ..Default::default()
        })
    };
    a.apply(&rename("Alpha Prime"), 1).unwrap();
    b.apply(&rename("Alpha Two"), 1).unwrap();
    sync(&mut a, &mut b);
    let meta = &a.state().meta;
    assert_eq!(meta, &b.state().meta);
    let mut all = meta.aliases.clone();
    all.push(meta.title.clone());
    all.sort();
    assert_eq!(all, vec!["Alpha", "Alpha Prime", "Alpha Two"]);
}

#[test]
fn frontier_round_trips() {
    let mut a = LoroPageDoc::new(page(), 1);
    a.apply(
        &Command::InsertBlock {
            id: "x".into(),
            parent: None,
            after: None,
            text: "t".into(),
            kind: BlockKind::Bullet,
        },
        0,
    )
    .unwrap();
    let f = a.frontier();
    assert_eq!(Frontier::from_vv(&f.to_vv().unwrap()), f);
    assert!(Frontier("zz".into()).to_vv().is_err());
}

#[test]
fn incremental_cache_matches_rebuild_under_dense_inserts() {
    // Many inserts at the same spot force Loro to rearrange sibling indexes.
    let mut doc = LoroPageDoc::new(page(), 1);
    let first = BlockId::from("first");
    doc.apply(
        &Command::InsertBlock {
            id: first.clone(),
            parent: None,
            after: None,
            text: String::new(),
            kind: BlockKind::Bullet,
        },
        0,
    )
    .unwrap();
    for i in 0..400 {
        let after = if i % 3 == 0 {
            None
        } else {
            Some(first.clone())
        };
        doc.apply(
            &Command::InsertBlock {
                id: BlockId(format!("n{i:04}")),
                parent: None,
                after,
                text: String::new(),
                kind: BlockKind::Bullet,
            },
            0,
        )
        .unwrap();
    }
    let rebuilt = doc.rebuilt_state();
    assert_eq!(doc.state().roots, rebuilt.roots);
    for (id, b) in &rebuilt.blocks {
        assert_eq!(doc.state().block(id).unwrap().position, b.position, "{id}");
    }
}

fn insert(id: &str, after: Option<&str>, text: &str) -> Command {
    Command::InsertBlock {
        id: BlockId::from(id),
        parent: None,
        after: after.map(BlockId::from),
        text: text.into(),
        kind: BlockKind::Bullet,
    }
}

/// The relay holds a sender's later changes but not the earlier ones they build on (a
/// device that first pushed to another graph id). `(first, later)` are the two updates.
fn gap() -> (Vec<u8>, Vec<u8>) {
    let mut sender = LoroPageDoc::new(page(), 7);
    sender.apply(&insert("one", None, "first"), 0).unwrap();
    let first = sender.export_since(&Frontier::default()).unwrap();
    let mid = sender.frontier();
    sender
        .apply(&insert("two", Some("one"), "second"), 0)
        .unwrap();
    let later = sender.export_since(&mid).unwrap();
    (first, later)
}

/// §12.3 step 3, §2.3.7: an update whose predecessors never arrived applies nothing, and
/// the doc says so instead of reporting a clean import.
#[test]
fn ac_12_3_import_keeps_changes_that_wait_for_a_missing_change() {
    let (_, later) = gap();
    let mut receiver = LoroPageDoc::new(page(), 9);
    assert!(!receiver.import(&later).unwrap(), "nothing applies yet");
    assert!(receiver.state().blocks.is_empty());
    let waiting = receiver.waiting().unwrap().expect("the update waits");
    let ranges = waiting_ranges(&waiting).unwrap();
    assert_eq!(ranges.len(), 1, "{ranges:?}");
    assert_eq!(ranges[0].peer, 7);
    assert!(ranges[0].start > 0, "it waits for counters below its start");
}

/// The waiting bytes are the only copy: a snapshot does not hold them, so they must
/// survive a reload through the sidecar, and apply when the missing change arrives.
#[test]
fn ac_12_3_waiting_changes_survive_a_snapshot_reload_and_apply_later() {
    let (first, later) = gap();
    let mut receiver = LoroPageDoc::new(page(), 9);
    receiver.import(&later).unwrap();
    let snap = receiver.snapshot().unwrap();
    let sidecar = receiver.waiting().unwrap().expect("the update waits");

    let mut reopened = LoroPageDoc::from_snapshot(page(), &snap, 9).unwrap();
    assert!(
        reopened.waiting().unwrap().is_none(),
        "a snapshot drops them"
    );
    reopened.import(&sidecar).unwrap();
    assert!(reopened.waiting().unwrap().is_some());

    assert!(reopened.import(&first).unwrap());
    assert!(
        reopened.waiting().unwrap().is_none(),
        "nothing waits any more"
    );
    let texts: Vec<_> = reopened
        .state()
        .blocks
        .values()
        .map(|b| b.text.clone())
        .collect();
    assert_eq!(texts.len(), 2, "{texts:?}");
    assert!(texts.contains(&"second".to_string()));
}

/// §12.3 step 4: a destination that holds only the later change has less than the sender,
/// because the change waits; one that holds both has everything.
#[test]
fn ac_12_3_a_destination_holding_only_waiting_changes_has_less() {
    let (first, later) = gap();
    let mut sender = LoroPageDoc::new(page(), 9);
    sender.import(&first).unwrap();
    sender.import(&later).unwrap();
    let mine = sender.frontier();

    let partial = version_of([later.as_slice()]).unwrap();
    assert!(mine.has_more_than(&partial).unwrap());
    let whole = version_of([later.as_slice(), first.as_slice()]).unwrap();
    assert!(!mine.has_more_than(&whole).unwrap());
    assert!(!Frontier::default().has_more_than(&whole).unwrap());
}

/// §12.6 diagnostics: a copy that holds only the later change reports it as waiting and
/// does not count it in its version; with both, nothing waits.
#[test]
fn ac_12_6_copy_state_names_changes_that_wait() {
    let (first, later) = gap();
    let partial = copy_state([later.as_slice()]).unwrap();
    assert_eq!(partial.waiting.len(), 1);
    assert_eq!(partial.waiting[0].peer, 7);
    assert!(partial.version.peers().unwrap().is_empty());
    let whole = copy_state([first.as_slice(), later.as_slice()]).unwrap();
    assert!(whole.waiting.is_empty());
    assert_eq!(whole.version.peers().unwrap().len(), 1);
}
