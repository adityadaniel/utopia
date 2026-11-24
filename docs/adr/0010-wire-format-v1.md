# 0010. Sync wire format v1

Status: Accepted. Source: `utopia-proto/src/lib.rs` (PRD §12.3, §12.6, §11.7, §14.4).

## Decision

- Payloads are opaque bytes (CRDT updates, or ciphertext with E2E), base64 in JSON. Request
  bodies may be zstd-compressed (`Content-Encoding: zstd`).
- Endpoints, all under `/v1/graphs/{gid}` with `Authorization: Bearer <token>`:
  `GET manifest` (`ETag: "<rev>"`, 304 on `If-None-Match`); `POST pages/{pid}/updates`
  (deduped by change id); `GET pages/{pid}/updates?since=<seq>`; `POST pages/{pid}/snapshot`
  (204, truncates the log rows it covers). A pull may start with a snapshot when the range
  was truncated. Compaction is client-driven (§11.7).
- `PROTOCOL_VERSION` goes up on any incompatible change to the shard or wire format.
- The manifest's `instance` is an opaque id of the relay store. A wiped relay gets a new one,
  so a client never mistakes a restarted sequence for the one it pulled. The field is
  additive: older relays send nothing and older clients ignore it.
