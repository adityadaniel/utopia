//! The setup entry for each MCP client (PRD §13.8 MC-12, MC-13; `docs/mcp-design.md` §6).
//!
//! Decision D6 A: Utopia shows the command or the config entry and the owner copies it. This
//! module makes that text, so `utopia mcp config <client>` and the Settings → Assistants tab
//! of a later slice show the same one. Each entry pins `--graph`, so a client can only reach
//! the graph the owner named (MC-13); `graph` is `None` when the text is shown before the
//! owner picks one, which leaves a `<file.utopia>` placeholder.
//!
//! The shapes are the ones in design §6: the Claude Code command puts its options before
//! the server name, and the Claude Desktop entry is the object that goes below
//! `mcpServers`.

use std::path::Path;

/// What an entry says when no graph is chosen yet. It is not a path, so it is never quoted.
pub const GRAPH_PLACEHOLDER: &str = "<file.utopia>";

/// The clients of the first version (design §1.1), in the order Settings shows them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Client {
    ClaudeCode,
    ClaudeDesktop,
    Codex,
}

impl Client {
    /// Every client, in the order the Settings tab lists them.
    pub const ALL: [Client; 3] = [Client::ClaudeCode, Client::ClaudeDesktop, Client::Codex];

    /// The name `utopia mcp config <name>` takes and the one the tab shows.
    pub const fn name(self) -> &'static str {
        match self {
            Client::ClaudeCode => "claude-code",
            Client::ClaudeDesktop => "claude-desktop",
            Client::Codex => "codex",
        }
    }

    /// Read a name back. An unknown name is `None`, so the command can say what it takes.
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.name() == name.trim())
    }

    /// The text the owner copies. For Claude Code and Codex it is one command; for Claude
    /// Desktop it is the JSON object to put below `mcpServers` (design §6).
    ///
    /// `bin` is the path of this binary and `graph` the graph the entry pins. Paths are
    /// quoted only when they need it, so the common case reads exactly as in the design.
    pub fn entry(self, bin: &Path, graph: Option<&Path>) -> String {
        // The commands need shell words; the JSON entry needs the paths themselves.
        let graph_word = graph.map_or(GRAPH_PLACEHOLDER.to_string(), shell_word);
        let bin_word = shell_word(bin);
        match self {
            Client::ClaudeCode => {
                format!("claude mcp add --scope user utopia -- {bin_word} mcp --graph {graph_word}")
            }
            Client::Codex => {
                format!("codex mcp add utopia -- {bin_word} mcp --graph {graph_word}")
            }
            Client::ClaudeDesktop => {
                let shown = graph.map_or(GRAPH_PLACEHOLDER.to_string(), |g| {
                    g.to_string_lossy().into_owned()
                });
                format!(
                    "\"utopia\": {{\n  \"command\": {},\n  \"args\": [\"mcp\", \"--graph\", {}]\n}}",
                    json_string(bin.to_string_lossy().into_owned()),
                    json_string(shown)
                )
            }
        }
    }

    /// Where the entry goes, as the prose beside the copy control in Settings (design §6).
    pub const fn note(self) -> &'static str {
        match self {
            Client::ClaudeCode => {
                "Run this in a terminal. The user scope keeps the entry in ~/.claude.json."
            }
            Client::ClaudeDesktop => {
                "Put this below mcpServers in ~/Library/Application Support/Claude/\
                 claude_desktop_config.json, then start Claude Desktop again."
            }
            Client::Codex => {
                "Run this in a terminal. It writes [mcp_servers.utopia] to \
                 ~/.codex/config.toml."
            }
        }
    }
}

/// A path as one shell word: bare when nothing in it needs quoting, single-quoted
/// otherwise.
fn shell_word(path: &Path) -> String {
    let s = path.to_string_lossy();
    let bare = !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "._-/+=:@,".contains(c));
    if bare {
        s.into_owned()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

/// A JSON string literal (the Claude Desktop entry is JSON, not a shell command).
fn json_string(s: String) -> String {
    serde_json::Value::String(s).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// MC-13, design §6: each entry pins the graph and names this binary, in the shape the
    /// client's documentation gives.
    #[test]
    fn mc_13_each_entry_pins_the_binary_and_the_graph() {
        let bin = Path::new("/Applications/Utopia.app/Contents/MacOS/utopia");
        let graph = Path::new("/Users/ana/Documents/Utopia/Notes.utopia");
        assert_eq!(
            Client::ClaudeCode.entry(bin, Some(graph)),
            "claude mcp add --scope user utopia -- \
             /Applications/Utopia.app/Contents/MacOS/utopia mcp --graph \
             /Users/ana/Documents/Utopia/Notes.utopia"
        );
        assert_eq!(
            Client::Codex.entry(bin, Some(graph)),
            "codex mcp add utopia -- /Applications/Utopia.app/Contents/MacOS/utopia mcp \
             --graph /Users/ana/Documents/Utopia/Notes.utopia"
        );
        assert_eq!(
            Client::ClaudeDesktop.entry(bin, Some(graph)),
            "\"utopia\": {\n  \"command\": \"/Applications/Utopia.app/Contents/MacOS/utopia\",\n  \
             \"args\": [\"mcp\", \"--graph\", \
             \"/Users/ana/Documents/Utopia/Notes.utopia\"]\n}"
        );
    }

    /// A path with a space stays one argument in the commands and one string in the JSON.
    #[test]
    fn a_path_with_a_space_is_quoted() {
        let bin = Path::new("/Users/ana/My Apps/utopia");
        let graph = Path::new("/Users/ana/My Notes/g.utopia");
        let code = Client::ClaudeCode.entry(bin, Some(graph));
        assert_eq!(
            code,
            "claude mcp add --scope user utopia -- '/Users/ana/My Apps/utopia' mcp --graph \
             '/Users/ana/My Notes/g.utopia'"
        );
        let desktop = Client::ClaudeDesktop.entry(bin, Some(graph));
        let json: serde_json::Value = serde_json::from_str(&format!("{{{desktop}}}")).unwrap();
        assert_eq!(json["utopia"]["command"], "/Users/ana/My Apps/utopia");
        assert_eq!(
            json["utopia"]["args"],
            serde_json::json!(["mcp", "--graph", "/Users/ana/My Notes/g.utopia"])
        );
    }

    /// Without a graph the entry is still the right shape, with the placeholder unquoted.
    #[test]
    fn an_unchosen_graph_is_a_placeholder() {
        let bin = Path::new("/bin/utopia");
        assert!(Client::ClaudeCode
            .entry(bin, None)
            .ends_with("mcp --graph <file.utopia>"));
        assert!(Client::ClaudeDesktop
            .entry(bin, None)
            .contains("\"--graph\", \"<file.utopia>\""));
    }

    #[test]
    fn names_round_trip() {
        for c in Client::ALL {
            assert_eq!(Client::parse(c.name()), Some(c));
        }
        assert_eq!(Client::parse("nope"), None);
        assert_eq!(Client::parse(" claude-code "), Some(Client::ClaudeCode));
    }
}
