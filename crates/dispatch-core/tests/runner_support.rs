//! Shared fakes for the runner tests: an in-memory Store, a scripted agent
//! adapter and classifier, and the deps every runner test starts from.
#![allow(dead_code)]

use dispatch_core::{
    Error,
    adapters::{Capabilities, Model, RunResult, probe::ProbeReport, quota::Quota, usage::Usage},
    config::Policy,
    runner::{
        AgentAdapter, AgentCall, AttemptError, AttemptResult, Classification, Classifier, Deps,
        ERR_NO_QUOTA, Task,
    },
    tiers::{Entry, Table},
    types::{
        Agent, DisabledModel, ERR_UNKNOWN_RUN, QuotaSnapshot, RouteDecision, Run, RunEvent, Store,
        Tier, WorktreeInfo,
    },
};
use futures::future::BoxFuture;
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Default)]
struct Inner {
    runs: Vec<Run>,
    events: Vec<RunEvent>,
    worktrees: Vec<WorktreeInfo>,
    quota: Vec<QuotaSnapshot>,
    tiers: Vec<Tier>,
    agents: Vec<Agent>,
    decisions: Vec<RouteDecision>,
    disabled_models: Vec<DisabledModel>,
}

/// An in-memory Store that keeps insertion order and counts run writes.
#[derive(Default)]
pub struct FakeStore {
    inner: Mutex<Inner>,
    pub run_writes: AtomicUsize,
}

impl FakeStore {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap()
    }

    pub fn decisions(&self) -> Vec<RouteDecision> {
        self.lock().decisions.clone()
    }

    pub fn worktrees(&self) -> Vec<WorktreeInfo> {
        self.lock().worktrees.clone()
    }

    pub fn quota(&self) -> Vec<QuotaSnapshot> {
        self.lock().quota.clone()
    }

    pub fn events(&self) -> Vec<RunEvent> {
        self.lock().events.clone()
    }
}

impl Store for FakeStore {
    fn save_run(&self, run: &Run) -> Result<(), Error> {
        self.run_writes.fetch_add(1, Ordering::SeqCst);
        let mut g = self.lock();
        match g.runs.iter_mut().find(|r| r.id == run.id) {
            Some(r) => *r = run.clone(),
            None => g.runs.push(run.clone()),
        }
        Ok(())
    }

    fn get_run(&self, id: &str) -> Result<Run, Error> {
        self.lock()
            .runs
            .iter()
            .find(|r| r.id == id)
            .cloned()
            .ok_or_else(|| Error::Message(ERR_UNKNOWN_RUN.into()))
    }

    fn list_runs(
        &self,
        project_id: &str,
        limit: i64,
        offset: i64,
        conversation_id: &str,
    ) -> Result<Vec<Run>, Error> {
        let mut out: Vec<Run> = self
            .lock()
            .runs
            .iter()
            .rev()
            .filter(|r| project_id.is_empty() || r.project_id == project_id)
            .filter(|r| conversation_id.is_empty() || r.conversation_id == conversation_id)
            .cloned()
            .collect();
        out.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        let out: Vec<Run> = out.into_iter().skip(offset.max(0) as usize).collect();
        if limit > 0 {
            return Ok(out.into_iter().take(limit as usize).collect());
        }
        Ok(out)
    }

    fn append_event(&self, event: &RunEvent) -> Result<(), Error> {
        let mut g = self.lock();
        let mut e = event.clone();
        if e.seq == 0 {
            e.seq = g.events.len() as i64 + 1;
        }
        g.events.push(e);
        Ok(())
    }

    fn list_events(
        &self,
        run_id: &str,
        after_seq: i64,
        limit: i64,
    ) -> Result<Vec<RunEvent>, Error> {
        let out: Vec<RunEvent> = self
            .lock()
            .events
            .iter()
            .filter(|e| e.run_id == run_id && e.seq > after_seq)
            .cloned()
            .collect();
        if limit > 0 {
            return Ok(out.into_iter().take(limit as usize).collect());
        }
        Ok(out)
    }

    fn list_history(
        &self,
        project_id: &str,
        agent: &str,
        tier: &str,
        _limit: i64,
        _offset: i64,
    ) -> Result<Vec<RunEvent>, Error> {
        Ok(self
            .lock()
            .events
            .iter()
            .filter(|e| project_id.is_empty() || e.project_id == project_id)
            .filter(|e| agent.is_empty() || e.agent == agent)
            .filter(|e| tier.is_empty() || e.tier == tier)
            .cloned()
            .collect())
    }

    fn save_worktree(&self, info: &WorktreeInfo) -> Result<(), Error> {
        let mut g = self.lock();
        match g.worktrees.iter_mut().find(|w| w.id == info.id) {
            Some(w) => *w = info.clone(),
            None => g.worktrees.push(info.clone()),
        }
        Ok(())
    }

    fn list_worktrees(&self, project_id: &str) -> Result<Vec<WorktreeInfo>, Error> {
        Ok(self
            .lock()
            .worktrees
            .iter()
            .filter(|w| project_id.is_empty() || w.project_id == project_id)
            .cloned()
            .collect())
    }

    fn save_quota(&self, snap: &QuotaSnapshot) -> Result<(), Error> {
        self.lock().quota.push(snap.clone());
        Ok(())
    }

    fn list_quota(&self, agent: &str, since: &str) -> Result<Vec<QuotaSnapshot>, Error> {
        Ok(self
            .lock()
            .quota
            .iter()
            .filter(|q| q.agent == agent && (since.is_empty() || q.sampled_at.as_str() >= since))
            .cloned()
            .collect())
    }

    fn list_tiers(&self) -> Result<Vec<Tier>, Error> {
        Ok(self.lock().tiers.clone())
    }

    fn replace_tiers(&self, tiers: &[Tier]) -> Result<(), Error> {
        self.lock().tiers = tiers.to_vec();
        Ok(())
    }

    fn upsert_agent(&self, agent: &Agent) -> Result<(), Error> {
        let mut g = self.lock();
        match g.agents.iter_mut().find(|a| a.name == agent.name) {
            Some(a) => {
                let cooldown = a.cooldown_until.clone();
                *a = agent.clone();
                if a.cooldown_until.is_none() {
                    a.cooldown_until = cooldown;
                }
            }
            None => g.agents.push(agent.clone()),
        }
        Ok(())
    }

    fn list_agents(&self) -> Result<Vec<Agent>, Error> {
        Ok(self.lock().agents.clone())
    }

    fn set_cooldown(&self, agent: &str, until: &str) -> Result<(), Error> {
        let mut g = self.lock();
        match g.agents.iter_mut().find(|a| a.name == agent) {
            Some(a) => a.cooldown_until = Some(until.into()),
            None => g.agents.push(Agent {
                name: agent.into(),
                cooldown_until: Some(until.into()),
                ..Agent::default()
            }),
        }
        Ok(())
    }

    fn clear_cooldown(&self, agent: &str) -> Result<(), Error> {
        if let Some(a) = self.lock().agents.iter_mut().find(|a| a.name == agent) {
            a.cooldown_until = None;
        }
        Ok(())
    }

    fn save_route_decision(&self, decision: &RouteDecision) -> Result<(), Error> {
        self.lock().decisions.push(decision.clone());
        Ok(())
    }

    fn get_route_decision(&self, run_id: &str) -> Result<Option<RouteDecision>, Error> {
        Ok(self
            .lock()
            .decisions
            .iter()
            .find(|d| d.run_id == run_id)
            .cloned())
    }

    fn list_route_decisions(&self, project_id: &str) -> Result<Vec<RouteDecision>, Error> {
        Ok(self
            .lock()
            .decisions
            .iter()
            .rev()
            .filter(|d| project_id.is_empty() || d.project_id == project_id)
            .cloned()
            .collect())
    }

    fn list_disabled_models(&self) -> Result<Vec<DisabledModel>, Error> {
        Ok(self.lock().disabled_models.clone())
    }

    fn set_model_enabled(&self, provider: &str, model: &str, enabled: bool) -> Result<(), Error> {
        let provider = provider.trim().to_ascii_lowercase();
        let model = model.trim();
        let model = if model.is_empty() {
            String::new()
        } else {
            let base = dispatch_core::catalog::base_model(model);
            match base.split_once('/') {
                Some((head, rest))
                    if !rest.is_empty()
                        && (provider.is_empty() || head.eq_ignore_ascii_case(&provider)) =>
                {
                    rest.to_owned()
                }
                _ => base.to_owned(),
            }
        };
        let mut g = self.lock();
        if enabled {
            if model.is_empty() {
                g.disabled_models.retain(|d| {
                    !(d.agent.is_empty()
                        && d.provider.eq_ignore_ascii_case(&provider)
                        && d.model.is_empty())
                });
            } else {
                g.disabled_models.retain(|d| {
                    !(d.agent.is_empty()
                        && d.model.eq_ignore_ascii_case(&model)
                        && (d.provider.is_empty() || d.provider.eq_ignore_ascii_case(&provider)))
                });
            }
        } else if !g.disabled_models.iter().any(|d| {
            d.agent.is_empty()
                && d.provider.eq_ignore_ascii_case(&provider)
                && d.model.eq_ignore_ascii_case(&model)
        }) {
            g.disabled_models.push(DisabledModel {
                provider,
                model,
                agent: String::new(),
            });
        }
        Ok(())
    }

    fn set_agent_model_enabled(
        &self,
        agent: &str,
        model: &str,
        enabled: bool,
    ) -> Result<(), Error> {
        let agent = agent.trim().to_ascii_lowercase();
        let model = dispatch_core::config::agent_model_key(model);
        let mut g = self.lock();
        if enabled {
            g.disabled_models.retain(|d| {
                !(d.agent.eq_ignore_ascii_case(&agent) && d.model.eq_ignore_ascii_case(&model))
            });
        } else if !g
            .disabled_models
            .iter()
            .any(|d| d.agent.eq_ignore_ascii_case(&agent) && d.model.eq_ignore_ascii_case(&model))
        {
            g.disabled_models.push(DisabledModel {
                provider: String::new(),
                model,
                agent,
            });
        }
        Ok(())
    }
}

pub type Ran = Arc<Mutex<Vec<String>>>;

pub fn ran() -> Ran {
    Arc::new(Mutex::new(Vec::new()))
}

pub fn ran_list(r: &Ran) -> Vec<String> {
    r.lock().unwrap().clone()
}

pub fn clear(r: &Ran) {
    r.lock().unwrap().clear();
}

pub type EditFn = Arc<dyn Fn(&str) + Send + Sync>;

/// A scripted agent: it records `name/model[@session]` and answers `out`,
/// or fails with `err`.
#[derive(Clone)]
pub struct Fake {
    pub name: String,
    pub out: String,
    pub log: String,
    pub err: Option<AttemptError>,
    pub ran: Ran,
    pub caps: Arc<Mutex<Capabilities>>,
    pub quota: Option<Quota>,
    pub run_quota: Option<Quota>,
    pub live_quota: Arc<Mutex<Option<Quota>>>,
    pub model_quotas: Option<HashMap<String, Quota>>,
    pub quota_calls: Arc<AtomicUsize>,
    pub models: Vec<String>,
    pub probe: Option<ProbeReport>,
    pub env: Arc<Mutex<Vec<String>>>,
    pub requests: Arc<Mutex<Vec<dispatch_core::adapters::Request>>>,
    pub edit: Option<EditFn>,
    pub delay: Duration,
    pub usage: Option<Usage>,
    pub pid: Option<i64>,
    pub resume_out: Option<String>,
    pub resume_err: Option<AttemptError>,
}

impl Fake {
    pub fn new(name: &str, ran: &Ran) -> Self {
        Self {
            name: name.into(),
            out: String::new(),
            log: String::new(),
            err: None,
            ran: ran.clone(),
            caps: Arc::new(Mutex::new(Capabilities {
                session_resume: true,
                read_only: true,
                file_editing: true,
                ..Capabilities::default()
            })),
            quota: None,
            run_quota: None,
            live_quota: Arc::new(Mutex::new(None)),
            model_quotas: None,
            quota_calls: Arc::new(AtomicUsize::new(0)),
            models: Vec::new(),
            probe: None,
            env: Arc::new(Mutex::new(Vec::new())),
            requests: Arc::new(Mutex::new(Vec::new())),
            edit: None,
            delay: Duration::ZERO,
            usage: None,
            pid: None,
            resume_out: None,
            resume_err: None,
        }
    }

    pub fn out(mut self, out: &str) -> Self {
        self.out = out.into();
        self
    }

    pub fn log(mut self, log: &str) -> Self {
        self.log = log.into();
        self
    }

    pub fn fail(mut self, err: &str) -> Self {
        self.err = Some(AttemptError::Failed(err.into()));
        self
    }

    pub fn error(mut self, err: AttemptError) -> Self {
        self.err = Some(err);
        self
    }

    pub fn caps(self, caps: Capabilities) -> Self {
        *self.caps.lock().unwrap() = caps;
        self
    }

    pub fn quota(mut self, q: Quota) -> Self {
        *self.live_quota.lock().unwrap() = Some(q.clone());
        self.quota = Some(q);
        self
    }

    pub fn run_quota(mut self, q: Quota) -> Self {
        self.run_quota = Some(q);
        self
    }

    pub fn resume_out(mut self, out: &str) -> Self {
        self.resume_out = Some(out.into());
        self
    }

    pub fn resume_fail(mut self, err: &str) -> Self {
        self.resume_err = Some(AttemptError::Failed(err.into()));
        self
    }

    pub fn probe(mut self, r: ProbeReport) -> Self {
        self.probe = Some(r);
        self
    }

    pub fn edit(mut self, f: impl Fn(&str) + Send + Sync + 'static) -> Self {
        self.edit = Some(Arc::new(f));
        self
    }

    pub fn delay(mut self, d: Duration) -> Self {
        self.delay = d;
        self
    }

    pub fn usage(mut self, u: Usage) -> Self {
        self.usage = Some(u);
        self
    }

    pub fn arc(self) -> Arc<dyn AgentAdapter> {
        Arc::new(self)
    }
}

impl AgentAdapter for Fake {
    fn name(&self) -> String {
        self.name.clone()
    }

    fn binary(&self) -> String {
        self.name.clone()
    }

    fn capabilities(&self) -> Capabilities {
        self.caps.lock().unwrap().clone()
    }

    fn models(&self) -> BoxFuture<'_, Result<Vec<Model>, Error>> {
        let ms = self
            .models
            .iter()
            .map(|id| Model {
                id: id.clone(),
                tier: String::new(),
            })
            .collect();
        Box::pin(async move { Ok(ms) })
    }

    fn quota(&self) -> BoxFuture<'_, Result<Quota, Error>> {
        self.quota_calls.fetch_add(1, Ordering::SeqCst);
        let q = self
            .live_quota
            .lock()
            .unwrap()
            .clone()
            .or_else(|| self.quota.clone());
        Box::pin(async move { q.ok_or_else(|| Error::Message(ERR_NO_QUOTA.into())) })
    }

    fn model_quotas(
        &self,
        _models: Vec<String>,
    ) -> Option<BoxFuture<'_, Result<HashMap<String, Quota>, Error>>> {
        let qs = self.model_quotas.clone()?;
        Some(Box::pin(async move { Ok(qs) }))
    }

    fn probe(&self) -> Option<BoxFuture<'_, ProbeReport>> {
        let r = self.probe.clone()?;
        Some(Box::pin(async move { r }))
    }

    fn run(&self, call: AgentCall) -> BoxFuture<'_, AttemptResult> {
        Box::pin(async move {
            let mut tag = format!("{}/{}", self.name, call.model);
            if !call.request.resume_session.is_empty() {
                tag.push('@');
                tag.push_str(&call.request.resume_session);
            }
            self.ran.lock().unwrap().push(tag);
            *self.env.lock().unwrap() = call.request.env.clone();
            self.requests.lock().unwrap().push(call.request.clone());
            if let Some(pid) = self.pid {
                let _ = (call.started)(pid);
            }
            let is_resume = !call.request.resume_session.is_empty();
            if !is_resume && let Some(rq) = &self.run_quota {
                *self.live_quota.lock().unwrap() = Some(rq.clone());
            }
            if is_resume && (self.resume_out.is_some() || self.resume_err.is_some()) {
                return AttemptResult {
                    result: RunResult {
                        output: self.resume_out.clone().unwrap_or_default(),
                        exit_code: 0,
                        session_id: call.request.resume_session.clone(),
                        usage: self.usage.clone(),
                    },
                    error: self.resume_err.clone(),
                };
            }
            if let Some(edit) = &self.edit {
                edit(&call.request.dir);
            }
            if !self.log.is_empty() && !call.request.log_path.is_empty() {
                use std::io::Write;
                let path = std::path::Path::new(&call.request.log_path);
                if let Some(parent) = path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                if let Ok(mut f) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path)
                {
                    let _ = f.write_all(self.log.as_bytes());
                }
            }
            if !self.delay.is_zero() {
                tokio::select! {
                    () = tokio::time::sleep(self.delay) => {}
                    () = call.cancel.cancelled() => {
                        return AttemptResult {
                            result: RunResult {
                                output: self.out.clone(),
                                exit_code: -1,
                                session_id: format!("sess-{}", self.name),
                                usage: self.usage.clone(),
                            },
                            error: Some(AttemptError::Failed("context canceled".into())),
                        };
                    }
                }
            }
            AttemptResult {
                result: RunResult {
                    output: self.out.clone(),
                    exit_code: 0,
                    session_id: format!("sess-{}", self.name),
                    usage: self.usage.clone(),
                },
                error: self.err.clone(),
            }
        })
    }
}

/// A classifier with a fixed answer, or a fixed error.
pub struct FakeClassifier {
    pub name: String,
    pub answer: Result<Classification, String>,
    pub tasks: Arc<Mutex<Vec<Task>>>,
}

impl FakeClassifier {
    pub fn new(name: &str, answer: Result<Classification, String>) -> Arc<Self> {
        Arc::new(Self {
            name: name.into(),
            answer,
            tasks: Arc::new(Mutex::new(Vec::new())),
        })
    }

    pub fn tasks(&self) -> Vec<Task> {
        self.tasks.lock().unwrap().clone()
    }
}

impl Classifier for FakeClassifier {
    fn name(&self) -> String {
        self.name.clone()
    }

    fn classify(&self, task: Task) -> BoxFuture<'_, Result<Classification, Error>> {
        self.tasks.lock().unwrap().push(task);
        let a = self.answer.clone().map_err(Error::Message);
        Box::pin(async move { a })
    }
}

pub fn entry(agent: &str, model: &str) -> Entry {
    Entry {
        agent: agent.into(),
        model: model.into(),
        ..Entry::default()
    }
}

pub fn table(rows: &[(&str, Vec<Entry>)]) -> Table {
    let mut t = Table::default();
    for (tier, es) in rows {
        t.tiers.insert((*tier).into(), es.clone());
    }
    t
}

pub type EnvMap = Arc<Mutex<HashMap<String, String>>>;

/// The deps of a runner test, with the temp dirs they live in.
pub struct TestDeps {
    pub deps: Deps,
    pub store: Arc<FakeStore>,
    pub env: EnvMap,
    pub state_dir: String,
    pub work_dir: String,
    _dirs: Vec<tempfile::TempDir>,
}

impl TestDeps {
    pub fn set_env(&self, key: &str, value: &str) {
        self.env.lock().unwrap().insert(key.into(), value.into());
    }

    pub fn get(&self, id: &str) -> Run {
        self.store.get_run(id).unwrap()
    }
}

pub fn temp_dir() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

pub fn path_of(d: &tempfile::TempDir) -> String {
    d.path()
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .into_owned()
}

/// The Go zero Policy: no discovery and no role pick.
pub fn zero_policy() -> Policy {
    Policy {
        discover: false,
        roles: false,
        ..Policy::default()
    }
}

pub fn test_deps(adapters: Vec<Arc<dyn AgentAdapter>>) -> TestDeps {
    let work = temp_dir();
    let state = temp_dir();
    let wt = temp_dir();
    let store = FakeStore::new();
    let env: EnvMap = Arc::new(Mutex::new(HashMap::new()));
    let state_dir = path_of(&state);
    let work_dir = path_of(&work);
    let getenv = env.clone();
    let deps = Deps {
        adapters,
        table: table(&[(
            "easy",
            vec![entry("a", "m1"), entry("b", "m2"), entry("ghost", "x")],
        )]),
        store: Some(store.clone()),
        installed: Some(Arc::new(|b: &str| b != "ghost")),
        worktree_dir: format!("{}/wt", path_of(&wt)),
        state_dir: state_dir.clone(),
        policy: zero_policy(),
        getenv: Some(Arc::new(move |k: &str| {
            getenv.lock().unwrap().get(k).cloned().unwrap_or_default()
        })),
        work_dir: work_dir.clone(),
        quota_poll: Duration::from_millis(10),
        ..Deps::default()
    };
    TestDeps {
        deps,
        store,
        env,
        state_dir,
        work_dir,
        _dirs: vec![work, state, wt],
    }
}

/// Runs git in `root` with a test identity.
pub fn git(root: &str, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .args([
            "-C",
            root,
            "-c",
            "user.name=test",
            "-c",
            "user.email=test@localhost",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// A repository with one commit holding `files`.
pub fn repo(files: &[(&str, &str)]) -> (tempfile::TempDir, String) {
    let dir = temp_dir();
    let root = path_of(&dir);
    git(&root, &["init", "-q", "-b", "main"]);
    for (path, body) in files {
        let full = std::path::Path::new(&root).join(path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(full, body).unwrap();
    }
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-qm", "base"]);
    (dir, root)
}

pub fn rfc(t: chrono::DateTime<chrono::Utc>) -> String {
    t.to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true)
}
