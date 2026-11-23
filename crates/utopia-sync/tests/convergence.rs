//! Multi-device convergence under destination switches, restarts and lost rows (§12.4,
//! §12.6, §2.3.7).
//!
//! The 2026-11-23 incident was a silent divergence: one device's text never reached the
//! other, and both said "Synced". This script drives three devices with on-disk stores
//! through random edits, syncs, destination switches (with and without the desktop's reset,
//! across two relays and two graph ids), restarts, local compaction, and outbox rows marked
//! sent without a push (the old build's gap). At the end every device moves to one
//! destination and syncs. Then:
//!
//! 1. no silent divergence: every page that differs between devices is named by some
//!    device's `utopia sync inspect` (§12.6);
//! 2. a device that `inspect` says has more runs the repair, as its advice says (and only
//!    that device: a receiver must not need a repair to keep what it was sent);
//! 3. every line any device wrote is on every device with its last text, every page reads
//!    the same everywhere, nothing waits, and `doctor` is clean.
//!
//! `make check` runs [`FAST_SEEDS`] seeds. The long run is ignored by default:
//! `cargo test -p utopia-sync --test convergence -- --ignored`, or set
//! `UTOPIA_SYNC_FUZZ_SEEDS=<n>` for either. A failure names its seed; rerun one with
//! `UTOPIA_SYNC_FUZZ_SEED=<seed>`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;

use chrono::NaiveDate;
use utopia_core::{outline, BlockId, PageId};
use utopia_store::{Destination, Store};
use utopia_sync::inspect::{inspect, Finding};
use utopia_sync::relay::{Relay, RelayTransport};
use utopia_sync::{SyncClient, SyncEngine};

const FAST_SEEDS: u64 = 20;
const SLOW_SEEDS: u64 = 2000;
const DEVICES: usize = 3;
const STEPS: usize = 60;
const TITLES: [&str; 3] = ["Alpha", "Beta", "Gamma"];
const GRAPH_IDS: [&str; 2] = ["utopia", "Utopia"];

/// xorshift64*, so a seed replays exactly without extra dependencies.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

/// Two relays × two graph ids.
struct World {
    relays: Vec<Arc<Relay>>,
}

impl World {
    fn destinations(&self) -> usize {
        self.relays.len() * GRAPH_IDS.len()
    }
    fn transport(&self, d: usize) -> RelayTransport {
        RelayTransport::new(
            self.relays[d / GRAPH_IDS.len()].clone(),
            GRAPH_IDS[d % GRAPH_IDS.len()],
        )
    }
    fn destination(&self, d: usize) -> Destination {
        let relay = &self.relays[d / GRAPH_IDS.len()];
        Destination::new(
            format!(
                "relay:r{}#{}",
                d / GRAPH_IDS.len(),
                GRAPH_IDS[d % GRAPH_IDS.len()]
            ),
            Some(relay.instance().to_string()),
        )
    }
}

struct Device {
    path: PathBuf,
    store: Store,
    client: SyncClient<RelayTransport>,
    at: usize,
}

fn open(path: &PathBuf) -> Store {
    let mut s = Store::open(path).unwrap();
    s.set_today(Some(NaiveDate::from_ymd_opt(2026, 11, 23).unwrap()));
    s
}

fn page_id(store: &mut Store, title: &str) -> PageId {
    match store.find_page(title).unwrap() {
        Some(p) => p,
        None => store.create_page(title).unwrap(),
    }
}

fn texts(store: &mut Store, page: &PageId) -> Vec<String> {
    if !store.page_exists(page).unwrap() {
        return Vec::new();
    }
    let st = store.page_state(page).unwrap().clone();
    st.live_preorder()
        .into_iter()
        .map(|(id, _)| st.block(&id).unwrap().text.clone())
        .collect()
}

/// The last text each block was given, and which device and page own it.
#[derive(Default)]
struct Expected {
    blocks: BTreeMap<BlockId, (usize, PageId, String)>,
}

/// Run one seed; returns how many relay snapshots its syncs uploaded.
fn run(seed: u64) -> usize {
    let mut rng = Rng::new(seed);
    let dir = tempfile::tempdir().unwrap();
    let world = World {
        relays: vec![Relay::new(), Relay::new()],
    };
    let mut devices: Vec<Device> = (0..DEVICES)
        .map(|i| {
            let path = dir.path().join(format!("device-{i}.utopia"));
            let store = open(&path);
            let client = SyncClient::new(world.transport(0), &store).unwrap();
            Device {
                path,
                store,
                client,
                at: 0,
            }
        })
        .collect();
    let mut expected = Expected::default();
    let mut written = 0usize;
    let mut log: Vec<String> = Vec::new();
    let mut snapshots = 0usize;

    for step in 0..STEPS {
        let d = rng.below(DEVICES);
        let op = rng.below(100);
        let dev = &mut devices[d];
        match op {
            // Insert a line on a shared page.
            0..=29 => {
                let title = TITLES[rng.below(TITLES.len())];
                let page = page_id(&mut dev.store, title);
                written += 1;
                let text = format!("d{d}-line{written}");
                let plan = outline::append_block(dev.store.page_state(&page).unwrap(), None);
                let id = dev
                    .store
                    .execute_plan(&page, plan, None)
                    .unwrap()
                    .unwrap()
                    .block;
                dev.store
                    .set_block_text(&page, &id, &text, None, None)
                    .unwrap();
                dev.store.commit_block(&page, &id).unwrap();
                log.push(format!("{step}: d{d} insert {text} on {title}"));
                expected.blocks.insert(id, (d, page, text));
            }
            // Edit a line this device wrote, once or (a burst) 80 times, so that relay logs
            // pass `SNAPSHOT_TAIL_UPDATES` and the snapshot paths run too.
            30..=39 => {
                let mine: Vec<BlockId> = expected
                    .blocks
                    .iter()
                    .filter(|(_, (owner, _, _))| *owner == d)
                    .map(|(id, _)| id.clone())
                    .collect();
                if mine.is_empty() {
                    continue;
                }
                let id = mine[rng.below(mine.len())].clone();
                let (_, page, _) = expected.blocks[&id].clone();
                let times = if rng.below(4) == 0 { 80 } else { 1 };
                let mut text = String::new();
                for _ in 0..times {
                    written += 1;
                    text = format!("d{d}-edit{written}");
                    dev.store
                        .set_block_text(&page, &id, &text, None, None)
                        .unwrap();
                    dev.store.commit_block(&page, &id).unwrap();
                }
                log.push(format!("{step}: d{d} edit {id} {times}x, last {text}"));
                expected.blocks.insert(id, (d, page, text));
            }
            // Sync with the current destination.
            40..=69 => {
                snapshots += dev.client.sync_once(&mut dev.store).unwrap().snapshots;
                log.push(format!("{step}: d{d} sync @{}", dev.at));
            }
            // Switch destination: like the desktop (with its reset) or like the phone.
            70..=79 => {
                let to = rng.below(world.destinations());
                let reset = rng.below(2) == 0;
                if reset {
                    dev.store
                        .prepare_sync_destination(&world.destination(to))
                        .unwrap();
                }
                dev.client = SyncClient::new(world.transport(to), &dev.store).unwrap();
                dev.at = to;
                log.push(format!("{step}: d{d} switch to @{to} reset={reset}"));
            }
            // Restart: the store reopens from disk.
            80..=87 => {
                dev.store = open(&dev.path);
                dev.client = SyncClient::new(world.transport(dev.at), &dev.store).unwrap();
                log.push(format!("{step}: d{d} restart"));
            }
            // Fold local updates into snapshots.
            88..=94 => {
                dev.store.sync_index().unwrap();
                dev.store.compact().unwrap();
                log.push(format!("{step}: d{d} compact"));
            }
            // The old build's gap: rows marked sent that no destination received.
            _ => {
                let lost: Vec<i64> = dev
                    .store
                    .outbox_pending(10_000)
                    .unwrap()
                    .into_iter()
                    .map(|(seq, _, _, _)| seq)
                    .collect();
                dev.store.outbox_mark_sent(&lost).unwrap();
                log.push(format!("{step}: d{d} lose {} row(s)", lost.len()));
            }
        }
    }

    let context = || format!("seed {seed}\n{}", log.join("\n"));

    // 1. Everyone meets at one destination and syncs, with no repair.
    for dev in devices.iter_mut() {
        dev.client = SyncClient::new(world.transport(0), &dev.store).unwrap();
        dev.at = 0;
    }
    settle(&mut devices);

    // 2. No silent divergence: a page that differs between devices is named by some
    //    device's `inspect`, which is how a person finds out (§12.6).
    let mut reported: BTreeSet<String> = BTreeSet::new();
    let mut repair: Vec<usize> = Vec::new();
    for (i, dev) in devices.iter_mut().enumerate() {
        let r = inspect(&mut dev.store, dev.client.transport_mut(), None, true).unwrap();
        for p in &r.pages {
            if !p.findings.is_empty() {
                reported.insert(p.page_id.clone());
            }
            // The destination holds changes that wait, and this device pulled them (it is
            // settled) without applying them: then they wait here too, and its own status
            // says so, without anyone running `inspect`. This is the "Synced" that hid the
            // 2026-11-23 incident, and it must survive a restart.
            let local: BTreeMap<&str, u64> = p
                .local
                .iter()
                .flatten()
                .map(|c| (c.peer.as_str(), c.changes))
                .collect();
            let unapplied = p
                .destination_waiting
                .iter()
                .any(|w| local.get(w.peer.as_str()).copied().unwrap_or(0) < w.end);
            if unapplied {
                assert!(
                    dev.store.has_waiting(&PageId(p.page_id.clone())).unwrap(),
                    "device {i} pulled waiting changes of page {} without applying them, \
                     and its status does not say so\n{}",
                    p.page_id,
                    context()
                );
            }
            if p.findings
                .iter()
                .any(|f| matches!(f, Finding::ThisDeviceHasMore | Finding::NotOnDestination))
                && !repair.contains(&i)
            {
                repair.push(i);
            }
        }
    }
    for page in divergent(&mut devices, &expected) {
        assert!(
            reported.contains(page.as_str()),
            "page {page} differs between devices and no device reports it\n{}",
            context()
        );
    }

    // 3. Follow the advice: only a device that has more runs the repair.
    for &i in &repair {
        SyncEngine::reset_positions(&devices[i].store).unwrap();
    }
    settle(&mut devices);

    for (i, dev) in devices.iter_mut().enumerate() {
        assert_eq!(
            dev.store.waiting_count().unwrap(),
            0,
            "device {i} still waits\n{}",
            context()
        );
        let report = dev.store.doctor().unwrap();
        assert!(
            report.is_ok(),
            "device {i}: {:?}\n{}",
            report.problems,
            context()
        );
        for (id, (owner, page, text)) in &expected.blocks {
            let st = dev.store.page_state(page).unwrap();
            let got = st.block(id).map(|b| b.text.clone());
            assert_eq!(
                got.as_deref(),
                Some(text.as_str()),
                "device {i} lost block {id} written by device {owner}\n{}",
                context()
            );
        }
    }
    let pages: Vec<PageId> = expected
        .blocks
        .values()
        .map(|(_, p, _)| p.clone())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    for page in pages {
        let first = texts(&mut devices[0].store, &page);
        for (i, dev) in devices.iter_mut().enumerate().skip(1) {
            assert_eq!(
                texts(&mut dev.store, &page),
                first,
                "device {i} reads page {page} differently\n{}",
                context()
            );
        }
    }
    snapshots
}

fn settle(devices: &mut [Device]) {
    for _ in 0..3 {
        for dev in devices.iter_mut() {
            dev.client.sync_once(&mut dev.store).unwrap();
        }
    }
}

/// Pages where some device lacks a line's last text, or devices read differently.
fn divergent(devices: &mut [Device], expected: &Expected) -> BTreeSet<PageId> {
    let mut out = BTreeSet::new();
    for (_, (_, page, text)) in expected.blocks.iter() {
        for dev in devices.iter_mut() {
            if !texts(&mut dev.store, page).contains(text) {
                out.insert(page.clone());
            }
        }
    }
    let pages: BTreeSet<PageId> = expected
        .blocks
        .values()
        .map(|(_, p, _)| p.clone())
        .collect();
    for page in pages {
        let first = texts(&mut devices[0].store, &page);
        if devices
            .iter_mut()
            .skip(1)
            .any(|d| texts(&mut d.store, &page) != first)
        {
            out.insert(page);
        }
    }
    out
}

fn seeds(default: u64) -> Vec<u64> {
    if let Some(one) = std::env::var("UTOPIA_SYNC_FUZZ_SEED")
        .ok()
        .and_then(|s| s.parse().ok())
    {
        return vec![one];
    }
    let n = std::env::var("UTOPIA_SYNC_FUZZ_SEEDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(default);
    (1..=n).collect()
}

/// §12.4, §12.6, §2.3.7: no silent divergence. Every device converges on every line once
/// the devices meet at one destination and repair.
#[test]
fn ac_12_4_devices_converge_after_switches_restarts_and_lost_rows() {
    let seeds = seeds(FAST_SEEDS);
    let snapshots: usize = seeds.iter().map(|s| run(*s)).sum();
    // Edit bursts must push some relay log past `SNAPSHOT_TAIL_UPDATES`, or the snapshot
    // paths (upload, a pull that starts from one, a comparison against one) go untested.
    if seeds.len() >= FAST_SEEDS as usize {
        assert!(snapshots > 0, "no relay snapshot in {} seeds", seeds.len());
    }
}

/// The long run of the same script.
#[test]
#[ignore = "slow: run with --ignored (or UTOPIA_SYNC_FUZZ_SEEDS)"]
fn ac_12_4_devices_converge_long_run() {
    for seed in seeds(SLOW_SEEDS) {
        run(seed);
    }
}
