//! `utopia mcp`: the MCP server on stdio, and the setup entry for each client (PRD §13.8
//! MC-5, MC-13, MC-14; `docs/mcp-design.md` §2.3, §6).
//!
//! ```text
//! utopia mcp --graph <file.utopia> [--read-only]
//! utopia mcp config <claude-code|claude-desktop|codex> [--graph <file.utopia>]
//! ```
//!
//! Three rules shape this file:
//!
//! - `--graph` is necessary (MC-13). The command never calls `paths::resolve_graph`, so an
//!   agent cannot reach a graph that the owner did not name.
//! - Start-up opens no store, so `initialize` and `server/discover` answer at once (MC-14).
//!   The store is opened for the first tool call and closed again for each of them
//!   ([`utopia_mcp::Router`] picks the headless call or the open app).
//! - The opener never prompts (MC-5): `stdin` is the protocol channel, so a passphrase
//!   prompt would be read as JSON-RPC input. `cli::open_store` asks, and this is why the
//!   command does not use it.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use utopia_mcp::{Options, Router};
use utopia_store::Store;

/// What both forms take.
pub const USAGE: &str = "\
usage:
  utopia mcp --graph <file.utopia> [--read-only]
        the MCP server on stdio for an AI client (Claude Code, Claude Desktop, Codex)
  utopia mcp config <claude-code|claude-desktop|codex> [--graph <file.utopia>]
        print the setup entry for that client, with this binary's path

--graph is necessary: utopia mcp never picks a graph by the start-up order of the app, so an
agent cannot write to a graph that you did not name. --read-only lowers the assistant access
level for this process and cannot raise it. The level itself is in the graph's access file
(<config>/mcp/<graph key>.json), which Settings → Assistants writes.";

/// `utopia mcp …`.
pub fn run(args: &[String]) -> Result<()> {
    if args
        .first()
        .is_some_and(|a| matches!(a.as_str(), "-h" | "--help" | "help"))
    {
        println!("{USAGE}");
        return Ok(());
    }
    match args.first().map(String::as_str) {
        Some("config") => config(&args[1..]),
        _ => serve(args),
    }
}

/// `utopia mcp --graph <file> [--read-only]`: serve one client until its input closes
/// (MC-6).
fn serve(args: &[String]) -> Result<()> {
    let parsed = parse(args, true)?;
    let Some(graph) = parsed.graph else {
        bail!(
            "utopia mcp needs --graph <file.utopia>: it never picks a graph by the start-up \
             order of the app (MC-13).\n\n{USAGE}"
        );
    };
    let config = crate::paths::config_dir();
    // No store here: the opener runs on the first tool call only (MC-14).
    let backend = Router::for_graph(graph.clone(), &config, open_without_prompt(graph));
    utopia_mcp::serve(
        std::io::stdin(),
        std::io::stdout(),
        &backend,
        Options {
            read_only: parsed.read_only,
            ..Options::new()
        },
    )
    .context("serving MCP on stdio")?;
    Ok(())
}

/// `utopia mcp config <client> [--graph <file>]`: the entry the owner copies (D6 A). The
/// text is `utopia_mcp::SetupClient`'s, so Settings → Assistants shows the same one.
fn config(args: &[String]) -> Result<()> {
    let Some(name) = args.first() else {
        bail!("utopia mcp config needs a client: claude-code, claude-desktop or codex.\n\n{USAGE}");
    };
    let Some(client) = utopia_mcp::SetupClient::parse(name) else {
        bail!(
            "unknown client {name:?}: utopia mcp config takes claude-code, claude-desktop \
             or codex.\n\n{USAGE}"
        );
    };
    // An entry without a graph is still shown, with a `<file.utopia>` placeholder: the
    // owner may copy it before choosing the graph.
    let parsed = parse(&args[1..], false)?;
    let graph = match parsed.graph {
        Some(graph) => Some(absolute(&graph)?),
        None => None,
    };
    let bin = std::env::current_exe().context("the path of this binary")?;
    println!("{}", client.entry(&bin, graph.as_deref()));
    Ok(())
}

/// The entry is used later, by a client that starts the server from whatever directory it
/// likes, so the graph it pins must not be relative (MC-13): resolve a relative `--graph`
/// against the directory the owner typed it in. Nothing here opens or requires the file.
fn absolute(graph: &Path) -> Result<PathBuf> {
    if graph.is_absolute() {
        return Ok(graph.to_path_buf());
    }
    Ok(std::env::current_dir()
        .context("the working directory")?
        .join(graph))
}

/// The arguments both forms share.
struct Parsed {
    graph: Option<PathBuf>,
    read_only: bool,
}

fn parse(args: &[String], read_only_allowed: bool) -> Result<Parsed> {
    let mut parsed = Parsed {
        graph: None,
        read_only: false,
    };
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        match arg {
            "--graph" => {
                let Some(value) = args.get(i + 1) else {
                    bail!("--graph needs a file.\n\n{USAGE}");
                };
                parsed.graph = Some(PathBuf::from(value));
                i += 2;
                continue;
            }
            "--read-only" if read_only_allowed => parsed.read_only = true,
            other if other.starts_with("--graph=") => {
                parsed.graph = Some(PathBuf::from(&other["--graph=".len()..]));
            }
            other => bail!("unknown argument {other:?}.\n\n{USAGE}"),
        }
        i += 1;
    }
    Ok(parsed)
}

/// The opener for the headless call: `Direct` calls it for each tool call and nothing else.
fn open_without_prompt(graph: PathBuf) -> utopia_mcp::Opener {
    Box::new(move || open_graph(&graph))
}

/// Open the graph at a given time, never asking for a passphrase (MC-5, design §2.3).
fn open_graph(graph: &Path) -> std::result::Result<Store, String> {
    if !graph.exists() {
        return Err(format!("There is no graph at {}.", graph.display()));
    }
    if !utopia_store::is_encrypted(graph) {
        return Store::open(graph)
            .map_err(|e| format!("The graph at {} could not be opened: {e}", graph.display()));
    }
    open_encrypted(graph, crate::paths::db_passphrase(graph))
}

/// The encrypted case, with the passphrase the caller found (a parameter so a test can
/// check both answers without an environment variable or the Keychain).
fn open_encrypted(graph: &Path, pass: Option<String>) -> std::result::Result<Store, String> {
    let Some(pass) = pass else {
        return Err(format!(
            "The graph at {} is encrypted at rest and no passphrase is available to utopia \
             mcp. Set UTOPIA_DB_PASSPHRASE, or keep the passphrase in the macOS Keychain \
             (service \"utopia-db\", account \"{}\"). utopia mcp cannot ask for one: its input \
             is the MCP protocol channel. Nothing was read or written.",
            graph.display(),
            crate::paths::graph_id(graph)
        ));
    };
    Store::open_with_key(graph, Some(&pass)).map_err(|e| {
        format!(
            "The graph at {} could not be opened with the stored passphrase: {e}",
            graph.display()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    /// MC-13: `--graph` is necessary, and a graph in the environment does not stand in for
    /// it: this command never uses the app's start-up order.
    #[test]
    fn mc_13_graph_is_necessary() {
        let err = run(&args(&[])).unwrap_err().to_string();
        assert!(err.contains("--graph"), "{err}");
        let err = run(&args(&["--read-only"])).unwrap_err().to_string();
        assert!(err.contains("--graph"), "{err}");
    }

    /// MC-13: the entry pins the graph. A relative path is resolved against the owner's
    /// working directory, because the client that reads the entry starts the server
    /// somewhere else.
    #[test]
    fn mc_13_a_graph_path_is_made_absolute() {
        assert_eq!(
            absolute(Path::new("/tmp/g.utopia")).unwrap(),
            PathBuf::from("/tmp/g.utopia")
        );
        let here = std::env::current_dir().unwrap();
        assert_eq!(
            absolute(Path::new("Notes.utopia")).unwrap(),
            here.join("Notes.utopia")
        );
    }

    #[test]
    fn both_forms_take_the_graph() {
        let parsed = parse(&args(&["--graph", "/tmp/g.utopia", "--read-only"]), true).unwrap();
        assert_eq!(parsed.graph.as_deref(), Some(Path::new("/tmp/g.utopia")));
        assert!(parsed.read_only);
        let parsed = parse(&args(&["--graph=/tmp/h.utopia"]), false).unwrap();
        assert_eq!(parsed.graph.as_deref(), Some(Path::new("/tmp/h.utopia")));
        assert!(!parsed.read_only);
        // `--read-only` belongs to the server, not to the entry printer.
        let err = parse(&args(&["--read-only"]), false)
            .err()
            .expect("--read-only belongs to the server")
            .to_string();
        assert!(err.contains("unknown argument"), "{err}");
        assert!(parse(&args(&["--graph"]), true).is_err());
        assert!(parse(&args(&["--nope"]), true).is_err());
    }

    /// MC-5: an encrypted graph without a passphrase gives a clear text instead of a
    /// prompt, because `stdin` carries the protocol. The check takes the passphrase as an
    /// argument here, so the test never reads the environment or the Keychain.
    #[test]
    fn mc_5_an_encrypted_graph_without_a_passphrase_is_a_text() {
        if !utopia_store::encryption_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let graph = dir.path().join("g.utopia");
        drop(Store::open(&graph).unwrap());
        utopia_store::set_encryption(&graph, None, Some("secret")).unwrap();
        let err = open_encrypted(&graph, None)
            .err()
            .expect("no passphrase, no open");
        assert!(err.contains("encrypted at rest"), "{err}");
        assert!(err.contains("UTOPIA_DB_PASSPHRASE"), "{err}");
        assert!(
            open_encrypted(&graph, Some("secret".into())).is_ok(),
            "the passphrase opens it"
        );
        let missing = open_graph(&dir.path().join("missing.utopia"))
            .err()
            .expect("a missing graph is an error");
        assert!(missing.contains("no graph"), "{missing}");
    }
}
