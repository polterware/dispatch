//! Each agent CLI's model list, kept fresh for a few minutes and on disk
//! across restarts.

use super::{Core, lock};
use crate::{Error, adapters::Model, runner::AgentAdapter};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
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
    /// The agent's model list. One asked within [`GAP`] is returned as is; an
    /// older one, or one a core before this one kept on disk, is returned at
    /// once while its CLI is asked again in the background. Only an agent
    /// never asked waits for its CLI, and calls for it meanwhile wait for that
    /// answer. A failure is kept like a list, so an agent that cannot list
    /// does not hold up every read; a refresh that fails keeps the list it had.
    pub(crate) async fn agent_models(
        self: &Core,
        a: &Arc<dyn AgentAdapter>,
    ) -> Result<Vec<Model>, Error> {
        let name = a.name();
        let entry = {
            let mut entries = lock(&self.agent_models.entries);
            entries
                .entry(name.clone())
                .or_insert_with(|| {
                    let mut e = Entry::default();
                    if let Some(saved) = self.saved_models().remove(&name) {
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
                self.refresh_models(entry.clone(), a.clone());
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
                self.save_models(&name, Utc::now(), models);
            }
            Err(err) => e.err = Some(err.to_string()),
        }
        r
    }

    fn refresh_models(&self, entry: Arc<tokio::sync::Mutex<Entry>>, a: Arc<dyn AgentAdapter>) {
        let core_state = self.state_dir();
        tokio::spawn(async move {
            let r = tokio::time::timeout(REFRESH_TIMEOUT, a.models()).await;
            let mut e = entry.lock().await;
            e.refreshing = false;
            e.at = Some(Utc::now());
            match r {
                Ok(Ok(models)) => {
                    save_to(&core_state, &a.name(), Utc::now(), &models);
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

    fn saved_models(&self) -> HashMap<String, Saved> {
        read_saved(&self.state_dir())
    }

    fn save_models(&self, agent: &str, at: DateTime<Utc>, models: &[Model]) {
        save_to(&self.state_dir(), agent, at, models);
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
