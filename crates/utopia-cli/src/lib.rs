pub mod api_launch;
pub mod cli;
pub mod mcp;
pub mod paths;
pub mod sync_cli;
pub mod sync_target;

use anyhow::{bail, Result};

pub fn dispatch(args: &[String]) -> Option<Result<i32>> {
    let usage = || -> Result<i32> { bail!("{}", cli::USAGE) };
    let code = match args.first().map(String::as_str)? {
        "doctor" => {
            let Some(graph) = args.get(1) else {
                return Some(usage());
            };
            let rebuild = args.iter().any(|a| a == "--rebuild");
            cli::doctor(graph.as_ref(), rebuild).map(|ok| if ok { 0 } else { 1 })
        }
        "export" => {
            let (Some(graph), Some(dir)) = (args.get(1), args.get(2)) else {
                return Some(usage());
            };
            cli::export(graph.as_ref(), dir.as_ref()).map(|()| 0)
        }
        "import-roam" => {
            let (Some(graph), Some(file)) = (args.get(1), args.get(2)) else {
                return Some(usage());
            };
            cli::import_roam(graph.as_ref(), file.as_ref()).map(|()| 0)
        }
        "capture" => {
            let text = args[1..].join(" ");
            match paths::resolve_graph(None, &paths::config_dir(), &paths::graphs_dir()) {
                Some(graph) => cli::capture(&graph, &paths::config_dir(), &text).map(|()| 0),
                None => Err(anyhow::anyhow!("no graph found; set UTOPIA_GRAPH")),
            }
        }
        "encrypt" => {
            let Some(graph) = args.get(1) else {
                return Some(usage());
            };
            cli::encrypt(graph.as_ref()).map(|()| 0)
        }
        "decrypt" => {
            let Some(graph) = args.get(1) else {
                return Some(usage());
            };
            cli::decrypt(graph.as_ref()).map(|()| 0)
        }
        "serve-api" => {
            let Some(graph) = args.get(1) else {
                return Some(usage());
            };
            let addr = args[2..]
                .iter()
                .find(|a| a.as_str() != "--sync")
                .map_or("127.0.0.1:0", String::as_str);
            if args.iter().any(|a| a == "--sync") {
                sync_cli::continuous(graph.as_ref(), &paths::config_dir(), addr)
            } else {
                cli::serve_api(graph.as_ref(), addr, &paths::config_dir()).map(|()| 0)
            }
        }
        "mcp" => mcp::run(&args[1..]).map(|()| 0),
        "sync" => sync_cli::run(&args[1..]),
        _ => return None,
    };
    Some(code)
}
