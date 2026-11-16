//! The MCP server for Utopia (PRD §13.8, `docs/mcp-design.md` §3).
//!
//! An AI client starts the server on stdio ([`protocol::serve`]) and calls the tools of
//! [`tools`]. Each call becomes one [`utopia_api::Op`] that a [`Backend`] runs; the
//! headless backend is [`Direct`], which opens the graph for that one call and runs the op
//! through [`utopia_api::handle`], so the bridge adds no second write path (rule 13, MC-3).
//!
//! Two rules hold for everything in this crate:
//!
//! - `stdout` carries protocol messages only (MC-6). Nothing here prints to it; logs go to
//!   `stderr`.
//! - No code path opens a store before the first tool call, so `initialize` and
//!   `server/discover` answer at once (MC-14).

pub mod backend;
pub mod protocol;
pub mod tools;

pub use backend::{Backend, BackendError, Direct, Opener};
pub use protocol::{serve, Options};
