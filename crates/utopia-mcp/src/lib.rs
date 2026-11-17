//! The MCP server for Utopia (PRD §13.8, `docs/mcp-design.md` §3).
//!
//! An AI client starts the server on stdio ([`protocol::serve`]) and calls the tools of
//! [`tools`]. Each call becomes one [`utopia_api::Op`] that a [`Backend`] runs. [`Router`]
//! picks where: [`Direct`] is the headless call (no app running, one store for the call,
//! [`utopia_api::handle`]), and [`Remote`] sends the op to the app that owns the graph
//! (`POST /v1/op`). Either way the bridge adds no second write path (rule 13, MC-3).
//!
//! Two rules hold for everything in this crate:
//!
//! - `stdout` carries protocol messages only (MC-6). Nothing here prints to it; logs go to
//!   `stderr`.
//! - No code path opens a store before the first tool call, so `initialize` and
//!   `server/discover` answer at once (MC-14).

pub mod backend;
pub mod clients;
pub mod protocol;
pub mod tools;

pub use backend::{
    Backend, BackendError, Direct, Opener, Remote, Router, ALREADY_OPEN, OPEN_UTOPIA, OWNER_NO_API,
};
pub use clients::{Client as SetupClient, GRAPH_PLACEHOLDER};
pub use protocol::{serve, Options};
