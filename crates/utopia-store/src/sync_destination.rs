//! Switching transports must carry the whole graph (§12.7), including imported pages.
use crate::{Result, Store};
use rusqlite::{params, TransactionBehavior};
use utopia_core::PageId;
use utopia_doc::Frontier;

/// Where a graph's updates go (§12.7), in the two parts a launch can learn.
///
/// `origin` is the transport's stable address: a relay's normalized URL plus the graph id
/// (`relay:https://host#graph`), or a folder's root (`folder:/path`). It is all an offline
/// launch knows. `instance` is the relay database's own id (`Manifest.instance`), which is
/// the finer identity: it survives a changed DHCP lease or LAN port, and it changes when
/// the store is wiped.
///
/// Comparing only the identity sampled at launch made an offline launch look like a new
/// destination: the probe could not read a manifest, so the origin stood in for the store
/// id and the whole graph was exported again (`sync-engine-outbox-and-cursor.md`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Destination {
    pub origin: String,
    pub instance: Option<String>,
}

impl Destination {
    pub fn new(origin: impl Into<String>, instance: Option<String>) -> Self {
        Self {
            origin: origin.into(),
            instance,
        }
    }

    /// Whether the destination just observed is the one `stored` was recorded for.
    ///
    /// Two known store ids decide on their own: the address may move (a lease, a LAN
    /// port, a host rename) without the data changing, and a wiped store keeps its address
    /// while its sequence restarts. When either side has no store id — it was recorded or
    /// observed before a manifest was read — the origin has to stand in, so an offline
    /// launch compares as the same destination it was recorded as.
    fn matches(&self, stored: &Destination) -> bool {
        match (&stored.instance, &self.instance) {
            (Some(stored), Some(observed)) => stored == observed,
            _ => stored.origin == self.origin,
        }
    }
}

/// The `sync_state` keys a destination is recorded under; two plain strings keep the row
/// readable in `doctor` output and in a hand-inspected database.
const ORIGIN: &str = "setup_destination";
const INSTANCE: &str = "setup_destination_instance";

impl Store {
    /// Queue full CRDT updates, never a truncating relay snapshot. Existing cursor spaces
    /// remain tagged; only a pull from the new relay may authorize its compaction (§12.6).
    pub fn prepare_sync_destination(&mut self, destination: &Destination) -> Result<()> {
        self.sync_index()?;
        let stored = self.sync_get(ORIGIN)?.map(|origin| Destination {
            origin,
            instance: self.sync_get(INSTANCE).ok().flatten(),
        });
        if let Some(stored) = &stored {
            if destination.matches(stored) {
                // A launch that reached the manifest learned the relay's own id; keep it,
                // so the next launch's URL-only identity cannot be mistaken for a change
                // and a wiped relay can be told apart from the same relay on a new port.
                if stored.instance.is_none() && destination.instance.is_some() {
                    if let Some(instance) = &destination.instance {
                        self.sync_set(INSTANCE, instance)?;
                    }
                }
                return Ok(());
            }
        }
        let ids = {
            let mut stmt = self
                .conn
                .prepare("SELECT page_id FROM page_doc ORDER BY page_id")?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
            rows.collect::<std::result::Result<Vec<_>, _>>()?
        };
        let mut updates = Vec::new();
        for id in ids {
            let page = PageId(id);
            let payload = self.export_since(&page, &Frontier::default())?;
            updates.push((page, payload));
        }
        let now = self.now();
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        for (page, payload) in updates {
            tx.execute("INSERT INTO outbox(page_id, payload, client_seq, created_at) SELECT ?1, ?2, ifnull(max(seq), 0) + 1, ?3 FROM outbox", params![page.as_str(), payload, now])?;
        }
        // Upload acknowledgement belongs to the old destination, not to the bytes.
        tx.execute(
            "UPDATE attachment SET state='pending', lost_at=NULL WHERE state='uploaded'",
            [],
        )?;
        tx.execute(
            "INSERT OR REPLACE INTO sync_state(key,value) VALUES (?1,?2)",
            params![ORIGIN, destination.origin],
        )?;
        match &destination.instance {
            Some(instance) => tx.execute(
                "INSERT OR REPLACE INTO sync_state(key,value) VALUES (?1,?2)",
                params![INSTANCE, instance],
            )?,
            None => tx.execute("DELETE FROM sync_state WHERE key=?1", [INSTANCE])?,
        };
        tx.commit()?;
        Ok(())
    }
}
