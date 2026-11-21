//! The relays the contract runs against.
//!
//! - [`in_process`]: a `utopia-syncd` router on 127.0.0.1, one per scenario, with a token
//!   and the quota the scenario needs. Always available.
//! - [`worker`]: the Cloudflare Worker that
//!   `relay-cloudflare/scripts/test-against-rust-client.sh` starts under
//!   `wrangler dev --local`, described by the environment:
//!   - `UTOPIA_RELAY_URL` — its origin (`http://127.0.0.1:<port>`);
//!   - `UTOPIA_RELAY_TOKEN` — the bearer token the script gave `RELAY_TOKENS`;
//!   - `UTOPIA_RELAY_STATE_DIR` — wrangler's `--persist-to` directory (DO SQLite + R2),
//!     scanned for plaintext by the encrypted scenarios;
//!   - `UTOPIA_RELAY_QUOTA` — set only in the script's small-quota run, to its quota;
//!   - `UTOPIA_RELAY_REQUIRED=1` — a missing relay, token or state directory is a failure
//!     instead of a printed skip. The script and CI set it, so they cannot pass by skipping.
//!
//! With `UTOPIA_RELAY_URL` unset (and the switch off) every Worker test prints a skip and
//! passes, so `cargo test --workspace` needs no Node.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use utopia_syncd::{router, Config, Db, DEFAULT_QUOTA_BYTES};

use super::{contains, Need, RelayUnderTest, Scan, SMALL_QUOTA};

/// The token the in-process relay accepts.
pub const IN_PROCESS_TOKEN: &str = "contract-token";

/// A `utopia-syncd` served on a free loopback port for the life of the value.
pub struct InProcess {
    pub base: String,
    pub db: Arc<Db>,
    token: Option<String>,
    small_quota: Option<u64>,
    _rt: tokio::runtime::Runtime,
}

impl InProcess {
    pub fn start(cfg: Config) -> Self {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let db = Arc::new(Db::open_in_memory().unwrap());
        let token = cfg.tokens.first().cloned();
        let small_quota = (cfg.quota_bytes != DEFAULT_QUOTA_BYTES).then_some(cfg.quota_bytes);
        let app = router(db.clone(), cfg);
        let listener = rt
            .block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))
            .unwrap();
        let addr = listener.local_addr().unwrap();
        rt.spawn(async move { axum::serve(listener, app).await.unwrap() });
        Self {
            base: format!("http://{addr}"),
            db,
            token,
            small_quota,
            _rt: rt,
        }
    }
}

impl RelayUnderTest for InProcess {
    fn url(&self) -> &str {
        &self.base
    }
    fn token(&self) -> Option<&str> {
        self.token.as_deref()
    }
    fn quota(&self) -> Option<u64> {
        self.small_quota
    }
    fn scan_storage(&self, needle: &[u8]) -> Option<Scan> {
        let stored = self.db.all_payloads().unwrap();
        let hits = stored
            .iter()
            .enumerate()
            .filter(|(_, p)| contains(p, needle))
            .map(|(i, _)| format!("payload #{i}"))
            .collect();
        Some(Scan {
            inspected: stored.len(),
            hits,
        })
    }
    fn log_rows(&self, graph: &str, page: &str) -> Option<usize> {
        Some(self.db.log_len(graph, page).unwrap())
    }
    fn required(&self) -> bool {
        true
    }
}

/// The contract's in-process relay: auth on, the quota the scenario needs.
pub fn in_process(need: Need) -> Option<Box<dyn RelayUnderTest>> {
    Some(Box::new(InProcess::start(Config {
        tokens: vec![IN_PROCESS_TOKEN.into()],
        quota_bytes: match need {
            Need::Default => DEFAULT_QUOTA_BYTES,
            Need::SmallQuota => SMALL_QUOTA,
        },
    })))
}

/// `UTOPIA_RELAY_REQUIRED=1`: the Worker must be there.
pub fn required() -> bool {
    std::env::var("UTOPIA_RELAY_REQUIRED").is_ok_and(|v| v.trim() == "1")
}

/// Skip with the reason printed, or fail when the switch is on.
pub fn unavailable<T>(why: &str) -> Option<T> {
    assert!(
        !required(),
        "UTOPIA_RELAY_REQUIRED=1, but {why} \
         (run relay-cloudflare/scripts/test-against-rust-client.sh)"
    );
    eprintln!("skipped: {why}; run relay-cloudflare/scripts/test-against-rust-client.sh");
    None
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

/// The Worker the environment describes, as far as it does.
pub struct WorkerEnv {
    pub url: String,
    pub token: Option<String>,
    /// `UTOPIA_RELAY_QUOTA`: the Worker's quota in the small-quota run.
    pub quota: Option<u64>,
    pub state: Option<PathBuf>,
}

/// Read the environment and prove the Worker answers. `None` when `UTOPIA_RELAY_URL` is
/// unset (after [`unavailable`]); a URL that does not answer always fails, since naming
/// one means a relay was meant to be there.
pub fn worker_env() -> Option<WorkerEnv> {
    let Some(url) = env("UTOPIA_RELAY_URL") else {
        return unavailable("UTOPIA_RELAY_URL is unset");
    };
    let url = url.trim_end_matches('/').to_string();
    let health = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(10))
        .build()
        .get(&format!("{url}/healthz"))
        .call();
    match health {
        Ok(r) if r.status() == 200 => {}
        Ok(r) => panic!("{url}/healthz answered {}", r.status()),
        Err(e) => panic!("UTOPIA_RELAY_URL={url} does not answer /healthz: {e}"),
    }
    let quota = env("UTOPIA_RELAY_QUOTA").map(|q| {
        q.trim()
            .parse::<u64>()
            .unwrap_or_else(|_| panic!("UTOPIA_RELAY_QUOTA={q:?} is not a byte count"))
    });
    Some(WorkerEnv {
        url,
        token: env("UTOPIA_RELAY_TOKEN"),
        quota,
        state: state_dir(),
    })
}

/// Where wrangler keeps the local relay's state: `UTOPIA_RELAY_STATE_DIR`, or the
/// repository's `relay-cloudflare/.wrangler/state` (what `wrangler dev --local` writes by
/// default). `None` for a relay elsewhere, where there is no local state to read.
fn state_dir() -> Option<PathBuf> {
    if let Some(dir) = env("UTOPIA_RELAY_STATE_DIR") {
        return Some(PathBuf::from(dir));
    }
    let default =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../relay-cloudflare/.wrangler/state");
    default.is_dir().then_some(default)
}

pub struct Worker {
    env: WorkerEnv,
}

impl RelayUnderTest for Worker {
    fn url(&self) -> &str {
        &self.env.url
    }
    fn token(&self) -> Option<&str> {
        self.env.token.as_deref()
    }
    fn quota(&self) -> Option<u64> {
        self.env.quota
    }
    /// Every file under wrangler's local state (DO SQLite and its WAL, R2 objects, the
    /// trace store), not the relay's notion of a payload: the bytes an operator could read.
    fn scan_storage(&self, needle: &[u8]) -> Option<Scan> {
        fn walk(dir: &Path, needle: &[u8], files: &mut usize, hits: &mut Vec<String>) {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, needle, files, hits);
                } else if let Ok(bytes) = std::fs::read(&path) {
                    *files += 1;
                    if contains(&bytes, needle) {
                        hits.push(path.display().to_string());
                    }
                }
            }
        }
        let dir = self.env.state.as_ref()?;
        let (mut inspected, mut hits) = (0, Vec::new());
        walk(dir, needle, &mut inspected, &mut hits);
        Some(Scan { inspected, hits })
    }
    fn required(&self) -> bool {
        required()
    }
}

/// The Worker for one scenario, or a skip: the script's default run serves the default
/// scenarios and its small-quota run the quota ones.
pub fn worker(need: Need) -> Option<Box<dyn RelayUnderTest>> {
    let env = worker_env()?;
    match (need, env.quota) {
        (Need::SmallQuota, None) => unavailable(
            "this scenario needs a small-quota relay, and UTOPIA_RELAY_QUOTA is unset \
             (the script's second run sets it)",
        ),
        (Need::Default, Some(q)) => unavailable(&format!(
            "the relay runs with a {q}-byte quota (UTOPIA_RELAY_QUOTA), and this scenario \
             needs the default one"
        )),
        _ => Some(Box::new(Worker { env })),
    }
}
