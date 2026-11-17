//! The assistant access setting of one graph (PRD §13.8 MC-1, MC-2; docs/mcp-design.md §5).
//!
//! One file for each graph, `<config>/mcp/<graph_key>.json`:
//!
//! ```json
//! {"version": 1, "access": "off" | "read" | "write", "when_closed": false}
//! ```
//!
//! It is local UI state (rule 9): never in the CRDT, written atomically as the sync settings
//! are. Only this file sets the level. No environment variable and no flag can raise it
//! (MC-1); a caller may only lower it for one client.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::OpKind;

/// The only version of the file this build reads.
pub const VERSION: u32 = 1;

/// How much an assistant may do with the graph (MC-2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    /// Nothing is read or written (the default, MC-1).
    #[default]
    Off,
    /// Read ops only.
    Read,
    /// Every op.
    Write,
}

impl Level {
    /// Whether an op of `kind` may run at this level (MC-2). The process that runs the op
    /// asks this with [`crate::Op::kind`], so there is one classification.
    pub const fn permits(self, kind: OpKind) -> bool {
        match self {
            Level::Off => false,
            Level::Read => matches!(kind, OpKind::Read),
            Level::Write => true,
        }
    }

    pub(crate) const fn code(self) -> u8 {
        match self {
            Level::Off => 0,
            Level::Read => 1,
            Level::Write => 2,
        }
    }

    /// Unknown codes are Off: the safe reading (MC-1).
    pub(crate) const fn of(code: u8) -> Self {
        match code {
            1 => Level::Read,
            2 => Level::Write,
            _ => Level::Off,
        }
    }

    /// What a refused op answers (403). It is for an AI agent to pass on to its user, so it
    /// names where the owner changes the level and says that nothing happened.
    pub fn refusal(self, kind: OpKind) -> Option<&'static str> {
        if self.permits(kind) {
            return None;
        }
        Some(match self {
            Level::Read => {
                "This graph gives assistants read access only. Change it in Utopia: \
                 Settings → Assistants. Nothing was written."
            }
            _ => {
                "Assistant access is off for this graph. Turn it on in Utopia: \
                 Settings → Assistants. Nothing was read or written."
            }
        })
    }
}

/// The access setting of one graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Access {
    pub level: Level,
    /// Whether a tool call may run while the app is closed (MC-5, decision D1).
    pub when_closed: bool,
}

/// The file as it is on disk. Unknown fields are refused, so a file from a later version
/// that adds one is read as Off rather than half-understood.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    version: u32,
    access: Level,
    #[serde(default)]
    when_closed: bool,
}

/// `<config>/mcp/<graph_key>.json`.
pub fn access_file(config: &Path, graph: &Path) -> PathBuf {
    config
        .join("mcp")
        .join(format!("{}.json", utopia_store::graph_key(graph)))
}

impl Access {
    /// Read the setting. A missing file is Off and silent. A file that cannot be read, is
    /// malformed, has an unknown version or an unknown `access` value is also Off (MC-1),
    /// and the second value is a notice for the caller to log. This never panics.
    pub fn load(path: &Path) -> (Self, Option<String>) {
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (Self::default(), None),
            Err(e) => {
                return (
                    Self::default(),
                    Some(format!(
                        "assistant access is off: {} could not be read ({e})",
                        path.display()
                    )),
                )
            }
        };
        match serde_json::from_slice::<File>(&bytes) {
            Ok(f) if f.version == VERSION => (
                Self {
                    level: f.access,
                    when_closed: f.when_closed,
                },
                None,
            ),
            Ok(f) => (
                Self::default(),
                Some(format!(
                    "assistant access is off: {} has version {}, this build reads {VERSION}",
                    path.display(),
                    f.version
                )),
            ),
            Err(e) => (
                Self::default(),
                Some(format!(
                    "assistant access is off: {} is malformed ({e})",
                    path.display()
                )),
            ),
        }
    }

    /// Write the setting atomically: a temporary file in the same directory, flushed, then
    /// renamed over the old one, so a reader sees the old file or the new one, never a part.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        use std::io::Write;
        let dir = path.parent().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "no parent directory")
        })?;
        std::fs::create_dir_all(dir)?;
        let body = serde_json::to_vec_pretty(&File {
            version: VERSION,
            access: self.level,
            when_closed: self.when_closed,
        })
        .map_err(std::io::Error::other)?;
        let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
        tmp.write_all(&body)?;
        tmp.as_file().sync_all()?;
        tmp.persist(path).map_err(|e| e.error)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn load_text(body: &str) -> (Access, Option<String>) {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("a.json");
        std::fs::write(&f, body).unwrap();
        Access::load(&f)
    }

    #[test]
    fn mc_1_a_missing_file_is_off_and_silent() {
        let dir = tempfile::tempdir().unwrap();
        let (a, notice) = Access::load(&dir.path().join("mcp").join("none.json"));
        assert_eq!(a, Access::default());
        assert_eq!(a.level, Level::Off);
        assert!(!a.when_closed);
        assert_eq!(notice, None);
    }

    #[test]
    fn mc_1_a_bad_file_is_off_with_a_notice() {
        for body in [
            "",
            "not json",
            "[]",
            r#"{"version": 1}"#,
            r#"{"version": 2, "access": "write", "when_closed": true}"#,
            r#"{"version": 1, "access": "admin", "when_closed": false}"#,
            r#"{"version": 1, "access": "WRITE", "when_closed": false}"#,
            r#"{"version": 1, "access": "write", "when_closed": "yes"}"#,
            r#"{"version": 1, "access": "write", "extra": 1}"#,
            "\u{0}\u{1}\u{2}",
        ] {
            let (a, notice) = load_text(body);
            assert_eq!(a, Access::default(), "{body:?} is Off");
            assert!(notice.is_some(), "{body:?} gives a notice");
        }
    }

    #[test]
    fn mc_1_a_valid_file_is_read() {
        let (a, notice) = load_text(r#"{"version": 1, "access": "read", "when_closed": false}"#);
        assert_eq!((a.level, a.when_closed, notice), (Level::Read, false, None));
        let (a, notice) = load_text(r#"{"version": 1, "access": "write", "when_closed": true}"#);
        assert_eq!((a.level, a.when_closed, notice), (Level::Write, true, None));
        let (a, notice) = load_text(r#"{"version": 1, "access": "off"}"#);
        assert_eq!((a.level, a.when_closed, notice), (Level::Off, false, None));
    }

    #[test]
    fn mc_2_levels_permit_by_op_kind() {
        assert!(!Level::Off.permits(OpKind::Read));
        assert!(!Level::Off.permits(OpKind::Write));
        assert!(Level::Read.permits(OpKind::Read));
        assert!(!Level::Read.permits(OpKind::Write));
        assert!(Level::Write.permits(OpKind::Read));
        assert!(Level::Write.permits(OpKind::Write));
        for l in [Level::Off, Level::Read, Level::Write] {
            assert_eq!(Level::of(l.code()), l);
        }
        assert_eq!(Level::of(200), Level::Off);
    }

    #[test]
    fn mc_1_save_then_load() {
        let dir = tempfile::tempdir().unwrap();
        let graph = dir.path().join("g.utopia");
        std::fs::write(&graph, b"").unwrap();
        let f = access_file(&dir.path().join("config"), &graph);
        assert_eq!(
            f,
            dir.path()
                .join("config/mcp")
                .join(format!("{}.json", utopia_store::graph_key(&graph)))
        );
        for a in [
            Access {
                level: Level::Write,
                when_closed: true,
            },
            Access {
                level: Level::Read,
                when_closed: false,
            },
            Access::default(),
        ] {
            a.save(&f).unwrap();
            assert_eq!(Access::load(&f), (a, None));
        }
        let v: serde_json::Value = serde_json::from_slice(&std::fs::read(&f).unwrap()).unwrap();
        assert_eq!(
            v,
            serde_json::json!({"version": 1, "access": "off", "when_closed": false})
        );
    }

    #[test]
    fn mc_1_save_is_atomic() {
        let dir = tempfile::tempdir().unwrap();
        let mcp = dir.path().join("mcp");
        let f = mcp.join("k.json");
        Access {
            level: Level::Read,
            when_closed: false,
        }
        .save(&f)
        .unwrap();
        #[cfg(unix)]
        let inode = {
            use std::os::unix::fs::MetadataExt;
            std::fs::metadata(&f).unwrap().ino()
        };
        Access {
            level: Level::Write,
            when_closed: true,
        }
        .save(&f)
        .unwrap();
        // The new body arrived by rename: a new file replaced the old one, not a rewrite in
        // place that a reader could see half done.
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            assert_ne!(std::fs::metadata(&f).unwrap().ino(), inode);
        }
        // No temporary file is left next to it.
        let names: Vec<_> = std::fs::read_dir(&mcp)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, vec![std::ffi::OsString::from("k.json")]);
        assert_eq!(Access::load(&f).0.level, Level::Write);
    }
}
