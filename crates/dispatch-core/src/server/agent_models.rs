//! Each agent CLI's model list, kept fresh for a few minutes and on disk
//! across restarts.

use super::{Core, lock};
use crate::{
    Error,
    adapters::{
        Capabilities, Model,
        probe::ProbeReport,
        quota::{Quota, QuotaSample},
    },
    runner::{AgentAdapter, AgentCall, AttemptResult},
};
use chrono::{DateTime, Utc};
use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// How long an agent CLI's model list stays fresh: models change rarely, and
/// asking a CLI takes seconds each time Runs, Add model, Stats, or the tier
/// check needs it.
const GAP: Duration = Duration::from_secs(5 * 60);

/// Bounds a refresh asked in the background, which no caller waits on.
const REFRESH_TIMEOUT: Duration = Duration::from_secs(20);

/// Keeps each agent's last list in the state directory, so a core that starts
/// answers with it at once.
const FILE: &str = "agent-models.json";

#[derive(Default)]
struct Entry {
    at: Option<DateTime<Utc>>,
    models: Vec<Model>,
    err: Option<String>,
    refreshing: bool,
}

#[derive(Default)]
pub(crate) struct Cache {
    entries: Mutex<HashMap<String, Arc<tokio::sync::Mutex<Entry>>>>,
}

#[derive(Serialize, Deserialize)]
struct Saved {
    at: DateTime<Utc>,
    #[serde(default)]
    models: Option<Vec<Model>>,
}

fn stale(at: DateTime<Utc>) -> bool {
    (Utc::now() - at).to_std().unwrap_or(Duration::ZERO) >= GAP
}

impl Core {
    /// The agent's model list, through the core's cache ([`Cache::models`]).
    pub(crate) async fn agent_models(
        self: &Core,
        a: &Arc<dyn AgentAdapter>,
    ) -> Result<Vec<Model>, Error> {
        self.agent_models.models(&self.state_dir(), a).await
    }

    /// Wraps each adapter so the runner's model discovery reads the same cache as
    /// `ListModels`, instead of asking every agent CLI again on each run and route.
    pub(crate) fn cached_adapters(
        &self,
        adapters: Vec<Arc<dyn AgentAdapter>>,
    ) -> Vec<Arc<dyn AgentAdapter>> {
        let state = self.state_dir();
        adapters
            .into_iter()
            .map(|inner| {
                Arc::new(Cached {
                    inner,
                    cache: self.agent_models.clone(),
                    state: state.clone(),
                }) as Arc<dyn AgentAdapter>
            })
            .collect()
    }
}

impl Cache {
    /// The agent's model list. One asked within [`GAP`] is returned as is; an
    /// older one, or one a core before this one kept on disk, is returned at
    /// once while its CLI is asked again in the background. Only an agent
    /// never asked waits for its CLI, and calls for it meanwhile wait for that
    /// answer. A failure is kept like a list, so an agent that cannot list
    /// does not hold up every read; a refresh that fails keeps the list it had.
    async fn models(&self, state: &Path, a: &Arc<dyn AgentAdapter>) -> Result<Vec<Model>, Error> {
        let name = a.name();
        let entry = {
            let mut entries = lock(&self.entries);
            entries
                .entry(name.clone())
                .or_insert_with(|| {
                    let mut e = Entry::default();
                    if let Some(saved) = read_saved(state).remove(&name) {
                        e.at = Some(saved.at);
                        e.models = saved.models.unwrap_or_default();
                    }
                    Arc::new(tokio::sync::Mutex::new(e))
                })
                .clone()
        };
        let mut e = entry.lock().await;
        if let Some(at) = e.at {
            if stale(at) && !e.refreshing {
                e.refreshing = true;
                refresh_models(state.to_path_buf(), entry.clone(), a.clone());
            }
            return match &e.err {
                Some(err) => Err(Error::Message(err.clone())),
                None => Ok(e.models.clone()),
            };
        }
        let r = a.models().await;
        e.at = Some(Utc::now());
        match &r {
            Ok(models) => {
                e.models.clone_from(models);
                e.err = None;
                save_to(state, &name, Utc::now(), models);
            }
            Err(err) => e.err = Some(err.to_string()),
        }
        r
    }
}

fn refresh_models(state: PathBuf, entry: Arc<tokio::sync::Mutex<Entry>>, a: Arc<dyn AgentAdapter>) {
    tokio::spawn(async move {
        let r = tokio::time::timeout(REFRESH_TIMEOUT, a.models()).await;
        let mut e = entry.lock().await;
        e.refreshing = false;
        e.at = Some(Utc::now());
        match r {
            Ok(Ok(models)) => {
                save_to(&state, &a.name(), Utc::now(), &models);
                e.models = models;
                e.err = None;
            }
            Ok(Err(err)) if e.models.is_empty() => e.err = Some(err.to_string()),
            Err(_) if e.models.is_empty() => {
                e.err = Some("context deadline exceeded".into());
            }
            _ => {}
        }
    });
}

/// An agent adapter whose model list comes through the core's [`Cache`]; every other
/// entry point goes to the adapter it wraps.
struct Cached {
    inner: Arc<dyn AgentAdapter>,
    cache: Arc<Cache>,
    state: PathBuf,
}

impl AgentAdapter for Cached {
    fn name(&self) -> String {
        self.inner.name()
    }
    fn binary(&self) -> String {
        self.inner.binary()
    }
    fn capabilities(&self) -> Capabilities {
        self.inner.capabilities()
    }
    fn models(&self) -> BoxFuture<'_, Result<Vec<Model>, Error>> {
        Box::pin(async move { self.cache.models(&self.state, &self.inner).await })
    }
    fn quota(&self) -> BoxFuture<'_, Result<Quota, Error>> {
        self.inner.quota()
    }
    fn model_quotas(
        &self,
        models: Vec<String>,
    ) -> Option<BoxFuture<'_, Result<HashMap<String, Quota>, Error>>> {
        self.inner.model_quotas(models)
    }
    fn usage_source(&self) -> Option<&'static str> {
        self.inner.usage_source()
    }
    fn usage(&self) -> BoxFuture<'_, Result<Vec<QuotaSample>, Error>> {
        self.inner.usage()
    }
    fn probe(&self) -> Option<BoxFuture<'_, ProbeReport>> {
        self.inner.probe()
    }
    fn run(&self, call: AgentCall) -> BoxFuture<'_, AttemptResult> {
        self.inner.run(call)
    }
    fn resume(&self, call: AgentCall) -> BoxFuture<'_, AttemptResult> {
        self.inner.resume(call)
    }
}

fn read_saved(state: &std::path::Path) -> HashMap<String, Saved> {
    std::fs::read(state.join(FILE))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn save_to(state: &std::path::Path, agent: &str, at: DateTime<Utc>, models: &[Model]) {
    // Keeps two agents' saves from losing each other's list.
    static FILE_MU: Mutex<()> = Mutex::new(());
    let _guard = lock(&FILE_MU);
    let mut saved = read_saved(state);
    saved.insert(
        agent.into(),
        Saved {
            at,
            models: Some(models.to_vec()),
        },
    );
    let Ok(b) = serde_json::to_vec(&saved) else {
        return;
    };
    if std::fs::create_dir_all(state).is_ok() {
        let _ = crate::reports::write_file_atomic(&state.join(FILE), &b);
    }
}
