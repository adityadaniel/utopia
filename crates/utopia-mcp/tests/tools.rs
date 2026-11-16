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

/// One call per write tool, with arguments that change the graph.
fn every_write_tool(graph: &Graph) -> Vec<(&'static str, Value)> {
    vec![
        (
            "append_blocks",
            json!({"page": "Written", "markdown": "- one\n", "create": true}),
        ),
        (
            "append_blocks",
            json!({"page": "Project X", "markdown": "- appended by an assistant\n"}),
        ),
        (
            "update_block",
            json!({"id": graph.ana_link, "text": "written by an assistant"}),
        ),
        ("move_block", json!({"id": graph.root, "page": "Ana"})),
        ("delete_block", json!({"id": graph.root})),
    ]
}

/// MC-1: with access Off (as a missing file means) no tool reads or writes, and each one
/// says where the owner changes it.
#[test]
fn mc_1_no_tool_works_with_access_off() {
    let graph = Graph::new();
    let pages_before = graph.page_count();
    let calls: Vec<(&'static str, Value)> = with_ids(&graph)
        .into_iter()
        .map(|(name, args, _)| (name, args))
        .chain(every_write_tool(&graph))
        .collect();
    for (name, args) in calls {
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
    let store = graph.open();
    assert!(store.find_page("Written").expect("a lookup").is_none());
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

    // With the file at write, read-only answers the read tools and leaves the write tool
    // out of the list (MC-2, design §5).
    graph.set_access(Level::Write, true);
    let reply = support::run(
        vec![
            initialize(1, "2025-11-25"),
            tool_call(2, "list_pages", json!({})),
            support::request(3, "tools/list", json!({})),
            tool_call(
                4,
                "append_blocks",
                json!({"page": "Project X", "markdown": "- from a read-only client\n"}),
            ),
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
    let (text, is_error) = text_of(&reply[3]);
    assert!(is_error, "{text}");
    assert!(text.contains("read-only"), "{text}");
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

/// MC-7: a continuation that names no block in the result is a failure for the model to
/// read, not a silently empty outline (a stale id after an edit is the likely cause).
#[test]
fn mc_7_a_cursor_that_names_no_block_is_a_failure() {
    let graph = Graph::new();
    for (tool, args) in [
        (
            "read_page",
            json!({"page": "Project X", "after": "01J8Z3K9V2AAAAAAAAAAAAAAAA"}),
        ),
        (
            "read_block",
            json!({"id": graph.root, "after": "01J8Z3K9V2AAAAAAAAAAAAAAAA"}),
        ),
    ] {
        let (text, is_error) = call(&graph, tool, args);
        assert!(is_error, "{tool}: {text}");
        assert!(text.contains("nothing to continue after"), "{tool}: {text}");
    }
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
    assert_eq!(a[1]["result"]["tools"].as_array().unwrap().len(), 10);
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

/// One `tools/call` at `level`, with the app closed and `when_closed` on.
fn call_at(graph: &Graph, level: Level, name: &str, args: Value) -> (String, bool) {
    text_of(&call_as(graph, level, true, name, &args))
}

/// One `update_block` call at the level `write`, with the app closed.
fn update(graph: &Graph, args: Value) -> (String, bool) {
    call_at(graph, Level::Write, "update_block", args)
}

/// MC-2, MC-8: at the level `read` the write tool fails and writes nothing; at `write` the
/// same call lands.
#[test]
fn mc_8_append_blocks_is_refused_at_read_and_writes_nothing() {
    let graph = Graph::new();
    let before = graph.page_count();
    let args = json!({"page": "Written", "markdown": "- one\n", "create": true});
    let (text, is_error) = call_at(&graph, Level::Read, "append_blocks", args.clone());
    assert!(is_error, "{text}");
    assert!(text.contains("read access only"), "{text}");
    assert!(text.contains("Nothing was written"), "{text}");
    assert_eq!(graph.page_count(), before, "no page was made");
    let mut store = graph.open();
    assert!(store.find_page("Written").expect("a lookup").is_none());
    assert!(store.doctor().expect("doctor runs").is_ok());
    drop(store);

    let (text, is_error) = call_at(&graph, Level::Write, "append_blocks", args);
    assert!(!is_error, "{text}");
}

/// MC-8, MC-3: one `append_blocks` call appends the whole outline, in order and with its
/// nesting, and `doctor` is clean afterwards (rule 2).
#[test]
fn mc_8_append_blocks_appends_an_outline_and_doctor_is_clean() {
    let graph = Graph::new();
    let before = graph.page_count();
    let (text, is_error) = call_at(
        &graph,
        Level::Write,
        "append_blocks",
        json!({
            "page": "Project X",
            "markdown": "- write the launch post {id:01J8Z3K9V2}\n  - ask [[Ana]] for the quote\n- send it\n",
        }),
    );
    assert!(!is_error, "{text}");
    assert!(
        text.contains("Appended 3 blocks to \"Project X\""),
        "{text}"
    );
    assert_eq!(graph.page_count(), before, "the page already existed");

    let (page, is_error) = call_at(
        &graph,
        Level::Read,
        "read_page",
        json!({"page": "Project X"}),
    );
    assert!(!is_error, "{page}");
    let at = |needle: &str| {
        page.find(needle)
            .unwrap_or_else(|| panic!("{needle} in the page: {page}"))
    };
    assert!(at("write the launch post") < at("ask [[Ana]] for the quote"));
    assert!(at("ask [[Ana]] for the quote") < at("send it"));
    assert!(
        page.contains("\n  - ask [[Ana]] for the quote {id:"),
        "one level in: {page}"
    );
    assert!(
        !page.contains("01J8Z3K9V2"),
        "the marker it was sent with is gone, and the block has its own id: {page}"
    );
    assert!(
        page.contains("write the launch post {id:"),
        "the line still ends with an id marker: {page}"
    );
    let mut store = graph.open();
    assert!(store.doctor().expect("doctor runs").is_ok());
}

/// MC-8, W3/FR-1.10: a `[[link]]` in the appended text makes its page and its backlink, as
/// the same text typed into a block does.
#[test]
fn mc_8_a_link_in_the_outline_gives_a_backlink() {
    let graph = Graph::new();
    let (text, is_error) = call_at(
        &graph,
        Level::Write,
        "append_blocks",
        json!({"page": "Project X", "markdown": "- brief [[Zoe]] on the launch\n"}),
    );
    assert!(!is_error, "{text}");
    let (backlinks, is_error) =
        call_at(&graph, Level::Read, "get_backlinks", json!({"page": "Zoe"}));
    assert!(!is_error, "{backlinks}");
    assert!(
        backlinks.contains("brief [[Zoe]] on the launch"),
        "{backlinks}"
    );
    // The link made the page: reading it is an empty outline, not a failure.
    let (zoe, is_error) = call_at(&graph, Level::Read, "read_page", json!({"page": "Zoe"}));
    assert!(!is_error, "{zoe}");
    assert_eq!(zoe, "", "the page exists and has no blocks");
}

/// MC-8, FR-1.2: `today` is the day's page, a day with no page gets one on its first write,
/// and a page that does not resolve is not made unless `create` says so.
#[test]
fn mc_8_append_blocks_reaches_a_day_and_respects_create() {
    let graph = Graph::new();
    let (text, is_error) = call_at(
        &graph,
        Level::Write,
        "append_blocks",
        json!({"page": "today", "markdown": "- another line for today\n"}),
    );
    assert!(!is_error, "{text}");
    let (today, _) = call_at(&graph, Level::Read, "read_page", json!({"page": "today"}));
    assert!(today.contains("Met [[Ana]] today"), "{today}");
    assert!(today.contains("another line for today"), "{today}");

    // A day that has no page yet: the first write makes it (FR-1.2).
    let (text, is_error) = call_at(
        &graph,
        Level::Write,
        "append_blocks",
        json!({"page": "2099-01-01", "markdown": "- the far future\n", "create": true}),
    );
    assert!(!is_error, "{text}");
    let (day, is_error) = call_at(
        &graph,
        Level::Read,
        "read_page",
        json!({"page": "2099-01-01"}),
    );
    assert!(!is_error, "{day}");
    assert!(day.contains("the far future"), "{day}");

    // `create` off: a page that does not resolve is a failure and nothing is made.
    let pages = graph.page_count();
    let (text, is_error) = call_at(
        &graph,
        Level::Write,
        "append_blocks",
        json!({"page": "No such page", "markdown": "- x\n"}),
    );
    assert!(is_error, "{text}");
    assert!(text.contains("no page titled"), "{text}");
    assert_eq!(graph.page_count(), pages);
    let store = graph.open();
    assert!(store.find_page("No such page").expect("a lookup").is_none());
}

/// MC-8, design §4: an outline read from a page can be appended as it was shown — the id
/// markers are taken off and the nesting is kept, so the copy reads like the original.
#[test]
fn mc_8_append_blocks_round_trips_the_outline_format() {
    let graph = Graph::new();
    let (outline, is_error) = call_at(
        &graph,
        Level::Read,
        "read_page",
        json!({"page": "Project X"}),
    );
    assert!(!is_error, "{outline}");
    let (text, is_error) = call_at(
        &graph,
        Level::Write,
        "append_blocks",
        json!({"page": "Copy of Project X", "markdown": outline, "create": true}),
    );
    assert!(!is_error, "{text}");
    let (copy, is_error) = call_at(
        &graph,
        Level::Read,
        "read_page",
        json!({"page": "Copy of Project X"}),
    );
    assert!(!is_error, "{copy}");
    assert_eq!(
        utopia_api::strip_id_markers(&copy),
        utopia_api::strip_id_markers(&outline),
        "the same text at the same nesting, with the new blocks' own ids"
    );
    let mut store = graph.open();
    assert!(store.doctor().expect("doctor runs").is_ok());
}

/// MC-8: an outline whose lines all carry the same indentation reads as siblings. The tool
/// must pass the text through as it was sent: trimming reaches only the first line, which
/// would make the second line its child.
#[test]
fn mc_8_a_uniformly_indented_outline_keeps_its_siblings() {
    let graph = Graph::new();
    let markdown = "  - one\n  - two\n";
    let (text, is_error) = call_at(
        &graph,
        Level::Write,
        "append_blocks",
        json!({"page": "Project X", "markdown": markdown}),
    );
    assert!(!is_error, "{text}");
    assert!(text.contains("Appended 2 blocks"), "{text}");
    let (page, is_error) = call_at(
        &graph,
        Level::Read,
        "read_page",
        json!({"page": "Project X"}),
    );
    assert!(!is_error, "{page}");
    let line = |want: &str| {
        page.lines()
            .find(|l| l.starts_with(&format!("- {want} ")) || *l == format!("- {want}"))
            .unwrap_or_else(|| panic!("a top-level line {want:?}: {page}"))
    };
    assert!(line("one").starts_with("- one {id:"), "{}", line("one"));
    assert!(line("two").starts_with("- two {id:"), "{}", line("two"));
}

/// The `rev:` line a `read_block` or an `update_block` answer carries.
fn rev_of(text: &str) -> String {
    text.lines()
        .find_map(|l| l.strip_prefix("rev: "))
        .unwrap_or_else(|| panic!("a rev line: {text}"))
        .trim_end()
        .to_string()
}

/// MC-10, §2.3.7: an `if_rev` that does not agree with the block makes the call fail with
/// no write, and the error tells the agent to read the block again. The rev the block does
/// have is the one `read_block` gave, and sending that one writes.
#[test]
fn mc_10_update_block_refuses_a_rev_that_does_not_agree() {
    let graph = Graph::new();
    let (read, is_error) = call_at(&graph, Level::Read, "read_block", json!({"id": graph.root}));
    assert!(!is_error, "{read}");
    let rev = rev_of(&read);
    let (text, is_error) = update(
        &graph,
        json!({"id": graph.root, "text": "overwritten", "if_rev": "000000000000"}),
    );
    assert!(is_error, "{text}");
    assert!(
        text.contains("read_block"),
        "the error says to read the block again: {text}"
    );
    assert!(
        text.contains(&rev),
        "it names the rev the block has: {text}"
    );
    assert!(text.contains("Nothing was written"), "{text}");

    // Nothing changed, and the projections agree with the shards (rule 2).
    let (page, _) = call_at(
        &graph,
        Level::Read,
        "read_page",
        json!({"page": "Project X"}),
    );
    assert!(page.contains("Plan the launch"), "{page}");
    assert!(!page.contains("overwritten"), "{page}");
    let mut store = graph.open();
    let report = store.doctor().expect("doctor runs");
    assert!(report.is_ok(), "doctor: {report:?}");
    drop(store);

    // The rev that agrees writes, and the answer carries the new one.
    let (text, is_error) = update(
        &graph,
        json!({"id": graph.root, "text": "shipping notes", "if_rev": rev}),
    );
    assert!(!is_error, "{text}");
    assert!(text.contains("- shipping notes {id:"), "{text}");
    let new = rev_of(&text);
    assert_ne!(new, rev, "the answer carries the new rev");
    let (read, _) = call_at(&graph, Level::Read, "read_block", json!({"id": graph.root}));
    assert_eq!(
        rev_of(&read),
        new,
        "the rev of the answer and the rev read_block gives are one hash"
    );
}

/// MC-8: `update_block` sets a block's kind and ticks a to-do, and the page reads that way
/// afterwards. Doctor is clean after the calls (rule 2).
#[test]
fn mc_8_update_block_changes_the_kind_and_ticks_a_to_do() {
    let graph = Graph::new();
    let (text, is_error) = update(
        &graph,
        json!({"id": graph.ana_link, "kind": "todo", "checked": true}),
    );
    assert!(!is_error, "{text}");
    assert!(
        text.contains("- [x] Ask [[Ana]] for the copy {id:"),
        "the answer shows the to-do as the page will: {text}"
    );
    let (page, _) = call_at(
        &graph,
        Level::Read,
        "read_page",
        json!({"page": "Project X"}),
    );
    assert!(
        page.contains("  - [x] Ask [[Ana]] for the copy {id:"),
        "{page}"
    );

    // Unticking sends `checked` alone: the kind stays.
    let (text, is_error) = update(&graph, json!({"id": graph.ana_link, "checked": false}));
    assert!(!is_error, "{text}");
    let (page, _) = call_at(
        &graph,
        Level::Read,
        "read_page",
        json!({"page": "Project X"}),
    );
    assert!(
        page.contains("  - [ ] Ask [[Ana]] for the copy {id:"),
        "{page}"
    );

    // A heading, in the outline format's own spelling.
    let (text, is_error) = update(&graph, json!({"id": graph.root, "kind": "h2"}));
    assert!(!is_error, "{text}");
    let (page, _) = call_at(
        &graph,
        Level::Read,
        "read_page",
        json!({"page": "Project X"}),
    );
    assert!(page.contains("- ## Plan the launch {id:"), "{page}");

    let mut store = graph.open();
    let report = store.doctor().expect("doctor runs");
    assert!(report.is_ok(), "doctor: {report:?}");
}

/// MC-2: at the level `read` the second write tool is refused and writes nothing.
#[test]
fn mc_2_update_block_is_refused_at_read() {
    let graph = Graph::new();
    let (text, is_error) = call_at(
        &graph,
        Level::Read,
        "update_block",
        json!({"id": graph.root, "text": "zebra"}),
    );
    assert!(is_error, "{text}");
    assert!(text.contains("read access only"), "{text}");
    let (page, _) = call_at(
        &graph,
        Level::Read,
        "read_page",
        json!({"page": "Project X"}),
    );
    assert!(page.contains("Plan the launch"), "{page}");
    assert!(!page.contains("zebra"), "{page}");
}

/// MC-8: the tool refuses what it cannot write — an empty `text` (it never wipes a block), a
/// call that names nothing to change, a kind that does not exist and `checked` on a bullet.
/// Nothing is written and the block still reads as it did.
#[test]
fn mc_8_update_block_refuses_a_call_it_cannot_write() {
    let graph = Graph::new();
    for (args, want) in [
        (json!({"id": graph.root, "text": "  "}), "empty"),
        (
            json!({"id": graph.root}),
            "nothing for update_block to change",
        ),
        (
            json!({"id": graph.root, "kind": "toodo"}),
            "unknown block kind",
        ),
        (
            json!({"id": graph.root, "text": " {id:01J8Z3K9V2}"}),
            "marker",
        ),
        (json!({"id": graph.root, "checked": true}), "todo"),
        (
            json!({"id": "01J8Z3K9V2AAAAAAAAAAAAAAAA", "text": "nowhere"}),
            "no block",
        ),
    ] {
        let (text, is_error) = update(&graph, args.clone());
        assert!(is_error, "{args}: {text}");
        assert!(text.contains(want), "{args}: {want:?} in {text}");
    }
    let (page, _) = call_at(
        &graph,
        Level::Read,
        "read_page",
        json!({"page": "Project X"}),
    );
    assert!(page.contains("- Plan the launch {id:"), "{page}");
    let mut store = graph.open();
    assert!(store.doctor().expect("doctor runs").is_ok());
}

/// MC-11: the two write tools' descriptions say what they do, and neither carries page
/// text. The `update_block` schema names the kinds the op accepts.
#[test]
fn mc_11_update_block_is_described_without_page_text() {
    let graph = Graph::new();
    let reply = support::run(
        vec![
            initialize(1, "2025-11-25"),
            support::request(2, "tools/list", json!({})),
        ],
        graph.backend(),
        Options::new(),
    );
    let tools = reply[1]["result"]["tools"].as_array().expect("a list");
    let tool = tools
        .iter()
        .find(|t| t["name"] == "update_block")
        .expect("update_block is in the catalog");
    let description = tool["description"].as_str().expect("a description");
    assert!(description.contains("if_rev"), "{description}");
    assert!(description.contains("undo"), "{description}");
    let kinds = tool["inputSchema"]["properties"]["kind"]["enum"]
        .as_array()
        .expect("the kinds")
        .clone();
    for kind in ["bullet", "todo", "quote", "code", "heading", "h2"] {
        assert!(
            kinds.contains(&json!(kind)),
            "{kind} is one of the kinds: {kinds:?}"
        );
    }
    assert_eq!(tool["annotations"]["destructiveHint"], json!(true));
    assert_eq!(tool["annotations"]["idempotentHint"], json!(true));
    assert_eq!(tool["annotations"]["readOnlyHint"], json!(false));
    // MC-11: no page text in the description, the same as for every other tool.
    assert!(!description.contains("Plan the launch"), "{description}");
}

/// MC-8, design §4: the id marker of the outline format is taken off the text an agent sends
/// back, as `append_blocks` does (handoff §5). The natural flow is `read_block` → copy its
/// line → `update_block`; the marker, and a trailing newline from copying the line, must not
/// land in the owner's note — and the answer's `rev` must cover the text that landed.
#[test]
fn mc_8_update_block_takes_the_id_marker_off_the_text_it_is_sent() {
    let graph = Graph::new();
    let (read, is_error) = call_at(&graph, Level::Read, "read_block", json!({"id": graph.root}));
    assert!(!is_error, "{read}");
    let line = read
        .lines()
        .find(|l| l.starts_with("- Plan the launch"))
        .unwrap_or_else(|| panic!("the root's line: {read}"));
    assert!(
        line.ends_with(&format!("{{id:{}}}", graph.root)),
        "the line carries the marker: {line}"
    );
    // One word changed and the line sent back exactly as `read_block` showed it: the `- `,
    // the marker and the newline are the outline's, not the block's.
    let sent = format!("{}\n", line.replace("Plan the launch", "Plan the shipment"));
    let (answer, is_error) = update(&graph, json!({"id": graph.root, "text": sent}));
    assert!(!is_error, "{answer}");

    // What is stored is the text, not the outline's or the copy's extras.
    let mut store = graph.open();
    let page = store
        .find_page("Project X")
        .expect("a lookup")
        .expect("the page");
    let st = store.page_state(&page).expect("the page state");
    let stored = st
        .block(&utopia_core::BlockId(graph.root.clone()))
        .expect("the block")
        .text
        .clone();
    assert_eq!(
        stored, "Plan the shipment",
        "the marker and the newline are gone"
    );
    // And the rev the answer gives is the one that stored text hashes to.
    assert_eq!(
        rev_of(&answer),
        utopia_api::block_rev("Plan the shipment"),
        "the rev describes the text that was written"
    );
}

/// One `move_block` call at the level `write`, with the app closed.
fn move_block(graph: &Graph, args: Value) -> (String, bool) {
    call_at(graph, Level::Write, "move_block", args)
}

/// One `delete_block` call at the level `write`, with the app closed.
fn delete_block(graph: &Graph, args: Value) -> (String, bool) {
    call_at(graph, Level::Write, "delete_block", args)
}

/// MC-2: at the level `read` neither of the last two write tools works, and neither writes
/// anything — the page still has the whole subtree, and `doctor` is clean (rule 2).
#[test]
fn mc_2_move_block_and_delete_block_are_refused_at_read() {
    let graph = Graph::new();
    let before = graph.page_count();
    for (name, args) in [
        ("move_block", json!({"id": graph.root, "page": "Ana"})),
        ("delete_block", json!({"id": graph.root})),
    ] {
        let (text, is_error) = call_at(&graph, Level::Read, name, args);
        assert!(is_error, "{name}: {text}");
        assert!(text.contains("read access only"), "{name}: {text}");
        assert!(text.contains("Nothing was written"), "{name}: {text}");

        // Nothing moved and nothing became a tombstone: the subtree reads as it did, on its
        // own page, and the block is still live there.
        let (page, _) = call_at(
            &graph,
            Level::Read,
            "read_page",
            json!({"page": "Project X"}),
        );
        assert!(
            page.contains(&format!("- Plan the launch {{id:{}}}", graph.root)),
            "{name}: {page}"
        );
        assert!(page.contains("Ask [[Ana]] for the copy"), "{name}: {page}");
        let mut store = graph.open();
        assert!(store.doctor().expect("doctor runs").is_ok(), "{name}");
    }
    assert_eq!(graph.page_count(), before, "no page was made");
}

/// MC-8: `move_block` carries the subtree into another page, ids and all, so a
/// `((reference))` to a moved block still resolves there; `doctor` is clean afterwards.
#[test]
fn mc_8_move_block_carries_the_subtree_to_another_page() {
    let graph = Graph::new();
    // A block on the target page that references the block about to arrive.
    let (text, is_error) = call_at(
        &graph,
        Level::Write,
        "append_blocks",
        json!({"page": "Ana", "markdown": format!("- see (({})) here\n", graph.root)}),
    );
    assert!(!is_error, "{text}");

    let (text, is_error) = move_block(&graph, json!({"id": graph.root, "page": "Ana"}));
    assert!(!is_error, "{text}");
    assert!(text.contains("Moved 2 blocks to \"Ana\""), "{text}");
    assert!(
        text.contains(&format!("The moved block is {}", graph.root)),
        "{text}"
    );

    // The target page holds the subtree, in its shape, ahead of what was there.
    let (ana, is_error) = call_at(&graph, Level::Read, "read_page", json!({"page": "Ana"}));
    assert!(!is_error, "{ana}");
    let root_line = ana
        .lines()
        .find(|l| l.starts_with("- Plan the launch"))
        .unwrap_or_else(|| panic!("the moved root: {ana}"));
    assert!(
        root_line.ends_with(&format!("{{id:{}}}", graph.root)),
        "{root_line}"
    );
    assert!(
        ana.contains("\n  - Ask [[Ana]] for the copy {id:"),
        "its child came with it, one level in: {ana}"
    );
    let referring = ana
        .lines()
        .find(|l| l.contains("see "))
        .unwrap_or_else(|| panic!("the referring block: {ana}"));
    assert!(
        referring.contains(&format!("(({}))", graph.root)),
        "the reference's text is untouched: {referring}"
    );

    // The source page lost the subtree; the block still resolves, on its new page.
    let (project, _) = call_at(
        &graph,
        Level::Read,
        "read_page",
        json!({"page": "Project X"}),
    );
    assert!(!project.contains("Plan the launch"), "{project}");
    let (block, is_error) = call_at(&graph, Level::Read, "read_block", json!({"id": graph.root}));
    assert!(!is_error, "the moved id still resolves: {block}");
    assert!(block.contains("Plan the launch"), "{block}");

    let mut store = graph.open();
    let report = store.doctor().expect("doctor runs");
    assert!(report.is_ok(), "doctor: {report:?}");
}

/// MC-8: `delete_block` leaves a tombstone, not a hole: the page no longer shows the subtree,
/// and the blocks keep their ids, so a reference reads "deleted block" rather than "missing".
/// One undo restores the subtree; it is asserted where the writing process still has the undo
/// stack, in `crates/utopia-api/tests/move_delete.rs` (each headless tool call opens and
/// closes its own store).
#[test]
fn mc_8_delete_block_tombstones_the_subtree_and_doctor_is_clean() {
    let graph = Graph::new();
    let (text, is_error) = delete_block(&graph, json!({"id": graph.root}));
    assert!(!is_error, "{text}");
    assert!(text.contains("Deleted 2 blocks on \"Project X\""), "{text}");
    assert!(
        text.contains(&format!("block {} keeps its id", graph.root)),
        "{text}"
    );

    let (project, is_error) = call_at(
        &graph,
        Level::Read,
        "read_page",
        json!({"page": "Project X"}),
    );
    assert!(!is_error, "{project}");
    assert!(!project.contains("Plan the launch"), "{project}");
    assert!(!project.contains("Ask [[Ana]] for the copy"), "{project}");
    assert!(
        project.contains("ship it"),
        "the rest of the page is untouched: {project}"
    );

    let mut store = graph.open();
    let report = store.doctor().expect("doctor runs");
    assert!(report.is_ok(), "doctor: {report:?}");
    // Rule 5: the row and the id stay, so a reference reads "deleted block", not "missing".
    let id = utopia_core::BlockId(graph.root.clone());
    let row = store
        .block_row(&id)
        .expect("a block lookup")
        .expect("the row of the tombstone");
    assert!(
        !store
            .page_state(&row.page_id)
            .expect("the page state")
            .is_live(&id),
        "the reference reads a deleted block"
    );
}

/// MC-11: the two new tools say what they do, carry no page text, and their schemas accept
/// exactly the arguments the op takes.
#[test]
fn mc_11_the_move_and_delete_tools_are_described_without_page_text() {
    let graph = Graph::new();
    let reply = support::run(
        vec![
            initialize(1, "2025-11-25"),
            support::request(2, "tools/list", json!({})),
        ],
        graph.backend(),
        Options::new(),
    );
    let tools = reply[1]["result"]["tools"].as_array().expect("a list");
    let tool = |name: &str| {
        tools
            .iter()
            .find(|t| t["name"] == name)
            .unwrap_or_else(|| panic!("{name} is in the catalog"))
    };

    let mv = tool("move_block");
    let description = mv["description"].as_str().expect("a description");
    assert!(description.contains("undo"), "{description}");
    assert!(description.contains("subtree"), "{description}");
    // Design §4.1: a move loses no text, so it is not destructive.
    assert_eq!(mv["annotations"]["readOnlyHint"], json!(false));
    assert_eq!(mv["annotations"]["destructiveHint"], json!(false));
    assert_eq!(mv["annotations"]["idempotentHint"], json!(true));
    assert_eq!(mv["annotations"]["openWorldHint"], json!(false));
    for key in ["id", "page", "parent", "after"] {
        assert!(
            mv["inputSchema"]["properties"][key].is_object(),
            "move_block takes {key}: {}",
            mv["inputSchema"]
        );
    }
    assert_eq!(mv["inputSchema"]["required"], json!(["id"]));

    let del = tool("delete_block");
    let description = del["description"].as_str().expect("a description");
    assert!(description.contains("tombstone"), "{description}");
    assert!(description.contains("undo"), "{description}");
    assert_eq!(del["annotations"]["readOnlyHint"], json!(false));
    assert_eq!(del["annotations"]["destructiveHint"], json!(true));
    assert_eq!(del["annotations"]["idempotentHint"], json!(true));
    assert_eq!(del["annotations"]["openWorldHint"], json!(false));
    assert_eq!(del["inputSchema"]["required"], json!(["id"]));

    // MC-11: no page text in either description, as for every other tool.
    for t in [mv, del] {
        let description = t["description"].as_str().unwrap();
        assert!(!description.contains("Plan the launch"), "{description}");
        assert!(!description.contains("Ana"), "{description}");
    }
}
