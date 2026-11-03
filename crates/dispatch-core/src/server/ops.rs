//! DispatchService operations that answer JSON: routing, classifying, the
//! model catalog, merge/clean/recover, config, roles, the skill, and the
//! agent setup.

use super::{
    Core, RpcError, RpcResult, project_config,
    runs::{installed, save_run_worktree},
    workspace::run_area,
};
use crate::{
    Error,
    adapters::{acp, known},
    classifier,
    config::{self, CLASSIFIER_JEV, CLASSIFIER_OFF, CLASSIFIER_OPENAI, Policy},
    pass_model::difficulty_tier,
    proto as pb, reports,
    roles::{self, GENERAL_ROLE, Profile},
    runner::{self, AgentAdapter, ERR_AUTO_OFF, Options, PRIORITY_BALANCED, Task, task_text},
    sandbox, setup, skill,
    tiers::{Entry, Table},
    tiers_build,
    types::{self, Filters, Store as _},
};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

fn body<T: Serialize>(v: &T) -> RpcResult<String> {
    serde_json::to_string(v).map_err(RpcError::internal)
}

fn op_error(e: Error) -> RpcError {
    if types::is_unknown_run(&e) {
        RpcError::not_found(e.to_string())
    } else {
        RpcError::invalid(sandbox::redact(&e.to_string()))
    }
}

fn run_id(id: &str) -> RpcResult<String> {
    let id = id.trim();
    if id.is_empty() {
        return Err(RpcError::invalid("run id is required"));
    }
    Ok(id.into())
}

fn invalid(e: impl std::fmt::Display) -> RpcError {
    RpcError::invalid(e.to_string())
}

fn known_names() -> HashSet<String> {
    known::known().iter().map(|a| a.name()).collect()
}

/// The home folder and this core's own binary, the one the command, the
/// skill, and the hook point agents at.
fn setup_paths() -> RpcResult<(PathBuf, String)> {
    let home = dirs::home_dir().ok_or_else(|| RpcError::internal("no home folder"))?;
    let binary = crate::runstate::command_binary().map_err(RpcError::internal)?;
    Ok((home, binary.to_string_lossy().into_owned()))
}

impl Core {
    /// Resolves a named role for `opts.dir`; `none` runs with no role and
    /// keeps the classifier from picking one.
    pub(crate) fn apply_profile(&self, opts: &mut Options, name: &str) -> Result<(), Error> {
        if name == "none" {
            opts.no_profile = true;
            return Ok(());
        }
        let profiles = self.profiles(&opts.dir)?;
        let Some(p) = profiles.get(name) else {
            return Err(Error::Message(format!(
                "unknown role {name:?} (have: {})",
                roles::profile_names(&profiles).join(", ")
            )));
        };
        runner::apply_profile(opts, name, p)
    }

    pub(crate) async fn route(&self, m: pb::RouteRequest) -> RpcResult<pb::RouteResponse> {
        let dir = m.dir.trim().to_owned();
        let deps = self.deps_in(&dir).map_err(RpcError::internal)?;
        let hints = self.run_hints(&m.project_id, &m.card_slug, &dir).await;
        let mut opts = Options {
            tier: m.tier.trim().into(),
            read_only: m.read_only,
            dir,
            prompt: m.prompt.trim().into(),
            area: run_area(&hints),
            card_text: hints.card_text.clone(),
            filters: Filters {
                provider: m.provider.trim().into(),
                model: m.model.trim().into(),
                effort: m.effort.trim().into(),
                agent: m.agent.trim().into(),
                priority: m.priority.trim().into(),
                relax: m.relax,
            },
            ..Options::default()
        };
        let profile = m.profile.trim();
        if !profile.is_empty() {
            self.apply_profile(&mut opts, profile).map_err(invalid)?;
        }
        let report = deps.route(opts).await.map_err(invalid)?;
        Ok(pb::RouteResponse {
            ok: !report.candidates.is_empty(),
            json: body(&report)?,
        })
    }

    pub(crate) async fn classify(&self, m: pb::ClassifyRequest) -> RpcResult<pb::ClassifyResponse> {
        let prompt = m.prompt.trim();
        if prompt.is_empty() {
            return Err(RpcError::invalid("prompt is required"));
        }
        let dir = m.dir.trim();
        let deps = self.deps_in(dir).map_err(RpcError::internal)?;
        let Some(c) = deps.classifier.clone() else {
            return Err(RpcError::invalid(ERR_AUTO_OFF));
        };
        let mut out = serde_json::Map::new();
        out.insert(
            "keyStatus".into(),
            json!(classifier::key_status(&deps.policy.classifier)),
        );
        let hints = self.run_hints(&m.project_id, &m.card_slug, dir).await;
        let roles = deps.policy.roles.then(|| roles::pick_roles(&deps.profiles));
        let task = Task {
            text: task_text(&hints.card_text, prompt),
            roles: roles.clone(),
        };
        let diff = tokio::time::timeout(Duration::from_secs(90), c.classify(task))
            .await
            .map_err(|_| Error::Message("context deadline exceeded".into()))
            .and_then(|r| r)
            .map_err(|e| {
                RpcError::invalid(format!(
                    "classifier {} failed: {}",
                    c.name(),
                    sandbox::redact(&e.to_string())
                ))
            })?;
        let mut view = json!({
            "provider": c.name(), "model": diff.model, "tier": diff.difficulty.tier,
            "d": diff.difficulty.d,
            "latencyMs": i64::try_from(diff.latency.as_millis()).unwrap_or(i64::MAX),
        });
        if let Some(p) = diff.difficulty.probs {
            view["probs"] = json!(p);
        }
        if let Some(conf) = diff.confidence {
            view["confidence"] = json!(conf);
        }
        out.insert("difficulty".into(), view);
        // role is the pick the classifier's role answer makes: absent when
        // the pick is off or no role is pickable, general when none fits.
        if let Some(roles) = roles {
            let pick = match &diff.role {
                Some(r) => {
                    let role = roles::pick_role(&r.role, &r.probs, &roles);
                    let mut pick = json!({
                        "role": role, "picked": role != GENERAL_ROLE,
                        "probs": r.probs, "answer": r.role,
                    });
                    if let Some(conf) = r.confidence {
                        pick["confidence"] = json!(conf);
                    }
                    pick
                }
                None => json!({"role": GENERAL_ROLE, "picked": false, "unsupported": true}),
            };
            out.insert("role".into(), pick);
        }
        Ok(pb::ClassifyResponse {
            ok: true,
            json: body(&out)?,
        })
    }

    pub(crate) async fn list_model_catalog(
        &self,
        _: pb::ListModelCatalogRequest,
    ) -> RpcResult<pb::ListModelCatalogResponse> {
        let table =
            Table::load_from_store(Some(self.store.as_ref())).map_err(RpcError::internal)?;
        let state = self.state_dir();
        // Every model the installed agent CLIs report is listed; Auto routes
        // to the ones outside the tier table only with discover on.
        let (installed, _) = self.installed_entries().await;
        let (mut priority, mut discover) = (PRIORITY_BALANCED.to_owned(), false);
        if let Ok(p) = self.global_policy() {
            if !p.priority.is_empty() {
                priority = p.priority;
            }
            discover = p.discover;
        }
        let mut report = reports::report_catalog(
            &runner::load_catalog(&state.to_string_lossy()),
            &table,
            &installed,
        );
        report.priority = priority;
        report.discover = discover;
        Ok(pb::ListModelCatalogResponse {
            ok: true,
            json: body(&report)?,
        })
    }

    /// The catalog's benchmarks for every model the installed agent CLIs report.
    pub(crate) async fn list_model_benchmarks(
        &self,
        _: pb::ListModelBenchmarksRequest,
    ) -> RpcResult<pb::ListModelBenchmarksResponse> {
        let (installed, _) = self.installed_entries().await;
        let report = reports::benchmark_models(
            &runner::load_catalog(&self.state_dir().to_string_lossy()),
            &installed,
        );
        Ok(pb::ListModelBenchmarksResponse {
            ok: true,
            json: body(&report)?,
        })
    }

    pub(crate) async fn refresh_model_catalog(
        &self,
        _: pb::RefreshModelCatalogRequest,
    ) -> RpcResult<pb::RefreshModelCatalogResponse> {
        let mut out = json!({"source": crate::catalog::MODELS_DEV_URL});
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(RpcError::internal)?;
        let r = reports::refresh_catalog(&client, &self.state_dir(), chrono::Utc::now()).await;
        match &r {
            Ok(n) => out["models"] = json!(n),
            Err(e) => out["error"] = json!(e.to_string()),
        }
        Ok(pb::RefreshModelCatalogResponse {
            ok: r.is_ok(),
            json: body(&out)?,
        })
    }

    pub(crate) async fn merge_run(
        &self,
        m: pb::MergeRunRequest,
    ) -> RpcResult<pb::MergeRunResponse> {
        let id = run_id(&m.id)?;
        let deps = self.deps_in("").map_err(RpcError::internal)?;
        let mu = self.run_mu(&id);
        let _guard = mu.lock().await;
        let keep = m.keep_worktree;
        let res = tokio::task::spawn_blocking(move || deps.merge(&id, keep))
            .await
            .map_err(RpcError::internal)?
            .map_err(op_error)?;
        Ok(pb::MergeRunResponse {
            ok: res.ok,
            json: body(&res)?,
        })
    }

    pub(crate) async fn clean_run(
        &self,
        m: pb::CleanRunRequest,
    ) -> RpcResult<pb::CleanRunResponse> {
        let id = run_id(&m.id)?;
        let deps = self.deps_in("").map_err(RpcError::internal)?;
        let mu = self.run_mu(&id);
        let _guard = mu.lock().await;
        let force = m.force;
        let rec = tokio::task::spawn_blocking(move || deps.clean(&id, force))
            .await
            .map_err(RpcError::internal)?
            .map_err(op_error)?;
        Ok(pb::CleanRunResponse {
            ok: true,
            json: body(&json!({"id": rec.id, "removed": rec.worktree}))?,
        })
    }

    pub(crate) async fn recover_run(
        &self,
        m: pb::RecoverRunRequest,
    ) -> RpcResult<pb::RecoverRunResponse> {
        let id = run_id(&m.id)?;
        let deps = self.deps_in("").map_err(RpcError::internal)?;
        let mu = self.run_mu(&id);
        let _guard = mu.lock().await;
        let (rec, stat) = deps.recover(&id).await.map_err(op_error)?;
        let _ = save_run_worktree(self.store.as_ref(), &rec);
        Ok(pb::RecoverRunResponse {
            ok: true,
            json: body(&json!({"id": rec.id, "status": rec.status, "diffStat": stat}))?,
        })
    }

    pub(crate) async fn config(&self, m: pb::ConfigRequest) -> RpcResult<pb::ConfigResponse> {
        let dir = m.dir.trim();
        if m.project && dir.is_empty() {
            return Err(RpcError::invalid(
                "a project directory is required for project config",
            ));
        }
        let global = config::policy_config_path(&self.state_dir());
        let project_path = project_config(dir);
        let path = if m.project {
            project_path.clone()
        } else {
            global.clone()
        };
        let key = m.key.trim().to_owned();
        let values = m.values;
        let respond = |v: Value| -> RpcResult<pb::ConfigResponse> {
            Ok(pb::ConfigResponse {
                ok: true,
                json: body(&v)?,
            })
        };
        let path_text = |p: &Path| p.to_string_lossy().into_owned();
        match m.action.as_str() {
            // key-set and key-clear keep a classifier's API key in the macOS
            // Keychain; the reply says only where the key is now found.
            "key-set" | "key-clear" => {
                let Some(k) = classifier::key_for(&key) else {
                    return Err(RpcError::invalid(format!(
                        "unknown classifier key {key} (want jev|openai)"
                    )));
                };
                if m.action == "key-set" {
                    let [value] = values.as_slice() else {
                        return Err(RpcError::invalid("key-set takes the key as its one value"));
                    };
                    k.save(value).map_err(invalid)?;
                } else {
                    k.clear().map_err(invalid)?;
                }
                respond(json!({"key": key, "status": k.status()}))
            }
            // agent-add and agent-remove keep the ACP agents the user added,
            // in the global config only: a repository must not add a program to run.
            "agent-add" | "agent-remove" => {
                if m.project {
                    return Err(RpcError::invalid(
                        "agents are added to the global config only",
                    ));
                }
                let mut f = config::load(&global).map_err(invalid)?;
                if m.action == "agent-add" {
                    acp::valid_custom_agent(&key, &values).map_err(invalid)?;
                    f.acp_agents.insert(key.clone(), values);
                } else if f.acp_agents.remove(&key).is_none() {
                    return Err(RpcError::invalid(format!("no added agent {key}")));
                }
                config::write_config_key(&global, "acp_agents", &f.acp_agents)
                    .map_err(RpcError::internal)?;
                acp::set_custom_agents(f.acp_agents.clone().into_iter().collect());
                respond(
                    json!({"path": path_text(&global), "key": "acp_agents", "value": f.acp_agents}),
                )
            }
            "get" => {
                let f = if m.project {
                    config::load(&path)
                } else {
                    config::load_all(&global, &project_path)
                }
                .map_err(invalid)?;
                let v = match key.as_str() {
                    "" => json!(f),
                    "priority" => json!(f.priority),
                    "discover" => json!(f.discover),
                    "roles" => json!(f.roles),
                    "allow" => json!(f.allow),
                    "deny" => json!(f.deny),
                    "classifier" => json!(f.classifier),
                    "classifier_model" => json!(f.classifier_model),
                    "acp_agents" => json!(f.acp_agents),
                    "classifier_keys" => json!({
                        CLASSIFIER_JEV: classifier::key_status(CLASSIFIER_JEV),
                        CLASSIFIER_OPENAI: classifier::key_status(CLASSIFIER_OPENAI),
                    }),
                    "classifier_cli" => {
                        let deps = self.deps_in(dir).map_err(RpcError::internal)?;
                        let cli = classifier::cli_for(&deps);
                        json!({"agent": cli.agent, "model": cli.model})
                    }
                    other => return Err(RpcError::invalid(format!("unknown config key {other}"))),
                };
                respond(v)
            }
            action => {
                let mut f = config::load(&path).map_err(invalid)?;
                let (write_key, value) = set_config(&mut f, action, &key, &values)?;
                config::validate_config(&f).map_err(invalid)?;
                config::write_config_key(&path, &write_key, &value).map_err(RpcError::internal)?;
                respond(json!({"path": path_text(&path), "key": write_key, "value": value}))
            }
        }
    }

    pub(crate) async fn subagents(
        &self,
        m: pb::SubagentsRequest,
    ) -> RpcResult<pb::SubagentsResponse> {
        let dir = m.dir.trim();
        let state = self.state_dir();
        let user = roles::global_profiles_path(&state);
        let respond = |ok: bool, v: Value| -> RpcResult<pb::SubagentsResponse> {
            Ok(pb::SubagentsResponse {
                ok,
                json: body(&v)?,
            })
        };
        match m.action.as_str() {
            "save" | "remove" => {
                let name = m.name.trim();
                if !roles::valid_profile_name(name) {
                    return Err(RpcError::invalid(format!(
                        "invalid role name {name:?} (lowercase letters, digits, and dashes, starting with a letter)"
                    )));
                }
                let mut p = Profile::default();
                if m.action == "save" {
                    p = serde_json::from_str(&m.profile)
                        .map_err(|e| RpcError::invalid(format!("invalid role: {e}")))?;
                    let problems = roles::validate_profile(name, &p, &known_names());
                    if !problems.is_empty() {
                        return Err(RpcError::invalid(problems.join("; ")));
                    }
                }
                roles::save_profile(&user, name, p, m.action == "remove").map_err(invalid)?;
                return respond(true, json!({"wrote": user, "name": name}));
            }
            "list" | "check" => {}
            _ => return Err(RpcError::invalid("unknown roles action")),
        }
        let profiles = self.profiles(dir).map_err(invalid)?;
        if m.action == "list" {
            // user names the roles in the user file, the ones save and remove
            // change; the rest come from the repository's .dispatch/roles.json
            // or the built-in set.
            let own = roles::load_profiles(&user).map_err(invalid)?;
            let from_repo = if dir.is_empty() {
                BTreeMap::new()
            } else {
                roles::load_profiles(&roles::project_profiles_path(Path::new(dir)))
                    .map_err(invalid)?
            };
            let source: BTreeMap<&String, &str> = profiles
                .keys()
                .map(|name| {
                    let from = if from_repo.contains_key(name) {
                        "repository"
                    } else if own.contains_key(name) {
                        "user"
                    } else {
                        "builtin"
                    };
                    (name, from)
                })
                .collect();
            let pick = config::load_all(&config::policy_config_path(&state), &project_config(dir))
                .map_or(config::default_policy().roles, |p| p.roles);
            return respond(
                true,
                json!({
                    "roles": profiles, "user": roles::profile_names(&own), "path": user,
                    "source": source, "builtin": roles::profile_names(&roles::builtin_profiles()),
                    "pick": pick,
                }),
            );
        }
        let deps = self.deps_in(dir).map_err(RpcError::internal)?;
        let adapters: HashMap<String, Arc<dyn AgentAdapter>> =
            known::known().into_iter().map(|a| (a.name(), a)).collect();
        let names: HashSet<String> = adapters.keys().cloned().collect();
        let mut problems: Vec<String> = Vec::new();
        for name in roles::profile_names(&profiles) {
            let p = &profiles[&name];
            problems.extend(roles::validate_profile(&name, p, &names));
            if !p.tier.is_empty() && deps.table.tiers.get(&p.tier).is_none_or(Vec::is_empty) {
                problems.push(format!(
                    "{name}: tier {:?} has no entries in the tier table",
                    p.tier
                ));
            }
            for agent in p.prefer.iter().chain(&p.avoid) {
                if let Some(a) = adapters.get(agent)
                    && !installed(a.as_ref())
                {
                    problems.push(format!("{name}: {agent} is not installed"));
                }
            }
        }
        respond(
            problems.is_empty(),
            json!({"roles": profiles.len(), "problems": problems}),
        )
    }

    /// Installs the dispatch skill for the agent CLIs, or refreshes the
    /// copies already installed.
    pub(crate) async fn skill(&self, m: pb::SkillRequest) -> RpcResult<pb::SkillResponse> {
        let home = dirs::home_dir().ok_or_else(|| RpcError::internal("no home folder"))?;
        let out = match m.action.as_str() {
            "refresh" => json!({"refreshed": skill::refresh(&home).map_err(RpcError::internal)?}),
            "install" => {
                let agent = m.agent.trim();
                let targets = skill::targets(&home);
                let names: Vec<&String> = targets
                    .keys()
                    .filter(|n| agent.is_empty() || agent == n.as_str())
                    .collect();
                if names.is_empty() {
                    return Err(RpcError::invalid(format!("unknown agent {agent:?}")));
                }
                let (mut done, mut skipped, mut seen) = (Vec::new(), Vec::new(), HashSet::new());
                for name in names {
                    let dir = &targets[name].dir;
                    if !seen.insert(dir.clone()) {
                        continue;
                    }
                    match skill::install(dir, m.force) {
                        Ok(p) => done.push(p),
                        Err(e) => skipped.push(format!("{name}: {e}")),
                    }
                }
                json!({"installed": done, "skipped": skipped})
            }
            _ => {
                return Err(RpcError::invalid(
                    "unknown skill action: use install or refresh",
                ));
            }
        };
        Ok(pb::SkillResponse {
            ok: true,
            json: body(&out)?,
        })
    }

    /// How the agent CLIs reach dispatch: the command on the PATH, each
    /// agent's skill, the Claude Code SessionStart hook, and the OpenCode plugin.
    pub(crate) async fn get_agent_setup(
        &self,
        _: pb::GetAgentSetupRequest,
    ) -> RpcResult<pb::GetAgentSetupResponse> {
        let (home, binary) = setup_paths()?;
        Ok(pb::GetAgentSetupResponse {
            json: body(&setup::read(&home, &binary, &self.state_dir()))?,
        })
    }

    /// Links the command, installs the skill, switches the hook, or
    /// installs, removes, or switches the OpenCode plugin, pointing each at
    /// this core's binary, and answers the setup after it.
    pub(crate) async fn apply_agent_setup(
        &self,
        m: pb::ApplyAgentSetupRequest,
    ) -> RpcResult<pb::ApplyAgentSetupResponse> {
        let (home, binary) = setup_paths()?;
        let state = self.state_dir();
        let r = match m.action.as_str() {
            "link_cli" => setup::link_cli(&home, &binary).map(|_| ()),
            "install_skill" => setup::install_skill(&home, &m.agent).map(|_| ()),
            "hook_on" | "hook_off" => {
                setup::set_hook(&home, &binary, &state, m.action == "hook_on")
            }
            "install_opencode" => setup::install_opencode_plugin(
                &home,
                &setup::opencode_binary(&home, &binary),
                &state,
            )
            .map(|_| ()),
            "remove_opencode" => setup::remove_opencode_plugin(&home, &state),
            "opencode_on" | "opencode_off" => {
                setup::set_opencode_delegation(&state, m.action == "opencode_on")
            }
            _ => return Err(RpcError::invalid("unknown setup action")),
        };
        r.map_err(invalid)?;
        Ok(pb::ApplyAgentSetupResponse {
            json: body(&setup::read(&home, &binary, &state))?,
        })
    }

    pub(crate) async fn get_route_decision(
        &self,
        m: pb::GetRouteDecisionRequest,
    ) -> RpcResult<pb::GetRouteDecisionResponse> {
        let id = m.run_id.trim();
        if id.is_empty() {
            return Err(RpcError::invalid("run_id is required"));
        }
        let Some(dec) = self
            .store
            .get_route_decision(id)
            .map_err(RpcError::internal)?
        else {
            return Ok(pb::GetRouteDecisionResponse {
                ok: false,
                json: "{}".into(),
            });
        };
        let mut out = json!({
            "runId": dec.run_id, "policy": dec.policy_version, "route": dec.route,
            "priority": dec.priority, "difficulty": dec.difficulty,
            "tier": dec.difficulty.map(difficulty_tier).unwrap_or_default(),
            "area": dec.area, "classifier": dec.classifier,
            "probs": if dec.probs.is_empty() { Value::Null } else { json!(dec.probs) },
            "candidates": if dec.candidates.is_empty() { Value::Null } else { json!(dec.candidates) },
            "chosen": dec.chosen,
            "createdAt": super::wire_time(&dec.created_at),
        });
        // The run keeps its role and how it was picked; the decision log does not.
        if let Ok(run) = self.store.get_run(id) {
            if !run.profile.is_empty() {
                out["role"] = json!(run.profile);
            }
            if let Some(pick) = run.role_pick {
                out["rolePick"] = json!(pick);
            }
        }
        Ok(pb::GetRouteDecisionResponse {
            ok: true,
            json: body(&out)?,
        })
    }

    /// Proposes a tier table from the model catalog over the stored table and
    /// every model the installed agent CLIs report, and stores it when asked,
    /// validated like ReplaceTiers. With an agent it ranks only that agent's
    /// entries and models, and applying replaces only that agent's entries.
    pub(crate) async fn build_tiers(
        &self,
        m: pb::BuildTiersRequest,
    ) -> RpcResult<pb::BuildTiersResponse> {
        let table =
            Table::load_from_store(Some(self.store.as_ref())).map_err(RpcError::internal)?;
        let agent = m.agent.trim().to_owned();
        if !agent.is_empty() && !known_names().contains(&agent) {
            return Err(RpcError::invalid(format!("unknown agent {agent}")));
        }
        let (models, problems) = self.installed_entries().await;
        let mut entries: Vec<Entry> = ["easy", "normal", "hard"]
            .iter()
            .flat_map(|t| table.tiers.get(*t).cloned().unwrap_or_default())
            .collect();
        entries.extend(models.iter().cloned());
        if !agent.is_empty() {
            entries = reports::agent_pool(&agent, &table, &models);
        }
        let priority = self
            .global_policy()
            .ok()
            .map(|p| p.priority)
            .filter(|p| !p.is_empty())
            .unwrap_or_else(|| PRIORITY_BALANCED.into());
        let mut built = tiers_build::build_tiers(
            &entries,
            &runner::load_catalog(&self.state_dir().to_string_lossy()),
            &priority,
        );
        let mut out = json!({
            "priority": priority, "agent": agent, "tiers": built.tiers,
            "problems": if problems.is_empty() { Value::Null } else { json!(problems) },
            "applied": false,
        });
        if m.apply {
            if !agent.is_empty() {
                built.table = reports::merge_agent(&table, &built.table, &agent);
            }
            let validated = built.table.validated().map_err(invalid)?;
            self.store
                .replace_tiers(&validated.rows())
                .map_err(RpcError::internal)?;
            out["applied"] = json!(true);
        }
        Ok(pb::BuildTiersResponse {
            ok: true,
            json: body(&out)?,
        })
    }
}

/// Applies a set, allow-, or deny- action to `f` and names the key it writes.
fn set_config(
    f: &mut Policy,
    action: &str,
    key: &str,
    values: &[String],
) -> RpcResult<(String, Value)> {
    let one = match values {
        [v] => Some(v.as_str()),
        _ => None,
    };
    let boolean = one
        .filter(|v| *v == "true" || *v == "false")
        .map(|v| v == "true");
    match (action, key) {
        ("set", "priority") if one.is_some() => {
            f.priority = one.unwrap_or_default().into();
            Ok(("priority".into(), json!(f.priority)))
        }
        ("set", "discover") if boolean.is_some() => {
            f.discover = boolean.unwrap_or_default();
            Ok(("discover".into(), json!(f.discover)))
        }
        ("set", "roles") if boolean.is_some() => {
            f.roles = boolean.unwrap_or_default();
            Ok(("roles".into(), json!(f.roles)))
        }
        ("set", "classifier") if one.is_some() => {
            let mut name = one.unwrap_or_default().trim().to_owned();
            if name == "off" {
                name = CLASSIFIER_OFF.into();
            }
            f.classifier.clone_from(&name);
            Ok(("classifier".into(), json!(name)))
        }
        ("set", "classifier_model") if one.is_some() => {
            f.classifier_model = one.unwrap_or_default().trim().into();
            Ok(("classifier_model".into(), json!(f.classifier_model)))
        }
        _ if action.starts_with("allow-") || action.starts_with("deny-") => {
            let (side, op) = action.split_once('-').unwrap_or_default();
            let rules = if side == "deny" {
                &mut f.deny
            } else {
                &mut f.allow
            };
            let dst = match key {
                "providers" => &mut rules.providers,
                "models" => &mut rules.models,
                "agents" => &mut rules.agents,
                _ => {
                    return Err(RpcError::invalid(
                        "rule dimension must be providers, models, or agents",
                    ));
                }
            };
            for v in values {
                match op {
                    "add" => {
                        if !dst.contains(v) {
                            dst.push(v.clone());
                        }
                    }
                    "rm" => dst.retain(|x| x != v),
                    _ => return Err(RpcError::invalid(format!("unknown config action {action}"))),
                }
            }
            Ok((side.into(), json!(rules)))
        }
        _ => Err(RpcError::invalid("invalid config request")),
    }
}
