//! Difficulty and role classifiers: Jev, OpenAI Decisions, the agent CLI, and their Keychain keys.

mod cli;
mod http;
mod jev;
mod keychain;
mod keys;
mod openai;

pub use cli::{Cli, cli_prompt, parse_role, parse_tier};
pub use jev::{Jev, JevChoice};
pub use keychain::{KEYCHAIN_SERVICE, SecretStore, set_secret_store};
pub use keys::{JEV_KEY, Key, OPENAI_KEY};
pub use openai::{OpenAi, parse_level_probs};

use crate::{
    config::{CLASSIFIER_CLI, CLASSIFIER_JEV, CLASSIFIER_OPENAI},
    runner::{Classifier, Deps},
};
use std::{sync::Arc, time::Duration};

/// Bounds the agent CLI classifier's one read-only call.
pub const CLI_CLASSIFY_TIMEOUT: Duration = Duration::from_secs(60);

/// The policy's difficulty classifier, or None when it is off. The CLI
/// classifier runs the agent `classifier_agent` names, else the easy tier's
/// first installed agent.
pub fn for_policy(deps: &Deps) -> Option<Arc<dyn Classifier>> {
    let model = deps.policy.classifier_model.clone();
    match deps.policy.classifier.as_str() {
        CLASSIFIER_JEV => Some(Arc::new(Jev::new(model))),
        CLASSIFIER_OPENAI => Some(Arc::new(OpenAi::new(model))),
        CLASSIFIER_CLI => Some(Arc::new(cli_for(deps))),
        _ => None,
    }
}

/// The agent and model the CLI classifier asks, whether or not it is the
/// policy's; with no easy-tier agent installed it has no agent and fails.
pub fn cli_for(deps: &Deps) -> Cli {
    let disabled = crate::runner::store_disabled(deps.store.as_deref(), &deps.state_dir);
    // `agent` or `agent/model`; a model can hold a slash of its own (OpenCode's
    // `provider/model`), so only the first one splits.
    let picked = deps.policy.classifier_agent.trim();
    let (agent, picked_model) = picked.split_once('/').unwrap_or((picked, ""));
    if !agent.is_empty()
        && let Some(a) = deps
            .adapters
            .iter()
            .find(|a| a.name() == agent && deps.is_installed(&a.binary()))
    {
        // With no model named, the agent's easy-tier model, else its CLI's default.
        let model = if picked_model.is_empty() {
            deps.table
                .tiers
                .get("easy")
                .into_iter()
                .flatten()
                .find(|e| {
                    e.agent == agent
                        && !crate::config::is_model_disabled(
                            &disabled,
                            &e.provider,
                            &e.agent,
                            &e.model,
                        )
                })
                .or_else(|| {
                    deps.table
                        .tiers
                        .get("easy")
                        .into_iter()
                        .flatten()
                        .find(|e| e.agent == agent)
                })
                .map(|e| e.model.clone())
                .unwrap_or_default()
        } else {
            picked_model.to_owned()
        };
        let reason = crate::config::disabled_reason(&disabled, "", agent, &model);
        if !reason.is_empty() {
            return Cli {
                agent: agent.to_string(),
                model,
                run: Some(Arc::new(move |_| {
                    let reason = reason.clone();
                    Box::pin(async move { Err(crate::Error::Message(reason)) })
                })),
            };
        }
        return Cli::with_adapter(a.clone(), model, CLI_CLASSIFY_TIMEOUT);
    }
    let mut model = deps.policy.classifier_model.clone();
    for e in deps.table.tiers.get("easy").into_iter().flatten() {
        let selected = if model.is_empty() { &e.model } else { &model };
        if crate::config::is_model_disabled(&disabled, &e.provider, &e.agent, selected) {
            continue;
        }
        for a in &deps.adapters {
            if a.name() != e.agent || !deps.is_installed(&a.binary()) {
                continue;
            }
            if model.is_empty() {
                model = e.model.clone();
            }
            return Cli::with_adapter(a.clone(), model, CLI_CLASSIFY_TIMEOUT);
        }
    }
    Cli::default()
}

/// The API key a classifier reads, for those that need one.
pub fn key_for(name: &str) -> Option<Key> {
    match name {
        CLASSIFIER_JEV => Some(JEV_KEY),
        CLASSIFIER_OPENAI => Some(OPENAI_KEY),
        _ => None,
    }
}

/// Where the classifier finds its key: env, keychain, missing, or none for a
/// classifier that needs no key.
pub fn key_status(name: &str) -> String {
    key_for(name).map_or_else(|| "none".into(), |k| k.status().into())
}

fn or_default(v: &str, d: &str) -> String {
    if v.is_empty() { d.into() } else { v.into() }
}
