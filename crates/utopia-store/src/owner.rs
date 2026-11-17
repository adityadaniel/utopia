//! One owner for each graph (PRD §13.8 MC-4, docs/mcp-design.md §2.2).
//!
//! The store keeps each page shard in memory after the first load and writes projections
//! from that copy, so two processes that write one graph each project from a stale copy of
//! the other's work. [`OwnerLock`] is held by the one process that writes: the desktop app,
//! `serve-api`, and the short write commands (`capture`, `import-roam`, `doctor --rebuild`).
//! Readers (`doctor`, `export`) and [`crate::Store::open`] do not take it.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::Result;

/// How often [`OwnerLock::acquire_within`] tries again while the lock is held.
const POLL: Duration = Duration::from_millis(50);

/// An exclusive advisory lock on a `<graph>.owner` sidecar, held for as long as a process
/// writes the graph (MC-4).
///
/// It follows `FileLock` in `crypt.rs`: the lock sits on a sidecar, not the database file,
/// because macOS `flock` would collide with SQLite's own locks on that file; and the sidecar
/// stays on disk, because removing it while another process waits would let a third process
/// lock a fresh inode next to the waiter. The kernel releases a `flock` on drop and when the
/// process stops, also by a crash, so no stale lock is left (MC-4).
///
/// The lock belongs to the open file description: a second [`OwnerLock::try_acquire`] in
/// the same process fails while the first is held. A caller that opens one graph more than
/// once (the unlock screen) must keep the lock it already has.
#[derive(Debug)]
pub struct OwnerLock {
    #[cfg(unix)]
    _file: std::fs::File,
}

impl OwnerLock {
    /// The sidecar for `graph`: `.owner` appended to the canonical path, never replacing
    /// the extension (a graph named `x.utopia-owner` must not lock itself).
    ///
    /// Canonical, so `./g.utopia`, `/abs/g.utopia` and a symlink name one sidecar. The graph
    /// may not exist yet (`import-roam` creates it), so the parent is canonicalized then.
    pub fn sidecar(graph: &Path) -> PathBuf {
        let canonical = std::fs::canonicalize(graph).unwrap_or_else(|_| {
            match (graph.parent(), graph.file_name()) {
                (Some(dir), Some(name)) => {
                    let dir = if dir.as_os_str().is_empty() {
                        Path::new(".")
                    } else {
                        dir
                    };
                    std::fs::canonicalize(dir)
                        .map(|d| d.join(name))
                        .unwrap_or_else(|_| graph.to_path_buf())
                }
                _ => graph.to_path_buf(),
            }
        });
        let mut owner = canonical.into_os_string();
        owner.push(".owner");
        PathBuf::from(owner)
    }

    /// Take the lock without waiting. `Ok(None)` means another owner holds it (MC-4).
    #[cfg(unix)]
    pub fn try_acquire(graph: &Path) -> Result<Option<Self>> {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(Self::sidecar(graph))?;
        // SAFETY: `file` is a live descriptor and these are valid flock flags.
        let r = unsafe {
            libc::flock(
                std::os::fd::AsRawFd::as_raw_fd(&file),
                libc::LOCK_EX | libc::LOCK_NB,
            )
        };
        if r != 0 {
            let e = std::io::Error::last_os_error();
            if e.kind() == std::io::ErrorKind::WouldBlock {
                return Ok(None);
            }
            return Err(e.into());
        }
        Ok(Some(Self { _file: file }))
    }

    /// Encryption at rest and the MCP bridge are macOS-first; elsewhere there is no lock.
    #[cfg(not(unix))]
    pub fn try_acquire(_graph: &Path) -> Result<Option<Self>> {
        Ok(Some(Self {}))
    }

    /// Take the lock, trying again until `wait` has passed. The app and the write commands
    /// wait 2 s (docs/mcp-design.md §2.2), because a headless call holds the lock briefly.
    pub fn acquire_within(graph: &Path, wait: Duration) -> Result<Option<Self>> {
        let deadline = Instant::now() + wait;
        loop {
            if let Some(lock) = Self::try_acquire(graph)? {
                return Ok(Some(lock));
            }
            let now = Instant::now();
            if now >= deadline {
                return Ok(None);
            }
            std::thread::sleep(POLL.min(deadline - now));
        }
    }
}

/// The key of a graph in the config directory: the SHA-256 hex of its canonical path, or
/// of the path as given when it cannot be canonicalized (it does not exist yet).
///
/// Per-graph local files are named by it: sync settings (`sync/<key>.json`), the
/// assistant access setting (`mcp/<key>.json`, docs/mcp-design.md §5) and the API
/// discovery file (`api/<key>.json`, §2.2). The value is frozen: changing it orphans every
/// saved sync setting.
pub fn graph_key(graph: &Path) -> String {
    let canonical = graph.canonicalize().unwrap_or_else(|_| graph.to_path_buf());
    crate::sha256_hex(canonical.to_string_lossy().as_bytes())
}

#[cfg(test)]
mod key_tests {
    use super::*;

    #[test]
    fn graph_key_is_the_sha256_of_the_canonical_path() {
        let dir = tempfile::tempdir().unwrap();
        let graph = dir.path().join("g.utopia");
        std::fs::write(&graph, b"").unwrap();
        let canonical = std::fs::canonicalize(&graph).unwrap();
        // The formula sync settings files were named by before this function existed.
        let old = crate::sha256_hex(canonical.to_string_lossy().as_bytes());
        assert_eq!(graph_key(&graph), old);
        assert_eq!(graph_key(&dir.path().join(".").join("g.utopia")), old);
        assert_eq!(graph_key(&graph).len(), 64);
        let missing = dir.path().join("missing.utopia");
        assert_eq!(
            graph_key(&missing),
            crate::sha256_hex(missing.to_string_lossy().as_bytes())
        );
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn mc_4_a_second_owner_is_refused_while_the_first_holds_the_lock() {
        let dir = tempfile::tempdir().unwrap();
        let graph = dir.path().join("g.utopia");
        let first = OwnerLock::try_acquire(&graph).unwrap();
        assert!(first.is_some());
        assert!(OwnerLock::try_acquire(&graph).unwrap().is_none());
        let t = Instant::now();
        assert!(
            OwnerLock::acquire_within(&graph, Duration::from_millis(200))
                .unwrap()
                .is_none()
        );
        assert!(t.elapsed() >= Duration::from_millis(200), "it waited");
    }

    #[test]
    fn mc_4_dropping_the_owner_releases_the_lock() {
        let dir = tempfile::tempdir().unwrap();
        let graph = dir.path().join("g.utopia");
        let first = OwnerLock::try_acquire(&graph).unwrap();
        drop(first);
        assert!(OwnerLock::try_acquire(&graph).unwrap().is_some());
    }

    #[test]
    fn mc_4_a_waiting_owner_gets_the_lock_when_it_is_released() {
        let dir = tempfile::tempdir().unwrap();
        let graph = dir.path().join("g.utopia");
        let first = OwnerLock::try_acquire(&graph).unwrap().unwrap();
        let release = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            drop(first);
        });
        let second = OwnerLock::acquire_within(&graph, Duration::from_secs(2)).unwrap();
        assert!(second.is_some());
        release.join().unwrap();
    }

    #[test]
    fn mc_4_the_sidecar_is_appended_and_canonical() {
        let dir = tempfile::tempdir().unwrap();
        let canon = std::fs::canonicalize(dir.path()).unwrap();
        let graph = dir.path().join("x.utopia-owner");
        // Not yet created: the parent is canonicalized.
        assert_eq!(
            OwnerLock::sidecar(&graph),
            canon.join("x.utopia-owner.owner")
        );
        std::fs::write(&graph, b"").unwrap();
        let _held = OwnerLock::try_acquire(&graph).unwrap().unwrap();
        assert!(canon.join("x.utopia-owner.owner").exists());
        // Another spelling of the same file names the same sidecar, so it is refused.
        let other = dir.path().join(".").join("x.utopia-owner");
        assert!(OwnerLock::try_acquire(&other).unwrap().is_none());
        // The lock is not on the graph itself.
        assert_eq!(std::fs::read(&graph).unwrap(), b"");
        // A different graph in the same directory has its own owner.
        assert!(OwnerLock::try_acquire(&dir.path().join("y.utopia"))
            .unwrap()
            .is_some());
    }

    #[test]
    fn mc_4_the_owner_lock_does_not_block_a_reader() {
        let dir = tempfile::tempdir().unwrap();
        let graph = dir.path().join("g.utopia");
        let _held = OwnerLock::try_acquire(&graph).unwrap().unwrap();
        let mut store = crate::Store::open(&graph).unwrap();
        assert!(store.doctor().unwrap().is_ok());
    }
}
