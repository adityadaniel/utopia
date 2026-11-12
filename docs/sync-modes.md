# Sync modes: how to choose, with pros and cons

This document compares the sync modes in **Settings → Sync**: Off, Relay, Nearby Mac and
Shared folder. Use it to choose a mode for a graph. It does not tell you how the engine
works. For that, read `PRD.md` §12 and the `docs/state-machines/sync-*.md` files.

The statements here agree with the code at the time of writing. Each statement gives its
source, so that you can examine it again when the code changes.

## Summary

| | Off | Relay | Nearby Mac | Shared folder |
|---|---|---|---|---|
| Infrastructure | None | A `utopia-syncd` server that you run | One Mac runs Utopia as host | A folder that a file-sync tool (iCloud Drive, Dropbox, Syncthing) copies |
| Works away from home | – | Yes | No (same network only) | Yes, if the sync tool does |
| Delay from edit to other device | – | Seconds (WebSocket notice) | Seconds (WebSocket notice) | Polling interval **plus** the delay of the sync tool |
| Account or token | – | Token (recommended) | Token on the host | None |
| End-to-end encryption | – | Yes | Yes | Yes |
| Attachments | – | Yes | Yes | Yes |
| Server-side storage grows | – | No: snapshots compact the log | No: snapshots compact the log | **Yes**: no compaction |
| Mobile shell (§14) | – | Yes | Yes, as a Relay URL | No |
| Host must stay on | – | The server | **The host Mac, with Utopia open** | No |

The short answer:

- **One Mac only:** use **Off**. Rolling backups (§15.3) still protect you.
- **Two Macs at home:** use **Nearby Mac**. It needs no server and no account.
- **Two Macs in different places, or a phone:** use **Relay**, with a passphrase for
  end-to-end encryption.
- **No server, and you already pay for iCloud Drive or Dropbox:** use **Shared folder**, and
  accept the slower delivery and the folder that always grows.

## What is the same in every mode

Read this first. These points are true for all modes, so they are not pros or cons of one
mode.

- **The merge is the same.** Every mode moves the same Loro updates, one shard per page
  (§12.2). The conflict rules in §12.4 apply to all modes: edits on two devices merge, and no
  mode can lose an edit because a different device wrote last.
- **Editing never waits for the network.** Edits go into the outbox in the same SQLite
  transaction as the change (§12.6, `sync-engine-outbox-and-cursor.md`). When the
  destination is not available, the outbox keeps them and the next round sends them.
- **The poll cadence is the same.** A round runs each 5 s while there is activity, and
  goes back to one round each 60 s when the rounds are quiet
  (`crates/utopia-ui/src/sync_runner.rs`, `FAST` and `SLOW`).
- **End-to-end encryption is available in every mode.** It wraps the transport
  (`crates/utopia-sync/src/e2e.rs`), so page updates, snapshots and attachments are sealed
  before they leave the device. See [End-to-end encryption](#end-to-end-encryption).
- **Attachments are verified.** A blob that does not hash to its name is refused (rule 12).
  At most 8 blobs go each way in one round (`BLOBS_PER_ROUND`).
- **Local UI state does not sync.** Collapse, scroll, caret, sidebar width and
  `settings.json` stay on the device (§12.5, rule 9).
- **The graph id is frozen after the first sync** (§12.5). Two devices sync one graph only
  if they use the same graph id. A copy of a graph file must use a different id.
- **A change of destination sends everything again.** Relay to folder, or one relay to a
  different relay, queues a full update of every shard and every attachment
  (`Store::prepare_sync_destination`). On a large graph, this takes time on the UI thread
  when you click Apply.

## Off

Sync is off. This is the default.

**Pros**

- Nothing leaves the device. There is nothing to set up, secure or pay for.
- No network work and no outbox growth.

**Cons**

- Your notes are on one device only. The rolling backups protect you from mistakes, not from
  the loss of the disk.
- When you start sync later, the first round sends the full graph.

## Relay

The app talks HTTP to a relay, `utopia-syncd` (§12.3, §12.8). The relay keeps an
append-only log of updates for each page. It does not know about the CRDT and does not
resolve conflicts. A WebSocket sends a short notice when a page changes, and the client then
pulls at once (§12.3 "Realtime").

Today, "Relay" means a relay that **you** run: `cargo run -p utopia-syncd`, configured with
`UTOPIA_SYNCD_ADDR`, `UTOPIA_SYNCD_DB`, `UTOPIA_SYNCD_TOKENS` and `UTOPIA_SYNCD_QUOTA_MB`. The
hosted relay in §12.7 item 1 does not exist yet.

**Pros**

- **Works from any network.** The devices do not have to be on at the same time. The relay
  keeps the updates until each device pulls them.
- **Fast.** The WebSocket notice makes delivery take seconds, not one poll interval.
- **Storage stays small.** When the log of a page is longer than 256 KB or 200 updates, a
  client uploads a snapshot and the relay deletes the updates that the snapshot covers
  (§12.3 step 5, `SNAPSHOT_TAIL_BYTES`, `SNAPSHOT_TAIL_UPDATES`). This also works with
  end-to-end encryption, because the client makes the snapshot (§11.7).
- **The mobile shell can use it.** The FFI core syncs over HTTP only
  (`crates/utopia-ffi/src/lib.rs`).
- **Clear failure reports.** The relay gives HTTP status codes, so the app can show
  "needs auth", "offline" or "unsupported version" (§12.6). A refused token goes back to
  one retry each 60 s and recovers without a restart (`sync-runner-round.md`).
- **Quota.** The relay limits the storage for each graph (default 1024 MB).

**Cons**

- **You must run and maintain a server.** Keep it on, back it up and update it.
- **No TLS in the relay.** `utopia-syncd` serves plain HTTP. Put it behind a reverse proxy
  with HTTPS, or reach it only through a private network such as Tailscale. Do not expose
  it to the internet over plain HTTP.
- **Without a token, there is no authentication.** If `UTOPIA_SYNCD_TOKENS` is not set, any
  client that can reach the relay can read and write. Use this only on a local machine.
- **Without end-to-end encryption, the operator can read your notes.** The relay stores the
  updates as they arrive. Settings → Sync shows "Not encrypted" in this case.
- **The token and passphrase must go to each device.** Settings → Sync shows the graph id
  and the steps for the other device, but you must type the secrets on it yourself.

**Use Relay when** your devices are in different places, when one of them is a phone, or
when you already have a small always-on server.

## Nearby Mac

One Mac **hosts** the graph: Utopia runs the same relay in-process, keeps its database at
`<config>/sync/<sha256>.relay.db`, and announces it on the local network with mDNS. The other
Mac chooses Nearby Mac, finds the host, and then syncs with it as with a relay
(`crates/utopia-syncd/src/lan.rs`, `docs/state-machines/lan-relay-discovery.md`).

**Pros**

- **No server and no account.** You only turn on "Host this graph from this Mac".
- **Fast.** It is a relay, so it has the WebSocket notices and the compaction of Relay.
- **The data stays in your home network.** It goes from one Mac to the other directly.
- **It can also work over Tailscale.** On the far device, choose Relay and type
  `http://<host IP>:<port>`. Then it works away from home as well.

**Cons**

- **The host Mac must be awake and Utopia must be open on it.** If the host sleeps, the
  other Mac cannot sync until it wakes. The other Mac keeps its edits in the outbox.
- **Same network only** for discovery. mDNS does not cross routers or most guest networks.
- **Discovery runs for 5 s, one time.** If no host answers, Apply shows "No Nearby Mac found
  for this graph" and the app does not search again. Turn on the host, then click Apply
  again (`crates/utopia-app/src/sync_setup.rs`).
- **The host needs a token**, and you must type it on the other Mac. Settings refuses to
  host without one ("Hosting needs a relay token."). The token keeps other devices on the
  network out; use end-to-end encryption as well if other people use the network.
- **Plain HTTP.** The traffic is not TLS. End-to-end encryption protects the content; the
  graph id and the timing are visible on the network.
- **Not QUIC.** §12.7 item 4 describes mDNS + QUIC. The implementation uses the HTTP relay
  protocol over TCP.
- **Not tested between two real Macs.** Tests and manual checks in the UI cover it
  (`AGENTS.md`, Known limits).

**Use Nearby Mac when** your Macs are on the same network most of the time and one of them
is usually open.

## Shared folder

The app writes each update as a new file, `pages/<page>/<device>/<seq>.upd`, in a folder that
a file-sync tool copies between devices (§12.7 item 3, `crates/utopia-sync/src/folder.rs`).
The app never overwrites or deletes a file, so the sync tool cannot destroy data with a
conflict copy.

**Pros**

- **No server, no account, no token.** You use the iCloud Drive, Dropbox or Syncthing that
  you already have.
- **Works from any network** where the sync tool works, and the devices do not have to be
  on at the same time.
- **Safe with file-sync tools.** Files are only added, and imports are idempotent, so a
  file that arrives two times or late does no harm.
- **End-to-end encryption works here too.** Without it, the provider of the folder can read
  your notes.

**Cons**

- **The folder always grows.** There is no compaction, because a device must not delete
  the files of other devices (`folder.rs`). Each edit adds a file for ever.
- **Slower and less predictable.** There is no WebSocket notice: the app finds changes only
  when it polls (5 s to 60 s), and only after the sync tool has copied the files. iCloud
  Drive can wait minutes, and it can leave files as placeholders that are not downloaded.
- **More work on each change.** The folder has no global order, so a pull reads every file
  of a changed page again (`AGENTS.md`, Known limits). A page with a long history costs more
  each time it changes.
- **Less clear failure reports.** A file that is not yet copied looks the same as no
  change. The app cannot tell you that the other device is behind.
- **"Test connection" creates the `pages/` directory** in the folder.
- **Use a folder of its own for each graph.** Settings → Sync asks for this. Two graphs in
  one folder mix their files.
- **Not for the mobile shell.** The FFI core does not offer the folder transport.

**Use Shared folder when** you do not want to run anything, you already use a file-sync
tool, and a delay of a minute or more is acceptable.

## End-to-end encryption

End-to-end encryption is not a mode. You can turn it on with Relay, Nearby Mac or Shared
folder: type a passphrase in Settings → Sync (§11.7, `crates/utopia-sync/src/e2e.rs`).

**Pros**

- The relay, the host Mac's relay database, or the folder provider stores only ciphertext
  (XChaCha20-Poly1305, key from Argon2id). Attachments are sealed as well.
- Compaction still works, because the client makes the snapshots.
- A key-check marker stops a device that has the wrong passphrase. It cannot push updates
  that the other devices could not read (`AGENTS.md`, Sync).

**Cons**

- **If you lose the passphrase, you lose the server copy.** The local graph stays readable
  (§11.7).
- **You cannot change the passphrase.** After the first save, Settings → Sync refuses a
  change or a removal. In-place re-keying is not supported. To change it, start a new graph
  id (`AGENTS.md`, Sync).
- **The marker is an offline verifier.** A person who has the graph file can test
  passphrase guesses against it at the Argon2 cost. Do not use a passphrase that you use
  somewhere else (`PRD.md`, security section).
- The graph id, page ids, sizes and times are not encrypted. The relay or folder can see
  how many pages you have and when they change, but not what they contain.

Recommendation: turn it on for every mode where a different person or company keeps the
data (a relay on a rented server, iCloud Drive, Dropbox).

## Sources

- `PRD.md` §11.7 (encryption), §12.1–12.8 (sync design), and the security section on the
  key-check marker.
- `AGENTS.md`: the "Sync" section and "Known limits".
- `crates/utopia-sync/src/{folder,http,e2e,lib}.rs`, `crates/utopia-sync/src/setup/`.
- `crates/utopia-syncd/src/{main,lan}.rs`.
- `crates/utopia-app/src/sync_setup.rs`, `crates/utopia-ui/src/sync_setup.rs`,
  `crates/utopia-ui/src/sync_runner.rs`.
- `docs/state-machines/`: `sync-engine-outbox-and-cursor.md`, `sync-runner-round.md`,
  `sync-compaction-snapshots.md`, `lan-relay-discovery.md`, `realtime-listener.md`,
  `ffi-mobile-sync.md`.
