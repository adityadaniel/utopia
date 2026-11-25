use utopia_cli::{cli, mcp, sync_cli};

const PROGRAM: &str = "ut";

fn is_help(arg: Option<&String>) -> bool {
    arg.is_some_and(|a| matches!(a.as_str(), "-h" | "--help" | "help"))
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || is_help(args.first()) {
        println!(
            "ut: Utopia's commands without the desktop app.\n\n{}",
            cli::for_program(cli::USAGE, PROGRAM)
        );
        return;
    }
    match (args[0].as_str(), is_help(args.get(1))) {
        ("sync", true) => return println!("{}", cli::for_program(sync_cli::USAGE, PROGRAM)),
        ("mcp", true) => return println!("{}", cli::for_program(mcp::USAGE, PROGRAM)),
        _ => {}
    }
    let code = match utopia_cli::dispatch(&args) {
        Some(Ok(code)) => code,
        Some(Err(e)) => {
            eprintln!("Error: {}", cli::for_program(&format!("{e:?}"), PROGRAM));
            1
        }
        None => {
            let what = if args[0].starts_with("utopia://") {
                "opening a utopia:// link needs the desktop app"
            } else {
                "unknown command"
            };
            eprintln!(
                "ut: {what}: {:?}\n\n{}",
                args[0],
                cli::for_program(cli::USAGE, PROGRAM)
            );
            64
        }
    };
    std::process::exit(code);
}
