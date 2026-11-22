//! Devices syncing through a real `utopia-syncd` over HTTP (M3 exit criteria).
//!
//! The relay contract (`tests/contract/mod.rs`) runs here against an in-process relay, on
//! every `cargo test`; `tests/cloudflare.rs` runs the same scenarios against the Cloudflare
//! Worker. What stays below is what only this relay can stage: a wiped database on the same
//! address, a relay that is down, the folder transport and the LAN relay.

mod contract;

use chrono::NaiveDate;
use utopia_store::Store;
use utopia_sync::http::HttpTransport;
use utopia_sync::{SyncClient, SyncStatus};
use utopia_syncd::{router, Config, Db};

use contract::{texts, write};

contract::contract_suite!(contract::targets::in_process);

/// Serve a relay on an address chosen earlier; the `Arc<Db>` is one store instance.
fn serve_on(
    rt: &tokio::runtime::Runtime,
    addr: std::net::SocketAddr,
    cfg: Config,
) -> std::sync::Arc<Db> {
    let db = std::sync::Arc::new(Db::open_in_memory().unwrap());
    let app = router(db.clone(), cfg);
    let listener = rt.block_on(tokio::net::TcpListener::bind(addr)).unwrap();
    rt.spawn(async move { axum::serve(listener, app).await.unwrap() });
    db
}

/// §12.6: a relay whose database was wiped keeps its URL but not its sequence. The
/// manifest's instance id is what tells the two apart, so the client pulls again instead
/// of trusting a cursor from the old store.
#[test]
fn ac_12_6_a_wiped_relay_is_a_new_cursor_space() {
    let mut rt = tokio::runtime::Runtime::new().unwrap();
    let addr = {
        let l = rt
            .block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))
            .unwrap();
        l.local_addr().unwrap()
    };
    let base = format!("http://{addr}");
    let first = serve_on(&rt, addr, Config::default());
    let mut a = Store::open_in_memory().unwrap();
    a.set_today(Some(NaiveDate::from_ymd_opt(2026, 11, 2).unwrap()));
    let mut ca = SyncClient::new(HttpTransport::new(&base, "graph-1", None).unwrap(), &a).unwrap();
    let page = a.create_page("Shared").unwrap();
    for i in 0..5 {
        write(&mut a, &page, &format!("before {i}"));
    }
    ca.sync_once(&mut a).unwrap();
    assert!(!first.manifest("graph-1").unwrap().instance.is_empty());
    drop(ca);
    drop(first);
    drop(rt);

    // The relay's database is wiped and served again on the same address.
    rt = tokio::runtime::Runtime::new().unwrap();
    let second = serve_on(&rt, addr, Config::default());
    let (mut b, mut cb) = {
        let mut s = Store::open_in_memory().unwrap();
        let c = SyncClient::new(HttpTransport::new(&base, "graph-1", None).unwrap(), &s).unwrap();
        s.set_today(Some(NaiveDate::from_ymd_opt(2026, 11, 2).unwrap()));
        (s, c)
    };
    let same = b.create_page("Shared").unwrap();
    assert_eq!(same, page);
    write(&mut b, &same, "after the wipe");
    cb.sync_once(&mut b).unwrap();

    let mut ca = SyncClient::new(HttpTransport::new(&base, "graph-1", None).unwrap(), &a).unwrap();
    ca.sync_once(&mut a).unwrap();
    let seen = texts(&mut a, &page);
    assert!(seen.contains(&"after the wipe".to_string()), "{seen:?}");
    assert!(!second.manifest("graph-1").unwrap().instance.is_empty());
}

#[test]
fn relay_down_means_offline_not_data_loss() {
    let (mut a, mut ca) = {
        let mut s = Store::open_in_memory().unwrap();
        let t = HttpTransport::new("http://127.0.0.1:9", "graph-1", None).unwrap();
        let c = SyncClient::new(t, &s).unwrap();
        s.create_page("Offline note").unwrap();
        (s, c)
    };
    assert!(matches!(ca.sync_status(&mut a), SyncStatus::Offline { queued } if queued > 0));
    assert!(a.find_page("Offline note").unwrap().is_some());
}

/// §12.8: with no tokens configured, `utopia-syncd` runs without auth (local development).
/// The Worker refuses everyone instead (`relay-cloudflare/README.md`, difference 1), so
/// this is not part of the contract.
#[test]
fn an_empty_token_list_disables_auth() {
    let server = contract::targets::InProcess::start(Config::default());
    let mut a = Store::open_in_memory().unwrap();
    let mut ca = SyncClient::new(
        HttpTransport::new(&server.base, "graph-1", None).unwrap(),
        &a,
    )
    .unwrap();
    a.create_page("Anonymous").unwrap();
    ca.sync_once(&mut a).unwrap();
    assert_eq!(SyncClient::<HttpTransport>::queued(&mut a).unwrap(), 0);
}

#[test]
fn folder_transport_converges_without_a_server() {
    use utopia_sync::folder::FolderTransport;
    let shared = tempfile::tempdir().unwrap();
    let mut a = Store::open_in_memory().unwrap();
    let mut b = Store::open_in_memory().unwrap();
    let mut ca =
        SyncClient::new(FolderTransport::new(shared.path(), "deviceA").unwrap(), &a).unwrap();
    let mut cb =
        SyncClient::new(FolderTransport::new(shared.path(), "deviceB").unwrap(), &b).unwrap();
    let p = a.create_page("Shared via iCloud").unwrap();
    write(&mut a, &p, "from A");
    ca.sync_once(&mut a).unwrap();
    cb.sync_once(&mut b).unwrap();
    write(&mut b, &p, "from B");
    cb.sync_once(&mut b).unwrap();
    ca.sync_once(&mut a).unwrap();
    let graph = |s: &mut Store| -> Vec<(String, Vec<String>)> {
        s.list_pages(true)
            .unwrap()
            .into_iter()
            .map(|p| (p.title.clone(), texts(s, &p.id)))
            .collect()
    };
    assert_eq!(graph(&mut a), graph(&mut b));
    assert_eq!(texts(&mut a, &p), vec!["from A", "from B"]);
    // Append-only: each device writes only into its own folder.
    let dirs: Vec<String> = std::fs::read_dir(shared.path().join("pages").join(p.as_str()))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
        .collect();
    assert_eq!(dirs.len(), 2, "{dirs:?}");
}

#[test]
fn lan_relay_serves_and_is_discoverable() {
    let dir = tempfile::tempdir().unwrap();
    let relay = utopia_syncd::lan::serve_lan(
        &dir.path().join("lan.db"),
        "127.0.0.1:0",
        "home-graph",
        Config::default(),
        true,
    )
    .unwrap();
    let base = format!("http://{}", relay.addr);
    // Two devices sync through the in-process LAN relay.
    let mk = |s: &Store| {
        SyncClient::new(HttpTransport::new(&base, "home-graph", None).unwrap(), s).unwrap()
    };
    let mut a = Store::open_in_memory().unwrap();
    let mut b = Store::open_in_memory().unwrap();
    let (mut ca, mut cb) = (mk(&a), mk(&b));
    let p = a.create_page("Kitchen list").unwrap();
    ca.sync_once(&mut a).unwrap();
    cb.sync_once(&mut b).unwrap();
    assert!(b.page_row(&p).unwrap().is_some());
    // mDNS discovery: the advertised relay for this graph is found on the local network.
    match utopia_syncd::lan::discover("home-graph", std::time::Duration::from_secs(4)) {
        Some(url) => assert!(url.ends_with(&format!(":{}", relay.addr.port())), "{url}"),
        // Multicast can be unavailable in sandboxed CI; serving above is the contract.
        None => eprintln!("mDNS discovery unavailable in this environment"),
    }
}
