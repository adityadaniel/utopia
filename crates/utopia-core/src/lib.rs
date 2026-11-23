//! Utopia domain model (PRD §13.1). Pure Rust: no CRDT, no SQL, no UI.
//!
//! Everything here must stay deterministic and platform-independent, because
//! title normalization, date parsing and reference parsing are part of the
//! frozen cross-platform contract (§14.4).

pub mod attach;
pub mod command;
pub mod date;
pub mod error;
pub mod graph;
pub mod ids;
pub mod inline;
pub mod markdown;
pub mod outline;
pub mod query;
pub mod text;
pub mod title;
pub mod tree;
pub mod undo;
pub mod vim;

pub use command::{BlockKind, Command, DocDelta, PageKind, PageMeta, TextEdit};
pub use date::DateFormat;
pub use error::DomainError;
pub use ids::{BlockId, PageId};
pub use tree::{BlockNode, PageState};

/// Current wall clock as Unix milliseconds (UTC). Display-only; never used for ordering (§10.7).
pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}
