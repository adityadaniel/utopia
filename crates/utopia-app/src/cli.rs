//! Headless commands: `utopia doctor <graph>` (§10.4, §15.2) and
//! `utopia export <graph> <dir>` (G4).

use std::path::Path;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use utopia_store::{OwnerLock, Store};

pub const USAGE: &str = "\
usage:
  utopia [--graph <file.utopia>]        open the desktop app
  utopia doctor <file.utopia> [--rebuild]  check invariants (and rebuild the index)
  utopia export <file.utopia> <dir>     write Markdown files, graph.json and graph.opml to <dir>
  utopia import-roam <file.utopia> <roam.json>   import a Roam JSON export
  utopia capture <text…>                 append a line to today's note (bind to any OS shortcut)
  utopia serve-api <file.utopia> [addr]  run the local automation API without the app
  utopia encrypt <file.utopia>          encrypt the graph at rest (or change its passphrase)
  utopia decrypt <file.utopia>          remove encryption at rest
  utopia utopia://page/<title>          open a utopia:// link (page, journal, block, search, capture)

encrypted graphs: the passphrase comes from UTOPIA_DB_PASSPHRASE, the macOS keychain
(service \"utopia-db\", account = graph name), or a prompt. It is never stored in the graph.

automation API: UTOPIA_API=1 (or UTOPIA_API=127.0.0.1:<port>) starts it with the app; the
URL and a per-launch bearer token are written to api.json in the config directory.

sync (optional): set UTOPIA_SYNC_URL=<relay origin>, and optionally UTOPIA_SYNC_GRAPH=<id>
and UTOPIA_SYNC_TOKEN=<token> (or a macOS keychain item, service \"utopia-sync\").";

/// What a process that cannot become the graph's owner says (MC-4), in the window and on
/// stderr.
pub const ALREADY_OPEN: &str = "This graph is already open in Utopia";

/// How long the app and the write commands try for the owner lock (docs/mcp-design.md
/// §2.2): a headless call holds it for a short time.
pub const OWNER_WAIT: Duration = Duration::from_secs(2);

/// Become the graph's one writer (MC-4), or fail with [`ALREADY_OPEN`] after [`OWNER_WAIT`].
pub fn take_owner(graph: &Path) -> Result<OwnerLock> {
    match OwnerLock::acquire_within(graph, OWNER_WAIT)
        .with_context(|| format!("locking {}", graph.display()))?
    {
        Some(lock) => Ok(lock),
        None => bail!("{ALREADY_OPEN}: {}", graph.display()),
    }
}

/// A store opened by its owner. The fields drop in order, so the store has closed before
/// the lock is released.
pub struct Owned {
    pub store: Store,
    _owner: OwnerLock,
}

/// Open a graph for writes (MC-4): the graph must exist, then the owner lock, then the store.
/// Nothing is written when the lock stays held.
pub fn open_owned(graph: &Path) -> Result<Owned> {
    if !graph.exists() {
        bail!("no graph at {}", graph.display());
    }
    let owner = take_owner(graph)?;
    Ok(Owned {
        store: open_store(graph)?,
        _owner: owner,
    })
}

/// Read a passphrase from the terminal without echoing it.
fn prompt_secret(prompt: &str) -> Result<String> {
    use std::io::Write;
    eprint!("{prompt}");
    std::io::stderr().flush()?;
    let tty = std::fs::File::open("/dev/tty").ok();
    let stty = |arg: &str| {
        if let Some(t) = &tty {
            let _ = std::process::Command::new("/bin/stty")
                .arg(arg)
                .stdin(t.try_clone().expect("tty"))
                .status();
        }
    };
    stty("-echo");
    let mut line = String::new();
    let r = std::io::stdin().read_line(&mut line);
    stty("echo");
    eprintln!();
    r?;
    Ok(line.trim_end_matches(['\n', '\r']).to_string())
}

/// Open a graph for a command, unlocking it if it is encrypted at rest.
pub fn open_store(graph: &Path) -> Result<Store> {
    if !graph.exists() {
        bail!("no graph at {}", graph.display());
    }
    if !utopia_store::is_encrypted(graph) {
        return Store::open(graph).with_context(|| format!("opening {}", graph.display()));
    }
    let pass = match crate::paths::db_passphrase(graph) {
        Some(p) => p,
        None => prompt_secret("passphrase: ")?,
    };
    Store::open_with_key(graph, Some(&pass)).with_context(|| format!("opening {}", graph.display()))
}

/// `utopia encrypt`: encrypt a plain graph, or change an encrypted graph's passphrase.
pub fn encrypt(graph: &Path) -> Result<()> {
    if !utopia_store::encryption_available() {
        bail!("this build has no encryption at rest (SQLCipher)");
    }
    if !graph.exists() {
        bail!("no graph at {}", graph.display());
    }
    let current = if utopia_store::is_encrypted(graph) {
        Some(match crate::paths::db_passphrase(graph) {
            Some(p) => p,
            None => prompt_secret("current passphrase: ")?,
        })
    } else {
        None
    };
    let new = match std::env::var("UTOPIA_DB_NEW_PASSPHRASE")
        .ok()
        .filter(|v| !v.is_empty())
    {
        Some(p) => p,
        None => {
            let a = prompt_secret("new passphrase: ")?;
            let b = prompt_secret("again: ")?;
            if a != b {
                bail!("the passphrases differ");
            }
            a
        }
    };
    utopia_store::set_encryption(graph, current.as_deref(), Some(&new))?;
    println!(
        "{} is encrypted at rest. Keep the passphrase safe: without it the graph cannot be read.",
        graph.display()
    );
    Ok(())
}

/// `utopia decrypt`: back to a plain SQLite file.
pub fn decrypt(graph: &Path) -> Result<()> {
    if !utopia_store::is_encrypted(graph) {
        bail!("{} is not encrypted", graph.display());
    }
    let pass = match crate::paths::db_passphrase(graph) {
        Some(p) => p,
        None => prompt_secret("passphrase: ")?,
    };
    utopia_store::set_encryption(graph, Some(&pass), None)?;
    println!("{} is no longer encrypted", graph.display());
    Ok(())
}

/// `doctor` only reads; `doctor --rebuild` rewrites the projections, so it must own the
/// graph (MC-4).
pub fn doctor(graph: &Path, rebuild: bool) -> Result<bool> {
    let report = if rebuild {
        let mut owned = open_owned(graph)?;
        owned.store.rebuild_index()?;
        println!("index rebuilt from page shards");
        owned.store.doctor()?
    } else {
        open_store(graph)?.doctor()?
    };
    println!(
        "{} live pages, {} live blocks, {} references",
        report.pages, report.blocks, report.refs
    );
    if report.is_ok() {
        println!("ok: all invariants hold");
    } else {
        for p in &report.problems {
            println!("problem: {p}");
        }
    }
    Ok(report.is_ok())
}

pub fn export(graph: &Path, dir: &Path) -> Result<()> {
    let mut store = open_store(graph)?;
    let n = store.export_markdown(dir)?;
    store.export_json(&dir.join("graph.json"))?;
    store.export_opml(&dir.join("graph.opml"))?;
    println!("exported {n} pages to {}", dir.display());
    Ok(())
}

pub fn import_roam(graph: &Path, file: &Path) -> Result<()> {
    let json =
        std::fs::read_to_string(file).with_context(|| format!("reading {}", file.display()))?;
    // Importing may create the graph, so the lock is taken before the file exists (MC-4).
    let _owner = take_owner(graph)?;
    let mut store = if graph.exists() {
        open_store(graph)?
    } else {
        Store::open(graph).with_context(|| format!("opening {}", graph.display()))?
    };
    let r = store.import_roam_json(&json)?;
    println!("imported {} pages, {} blocks", r.pages, r.blocks);
    Ok(())
}

pub fn capture(graph: &Path, text: &str) -> Result<()> {
    if text.trim().is_empty() {
        bail!("nothing to capture");
    }
    let mut owned = open_owned(graph)?;
    let store = &mut owned.store;
    let (page, _) = store.capture(text.trim())?;
    let title = store.page_row(&page)?.map(|r| r.title).unwrap_or_default();
    println!("captured to {title}");
    Ok(())
}

/// `utopia serve-api`: the automation API over a graph with no window, for scripts and
/// servers. Runs until killed, and owns the graph for all of that time (MC-4).
pub fn serve_api(graph: &Path, addr: &str, discovery: &Path) -> Result<()> {
    let mut owned = open_owned(graph)?;
    let server = utopia_api::serve(addr, utopia_api::new_token())?;
    server.write_discovery(discovery)?;
    println!(
        "automation API on {} (token in {})",
        server.url(),
        discovery.display()
    );
    loop {
        if let Some(call) = server.recv_timeout(std::time::Duration::from_secs(1)) {
            call.run(&mut owned.store);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doctor_and_export_on_a_real_file() {
        let dir = tempfile::tempdir().unwrap();
        let graph = dir.path().join("g.utopia");
        {
            let mut s = Store::open(&graph).unwrap();
            let (p, b) = s.open_daily(s.today()).unwrap();
            s.set_block_text(&p, &b, "hello [[World]]", None, None)
                .unwrap();
            s.commit_block(&p, &b).unwrap();
        }
        assert!(doctor(&graph, false).unwrap());
        assert!(doctor(&graph, true).unwrap());
        let out = dir.path().join("out");
        export(&graph, &out).unwrap();
        assert!(out.join("graph.json").exists());
        assert!(out.join("graph.opml").exists());
        capture(&graph, "from the command line").unwrap();
        let roam = dir.path().join("roam.json");
        std::fs::write(
            &roam,
            r#"[{"title":"Imported","children":[{"string":"hi","uid":"u1"}]}]"#,
        )
        .unwrap();
        import_roam(&graph, &roam).unwrap();
        let mut s = Store::open(&graph).unwrap();
        assert!(s.find_page("Imported").unwrap().is_some());
        assert!(s.doctor().unwrap().is_ok());
        assert!(out.join("World.md").exists());
        assert!(doctor(&dir.path().join("missing.utopia"), false).is_err());
    }
}
