//! Polterware Dispatch's local core (`dispatch serve`): the only process that
//! opens the dispatch store. The CLI, the app, and Polterware attach to it
//! through core.json over Connect.

mod agent_models;
mod connect;
pub(crate) mod convert;
mod event_pages;
mod ops;
mod routes;
mod runs;
mod seed;
mod workspace;

pub use connect::{RpcError, RpcResult};

use crate::{
    Error,
    adapters::known,
    classifier,
    config::{self, Policy},
    host::Host,
    roles::{self, Profile},
    runner::{self, Deps},
    store::Store,
    tiers::Table,
    types,
};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI32, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

/// Header naming the calling client; the app is the default.
pub const CLIENT_HEADER: &str = crate::coreclient::CLIENT_HEADER;

/// What the per-run coordination of the core keeps.
#[derive(Default)]
struct WatchState {
    log_off: HashMap<String, u64>,
    log_ready: HashSet<String>,
    finalized: HashSet<String>,
}

/// The local core: the store it alone opens, the session token, and the
/// coordination of the runs it executes.
pub struct Core {
    pub store: Arc<Store>,
    pub token: String,
    pub started: Instant,
    /// Identifies the core binary (`runstate::build_id`).
    pub build: String,
    /// The database directory; the dispatch state (tiers, catalog prices,
    /// live logs) lives in its dispatch folder.
    pub db_dir: PathBuf,
    /// Reads Polterware: projects, run directories, cards, and AgentRuns.
    pub host: Arc<dyn Host>,

    last_active: AtomicI64,
    streams: AtomicI32,
    done: CancellationToken,

    start_mu: tokio::sync::Mutex<()>,
    cancels: Mutex<HashMap<String, CancellationToken>>,
    finish: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    watch: Mutex<WatchState>,
    agent_models: agent_models::Cache,
    probes: tokio::sync::Mutex<HashMap<String, Instant>>,
}

fn unix_nanos() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_nanos()).unwrap_or(i64::MAX))
}

impl Core {
    /// Wires the core around the only open store of this database.
    pub fn new(
        store: Arc<Store>,
        token: String,
        host: Arc<dyn Host>,
        db_dir: PathBuf,
        build: String,
    ) -> Arc<Self> {
        Arc::new(Self {
            store,
            token,
            started: Instant::now(),
            build,
            db_dir,
            host,
            last_active: AtomicI64::new(unix_nanos()),
            streams: AtomicI32::new(0),
            done: CancellationToken::new(),
            start_mu: tokio::sync::Mutex::new(()),
            cancels: Mutex::new(HashMap::new()),
            finish: Mutex::new(HashMap::new()),
            watch: Mutex::new(WatchState::default()),
            agent_models: agent_models::Cache::default(),
            probes: tokio::sync::Mutex::new(HashMap::new()),
        })
    }

    pub(crate) fn touch(&self) {
        self.last_active.store(unix_nanos(), Ordering::Relaxed);
    }

    /// Cancelled when the core was asked to stop.
    pub fn done(&self) -> CancellationToken {
        self.done.clone()
    }

    /// Asks the core to shut down; the serve loop owns the actual exit.
    pub fn stop(&self) {
        self.done.cancel();
    }

    /// Whether nothing used the core for at least `after`.
    pub fn idle(&self, after: Duration) -> bool {
        if self.streams.load(Ordering::Relaxed) > 0 {
            return false;
        }
        let since = unix_nanos() - self.last_active.load(Ordering::Relaxed);
        since >= i64::try_from(after.as_nanos()).unwrap_or(i64::MAX)
    }

    /// Counts an open stream until the guard drops.
    pub(crate) fn stream_guard(self: &Arc<Self>) -> StreamGuard {
        self.streams.fetch_add(1, Ordering::Relaxed);
        StreamGuard(self.clone())
    }

    /// The dispatch state folder: tiers.json, the catalog prices, and live logs.
    pub fn state_dir(&self) -> PathBuf {
        if !self.db_dir.as_os_str().is_empty() {
            return self.db_dir.join("dispatch");
        }
        std::env::temp_dir().join("polterware-dispatch")
    }

    pub(crate) fn store_dyn(&self) -> Arc<dyn types::Store> {
        self.store.clone()
    }

    fn put_cancel(&self, id: &str, token: CancellationToken) {
        lock(&self.cancels).insert(id.into(), token);
    }

    fn pop_cancel(&self, id: &str) -> Option<CancellationToken> {
        lock(&self.cancels).remove(id)
    }

    /// The lock that orders a run's execution, cancellation, merge, and clean.
    fn run_mu(&self, id: &str) -> Arc<tokio::sync::Mutex<()>> {
        lock(&self.finish).entry(id.into()).or_default().clone()
    }

    /// The core's runner, with the stored tier table.
    pub(crate) fn deps(&self) -> Result<Deps, Error> {
        let table = Table::load_from_store(Some(self.store.as_ref()))?;
        Ok(Deps {
            adapters: known::known(),
            table,
            store: Some(self.store_dyn()),
            state_dir: self.state_dir().to_string_lossy().into_owned(),
            ..Deps::default()
        })
    }

    /// The runner with the routing policy, which is global, the classifier, and the roles of
    /// `dir`.
    pub(crate) fn deps_in(&self, dir: &str) -> Result<Deps, Error> {
        let mut deps = self.deps()?;
        deps.policy = self.global_policy()?;
        deps.classifier = classifier::for_policy(&deps);
        deps.profiles = self.profiles(dir)?;
        Ok(deps)
    }

    pub(crate) fn profiles(&self, dir: &str) -> Result<BTreeMap<String, Profile>, Error> {
        let project = if dir.is_empty() {
            PathBuf::new()
        } else {
            roles::project_profiles_path(Path::new(dir))
        };
        roles::load_all_profiles(&roles::global_profiles_path(&self.state_dir()), &project)
    }

    /// The routing policy: one for every repository.
    pub(crate) fn global_policy(&self) -> Result<Policy, Error> {
        config::load_with_store(
            &config::policy_config_path(&self.state_dir()),
            self.store.as_ref(),
        )
    }

    pub(crate) fn disabled_models(&self) -> Vec<types::DisabledModel> {
        let _ = config::migrate_legacy_deny(
            &config::policy_config_path(&self.state_dir()),
            self.store.as_ref(),
        );
        types::Store::list_disabled_models(self.store.as_ref()).unwrap_or_default()
    }

    pub(crate) fn disabled_agents(&self) -> Vec<String> {
        types::Store::list_disabled_agents(self.store.as_ref()).unwrap_or_default()
    }

    /// Marks RUNNING and PENDING runs whose runner died with the previous
    /// core as ORPHANED, before the server accepts work.
    async fn reconcile_on_boot(&self) {
        if let Ok(deps) = self.deps() {
            let cancel = CancellationToken::new();
            let _ = tokio::time::timeout(Duration::from_secs(15), deps.reconcile(&cancel)).await;
        }
    }
}

pub(crate) fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Keeps the core alive while a run stream is open.
pub(crate) struct StreamGuard(Arc<Core>);

impl Drop for StreamGuard {
    fn drop(&mut self) {
        self.0.streams.fetch_sub(1, Ordering::Relaxed);
        self.0.touch();
    }
}

/// Mints a session token: 32 random bytes in hex.
pub fn mint_token() -> String {
    let b: [u8; 32] = rand::random();
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Listens on a loopback address and returns the listener and its endpoint.
pub async fn listen(address: &str) -> Result<(TcpListener, String), Error> {
    let listener = TcpListener::bind(address).await?;
    let endpoint = format!("http://{}", listener.local_addr()?);
    Ok((listener, endpoint))
}

/// Reconciles orphaned runs, applies the agent config, and serves the core's
/// Connect services on `listener` until the core stops. HTTP/1.1 and h2c are
/// both accepted.
pub async fn serve(listener: TcpListener, core: Arc<Core>) -> Result<(), Error> {
    core.reconcile_on_boot().await;
    config::migrate_legacy_deny(
        &config::policy_config_path(&core.state_dir()),
        core.store.as_ref(),
    )?;
    let _ = config::apply_agent_config(&core.state_dir());
    let app = routes::router(core.clone());
    let done = core.done();
    loop {
        let (stream, _) = tokio::select! {
            accepted = listener.accept() => accepted?,
            () = done.cancelled() => return Ok(()),
        };
        let app = app.clone();
        let done = done.clone();
        tokio::spawn(async move {
            let io = hyper_util::rt::TokioIo::new(stream);
            let service = hyper_util::service::TowerToHyperService::new(app);
            let builder =
                hyper_util::server::conn::auto::Builder::new(hyper_util::rt::TokioExecutor::new());
            let conn = builder.serve_connection_with_upgrades(io, service);
            tokio::pin!(conn);
            tokio::select! {
                _ = conn.as_mut() => {}
                () = done.cancelled() => {
                    conn.as_mut().graceful_shutdown();
                    let _ = tokio::time::timeout(Duration::from_secs(5), conn).await;
                }
            }
        });
    }
}

/// The time a stored RFC 3339 value names, in UTC with whole seconds as the
/// wire carries it; empty when unset.
pub(crate) fn wire_time(s: &str) -> String {
    let Ok(t) = chrono::DateTime::parse_from_rfc3339(s.trim()) else {
        return String::new();
    };
    let t = t.with_timezone(&chrono::Utc);
    if t.timestamp() <= -62_135_596_800 {
        return String::new();
    }
    t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

pub(crate) fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Nanos, true)
}

pub(crate) fn is_terminal(status: &str) -> bool {
    runner::terminal_status(status)
}
