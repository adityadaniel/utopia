//! The real Utopia sync client against the Cloudflare Worker relay (PRD §12.8,
//! `relay-cloudflare/`): the relay contract of `tests/contract/mod.rs`, the same scenarios
//! `tests/http.rs` runs against `utopia-syncd`, so the Worker is proven by the app's own
//! transport rather than by the TypeScript tests that mirror it. `tests/differential.rs`
//! compares the two relays' raw HTTP answers.
//!
//! `relay-cloudflare/scripts/test-against-rust-client.sh` (`make relay-check`) starts
//! `wrangler dev --local` and sets the environment `tests/contract/targets.rs` describes.
//! With `UTOPIA_RELAY_URL` unset every test prints a skip and returns, so
//! `cargo test --workspace` (`make check`) stays green without Node; with
//! `UTOPIA_RELAY_REQUIRED=1` (the script, CI) a skip is a failure.

mod contract;

contract::contract_suite!(contract::targets::worker);
