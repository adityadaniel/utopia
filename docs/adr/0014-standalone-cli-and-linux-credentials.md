# 0014. A standalone `ut` binary, and sync credentials in a file on Linux

Status: Accepted (2026-11-25). Source: `crates/utopia-cli`, `crates/utopia-sync/src/setup/file_secrets.rs`
(PRD §12, §13.8, §15.6; `docs/sync-modes.md` "Headless sync").

## Context

The headless commands (`doctor`, `export`, `capture`, `serve-api`, `mcp`, `sync`) never used
the window, but they lived in the app binary, so running them meant building GPUI. GPUI needs
a desktop toolkit, so the commands could not be built for a server, and a Mac that only syncs
carried the whole app.

On Linux there is no Keychain, and a headless server usually has no Secret Service (GNOME
Keyring, KWallet) either, so `sync setup` could not save the relay token and the E2E
passphrase at all; only `UTOPIA_SYNC_TOKEN` / `UTOPIA_SYNC_PASSPHRASE` on every run worked.

## Decision

- **One command table.** `utopia-cli` holds the headless modules and `dispatch`. The app
  binary (`utopia`) calls `dispatch` first and keeps only the window, `utopia://` links and
  its help. `ut` is a second binary of `utopia-cli` with the same commands. A command is added
  in `dispatch`, never in one binary, so the two cannot drift.
- **No GPUI below `utopia-cli`.** Its dependency tree is `api`, `mcp`, `store`, `sync`,
  `syncd`; `cargo tree -p utopia-cli | grep gpui` is empty.
- **`ut` speaks as `ut`.** The usage and error text is the app's, rewritten at the binary's
  edge by `cli::for_program`: `utopia <command>` at the start of a line or after a backtick
  becomes `ut <command>`, and the window and `utopia://` lines go. The modules keep one text.
- **Linux credentials: one private file.** `FileSecrets` stores
  `{"<service>": {"<account>": "<value>"}}` in `<config>/credentials.json`, with `<config>`
  resolved as the app resolves it on Linux (`UTOPIA_CONFIG_DIR`, `$XDG_CONFIG_HOME/utopia`,
  `~/.config/utopia`). `Keychain` uses it on Linux only; macOS keeps the Keychain and other
  platforms keep their behaviour.
  - The directory is created `0700` and the file is written `0600`.
  - A file that group or others can read is refused, with a message that names the path and
    `chmod 600`, so a credential never stays readable by mistake.
  - Each write goes to a new `0600` file in the same directory and is renamed over the old
    one, so a crash leaves the old file or the new one, never half of one.
  - A damaged file is an error, not an empty store, so a write never replaces credentials it
    could not read.
  - Errors are fixed messages and never quote a value (§15.6).

## Consequences

- A server needs only `ut` and the graph file. The secret is as safe as the user's account on
  that machine: root and the user can read it, as with an SSH key. A Secret Service store can
  be added later behind the same `Secrets` trait when a Linux desktop needs it.
- `sync launchd-plist` stays macOS-only; on Linux, `ut sync --continuous` runs under a
  systemd user unit or similar, which this repository does not ship.
- The encryption-at-rest passphrase (`UTOPIA_DB_PASSPHRASE`, Keychain `utopia-db`) has no
  Linux store yet; it comes from the environment or a prompt.
