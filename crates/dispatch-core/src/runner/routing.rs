use super::{
    Candidate, Deps, LOW_QUOTA_PERCENT, Options, PRIORITY_BALANCED, TIER_AUTO, fmt_time, join_note,
    msg, parse_time,
};
use crate::{
    Error,
    adapters::{
        model::{base_model, effort_of, provider_of, valid_effort},
        quota::{Quota, SOURCE_AGY_USAGE, SOURCE_CLAUDE_STREAM, SOURCE_CODEX_APP_SERVER},
        valid_tier,
    },
    pass_model::difficulty_tier,
    router::{self, AutoInfo, Pace, Term},
    tiers::Table,
    types::{DisabledModel, Filters, QuotaSnapshot, RolePick, Store},
};
use chrono::{DateTime, Utc};
use serde::{Serialize, Serializer};
use std::collections::{HashMap, HashSet};
use std::time::Duration;

pub type Cooldowns = HashMap<String, DateTime<Utc>>;

/// Scopes a credit cooldown to one provider inside one CLI (e.g.
/// "cursor:openai"): Cursor stops serving third-party models while its own
/// and Grok's keep working, and every effort variant of the model is out.
pub fn credit_key(agent: &str, provider: &str) -> String {
    format!("{agent}:{provider}")
}

/// The cooldown key when the agent, or this provider inside it, is cooling down.
pub fn cooling(cool: &Cooldowns, agent: &str, provider: &str) -> Option<String> {
    if cool.contains_key(agent) {
        return Some(agent.to_owned());
    }
    if !provider.is_empty() && cool.contains_key(&credit_key(agent, provider)) {
        return Some(credit_key(agent, provider));
    }
    None
}

/// The provider, or the model when the provider is unknown.
pub(crate) fn credit_scope(agent: &str, provider: &str, model: &str) -> String {
    let provider = if provider.is_empty() {
        provider_of(agent, model)
    } else {
        provider.to_owned()
    };
    if provider.is_empty() {
        model.to_owned()
    } else {
        provider
    }
}

pub(crate) fn candidate_provider(c: &Candidate) -> String {
    credit_scope(&c.route.agent, &c.route.provider, &c.route.model)
}

pub fn store_cooldowns(st: Option<&dyn Store>, now: DateTime<Utc>) -> Cooldowns {
    let mut out = Cooldowns::new();
    let Some(st) = st else {
        return out;
    };
    let Ok(agents) = st.list_agents() else {
        return out;
    };
    for a in agents {
        if let Some(until) = a.cooldown_until.as_deref().and_then(parse_time)
            && !a.name.is_empty()
            && until > now
        {
            out.insert(a.name, until);
        }
    }
    out
}

pub fn store_disabled(st: Option<&dyn Store>, state_dir: &str) -> Vec<DisabledModel> {
    let Some(st) = st else {
        return Vec::new();
    };
    if !state_dir.is_empty() {
        let _ = crate::config::migrate_legacy_deny(
            &crate::config::policy_config_path(std::path::Path::new(state_dir)),
            st,
        );
    }
    st.list_disabled_models().unwrap_or_default()
}

pub fn validate_options(o: &Options) -> Result<(), Error> {
    if !o.tier.is_empty() && o.tier != TIER_AUTO && !valid_tier(&o.tier) {
        return Err(msg(format!(
            "invalid tier {:?} (want auto|easy|normal|hard)",
            o.tier
        )));
    }
    if o.tier == TIER_AUTO && (!o.filters.model.is_empty() || !o.filters.provider.is_empty()) {
        return Err(msg(
            "tier auto picks the model; drop --model and --provider",
        ));
    }
    if !o.filters.effort.is_empty() && !valid_effort(&o.filters.effort) {
        return Err(msg(format!(
            "invalid effort {:?} (want low|medium|high|xhigh|max)",
            o.filters.effort
        )));
    }
    let suffix = effort_of(&o.filters.model);
    if !o.filters.effort.is_empty() && !suffix.is_empty() && suffix != o.filters.effort {
        return Err(msg("model suffix conflicts with requested effort"));
    }
    if !o.filters.priority.is_empty() && !router::valid_priority(&o.filters.priority) {
        return Err(msg(format!(
            "invalid priority {:?} (want speed|balanced|quality)",
            o.filters.priority
        )));
    }
    Ok(())
}

pub fn normalize_priority(o: &mut Options) -> Result<(), Error> {
    if o.filters.priority.is_empty() {
        o.filters.priority = PRIORITY_BALANCED.into();
        return Ok(());
    }
    if !router::valid_priority(&o.filters.priority) {
        return Err(msg(format!(
            "invalid priority {:?} (want speed|balanced|quality)",
            o.filters.priority
        )));
    }
    Ok(())
}

pub fn matches_model(model: &str, requested: &str) -> bool {
    !requested.is_empty() && (model == requested || base_model(model) == base_model(requested))
}

/// Catches a requested filter that names a disabled provider or model.
/// In that case `--relax` must not silently substitute a different model.
pub fn disabled_request_reason(disabled: &[DisabledModel], f: &Filters) -> String {
    crate::config::disabled_request_reason(disabled, &f.provider, &f.agent, &f.model)
}

/// Pace for a window of the given length that resets at `resets_at`, with
/// `used_percent` consumed so far.
pub fn linear(
    used_percent: f64,
    window: Duration,
    resets_at: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> Pace {
    let window = window.as_secs_f64();
    let remaining = resets_at
        .map_or(0.0, |r| (r - now).as_seconds_f64())
        .max(0.0);
    let mut p = Pace {
        actual: used_percent,
        ..Pace::default()
    };
    if window <= 0.0 {
        p.will_last = true;
        return p;
    }
    let elapsed = (window - remaining).max(0.0);
    let expected = elapsed / window * 100.0;
    p.delta = used_percent - expected;
    p.deficit = p.delta > 0.0;
    if used_percent <= 0.0 || elapsed == 0.0 {
        p.will_last = true;
        return p;
    }
    let rate = used_percent / elapsed;
    let eta = if used_percent >= 100.0 {
        0.0
    } else {
        (100.0 - used_percent) / rate
    };
    p.will_last = eta >= remaining;
    p
}

fn evaluate_pace(c: &mut Candidate, now: DateTime<Utc>) {
    if let Some(q) = &c.quota {
        let window = Duration::from_secs(u64::try_from(q.window_minutes).unwrap_or(0) * 60);
        c.route.pace = Some(linear(q.used_percent, window, q.resets_at, now));
    }
}

/// A route that names only a tier: no model, agent, or provider filter. It
/// follows the table order instead of ranking by reserve.
pub(crate) fn tier_route(o: &Options) -> bool {
    !o.tier.is_empty()
        && o.tier != TIER_AUTO
        && o.filters.model.is_empty()
        && o.filters.agent.is_empty()
        && o.filters.provider.is_empty()
}

/// Reorders `cs` the way `f` ordered their router view.
pub(crate) fn reorder(
    cs: &[Candidate],
    f: impl Fn(&[router::Candidate]) -> Vec<router::Candidate>,
) -> Vec<Candidate> {
    let views: Vec<router::Candidate> = cs.iter().map(|c| c.route.clone()).collect();
    let mut used = vec![false; cs.len()];
    f(&views)
        .into_iter()
        .filter_map(|r| {
            let i = (0..cs.len()).find(|&i| {
                !used[i]
                    && cs[i].route.agent == r.agent
                    && cs[i].route.model == r.model
                    && cs[i].route.effort == r.effort
            })?;
            used[i] = true;
            let mut c = cs[i].clone();
            c.route = r;
            Some(c)
        })
        .collect()
}

pub fn rank(cs: &[Candidate]) -> Vec<Candidate> {
    reorder(cs, router::rank)
}

pub(crate) fn price_auto(
    cs: &[Candidate],
    d: f64,
    priority: &str,
    cat: &crate::catalog::Catalog,
) -> Vec<Candidate> {
    reorder(cs, |v| router::price_auto(v, d, priority, cat))
}

fn shift_priority(cs: &[Candidate], priority: &str) -> Vec<Candidate> {
    let mut out = cs.to_vec();
    for c in &mut out {
        c.route.priority = priority.into();
    }
    out
}

pub(crate) struct Planned {
    pub ordered: Vec<Candidate>,
    pub balanced: Vec<Candidate>,
    pub rejected: Vec<String>,
    pub relaxed: bool,
}

impl Deps {
    /// Resolves the tier table against installed adapters, attaches quota,
    /// pace, history and cooldowns, and ranks them.
    pub async fn candidates(
        &self,
        tier: &str,
        need_read_only: bool,
        prefer: &[String],
        avoid: &[String],
    ) -> (Vec<Candidate>, Vec<String>) {
        let o = Options {
            tier: tier.into(),
            read_only: need_read_only,
            prefer: prefer.to_vec(),
            avoid: avoid.to_vec(),
            ..Options::default()
        };
        let (cs, rejected) = self.select_candidates(&o).await;
        (self.evaluate(cs, true).await, rejected)
    }

    pub async fn evaluate(&self, cs: Vec<Candidate>, live: bool) -> Vec<Candidate> {
        rank(&self.observe(cs, live).await)
    }

    /// Applies hard constraints before quota/model ranking. A fixed model
    /// never competes with a different model. Priority never rejects one.
    pub async fn select_candidates(&self, o: &Options) -> (Vec<Candidate>, Vec<String>) {
        let cool = store_cooldowns(self.store.as_deref(), self.now());
        self.select_candidates_cool(o, &cool).await
    }

    pub(crate) async fn select_candidates_cool(
        &self,
        o: &Options,
        cool: &Cooldowns,
    ) -> (Vec<Candidate>, Vec<String>) {
        // Discovery only augments normal routing and only asks the model-list
        // endpoint; it never invokes a model.
        let discover = self.policy.discover && matches!(o.tier.as_str(), "" | TIER_AUTO | "normal");
        let mut agents = Vec::with_capacity(self.adapters.len());
        for a in &self.adapters {
            let caps = a.capabilities();
            let installed = self.is_installed(&a.binary());
            let discovered = if discover && installed {
                a.models()
                    .await
                    .map(|ms| ms.into_iter().map(|m| m.id).collect())
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
            agents.push(router::Agent {
                name: a.name(),
                installed,
                read_only: caps.read_only,
                file_editing: caps.file_editing,
                effort_mode: caps.effort_mode,
                efforts: caps.efforts,
                discovered,
                pace: None,
                cooled_down: false,
            });
        }
        let disabled = store_disabled(self.store.as_deref(), &self.state_dir);
        let ro = router::RouteOptions {
            tier: o.tier.clone(),
            difficulty: None,
            filters: o.filters.clone(),
            read_only: o.read_only,
            prefer: o.prefer.clone(),
            avoid: o.avoid.clone(),
            role_board: Table::default(),
            role_columns: Vec::new(),
            disabled,
        };
        let table = self.open_code_table(o, &mut agents, &ro.disabled).await;
        let (cs, mut rejected) = router::select_candidates(&table, &self.policy, &agents, &ro);
        let mut out = Vec::with_capacity(cs.len());
        for c in cs {
            let scope = credit_scope(&c.agent, &c.provider, &c.model);
            if let Some(key) = cooling(cool, &c.agent, &scope) {
                rejected.push(format!("{}/{}: {key} in cooldown", c.agent, c.model));
                continue;
            }
            let tier = self.tier_of(o, &c);
            out.push(Candidate {
                adapter: self.adapter(&c.agent),
                route: c,
                tier,
                quota: None,
            });
        }
        if out.is_empty() && rejected.is_empty() {
            rejected.push("no candidate matches tier/model intersection".into());
        }
        (out, rejected)
    }

    /// Widens a route restricted to OpenCode with the models it lists. Auto
    /// weighs all of them. A tier route keeps the tier's OpenCode entries when
    /// the table has some; otherwise it takes the models `build_tiers` would
    /// put in the tier, best first.
    async fn open_code_table(
        &self,
        o: &Options,
        agents: &mut [router::Agent],
        disabled: &[DisabledModel],
    ) -> Table {
        const OPEN_CODE: &str = crate::reports::OPEN_CODE_AGENT;
        let mut table = self.table.clone();
        if o.filters.agent != OPEN_CODE || !o.filters.model.is_empty() {
            return table;
        }
        let Some(a) = self
            .adapter(OPEN_CODE)
            .filter(|a| self.is_installed(&a.binary()))
        else {
            return table;
        };
        let tier = o.tier.as_str();
        if valid_tier(tier)
            && table
                .tiers
                .get(tier)
                .is_some_and(|es| es.iter().any(|e| e.agent == OPEN_CODE))
        {
            return table;
        }
        let Ok(models) = a.models().await else {
            return table;
        };
        if !valid_tier(tier) {
            // Auto, or no tier: every listed model is weighed, as if discovered.
            if let Some(agent) = agents.iter_mut().find(|a| a.name == OPEN_CODE) {
                agent.discovered = models
                    .into_iter()
                    .filter(|m| !crate::config::is_model_disabled(disabled, "", OPEN_CODE, &m.id))
                    .map(|m| m.id)
                    .collect();
            }
            return table;
        }
        let listed: Vec<crate::tiers::Entry> = models
            .into_iter()
            .map(|m| crate::tiers::Entry {
                agent: OPEN_CODE.into(),
                provider: provider_of(OPEN_CODE, &m.id),
                model: m.id,
                ..crate::tiers::Entry::default()
            })
            .collect();
        let pool: Vec<_> = crate::reports::agent_pool(OPEN_CODE, &table, &listed)
            .into_iter()
            .filter(|e| {
                !crate::config::is_model_disabled(disabled, &e.provider, &e.agent, &e.model)
            })
            .collect();
        let priority = if o.filters.priority.is_empty() {
            PRIORITY_BALANCED
        } else {
            o.filters.priority.as_str()
        };
        let built = crate::tiers_build::build_tiers(&pool, &self.catalog(), priority);
        if let Some(picks) = built.tiers.get(tier) {
            let rows = table.tiers.entry(tier.into()).or_default();
            rows.retain(|e| e.agent != OPEN_CODE);
            rows.extend(picks.iter().map(|b| b.entry.clone()));
        }
        table
    }

    /// The tier a candidate came from, recorded as the run's execution tier.
    fn tier_of(&self, o: &Options, c: &router::Candidate) -> String {
        match c.note.as_str() {
            "explicit model outside tier table" => return o.tier.clone(),
            "discovered" => return "normal".into(),
            _ => {}
        }
        for t in ["easy", "normal", "hard"] {
            if valid_tier(&o.tier) && o.tier != t {
                continue;
            }
            let found = self.table.tiers.get(t).is_some_and(|es| {
                es.iter().any(|e| {
                    e.agent == c.agent
                        && (e.model == c.model || base_model(&e.model) == base_model(&c.model))
                })
            });
            if found {
                return t.into();
            }
        }
        String::new()
    }

    /// Routes a run whose role has models in the run's band over those alone,
    /// in board order, as a tier route of that band. A band the role deleted
    /// moves to its nearest kept column, the next harder before the next easier.
    /// A kept but empty band, and a model or provider filter, keep the general
    /// route.
    fn with_role_board(&self, mut o: Options) -> (Deps, Options, String) {
        let mut d = self.clone();
        if o.role_board.tiers.is_empty()
            || !o.filters.model.is_empty()
            || !o.filters.provider.is_empty()
        {
            return (d, o, String::new());
        }
        let mut band = o.tier.clone();
        if band == TIER_AUTO {
            band = difficulty_tier(o.difficulty.as_ref().map_or(-1.8, |c| c.difficulty.d)).into();
        }
        if !valid_tier(&band) {
            band = "normal".into();
        }
        let (band, note) =
            crate::roles::nearest_column(&o.role_columns, &band).unwrap_or((band, String::new()));
        let Some(entries) = o.role_board.tiers.get(&band).filter(|e| !e.is_empty()) else {
            return (d, o, String::new());
        };
        d.table = Table::default();
        d.table.tiers.insert(band.clone(), entries.clone());
        d.policy.discover = false;
        o.tier = band;
        (d, o, note)
    }

    /// The one candidate list behind both Route (live=false) and Execute
    /// (live=true): filtered candidates first, then, with --relax and no
    /// policy objection, tier-routed ones that are not already listed. Each
    /// group is ranked on its own so a relaxed candidate never outranks a
    /// filtered one.
    pub(crate) async fn plan(&self, o: &Options, live: bool) -> Planned {
        let cool = store_cooldowns(self.store.as_deref(), self.now());
        self.plan_with_cooldowns(o, live, &cool).await
    }

    pub(crate) async fn plan_with_cooldowns(
        &self,
        o: &Options,
        live: bool,
        cool: &Cooldowns,
    ) -> Planned {
        let mut o = o.clone();
        if o.tier == TIER_AUTO && o.difficulty.is_none() {
            o.tier = "normal".into();
        }
        let (d, o, role_note) = self.with_role_board(o);
        let (primary, mut rejected) = d.select_candidates_cool(&o, cool).await;
        let mut extra = Vec::new();
        let disabled = store_disabled(d.store.as_deref(), &d.state_dir);
        if o.filters.relax && disabled_request_reason(&disabled, &o.filters).is_empty() {
            let mut fb = o.clone();
            fb.filters = Filters {
                priority: o.filters.priority.clone(),
                ..Filters::default()
            };
            if fb.tier.is_empty() {
                fb.tier = "normal".into();
            }
            let (found, why) = d.select_candidates_cool(&fb, cool).await;
            for mut c in found {
                let dup = primary.iter().any(|p| {
                    p.route.agent == c.route.agent
                        && p.route.model == c.route.model
                        && p.route.effort == c.route.effort
                });
                if !dup {
                    c.route.note =
                        join_note(&c.route.note, "--relax: filters relaxed to tier routing");
                    extra.push(c);
                }
            }
            if primary.is_empty() {
                rejected.extend(why);
            }
        }
        let n = primary.len();
        let relaxed = !extra.is_empty();
        let mut all = primary;
        all.extend(extra);
        if !role_note.is_empty() {
            for c in &mut all {
                c.route.note = join_note(&c.route.note, &role_note);
            }
        }
        let all = d.observe_with_cooldowns(all, live, cool).await;
        if o.tier == TIER_AUTO {
            let diff = o.difficulty.as_ref().map_or(-1.8, |c| c.difficulty.d);
            let cat = d.catalog();
            let prio = o.filters.priority.clone();
            return Planned {
                ordered: reorder(&all, |v| router::rank_auto(v, diff, &prio, &cat)),
                balanced: reorder(&all, |v| {
                    router::rank_auto(v, diff, PRIORITY_BALANCED, &cat)
                }),
                rejected,
                relaxed: false,
            };
        }
        let order = |cs: &[Candidate]| {
            if tier_route(&o) {
                reorder(cs, router::order_tier)
            } else {
                rank(cs)
            }
        };
        let mut ordered = order(&all[..n]);
        ordered.extend(order(&all[n..]));
        let sh = shift_priority(&all, PRIORITY_BALANCED);
        let mut balanced = order(&sh[..n]);
        balanced.extend(order(&sh[n..]));
        Planned {
            ordered,
            balanced,
            rejected,
            relaxed,
        }
    }

    pub async fn observe(&self, cs: Vec<Candidate>, live: bool) -> Vec<Candidate> {
        let cool = store_cooldowns(self.store.as_deref(), self.now());
        self.observe_with_cooldowns(cs, live, &cool).await
    }

    pub(crate) async fn observe_with_cooldowns(
        &self,
        mut cs: Vec<Candidate>,
        live: bool,
        cool: &Cooldowns,
    ) -> Vec<Candidate> {
        if cs.is_empty() {
            return cs;
        }
        if live {
            // One quota fetch per agent, in parallel; per-model adapters
            // resolve per model, others share one quota across their models.
            let mut by_agent: Vec<(String, Vec<usize>)> = Vec::new();
            for (i, c) in cs.iter().enumerate() {
                match by_agent.iter_mut().find(|(a, _)| *a == c.route.agent) {
                    Some((_, idxs)) => idxs.push(i),
                    None => by_agent.push((c.route.agent.clone(), vec![i])),
                }
            }
            let fetches = by_agent.iter().map(|(_, idxs)| {
                let a = cs[idxs[0]].adapter.clone();
                let models: Vec<String> = idxs.iter().map(|&i| cs[i].route.model.clone()).collect();
                async move {
                    let a = a?;
                    let limit = Duration::from_secs(100);
                    if let Some(f) = a.model_quotas(models) {
                        let qs = tokio::time::timeout(limit, f).await.ok()?.ok()?;
                        return Some((None, qs));
                    }
                    let mut q = tokio::time::timeout(limit, a.quota()).await.ok()?.ok()?;
                    if q.key.is_empty() {
                        q.key = a.name();
                    }
                    Some((Some(q), HashMap::new()))
                }
            });
            let results = futures::future::join_all(fetches).await;
            for ((_, idxs), res) in by_agent.iter().zip(results) {
                let Some((shared, per_model)) = res else {
                    continue;
                };
                for &i in idxs {
                    cs[i].quota = match &shared {
                        Some(q) => Some(q.clone()),
                        None => per_model.get(&cs[i].route.model).cloned(),
                    };
                }
            }
        }
        let now = self.now();
        let mut recorded = HashSet::new();
        let mut snaps: HashMap<String, Vec<QuotaSnapshot>> = HashMap::new();
        let store = self.store.as_deref();
        let mut snapshots = |key: &str| -> Vec<QuotaSnapshot> {
            snaps
                .entry(key.to_owned())
                .or_insert_with(|| match store {
                    Some(st) if !key.is_empty() => st
                        .list_quota(key, "")
                        .unwrap_or_default()
                        .into_iter()
                        .filter(|s| s.used_percent.is_some())
                        .collect(),
                    _ => Vec::new(),
                })
                .clone()
        };
        for c in &mut cs {
            c.route.cooled_down = cooling(cool, &c.route.agent, &candidate_provider(c)).is_some();
            if !live {
                let mut key = c.route.agent.clone();
                if key == "antigravity" {
                    key.push_str(":Claude and GPT models");
                    if c.route.model.to_lowercase().contains("gemini") {
                        key = "antigravity:Gemini Models".into();
                    }
                }
                for snap in snapshots(&key) {
                    let resets = parse_time(&snap.resets_at);
                    if resets.is_some_and(|r| r > now) {
                        c.quota = Some(Quota {
                            key: key.clone(),
                            used_percent: snap.used_percent.unwrap_or_default(),
                            resets_at: resets,
                            window_minutes: snap.window_minutes,
                            ..Quota::default()
                        });
                    }
                }
            }
            let Some(q) = c.quota.clone() else {
                continue;
            };
            evaluate_pace(c, now);
            if live
                && let Some(st) = store
                && recorded.insert(q.key.clone())
            {
                let source = match c.route.agent.as_str() {
                    "codex" => SOURCE_CODEX_APP_SERVER,
                    "antigravity" => SOURCE_AGY_USAGE,
                    "claude" => SOURCE_CLAUDE_STREAM,
                    _ => "",
                };
                let _ = st.save_quota(&QuotaSnapshot {
                    agent: q.key.clone(),
                    provider: c.route.provider.clone(),
                    window: String::new(),
                    source: source.into(),
                    confidence: "official".into(),
                    used_percent: Some(q.used_percent),
                    window_minutes: q.window_minutes,
                    resets_at: q.resets_at.map(fmt_time).unwrap_or_default(),
                    sampled_at: fmt_time(q.sampled_at.unwrap_or(now)),
                });
            }
        }
        cs
    }

    /// Reads cached quota and cooldown state. It never records a snapshot or
    /// run, reconciles processes, or creates a worktree; on Auto it asks the
    /// classifier.
    pub async fn route(&self, o: Options) -> Result<RouteReport, Error> {
        let mut o = o;
        normalize_priority(&mut o)?;
        let mut r = RouteReport {
            requested: (&o.filters).into(),
            priority: o.filters.priority.clone(),
            ..RouteReport::default()
        };
        r.note = self.resolve_auto(&mut o).await?;
        r.role.clone_from(&o.profile);
        r.role_pick.clone_from(&o.pick);
        validate_options(&o)?;
        if let Some(diff) = &o.difficulty {
            r.classifier.clone_from(&diff.provider);
            if let Some(p) = diff.difficulty.probs {
                r.probs = p.to_vec();
            }
        }
        let planned = self.plan(&o, false).await;
        r.eliminated = planned.rejected;
        if planned.relaxed {
            r.note = join_note(&r.note, "--relax: filters relaxed to tier routing");
        }
        for c in &planned.ordered {
            let mut rc = RouteCandidate {
                filters: (&route_filters(c)).into(),
                score: router::score(&c.route),
                reason: router::reason(&c.route),
                terms: router::terms(&c.route),
                auto: None,
            };
            if let Some(a) = &c.route.auto {
                rc.auto = Some(a.clone());
                rc.reason = router::auto_reason(&c.route).unwrap_or_default();
                rc.terms = Vec::new();
                r.difficulty = Some(a.difficulty);
            }
            r.candidates.push(rc);
            if !c.route.note.is_empty() {
                r.note = join_note(&r.note, &c.route.note);
            }
        }
        let Some(first) = r.candidates.first() else {
            r.reason = format!("no eligible candidate: {}", r.eliminated.join("; "));
            return Ok(r);
        };
        let first_reason = first.reason.clone();
        r.winner = Some(first.filters.clone());
        if let Some(b) = planned.balanced.first() {
            let mut bw = route_filters(b);
            bw.priority.clone_from(&o.filters.priority);
            r.balanced_winner = Some((&bw).into());
        }
        let ranked: Vec<router::Candidate> =
            planned.ordered.iter().map(|c| c.route.clone()).collect();
        let balanced: Vec<router::Candidate> =
            planned.balanced.iter().map(|c| c.route.clone()).collect();
        r.priority_note = router::priority_effect(&o.filters.priority, &ranked, &balanced);
        r.reason = format!(
            "lowest score wins; stable table order breaks ties; cached quota only: {first_reason}"
        );
        if tier_route(&o) {
            r.reason = format!(
                "the tier's table order wins, passing over an account with low quota ({LOW_QUOTA_PERCENT}% used or out before reset) or a rate limit; cached quota only: {first_reason}"
            );
        }
        if ranked[0].auto.is_some() {
            r.note = join_note(&r.note, &router::auto_note(&ranked));
            r.reason = format!(
                "auto: the cheapest candidate whose pass bound meets the threshold wins, else the cheapest within 0.02 of the best bound; cached quota only: {first_reason}"
            );
        }
        if !r.priority_note.is_empty() {
            r.reason.push_str("; ");
            r.reason.push_str(&r.priority_note);
        }
        Ok(r)
    }
}

pub(crate) fn route_filters(c: &Candidate) -> Filters {
    Filters {
        provider: c.route.provider.clone(),
        model: c.route.model.clone(),
        effort: c.route.effort.clone(),
        agent: c.route.agent.clone(),
        priority: if c.route.priority.is_empty() {
            PRIORITY_BALANCED.into()
        } else {
            c.route.priority.clone()
        },
        relax: false,
    }
}

/// Filters as Go encodes them: field names, no omissions.
#[derive(Clone, Debug, Default, PartialEq, Serialize, serde::Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct FiltersJson {
    pub provider: String,
    pub model: String,
    pub effort: String,
    pub agent: String,
    pub priority: String,
    pub relax: bool,
}

impl From<&Filters> for FiltersJson {
    fn from(f: &Filters) -> Self {
        Self {
            provider: f.provider.clone(),
            model: f.model.clone(),
            effort: f.effort.clone(),
            agent: f.agent.clone(),
            priority: f.priority.clone(),
            relax: f.relax,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct RouteCandidate {
    #[serde(flatten)]
    pub filters: FiltersJson,
    pub score: f64,
    pub reason: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub terms: Vec<Term>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto: Option<AutoInfo>,
}

fn null_if_empty<S: Serializer>(v: &[String], s: S) -> Result<S::Ok, S::Error> {
    if v.is_empty() {
        s.serialize_none()
    } else {
        v.serialize(s)
    }
}

/// Only structured routing decisions, never agent output.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RouteReport {
    pub requested: FiltersJson,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub priority: String,
    pub candidates: Vec<RouteCandidate>,
    #[serde(serialize_with = "null_if_empty")]
    pub eliminated: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub winner: Option<FiltersJson>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub balanced_winner: Option<FiltersJson>,
    pub reason: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub priority_note: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub note: String,
    /// d of the prompt on an Auto route, and the answer it came from.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub difficulty: Option<f64>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub classifier: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub probs: Vec<f64>,
    /// The role the route runs as, and how the classifier picked it.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role_pick: Option<RolePick>,
}
