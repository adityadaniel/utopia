//! Encryption at rest (P2, §15.6, open question 7): the graph file can be a SQLCipher
//! database. The passphrase never touches the file or the sync relay; the app reads it
//! from the macOS keychain or asks for it.

use std::io::Read;
use std::path::Path;

use rusqlite::{Connection, ErrorCode};

use crate::{Result, StoreError};

/// SQLite's fixed file header. A plain database starts with it; SQLCipher replaces the
/// header with a random salt, so a file that still starts with it is never ciphertext
/// (`docs/state-machines/encryption-at-rest-state.md`).
const SQLITE_HEADER: &[u8; 16] = b"SQLite format 3\0";

/// What a graph file is before a passphrase is applied: an ordinary SQLite database, an
/// encrypted one, or damage. `Corrupt` is the state that used to be reported as
/// `Encrypted`: a file that still carries SQLite's header cannot be unlocked by a
/// passphrase, so the UI must not show it as locked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    /// Missing, empty, or a readable/unreadable SQLite file.
    Plain,
    /// No SQLite header, and SQLite cannot read it without a key.
    Encrypted,
    /// Carries the SQLite header but does not open as a database.
    Corrupt,
}

impl FileKind {
    /// Classify a file SQLite refused to read (`SQLITE_NOTADB`) from its header alone.
    fn after_not_a_database(path: &Path) -> Self {
        if has_sqlite_header(path) {
            Self::Corrupt
        } else {
            Self::Encrypted
        }
    }
}

/// Whether `path` starts with SQLite's header. A missing or empty file counts as plain:
/// SQLite creates a fresh database there.
fn has_sqlite_header(path: &Path) -> bool {
    let Ok(mut file) = std::fs::File::open(path) else {
        return true;
    };
    let mut header = [0u8; 16];
    match file.read_exact(&mut header) {
        Ok(()) => header == *SQLITE_HEADER,
        // Shorter than the header: an empty file is a new plain database, and anything
        // else is too short to be SQLCipher's 16-byte salt.
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => true,
        Err(_) => true,
    }
}

/// Classify a graph without a passphrase. A headerless file that SQLite refuses is
/// `Encrypted`: ciphertext and damage are indistinguishable there, so the safe reading is
/// the one a passphrase can still resolve.
pub fn file_kind(path: &Path) -> FileKind {
    if !path.metadata().is_ok_and(|m| m.len() > 0) {
        return FileKind::Plain;
    }
    match open_keyed(path, None) {
        Ok(_) => FileKind::Plain,
        Err(StoreError::Encrypted) => FileKind::after_not_a_database(path),
        Err(StoreError::Corrupt) => FileKind::Corrupt,
        // Permissions or another I/O error: not a graph we can show; still not a lock.
        Err(_) => FileKind::Corrupt,
    }
}

/// Open a connection, applying the key first (SQLCipher requires it before any read)
/// and checking it can read the schema.
pub(crate) fn open_keyed(path: &Path, key: Option<&str>) -> Result<Connection> {
    if key.is_some_and(str::is_empty) {
        return Err(StoreError::Encrypted);
    }
    let conn = Connection::open(path)?;
    if let Some(k) = key {
        if !encryption_available() {
            return Err(StoreError::Invalid(
                "this build has no encryption at rest (SQLCipher)".into(),
            ));
        }
        conn.pragma_update(None, "key", k)?;
    }
    match conn.query_row("SELECT count(*) FROM sqlite_master", [], |r| {
        r.get::<_, i64>(0)
    }) {
        Ok(_) => Ok(conn),
        Err(rusqlite::Error::SqliteFailure(e, _)) if e.code == ErrorCode::NotADatabase => {
            Err(match FileKind::after_not_a_database(path) {
                FileKind::Corrupt => StoreError::Corrupt,
                FileKind::Plain | FileKind::Encrypted => StoreError::Encrypted,
            })
        }
        Err(e) => Err(e.into()),
    }
}

/// Whether this build links SQLCipher.
pub fn encryption_available() -> bool {
    Connection::open_in_memory()
        .and_then(|c| c.query_row("PRAGMA cipher_version", [], |r| r.get::<_, String>(0)))
        .is_ok()
}

/// Whether the file at `path` is an encrypted graph (unreadable without a key).
pub fn is_encrypted(path: &Path) -> bool {
    file_kind(path) == FileKind::Encrypted
}

/// Encrypt, re-key or decrypt a graph file that no process has open. `from` is the
/// current passphrase (`None`: plain), `to` the new one (`None`: decrypt). The new file
/// is written beside the old and renamed over it, so a failure leaves the original.
pub fn set_encryption(path: &Path, from: Option<&str>, to: Option<&str>) -> Result<()> {
    if to.is_some_and(str::is_empty) {
        return Err(StoreError::Invalid("the passphrase is empty".into()));
    }
    let conn = open_keyed(path, from)?;
    // Fold the WAL in so the export sees everything.
    conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    let tmp = path.with_extension("utopia-rekey");
    let _ = std::fs::remove_file(&tmp);
    conn.execute(
        "ATTACH DATABASE ?1 AS rekeyed KEY ?2",
        rusqlite::params![tmp.to_string_lossy(), to.unwrap_or("")],
    )?;
    conn.query_row("SELECT sqlcipher_export('rekeyed')", [], |_| Ok(()))?;
    conn.execute_batch(&format!(
        "PRAGMA rekeyed.user_version = {version}; DETACH DATABASE rekeyed;"
    ))?;
    drop(conn);
    for ext in ["utopia-wal", "utopia-shm"] {
        let _ = std::fs::remove_file(path.with_extension(ext));
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}
