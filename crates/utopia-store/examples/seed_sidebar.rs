//! Throwaway seeder for the left-sidebar verification shots (not committed). Drives the
//! Store directly, so the sidebar has deterministic content: pages, tag-only pages, six
//! days of notes and two pins.
//!
//! `cargo run -p utopia-store --example seed_sidebar -- <graph> <config dir>`

use std::path::Path;

use utopia_core::{BlockId, BlockKind, Command, PageId};
use utopia_store::{EditOpts, Store};

type Rows<'a> = &'a [(usize, BlockKind, &'a str)];

fn insert_tree(s: &mut Store, page: &PageId, rows: Rows<'_>) {
    let mut cmds = Vec::new();
    let mut stack: Vec<BlockId> = Vec::new();
    for &(depth, kind, text) in rows {
        let id = BlockId::new();
        let parent = depth.checked_sub(1).map(|d| stack[d].clone());
        let after = stack.get(depth).cloned();
        cmds.push(Command::InsertBlock {
            id: id.clone(),
            parent,
            after,
            text: text.to_string(),
            kind,
        });
        stack.truncate(depth);
        stack.push(id);
    }
    s.execute(
        page,
        cmds,
        EditOpts {
            no_undo: true,
            ..Default::default()
        },
    )
    .unwrap();
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let graph = Path::new(&args[1]);
    let config = Path::new(&args[2]);
    std::fs::create_dir_all(config).unwrap();
    let _ = std::fs::remove_file(graph);
    let mut s = Store::open(graph).unwrap();
    let today = s.today();

    let atlas = s.create_page("Atlas Launch").unwrap();
    insert_tree(
        &mut s,
        &atlas,
        &[
            (0, BlockKind::Bullet, "status:: in progress"),
            (0, BlockKind::Bullet, "owner:: [[Ana Lovelace]]"),
            (0, BlockKind::Heading(2), "Scope"),
            (
                1,
                BlockKind::Bullet,
                "A beta for twenty testers, invited by hand",
            ),
            (1, BlockKind::Bullet, "Import from the old format"),
            (1, BlockKind::Todo, "Draft the release notes"),
            (0, BlockKind::Heading(2), "Risks"),
            (
                1,
                BlockKind::Bullet,
                "The beta date depends on [[Ana Lovelace]]'s review",
            ),
            (1, BlockKind::Quote, "Ship the small thing that works."),
        ],
    );
    let ana = s.create_page("Ana Lovelace").unwrap();
    insert_tree(
        &mut s,
        &ana,
        &[
            (0, BlockKind::Bullet, "role:: reviewer on [[Atlas Launch]]"),
            (0, BlockKind::Bullet, "Prefers a written summary"),
        ],
    );
    let reading = s.create_page("Reading").unwrap();
    insert_tree(
        &mut s,
        &reading,
        &[
            (0, BlockKind::Bullet, "A list of what is worth finishing"),
            (
                0,
                BlockKind::Bullet,
                "On conflict-free replicated data types",
            ),
            (0, BlockKind::Todo, "Finish the chapter on merge semantics"),
        ],
    );
    // Tag pages: live pages with no blocks of their own (#atlas, #beta in today's note).
    let _atlas_tag = s.create_page("atlas").unwrap();
    let _beta_tag = s.create_page("beta").unwrap();

    let days: &[(i64, Rows<'_>)] = &[
        (
            5,
            &[
                (
                    0,
                    BlockKind::Bullet,
                    "Started sketching the [[Atlas Launch]] scope",
                ),
                (1, BlockKind::Bullet, "Three pages of notes, one diagram"),
                (0, BlockKind::Bullet, "[[Reading]] — two chapters on merges"),
            ],
        ),
        (
            3,
            &[
                (
                    0,
                    BlockKind::Bullet,
                    "Old files have to keep opening, no exceptions",
                ),
                (0, BlockKind::Todo, "Ask [[Ana Lovelace]] to review it"),
            ],
        ),
        (
            1,
            &[
                (
                    0,
                    BlockKind::Bullet,
                    "Wrote up the import plan for [[Atlas Launch]]",
                ),
                (1, BlockKind::Bullet, "Migration notes in the shared folder"),
                (
                    0,
                    BlockKind::Bullet,
                    "[[Reading]] — finished the CRDT chapter",
                ),
                (0, BlockKind::Todo, "Send [[Ana Lovelace]] the notes"),
            ],
        ),
        (
            0,
            &[
                (
                    0,
                    BlockKind::Bullet,
                    "Met [[Ana Lovelace]] about the [[Atlas Launch]] timeline",
                ),
                (1, BlockKind::Bullet, "She wants beta testers this month"),
                (0, BlockKind::Bullet, "[[Reading]] — started the appendix"),
                (0, BlockKind::Bullet, "#atlas #beta"),
            ],
        ),
    ];
    for &(back, rows) in days {
        let date = today - chrono::Duration::days(back);
        let page = s.ensure_daily_page(date).unwrap();
        insert_tree(&mut s, &page, rows);
    }
    s.sync_index().unwrap();

    let prefs = format!(
        "{{\n  \"version\": 1,\n  \"font\": \"system\",\n  \"density\": \"normal\",\n  \"pinned\": [\"{}\", \"{}\"]\n}}\n",
        atlas.as_str(),
        reading.as_str()
    );
    std::fs::write(config.join("settings.json"), prefs).unwrap();
    println!(
        "seeded {}\n  atlas={} reading={}",
        graph.display(),
        atlas.as_str(),
        reading.as_str()
    );
}
