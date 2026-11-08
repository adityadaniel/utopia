//! Plugins run sandboxed WebAssembly against the automation API.

use chrono::NaiveDate;
use serde_json::json;
use utopia_plugin::{Plugin, PluginError};
use utopia_store::Store;

/// A plugin directory from WAT source. `body` is the body of `utopia_run`; data
/// segments are given as `(offset, bytes)`.
fn plugin(perms: &[&str], data: &[(u32, &str)], body: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let segments: String = data
        .iter()
        .map(|(off, s)| format!("(data (i32.const {off}) \"{}\")\n", s.replace('"', "\\\"")))
        .collect();
    let wat = format!(
        r#"(module
  (import "utopia" "call" (func $call (param i32 i32) (result i64)))
  (import "utopia" "log" (func $log (param i32 i32)))
  (memory (export "memory") 1)
  (global $heap (mut i32) (i32.const 8192))
  {segments}
  (func (export "utopia_alloc") (param $n i32) (result i32)
    (local $p i32)
    (local.set $p (global.get $heap))
    (global.set $heap (i32.add (global.get $heap) (local.get $n)))
    (local.get $p))
  (func (export "utopia_run") (param $p i32) (param $n i32) (result i64)
    {body}))"#
    );
    std::fs::write(dir.path().join("plugin.wat"), wat).unwrap();
    std::fs::write(
        dir.path().join("plugin.json"),
        json!({
            "name": "test",
            "commands": [{"id": "go", "title": "Go"}],
            "permissions": perms,
            "module": "plugin.wat",
        })
        .to_string(),
    )
    .unwrap();
    dir
}

fn packed(off: u32, s: &str) -> String {
    format!("(i64.const {})", ((off as u64) << 32) | s.len() as u64)
}

fn store() -> Store {
    let mut s = Store::open_in_memory().unwrap();
    s.set_today(NaiveDate::from_ymd_opt(2026, 11, 2));
    s
}

#[test]
fn writes_through_the_api_and_answers_the_editor() {
    let req = r#"{"op":"capture","text":"captured by a [[Plugin]]"}"#;
    let out = r#"{"insert":"done ✓"}"#;
    let dir = plugin(
        &["write"],
        &[(0, req), (1024, out), (2048, "hello log")],
        &format!(
            "(call $log (i32.const 2048) (i32.const 9))
             (drop (call $call (i32.const 0) (i32.const {})))
             {}",
            req.len(),
            packed(1024, out)
        ),
    );
    let p = Plugin::load(dir.path()).unwrap();
    let mut s = store();
    let r = p.run(&mut s, "go", json!({"text": "x"}));
    let out = r.outcome.expect("the run finished");
    assert_eq!(out.insert.as_deref(), Some("done ✓"));
    assert_eq!(r.logs, vec!["hello log"]);
    assert_eq!(r.changed.len(), 1, "the capture touched today's page");
    let plugin_page = s.find_page("Plugin").unwrap().unwrap();
    assert_eq!(s.backlink_count(&plugin_page).unwrap(), 1);
    assert!(r.fuel_used > 0);
    assert!(matches!(
        p.run(&mut s, "nope", json!({})).outcome,
        Err(PluginError::Run(_))
    ));
}

#[test]
fn permissions_gate_operations() {
    let write = r#"{"op":"capture","text":"sneaky"}"#;
    // Returns the host's response as its output.
    let dir = plugin(
        &["read"],
        &[(0, write)],
        &format!("(call $call (i32.const 0) (i32.const {}))", write.len()),
    );
    let p = Plugin::load(dir.path()).unwrap();
    let mut s = store();
    let r = p.run(&mut s, "go", json!({}));
    let err = r.output().unwrap().raw["error"].as_str().unwrap();
    assert!(err.contains("permission denied"), "{err}");
    assert!(s.search_blocks("sneaky", 5).unwrap().is_empty());
    // Reads are allowed with "read".
    let today = s
        .open_daily(NaiveDate::from_ymd_opt(2026, 11, 2).unwrap())
        .unwrap();
    s.set_block_text(&today.0, &today.1, "feedback loops", None, None)
        .unwrap();
    s.commit_block(&today.0, &today.1).unwrap();
    let search = r#"{"op":"search","q":"loops"}"#;
    let dir = plugin(
        &["read"],
        &[(0, search)],
        &format!("(call $call (i32.const 0) (i32.const {}))", search.len()),
    );
    let r = Plugin::load(dir.path())
        .unwrap()
        .run(&mut s, "go", json!({}));
    assert_eq!(
        r.output().unwrap().raw["ok"]["blocks"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    // No permissions at all: nothing.
    let dir = plugin(
        &[],
        &[(0, search)],
        &format!("(call $call (i32.const 0) (i32.const {}))", search.len()),
    );
    let r = Plugin::load(dir.path())
        .unwrap()
        .run(&mut s, "go", json!({}));
    assert!(r.output().unwrap().raw["error"]
        .as_str()
        .unwrap()
        .contains("permission denied"));
}

#[test]
fn runaway_plugins_are_stopped() {
    let mut s = store();
    // An infinite loop runs out of fuel.
    let dir = plugin(&["read"], &[], "(loop $l (br $l)) (i64.const 0)");
    let e = Plugin::load(dir.path())
        .unwrap()
        .run(&mut s, "go", json!({}))
        .outcome
        .unwrap_err();
    assert!(e.to_string().to_lowercase().contains("fuel"), "{e}");
    // Memory beyond the cap is refused (memory.grow returns -1), so this traps.
    let dir = plugin(
        &[],
        &[],
        "(if (i32.eq (memory.grow (i32.const 1024)) (i32.const -1)) (then unreachable)) (i64.const 0)",
    );
    let e = Plugin::load(dir.path())
        .unwrap()
        .run(&mut s, "go", json!({}))
        .outcome
        .unwrap_err();
    assert!(matches!(e, PluginError::Run(_)), "{e}");
    // Output pointing outside memory is an error, not a crash.
    let dir = plugin(&[], &[], "(i64.const 0x7fffffff00000010)");
    assert!(Plugin::load(dir.path())
        .unwrap()
        .run(&mut s, "go", json!({}))
        .outcome
        .is_err());
}

#[test]
fn manifests_are_checked() {
    let dir = plugin(&[], &[], "(i64.const 0)");
    let m = std::fs::read_to_string(dir.path().join("plugin.json")).unwrap();
    std::fs::write(
        dir.path().join("plugin.json"),
        m.replace("plugin.wat", "../../etc/passwd"),
    )
    .unwrap();
    assert!(matches!(
        Plugin::load(dir.path()),
        Err(PluginError::Manifest(_))
    ));
    std::fs::write(
        dir.path().join("plugin.json"),
        "{\"name\":\"x\",\"commands\":[]}",
    )
    .unwrap();
    assert!(Plugin::load(dir.path()).is_err());
    // A module importing anything else (say, WASI files) does not instantiate.
    let bad = tempfile::tempdir().unwrap();
    std::fs::write(
        bad.path().join("plugin.wat"),
        r#"(module (import "wasi_snapshot_preview1" "fd_write" (func (param i32 i32 i32 i32) (result i32)))
           (memory (export "memory") 1)
           (func (export "utopia_alloc") (param i32) (result i32) (i32.const 0))
           (func (export "utopia_run") (param i32 i32) (result i64) (i64.const 0)))"#,
    )
    .unwrap();
    std::fs::write(
        bad.path().join("plugin.json"),
        json!({"name":"bad","commands":[{"id":"go","title":"Go"}],"module":"plugin.wat"})
            .to_string(),
    )
    .unwrap();
    let mut s = store();
    assert!(Plugin::load(bad.path())
        .unwrap()
        .run(&mut s, "go", json!({}))
        .outcome
        .is_err());
    // `load_all` reports each plugin directory, broken ones included.
    let root = tempfile::tempdir().unwrap();
    let good = plugin(&["read"], &[], "(i64.const 0)");
    std::fs::create_dir(root.path().join("a-good")).unwrap();
    for f in ["plugin.json", "plugin.wat"] {
        std::fs::copy(good.path().join(f), root.path().join("a-good").join(f)).unwrap();
    }
    std::fs::create_dir(root.path().join("b-broken")).unwrap();
    std::fs::write(root.path().join("b-broken/plugin.json"), "not json").unwrap();
    std::fs::create_dir(root.path().join("c-not-a-plugin")).unwrap();
    let all = utopia_plugin::load_all(root.path());
    assert_eq!(all.len(), 2);
    assert!(all[0].1.is_ok() && all[1].1.is_err());
}

#[test]
fn example_word_count_plugin() {
    let dir =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/plugins/word-count");
    let p = Plugin::load(&dir).unwrap();
    assert_eq!(p.manifest.commands[0].title, "Count words in this block");
    let mut s = store();
    for (text, n) in [
        ("hello  wonderful\nworld", 3),
        ("  ", 0),
        ("a \"quoted\" word", 3),
        ("one", 1),
        (&"word ".repeat(1234), 1234),
    ] {
        let r = p.run(
            &mut s,
            "count",
            json!({"text": text, "page": "p", "block": "b"}),
        );
        assert_eq!(
            r.output().unwrap().message.as_deref(),
            Some(format!("{n} words").as_str()),
            "{text:?}"
        );
    }
}

/// §4.2 P2 (WASM plugin sandbox), rule 13: a host write commits inside the call, so every
/// way a run can end — a trap, fuel exhaustion, output too large, output not JSON — must
/// still report the pages it wrote and the log lines it printed. Before the run result
/// carried host state, `changed` and `logs` were dropped on all of them, so the store had
/// moved and no view was told (plugin run lifecycle issue 1).
#[test]
fn a_run_that_fails_still_reports_the_pages_it_wrote() {
    let req = r#"{"op":"capture","text":"committed then trapped"}"#;
    let log = "before the failure";
    let cases = [
        // Trap.
        "(unreachable)".to_string(),
        // Return something that is not JSON.
        packed(4096, "not json").to_string(),
        // Claim an output larger than the message cap.
        format!("(i64.const {})", (1u64 << 32) | (4 << 20) as u64 + 1),
        // Burn the whole fuel budget.
        "(loop $l (br $l)) (i64.const 0)".to_string(),
    ];
    for tail in cases {
        let dir = plugin(
            &["write"],
            &[(0, req), (2048, log), (4096, "not json")],
            &format!(
                "(call $log (i32.const 2048) (i32.const {}))
                 (drop (call $call (i32.const 0) (i32.const {})))
                 {tail}",
                log.len(),
                req.len()
            ),
        );
        let p = Plugin::load(dir.path()).unwrap();
        let mut s = store();
        let r = p.run(&mut s, "go", json!({}));
        assert!(
            r.outcome.is_err(),
            "the run failed, as the module made it: {tail}"
        );
        assert_eq!(
            r.changed.len(),
            1,
            "the capture it committed is reported back: {tail}"
        );
        assert_eq!(r.logs, vec![log], "and so is what it logged: {tail}");
        assert!(
            !s.search_blocks("committed then trapped", 5)
                .unwrap()
                .is_empty(),
            "the write really landed: {tail}"
        );
    }
}
