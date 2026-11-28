use std::process::Command;

use utopia_store::Store;

fn ut(config: &std::path::Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_ut"));
    cmd.env("UTOPIA_CONFIG_DIR", config)
        .env("UTOPIA_SYNC_KEYCHAIN_PREFIX", "ut-test")
        .env_remove("UTOPIA_GRAPH")
        .env_remove("UTOPIA_SYNC_URL");
    cmd
}

#[test]
fn help_names_ut_and_leaves_out_the_window() {
    let cfg = tempfile::tempdir().unwrap();
    for args in [&[][..], &["--help"][..]] {
        let out = ut(cfg.path()).args(args).output().unwrap();
        assert!(out.status.success());
        let text = String::from_utf8(out.stdout).unwrap();
        assert!(text.contains("  ut sync --graph <file.utopia>"), "{text}");
        assert!(!text.contains("open the desktop app"), "{text}");
    }
    let out = ut(cfg.path()).args(["sync", "--help"]).output().unwrap();
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("ut sync setup"), "{text}");
    assert!(!text.contains("  utopia sync"), "{text}");
}

#[test]
fn doctor_and_export_run_with_no_app() {
    let cfg = tempfile::tempdir().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let graph = dir.path().join("g.utopia");
    {
        let mut s = Store::open(&graph).unwrap();
        let (p, b) = s.open_daily(s.today()).unwrap();
        s.set_block_text(&p, &b, "hello [[World]]", None, None)
            .unwrap();
    }
    let out = ut(cfg.path()).arg("doctor").arg(&graph).output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let export = dir.path().join("out");
    let out = ut(cfg.path())
        .arg("export")
        .arg(&graph)
        .arg(&export)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        export.join("graph.json").is_file(),
        "export writes graph.json"
    );
}

#[test]
fn a_command_for_the_window_is_refused() {
    let cfg = tempfile::tempdir().unwrap();
    let out = ut(cfg.path()).arg("utopia://page/Inbox").output().unwrap();
    assert_eq!(
        out.status.code(),
        Some(64),
        "EX_USAGE, not a sync exit code"
    );
    let err = String::from_utf8(out.stderr).unwrap();
    assert!(err.contains("needs the desktop app"), "{err}");
    let out = ut(cfg.path()).arg("doctor").output().unwrap();
    assert_eq!(
        out.status.code(),
        Some(1),
        "a missing argument is a usage error"
    );
    let err = String::from_utf8(out.stderr).unwrap();
    assert!(err.contains("ut doctor"), "{err}");
}
