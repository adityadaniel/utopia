//! Test-only access to the crate's own source text.
//!
//! The design-system rules that a type system cannot express are policed by reading the
//! code: no shadows (DESIGN.md §2.4), no colour named outside [`crate::theme`] (§2), and
//! `bullet`/`disabled` never used for text (§2.1). The tests live next to the tokens; this
//! module is the file walk they share.

use std::path::{Path, PathBuf};

/// Every Rust source file of this crate (and of the app, when this is the repo checkout),
/// with its text.
pub fn ui_sources() -> Vec<(PathBuf, String)> {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut roots = vec![manifest.join("src")];
    let app = manifest.join("../utopia-app/src");
    if app.exists() {
        roots.push(app);
    }
    let mut out = Vec::new();
    for root in roots {
        collect(&root, &mut out);
    }
    out
}

fn collect(dir: &Path, out: &mut Vec<(PathBuf, String)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            if let Ok(src) = std::fs::read_to_string(&path) {
                out.push((path, src));
            }
        }
    }
}
