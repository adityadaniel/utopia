//! First-use protection for pairing (§12.6): prove the selected key can read the target
//! before publishing any local data. The guard also runs after an offline setup reconnects.
use crate::{
    CursorSpace, Manifest, PullResponse, PushRequest, PushResponse, Result, SyncError, Transport,
    PROTOCOL_VERSION,
};
use utopia_core::PageId;
use utopia_doc::{LoroPageDoc, PageDoc};

pub const UNREADABLE: &str = "The target's existing data cannot be read with these encryption settings. Use the original E2E passphrase; changing encryption on an existing target is not supported.";

pub struct VerifiedTarget<T> {
    inner: T,
    verified: Option<CursorSpace>,
    /// The target held a payload that decrypted with this key. An empty target proves
    /// nothing: a device that joined a fresh relay with the wrong passphrase would
    /// otherwise keep that proof and push its ciphertext into a graph another device is
    /// already encrypting with the right one.
    proven: bool,
    pub unreadable: bool,
}
impl<T: Transport> VerifiedTarget<T> {
    pub fn new(inner: T) -> Self {
        Self {
            inner,
            verified: None,
            proven: false,
            unreadable: false,
        }
    }
    /// Whether the key was proven by payloads, not merely by an empty manifest. Setup uses
    /// this to decide whether the graph may freeze its key marker (§12.6).
    pub fn proven(&self) -> bool {
        self.proven
    }
    fn writable(&self) -> Result<()> {
        if self.verified.as_ref() == Some(&self.inner.cursor_space()) {
            Ok(())
        } else {
            Err(SyncError::Transport(UNREADABLE.into()))
        }
    }
}
impl<T: Transport> Transport for VerifiedTarget<T> {
    fn manifest(&mut self, conditional: Option<u64>) -> Result<Option<Manifest>> {
        let space = self.inner.cursor_space();
        // Re-check on every read until a payload proves the key, so a target that was
        // empty at verification cannot quietly gain foreign ciphertext.
        let needs_check = self.verified.as_ref() != Some(&space) || !self.proven;
        let manifest = self
            .inner
            .manifest(if needs_check { None } else { conditional })?;
        let space = self.inner.cursor_space();
        if self.verified != Some(space.clone()) || !self.proven {
            let Some(m) = &manifest else {
                return Ok(None);
            };
            if m.protocol_version != PROTOCOL_VERSION {
                return Err(SyncError::ProtocolVersion(m.protocol_version));
            }
            // A full pull of one existing shard establishes the encryption choice. Use a
            // throwaway document, never the real store: validation cannot import or write.
            if let Some(page) = m.pages.first() {
                let id = PageId(page.page_id.clone());
                let pulled = self.inner.pull(&id, 0);
                if let Err(error) = &pulled {
                    let encryption_error = matches!(error, SyncError::Transport(message) if message.starts_with("decryption failed") || message == "not an encrypted Utopia payload");
                    if !encryption_error {
                        return pulled.map(|_| None);
                    }
                }
                let valid = pulled.and_then(|pull| {
                    let mut doc = LoroPageDoc::new(id, 0);
                    for bytes in pull
                        .snapshot
                        .iter()
                        .chain(pull.updates.iter().map(|u| &u.payload))
                    {
                        doc.import(bytes)
                            .map_err(|_| SyncError::Transport(UNREADABLE.into()))?;
                    }
                    Ok(())
                });
                if valid.is_err() {
                    // Another device is writing this target under a key this one cannot
                    // read. Revoke the proof and refuse to push: mixing ciphertexts would
                    // leave every device with pages it can never read.
                    self.unreadable = true;
                    self.verified = None;
                    self.proven = false;
                    return Err(SyncError::Transport(UNREADABLE.into()));
                }
                self.proven = true;
            } else {
                // Nothing to read back yet; pushes are still allowed so the first device
                // can establish the key, but this is not a proof.
                self.proven = false;
            }
            self.unreadable = false;
            self.verified = Some(space);
        }
        Ok(manifest)
    }
    fn cursor_space(&self) -> CursorSpace {
        self.inner.cursor_space()
    }
    fn push(&mut self, page: &PageId, req: PushRequest) -> Result<PushResponse> {
        self.writable()?;
        self.inner.push(page, req)
    }
    fn pull(&mut self, page: &PageId, since: u64) -> Result<PullResponse> {
        self.inner.pull(page, since)
    }
    fn put_snapshot(&mut self, page: &PageId, covers: u64, bytes: Vec<u8>) -> Result<()> {
        self.writable()?;
        self.inner.put_snapshot(page, covers, bytes)
    }
    fn put_blob(&mut self, hash: &str, bytes: Vec<u8>) -> Result<bool> {
        self.writable()?;
        self.inner.put_blob(hash, bytes)
    }
    fn get_blob(&mut self, hash: &str) -> Result<Option<Vec<u8>>> {
        self.inner.get_blob(hash)
    }
}
