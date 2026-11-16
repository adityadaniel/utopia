//! The tools against the access setting, and what a read returns (PRD §13.8 MC-1, MC-2,
//! MC-5, MC-7, MC-11; docs/mcp-design.md §4, §4.2, §5).
//!
//! Every call goes through the real `serve`, the real `Direct` backend and a scratch graph
//! under a temporary directory, so the level is enforced where it is in production.

mod support;

use serde_json::{json, Value};
use support::{initialize, start, text_of, tool_call, Graph};
use utopia_api::access::Level;
use utopia_mcp::Options;

/// One `tools/call` over a legacy session, with the graph's access file as given.
fn call_as(graph: &Graph, level: Level, when_closed: bool, name: &str, args: &Value) -> Value {
    graph.set_access(level, when_closed);
    let reply = support::run(
        vec![
            initialize(1, "2025-11-25"),
            tool_call(2, name, args.clone()),
        ],
        graph.backend(),
        Options::new(),
    );
    reply[1].clone()
}

fn call(graph: &Graph, name: &str, args: Value) -> (String, bool) {
    text_of(&call_as(graph, Level::Read, true, name, &args))
}

/// One call per tool with arguments that are valid for it.
fn every_tool() -> Vec<(&'static str, Value, &'static str)> {
    // The last field is a word the successful result must contain.
    vec![
        ("search", json!({"query": "launch"}), "Plan the launch"),
        ("list_pages", json!({}), "Project X"),
        ("read_page", json!({"page": "Project X"}), "Plan the launch"),
        ("read_block", json!({"id": "PLACEHOLDER"}), "rev:"),
        (
            "get_backlinks",
            json!({"page": "Ana"}),
            "Ask [[Ana]] for the copy",
        ),
        ("run_query", json!({"query": "[[Ana]]"}), "Ana"),
    ]
}

fn with_ids(graph: &Graph) -> Vec<(&'static str, Value, &'static str)> {
    every_tool()
        .into_iter()
        .map(|(name, args, want)| {
            let args = if name == "read_block" {
                json!({"id": graph.ana_link})
            } else {
                args
            };
            (name, args, want)
        })
        .collect()
}

/// MC-1: with access Off (as a missing file means) no tool reads or writes, and each one
/// says where the owner changes it.
#[test]
fn mc_1_no_tool_works_with_access_off() {
    let graph = Graph::new();
    let pages_before = graph.page_count();
    for (name, args, _) in with_ids(&graph) {
        let (text, is_error) = text_of(&call_as(&graph, Level::Off, true, name, &args));
        assert!(is_error, "{name}: {text}");
        assert!(text.contains("Settings → Assistants"), "{name}: {text}");
        assert_eq!(
            call_as(&graph, Level::Off, true, name, &args)["error"],
            Value::Null,
            "{name}: a refusal is a result for the model, not a protocol error"
        );
    }
    assert_eq!(graph.page_count(), pages_before, "nothing was written");
}

/// MC-2: at `read` and at `write` every read tool works (the level permits by `Op::kind`).
#[test]
fn mc_2_the_levels_permit_their_ops() {
    let graph = Graph::new();
    for level in [Level::Read, Level::Write] {
        for (name, args, want) in with_ids(&graph) {
            let (text, is_error) = text_of(&call_as(&graph, level, true, name, &args));
            assert!(!is_error, "{name} at {level:?}: {text}");
            assert!(text.contains(want), "{name} at {level:?}: {text}");
        }
    }
}

/// MC-5: with the app closed, `when_closed` off says "Open Utopia" and reads nothing;
/// with it on, the call runs.
#[test]
fn mc_5_a_closed_app_needs_the_second_switch() {
    let graph = Graph::new();
    let before = graph.page_count();
    let (text, is_error) = text_of(&call_as(
        &graph,
        Level::Read,
        false,
        "read_page",
        &json!({"page": "Project X"}),
    ));
    assert!(is_error, "{text}");
    assert!(text.contains("Open Utopia"), "{text}");
    assert_eq!(graph.page_count(), before);

    let (text, is_error) = text_of(&call_as(
        &graph,
        Level::Read,
        true,
        "read_page",
        &json!({"page": "Project X"}),
    ));
    assert!(!is_error, "{text}");
    assert!(text.contains("Plan the launch"), "{text}");
}

/// MC-2: `--read-only` lowers the level for the process. It cannot raise it, and it does
/// not change the tool list until there are write tools to remove (design §5).
#[test]
fn mc_2_read_only_lowers_the_level_and_filters_the_list() {
    let graph = Graph::new();
    graph.set_access(Level::Off, true);
    let readonly = Options {
        read_only: true,
        log: None,
    };
    // It cannot raise Off.
    let reply = support::run(
        vec![
            initialize(1, "2025-11-25"),
            tool_call(2, "list_pages", json!({})),
        ],
        graph.backend(),
        readonly,
    );
    let (text, is_error) = text_of(&reply[1]);
    assert!(is_error);
    assert!(text.contains("Settings → Assistants"), "{text}");

    // With the file at write, read-only still answers the read tools and lists the same
    // six tools, because none of them writes yet.
    graph.set_access(Level::Write, true);
    let reply = support::run(
        vec![
            initialize(1, "2025-11-25"),
            tool_call(2, "list_pages", json!({})),
            support::request(3, "tools/list", json!({})),
        ],
        graph.backend(),
        Options {
            read_only: true,
            log: None,
        },
    );
    assert!(!text_of(&reply[1]).1);
    let list = &reply[2]["result"]["tools"];
    assert_eq!(list.as_array().unwrap().len(), 6);
    assert!(list
        .as_array()
        .unwrap()
        .iter()
        .all(|t| t["annotations"]["readOnlyHint"] == json!(true)));
}

/// MC-7: a read never makes a page. `read_page today` on a graph whose day has no page
/// answers an empty outline, the page count is unchanged and `doctor` is clean (FR-1.2).
#[test]
fn mc_7_a_read_never_makes_a_page() {
    let graph = Graph::new();
    // A graph with no daily page at all.
    let empty = Graph::empty();
    empty.set_access(Level::Read, true);
    let before = empty.page_count();
    let reply = support::run(
        vec![
            initialize(1, "2025-11-25"),
            tool_call(2, "read_page", json!({"page": "today"})),
            tool_call(3, "read_page", json!({"page": "2099-01-01"})),
        ],
        empty.backend(),
        Options::new(),
    );
    for i in [1, 2] {
        let (text, is_error) = text_of(&reply[i]);
        assert!(!is_error, "a day without a page is not an error: {text}");
        assert_eq!(text, "", "an empty outline");
    }
    assert_eq!(empty.page_count(), before, "no page was made");
    let mut store = empty.open();
    let report = store.doctor().expect("doctor runs");
    assert!(report.is_ok(), "doctor: {report:?}");
    assert_eq!(report.pages, before);

    // A day that does have a page reads as its blocks.
    let (text, is_error) = call(&graph, "read_page", json!({"page": "today"}));
    assert!(!is_error, "{text}");
    assert!(text.contains("Met [[Ana]] today"), "{text}");
}

/// MC-7: `read_page` returns an indented outline with a marker for each block, and
/// `read_block` returns one subtree with the `rev` of its text.
#[test]
fn mc_7_read_page_and_read_block_return_marked_outlines() {
    let graph = Graph::new();
    let (text, is_error) = call(&graph, "read_page", json!({"page": "Project X"}));
    assert!(!is_error, "{text}");
    let root_line = text
        .lines()
        .find(|l| l.starts_with("- Plan the launch"))
        .unwrap_or_else(|| panic!("the root block: {text}"));
    assert!(
        root_line.ends_with(&format!("{{id:{}}}", graph.root)),
        "{root_line}"
    );
    let child_line = text
        .lines()
        .find(|l| l.contains("Ask [[Ana]]"))
        .unwrap_or_else(|| panic!("the child block: {text}"));
    assert!(child_line.starts_with("  - "), "one level in: {child_line}");
    assert!(child_line.ends_with('}'), "{child_line}");

    let reply = support::run(
        vec![
            initialize(1, "2025-11-25"),
            tool_call(2, "read_block", json!({"id": graph.root})),
            tool_call(3, "read_block", json!({"id": graph.root})),
            tool_call(4, "read_block", json!({"id": "01J8Z3K9V2AAAAAAAAAAAAAAAA"})),
        ],
        graph.backend(),
        Options::new(),
    );
    let (block, is_error) = text_of(&reply[1]);
    assert!(!is_error, "{block}");
    let (rev_line, outline) = block
        .split_once("\n\n")
        .expect("a rev line, then the outline");
    let rev = rev_line.strip_prefix("rev: ").expect("rev: ");
    assert_eq!(rev.len(), 12);
    assert!(rev.chars().all(|c| c.is_ascii_hexdigit()));
    assert!(outline.starts_with("- Plan the launch "), "{outline}");
    assert!(
        outline.contains("\n  - Ask [[Ana]] for the copy "),
        "{outline}"
    );
    assert_eq!(
        text_of(&reply[2]).0,
        block,
        "the same text gives the same rev"
    );

    let (missing, is_error) = text_of(&reply[3]);
    assert!(
        is_error,
        "a block that does not exist is a failure: {missing}"
    );
    assert!(missing.contains("no block"), "{missing}");
}

/// MC-7: a page bigger than the result budget is cut at a block boundary, says how many
/// blocks it left out and gives the `after` id that reads them (design §4.2).
#[test]
fn mc_7_a_large_page_is_cut_and_says_how_to_read_the_rest() {
    let graph = Graph::new();
    let (text, is_error) = call(&graph, "read_page", json!({"page": "Big Page"}));
    assert!(!is_error, "{text}");
    assert!(
        text.len() <= utopia_mcp::tools::BUDGET,
        "{} bytes",
        text.len()
    );
    assert!(text.len() > 16 * 1024, "it fills most of the budget");
    let last = text.lines().last().expect("a last line").to_string();
    assert!(last.starts_with('('), "{last}");
    assert!(last.contains("not shown"), "{last}");
    let blocks = text.lines().filter(|l| l.starts_with("- block ")).count();
    assert!(blocks > 50 && blocks < 600, "{blocks} of 600 blocks kept");

    // Following the continuation returns the blocks the first call left out: the next
    // sibling, not the same prefix again.
    let cursor = continuation(&text).expect("a continuation");
    let shown = block_ids(&text);
    assert_eq!(
        Some(&cursor),
        shown.last(),
        "the cursor is the last block shown"
    );
    let (rest, is_error) = call(
        &graph,
        "read_page",
        json!({"page": "Big Page", "after": cursor}),
    );
    assert!(!is_error, "{rest}");
    let next = block_ids(&rest);
    assert!(!next.is_empty(), "{rest}");
    assert!(
        next.iter().all(|id| !shown.contains(id)),
        "the continuation repeats nothing"
    );
    assert!(
        rest.contains("- block"),
        "the continuation is the following siblings: {rest}"
    );
}

/// Append one block under `parent` (`None` puts it at the top level) and give its id back.
fn append_block(graph: &Graph, page: &str, text: &str, parent: Option<String>) -> String {
    let mut store = graph.open();
    utopia_api::handle(
        &mut store,
        utopia_api::Op::AppendBlock {
            title: page.to_string(),
            text: text.to_string(),
            parent,
            create: true,
        },
    )
    .expect("append a block")
    .body["id"]
        .as_str()
        .expect("an id")
        .to_string()
}

/// The block ids of an outline, in order.
fn block_ids(text: &str) -> Vec<String> {
    text.lines()
        .filter(|l| l.starts_with('-') || l.starts_with("  -"))
        .filter_map(|l| l.rsplit_once("{id:"))
        .map(|(_, rest)| rest.trim_end_matches('}').to_string())
        .collect()
}

/// The continuation a cut result names, if it names one.
fn continuation(text: &str) -> Option<String> {
    let (_, rest) = text.rsplit_once("after: \"")?;
    Some(rest.split('"').next()?.to_string())
}

/// MC-7: a cut result can be followed until every block has been read. A continuation
/// that only reaches the first omitted subtree would leave the later siblings unseen.
#[test]
fn mc_7_the_cut_says_how_to_read_every_removed_block() {
    let graph = Graph::new();
    let mut seen: Vec<String> = Vec::new();
    let mut after: Option<String> = None;
    for call_number in 0..20 {
        let mut args = json!({"page": "Big Page"});
        if let Some(a) = &after {
            args["after"] = json!(a);
        }
        let (text, is_error) = call(&graph, "read_page", args);
        assert!(!is_error, "call {call_number}: {text}");
        for id in block_ids(&text) {
            assert!(
                !seen.contains(&id),
                "call {call_number} returned block {id} again: the continuation went backwards"
            );
            seen.push(id);
        }
        match continuation(&text) {
            Some(next) => after = Some(next),
            None => break,
        }
    }
    assert_eq!(seen.len(), 600, "every block of the page is reachable");
}

/// MC-7: `read_block` follows the same continuation, so a subtree bigger than a result is
/// read in parts too.
#[test]
fn mc_7_read_block_continues_after_the_last_block() {
    let graph = Graph::new();
    let mut store = graph.open();
    let page = store.create_page("Nested").expect("a page");
    drop(store);
    let root = append_block(&graph, "Nested", "root", None);
    let kids = append_block(&graph, "Nested", "kids", Some(root.clone()));
    {
        // A `paste` of many lines under `kids` makes them siblings of `kids`, one level
        // down from `root`: a subtree with more blocks than one result holds.
        let mut store = graph.open();
        let st = store.page_state(&page).expect("the page state").clone();
        let markdown: String = (0..400)
            .map(|i| format!("- child {i} {}\n", "z".repeat(20)))
            .collect();
        let plan =
            utopia_core::outline::paste(&st, &utopia_core::BlockId(kids.clone()), 0..0, &markdown)
                .expect("a plan");
        store.execute_plan(&page, plan, None).expect("the paste");
        store.sync_index().expect("index");
    }

    // What the whole page holds, read the same way.
    let mut all: Vec<String> = Vec::new();
    let mut after_page: Option<String> = None;
    for call_number in 0..20 {
        let (text, is_error) = call(
            &graph,
            "read_page",
            json!({"page": "Nested", "after": after_page}),
        );
        assert!(!is_error, "page call {call_number}: {text}");
        all.extend(block_ids(&text));
        match continuation(&text) {
            Some(next) => after_page = Some(next),
            None => break,
        }
    }
    assert!(all.len() > 300, "{} blocks on the page", all.len());

    let mut seen: Vec<String> = Vec::new();
    let mut after: Option<String> = None;
    for call_number in 0..20 {
        let (text, is_error) = call(&graph, "read_block", json!({"id": root, "after": after}));
        assert!(!is_error, "call {call_number}: {text}");
        assert!(text.len() <= utopia_mcp::tools::BUDGET);
        for id in block_ids(&text) {
            assert!(
                !seen.contains(&id),
                "call {call_number} returned block {id} again"
            );
            seen.push(id);
        }
        match continuation(&text) {
            Some(next) => after = Some(next),
            None => break,
        }
    }
    assert_eq!(seen, all, "the subtree reads the same blocks as the page");
}

/// MC-7: a block bigger than the budget keeps its id marker and says that it was cut, even
/// when it is the only block in the result.
#[test]
fn mc_7_an_oversized_block_keeps_its_marker_and_says_it_was_cut() {
    let graph = Graph::new();
    let mut store = graph.open();
    let body = "y".repeat(30 * 1024);
    let id = utopia_api::handle(
        &mut store,
        utopia_api::Op::AppendBlock {
            title: "Huge Block".into(),
            text: body,
            parent: None,
            create: true,
        },
    )
    .expect("a block")
    .body["id"]
        .as_str()
        .expect("an id")
        .to_string();
    drop(store);

    let (text, is_error) = call(&graph, "read_page", json!({"page": "Huge Block"}));
    assert!(!is_error, "{text}");
    assert!(text.len() <= utopia_mcp::tools::BUDGET, "{}", text.len());
    let first = text.lines().next().expect("a first line");
    assert!(
        first.ends_with(&format!("{{id:{id}}}")),
        "the marker survives the cut: {first}"
    );
    assert!(text.contains("cut"), "the cut is reported: {text}");
    assert!(text.contains("longer than one result"), "{text}");
}

/// MC-7: the query example the tool description advertises must parse. A model that
/// follows the description must not be taught syntax the parser refuses.
#[test]
fn mc_7_the_advertised_query_example_parses() {
    let graph = Graph::new();
    let described = support::run(
        vec![
            initialize(1, "2025-11-25"),
            support::request(2, "tools/list", json!({})),
        ],
        graph.backend(),
        Options::new(),
    );
    let tools = described[1]["result"]["tools"].as_array().unwrap();
    let description = tools.iter().find(|t| t["name"] == "run_query").unwrap()["description"]
        .as_str()
        .unwrap();
    let example = description
        .split('`')
        .nth(1)
        .unwrap_or_else(|| panic!("the description shows an example: {description}"));
    assert!(
        utopia_core::query::parse(example).is_ok(),
        "{example:?} must parse: {:?}",
        utopia_core::query::parse(example)
    );
    // And it is a query the backend accepts.
    let (text, is_error) = call(&graph, "run_query", json!({"query": example}));
    assert!(!is_error, "{text}");
}

/// MC-7: `list_pages` labels the daily notes it was asked to include.
#[test]
fn mc_7_list_pages_labels_the_daily_notes() {
    let graph = Graph::new();
    let (text, is_error) = call(&graph, "list_pages", json!({"include_daily": true}));
    assert!(!is_error, "{text}");
    assert!(text.contains("Project X"), "{text}");
    assert!(
        text.lines().any(|l| l.ends_with("(daily note)")),
        "one line is a daily note: {text}"
    );
}

/// MC-11: the catalog and the guide are the same for two graphs with different contents.
#[test]
fn mc_11_the_catalog_does_not_depend_on_the_graph() {
    let seeded = Graph::new();
    let empty = Graph::empty();
    let reply = |graph: &Graph| {
        graph.set_access(Level::Read, true);
        support::run(
            vec![
                initialize(1, "2025-11-25"),
                support::request(2, "tools/list", json!({})),
                support::modern(3, "server/discover", json!({})),
            ],
            graph.backend(),
            Options::new(),
        )
    };
    let a = reply(&seeded);
    let b = reply(&empty);
    assert_eq!(a[1]["result"]["tools"], b[1]["result"]["tools"]);
    assert_eq!(
        a[0]["result"]["instructions"],
        b[0]["result"]["instructions"]
    );
    assert_eq!(
        a[2]["result"]["instructions"], a[0]["result"]["instructions"],
        "one guide for both eras"
    );
    assert_eq!(a[1]["result"]["tools"].as_array().unwrap().len(), 6);
}

/// A tool call that fails on its arguments is still a `result` with `isError`, not an
/// error object (the order of the checks in the brief).
#[test]
fn mc_7_invalid_arguments_are_an_error_result() {
    let graph = Graph::new();
    let (text, is_error) = call(&graph, "search", json!({"limit": 5}));
    assert!(is_error, "{text}");
    assert!(text.contains("`query`"), "{text}");
}

/// The first call after the access file changes sees it: no restart (design §5).
#[test]
fn mc_1_an_access_change_applies_to_the_next_call() {
    let graph = Graph::new();
    graph.set_access(Level::Off, true);
    let server = start(graph.backend(), Options::new());
    server.send(initialize(1, "2025-11-25"));
    server.send(tool_call(2, "list_pages", json!({})));
    let reply = server.out.wait_for(2);
    assert!(text_of(&reply[1]).1, "Off refuses");

    graph.set_access(Level::Read, true);
    server.send(tool_call(3, "list_pages", json!({})));
    let reply = server.out.wait_for(3);
    assert!(!text_of(&reply[2]).1, "Read permits, with no restart");
    server.finish().expect("serve returns at end of input");
}
