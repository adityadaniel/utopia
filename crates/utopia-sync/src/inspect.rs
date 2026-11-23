//! Read-only sync diagnostics (`utopia sync inspect`, §12.6).
//!
//! The 2026-11-23 incident took raw `sqlite3` reads, a throwaway Loro dump, the relay token
//! from the Keychain and `curl` to see one fact: the relay's copy of a page held a device's
//! later changes but not the earlier ones they build on. This module answers that question
//! for each page, from this device's store and its destination, and says which device has
//! to act. It writes nothing: no cursor, no outbox row, no setting.
//!
//! A gap in one client's change ids on a page is *not* a finding: `client_seq` counts the
//! outbox of the whole graph, so a client's ids on one page always skip. The finding comes
//! from the CRDT: changes in the destination's copy that wait for changes it does not have.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;
use utopia_core::PageId;
use utopia_doc::{copy_state, Frontier, WaitingRange};
use utopia_store::Store;

use crate::{check_protocol, Cursor, Result, SyncError, Transport};

/// Something about one page that a person may have to act on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Finding {
    /// This device holds another device's changes that wait for earlier ones it never
    /// received. They are kept (`page_pending`); the other device must send what it lacks.
    WaitingHere,
    /// The destination's copy holds changes that wait for earlier ones it does not have: a
    /// device marked changes sent that never reached it.
    DestinationHasGap,
    /// This device has changes of the page that the destination lacks.
    ThisDeviceHasMore,
    /// The destination has changes of the page that this device has not pulled yet.
    DestinationHasMore,
    /// The destination does not list this page at all.
    NotOnDestination,
    /// Changes of the page are queued here and not sent yet.
    Unsent,
    /// The destination's log is ahead of where this device last pulled.
    NotPulledYet,
}

impl Finding {
    /// What to do about it, in one sentence.
    pub fn advice(self) -> &'static str {
        match self {
            Finding::WaitingHere => {
                "another device wrote changes this device cannot apply yet; on the device \
                 that wrote them, update Utopia or run `utopia sync repair`, then sync"
            }
            Finding::DestinationHasGap => {
                "the destination holds changes whose earlier changes never reached it; the \
                 device that wrote them must run `utopia sync repair` (inspect each device: \
                 that one reports this_device_has_more)"
            }
            Finding::ThisDeviceHasMore => {
                "run `utopia sync`; if this stays after it, run `utopia sync repair` on this \
                 device"
            }
            Finding::DestinationHasMore | Finding::NotPulledYet => "run `utopia sync`",
            Finding::NotOnDestination => {
                "run `utopia sync`; if this stays after it, run `utopia sync repair` on this \
                 device"
            }
            Finding::Unsent => "run `utopia sync`",
        }
    }
}

/// A Loro peer and a count of its changes. The peer is a string because it does not fit
/// a JSON number safely.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PeerCount {
    pub peer: String,
    pub changes: u64,
}

/// Changes of one peer that wait for earlier ones (counters `start..end`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Waiting {
    pub peer: String,
    pub start: u64,
    pub end: u64,
}

impl From<&WaitingRange> for Waiting {
    fn from(r: &WaitingRange) -> Self {
        Waiting {
            peer: r.peer.to_string(),
            start: r.start,
            end: r.end,
        }
    }
}

fn peers(f: &Frontier) -> Vec<PeerCount> {
    f.peers()
        .unwrap_or_default()
        .into_iter()
        .map(|(peer, changes)| PeerCount {
            peer: peer.to_string(),
            changes,
        })
        .collect()
}

/// One page as this device and its destination see it.
#[derive(Debug, Clone, Default, Serialize)]
pub struct PageReport {
    pub page_id: String,
    pub title: Option<String>,
    /// This device's version of the page (absent when it does not have the page).
    pub local: Option<Vec<PeerCount>>,
    pub unsent: usize,
    pub waiting_here: Vec<Waiting>,
    /// Where this device last pulled the page from this destination.
    pub cursor: Option<u64>,
    /// The destination's log head and the sequence its snapshot covers.
    pub head: Option<u64>,
    pub snapshot_seq: Option<u64>,
    /// The destination's copy, read only on a deep inspection.
    pub destination: Option<Vec<PeerCount>>,
    pub destination_waiting: Vec<Waiting>,
    pub findings: Vec<Finding>,
}

/// The whole answer.
#[derive(Debug, Clone, Serialize)]
pub struct Inspection {
    pub client_id: Option<String>,
    /// The destination's identity, e.g. `relay:https://host#graph` (never a secret).
    pub destination: String,
    pub rev: u64,
    pub destination_pages: usize,
    pub local_pages: usize,
    pub unsent: usize,
    /// Whether each page's copy on the destination was read and compared.
    pub deep: bool,
    /// The pages asked for, or every page with a finding.
    pub pages: Vec<PageReport>,
}

impl Inspection {
    /// Whether any page needs attention.
    pub fn has_findings(&self) -> bool {
        self.pages.iter().any(|p| !p.findings.is_empty())
    }
}

/// Inspect `only` (or every page), comparing each page's copy on the destination when
/// `deep` is set or pages were named. Read-only on the store and on the destination.
pub fn inspect<T: Transport + ?Sized>(
    store: &mut Store,
    t: &mut T,
    only: Option<&[PageId]>,
    deep: bool,
) -> Result<Inspection> {
    let manifest = t
        .manifest(None)?
        .ok_or_else(|| SyncError::Transport("the destination sent no manifest".into()))?;
    check_protocol(&Some(manifest.clone()))?;
    let space = t.cursor_space();
    let deep = deep || only.is_some();

    let local_pages: BTreeSet<PageId> = store.all_page_ids()?.into_iter().collect();
    let unsent: BTreeMap<PageId, usize> = store.outbox_pending_by_page()?.into_iter().collect();
    let waiting: BTreeMap<PageId, Vec<Waiting>> = store
        .waiting_pages()?
        .into_iter()
        .map(|w| (w.page, w.ranges.iter().map(Waiting::from).collect()))
        .collect();
    let cursors: BTreeMap<PageId, u64> = store
        .sync_prefixed("cursor")?
        .into_iter()
        .filter_map(|(k, raw)| {
            let c = Cursor::decode(&raw)?;
            (c.space == space).then_some((PageId(k), c.pos))
        })
        .collect();
    let listed: BTreeMap<PageId, (u64, u64)> = manifest
        .pages
        .iter()
        .map(|p| (PageId(p.page_id.clone()), (p.head_seq, p.snapshot_seq)))
        .collect();

    let candidates: Vec<PageId> = match only {
        Some(pages) => pages.to_vec(),
        None => local_pages
            .iter()
            .chain(listed.keys())
            .chain(waiting.keys())
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect(),
    };

    let mut pages = Vec::new();
    for page in candidates {
        let mut r = PageReport {
            page_id: page.to_string(),
            title: store.page_row(&page).ok().flatten().map(|row| row.title),
            unsent: unsent.get(&page).copied().unwrap_or(0),
            waiting_here: waiting.get(&page).cloned().unwrap_or_default(),
            cursor: cursors.get(&page).copied(),
            head: listed.get(&page).map(|(h, _)| *h),
            snapshot_seq: listed.get(&page).map(|(_, s)| *s),
            ..Default::default()
        };
        let here = local_pages.contains(&page);
        let local = if here {
            Some(store.frontier(&page)?)
        } else {
            None
        };
        r.local = local.as_ref().map(peers);
        if r.unsent > 0 {
            r.findings.push(Finding::Unsent);
        }
        if !r.waiting_here.is_empty() {
            r.findings.push(Finding::WaitingHere);
        }
        match r.head {
            None if here => r.findings.push(Finding::NotOnDestination),
            Some(head) if r.cursor.unwrap_or(0) < head => r.findings.push(Finding::NotPulledYet),
            _ => {}
        }
        if deep && r.head.is_some() {
            let resp = t.pull(&page, 0)?;
            let blobs = resp
                .snapshot
                .iter()
                .map(Vec::as_slice)
                .chain(resp.updates.iter().map(|u| u.payload.as_slice()));
            let copy = copy_state(blobs).map_err(|e| SyncError::Transport(e.to_string()))?;
            r.destination = Some(peers(&copy.version));
            r.destination_waiting = copy.waiting.iter().map(Waiting::from).collect();
            if !copy.waiting.is_empty() {
                r.findings.push(Finding::DestinationHasGap);
            }
            let local = local.clone().unwrap_or_default();
            if local
                .has_more_than(&copy.version)
                .map_err(|e| SyncError::Transport(e.to_string()))?
            {
                r.findings.push(Finding::ThisDeviceHasMore);
            }
            if copy
                .version
                .has_more_than(&local)
                .map_err(|e| SyncError::Transport(e.to_string()))?
            {
                r.findings.push(Finding::DestinationHasMore);
            }
        }
        r.findings.sort();
        r.findings.dedup();
        if only.is_some() || !r.findings.is_empty() {
            pages.push(r);
        }
    }

    Ok(Inspection {
        client_id: store.sync_get("client_id")?,
        destination: space.transport.clone(),
        rev: manifest.rev,
        destination_pages: manifest.pages.len(),
        local_pages: local_pages.len(),
        unsent: unsent.values().sum(),
        deep,
        pages,
    })
}
