//! Utopia's headless commands, with no window toolkit (no GPUI): `doctor`, `export`,
//! `import-roam`, `capture`, `encrypt`/`decrypt`, `serve-api`, `mcp` and `sync`.
//!
//! The desktop app (`utopia-app`) and the standalone `ut` binary both run these modules, so
//! a command behaves the same from either; the app adds only the window and what needs it.

pub mod api_launch;
pub mod cli;
pub mod mcp;
pub mod paths;
pub mod sync_cli;
pub mod sync_target;
