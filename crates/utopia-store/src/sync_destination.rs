//! Switching transports must carry the whole graph (§12.7), including imported pages.
use crate::{Result, Store};
use rusqlite::{params, TransactionBehavior};
use utopia_core::PageId;
use utopia_doc::Frontier;

impl Store {
    /// Queue full CRDT updates, never a truncating relay snapshot. Existing cursor spaces
    /// remain tagged; only a pull from the new relay may authorize its compaction (§12.6).
    pub fn prepare_sync_destination(&mut self, identity: &str) -> Result<()> {
        self.sync_index()?;
        if self.sync_get("setup_destination")?.as_deref() == Some(identity) {
            return Ok(());
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
            "INSERT OR REPLACE INTO sync_state(key,value) VALUES ('setup_destination',?1)",
            [identity],
        )?;
        tx.commit()?;
        Ok(())
    }
}
