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

pub use command::{BlockKind, Command, DocDelta, PageKind, PageMeta, TextEdit};
pub use date::DateFormat;
pub use error::DomainError;
pub use ids::{BlockId, PageId};
pub use tree::{BlockNode, PageState};

pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}
