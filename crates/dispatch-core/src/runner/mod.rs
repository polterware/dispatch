//! The run lifecycle: routing a request, attempts and fallbacks, worktrees, recovery, merge, clean, summaries.

mod classify;
mod decision;
mod empirical;
mod execute;
mod handoff;
mod host;
mod lifecycle;
pub mod linked;
mod ops;
mod routing;
mod summary;
mod usage;
mod worktrees;

pub use classify::{run_difficulty, task_text, tier_warning};
pub use decision::route_kind;
pub use empirical::empirical_quota;
pub use execute::{
    agent_log_path, child_environ, complete_telemetry, exit_class, join_note, terminal_status,
};
pub use handoff::{
    HANDOFF_FILE, HANDOFF_INSTRUCTION, HANDOFF_MAX_CHARS, build_handoff, is_limit_class,
    prompt_with_handoff, write_worktree_handoff,
};
pub use host::{AgentRunHost, HOST_TIMEOUT, complete_linked_agent_run, dispatch_run_task};
pub use lifecycle::{pid_alive, process_start};
pub use ops::MergeResult;
pub use routing::{
    FiltersJson, RouteCandidate, RouteReport, cooling, credit_key, disabled_request_reason, linear,
    matches_model, normalize_priority, store_cooldowns, store_disabled, store_disabled_agents,
    validate_options,
};
pub use summary::{
    RunSummary, SUMMARY_SCHEMA, class_of, classify_run, go_duration, limit_notice, observed_ms,
    page, resets_in, summarize, summary,
};
pub use usage::{add_usage, attempt_usage_from_log, load_catalog, usage_cost, usage_with_log};
pub use worktrees::{WORKTREES_DIR, WorktreeFolder};

use crate::{
    Error,
    adapters::{
        Capabilities, Model, Request, RunResult,
        probe::ProbeReport,
        quota::{Quota, QuotaSample},
    },
    catalog::Catalog,
    config::Policy,
    difficulty::Difficulty,
    roles::{Profile, ProfileOptions},
    router,
    tiers::Table,
    types::{Area, Caller, Filters, RolePick, Store},
};
use chrono::{DateTime, SecondsFormat, Utc};
use futures::future::BoxFuture;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

pub const DEPTH_ENV: &str = "DISPATCH_DEPTH";
pub const NO_DELEGATE_ENV: &str = "DISPATCH_NO_DELEGATE";
pub const RUN_ID_ENV: &str = "DISPATCH_RUN_ID";
pub const PARENT_RUN_ID_ENV: &str = "DISPATCH_PARENT_RUN_ID";

pub const TIER_AUTO: &str = "auto";
pub const PRIORITY_SPEED: &str = "speed";
pub const PRIORITY_BALANCED: &str = "balanced";
pub const PRIORITY_QUALITY: &str = "quality";
pub const LOW_QUOTA_PERCENT: i32 = 90;

pub const ERR_RECURSIVE: &str = "refusing recursive delegation (DISPATCH_DEPTH set)";
/// An explicit Auto request with no classifier configured.
pub const ERR_AUTO_OFF: &str =
    "auto needs a classifier: set one with `config --classifier jev|openai|cli`";
/// What an adapter returns when it has no reliable usage source.
pub const ERR_NO_QUOTA: &str = "quota unavailable";

pub fn is_recursive(e: &Error) -> bool {
    matches!(e, Error::Message(m) if m == ERR_RECURSIVE)
}

pub fn is_auto_off(e: &Error) -> bool {
    matches!(e, Error::Message(m) if m == ERR_AUTO_OFF)
}

pub(crate) fn msg(s: impl Into<String>) -> Error {
    Error::Message(s.into())
}

const COOLDOWN_FOR: chrono::TimeDelta = chrono::TimeDelta::hours(1);
/// The longest a reset a limit notice names may hold an agent back.
const MAX_COOLDOWN_FOR: chrono::TimeDelta = chrono::TimeDelta::days(7);
/// A model outside the plan's allowance stays out until the user buys
/// credits or the allowance resets.
pub const CREDIT_COOLDOWN_FOR: chrono::TimeDelta = chrono::TimeDelta::hours(12);

/// Called with the agent's pid once its process started.
pub type Started = Arc<dyn Fn(i64) -> Result<(), Error> + Send + Sync>;

/// One agent invocation.
#[derive(Clone)]
pub struct AgentCall {
    pub request: Request,
    pub model: String,
    pub cancel: CancellationToken,
    pub started: Started,
}

/// Why an attempt failed. The deadline and the process cleanup are told apart
/// because they end the run instead of falling back.
#[derive(Clone, Debug, PartialEq)]
pub enum AttemptError {
    DeadlineExceeded,
    ProcessesAlive,
    Failed(String),
}

impl std::fmt::Display for AttemptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AttemptError::DeadlineExceeded => f.write_str("context deadline exceeded"),
            AttemptError::ProcessesAlive => f.write_str("agent process cleanup incomplete"),
            AttemptError::Failed(m) => f.write_str(m),
        }
    }
}

/// An attempt's result, with what the agent reported even when it failed.
#[derive(Clone, Debug, Default)]
pub struct AttemptResult {
    pub result: RunResult,
    pub error: Option<AttemptError>,
}

/// The agent CLI entry points the runner drives. The adapters implement it.
pub trait AgentAdapter: Send + Sync {
    fn name(&self) -> String;
    fn binary(&self) -> String;
    fn capabilities(&self) -> Capabilities;
    fn models(&self) -> BoxFuture<'_, Result<Vec<Model>, Error>>;
    /// Current usage; fails with [`ERR_NO_QUOTA`] when unavailable.
    fn quota(&self) -> BoxFuture<'_, Result<Quota, Error>>;
    /// Some when the adapter resolves quota per model.
    fn model_quotas(
        &self,
        _models: Vec<String>,
    ) -> Option<BoxFuture<'_, Result<HashMap<String, Quota>, Error>>> {
        None
    }
    /// The official quota source, None for an adapter without one, which
    /// must not invent a used percentage.
    fn usage_source(&self) -> Option<&'static str> {
        None
    }
    /// Every official quota window; fails with [`ERR_NO_QUOTA`] when unavailable.
    fn usage(&self) -> BoxFuture<'_, Result<Vec<QuotaSample>, Error>> {
        Box::pin(async { Err(Error::Message(ERR_NO_QUOTA.into())) })
    }
    /// Some when the adapter can probe its CLI's flags.
    fn probe(&self) -> Option<BoxFuture<'_, ProbeReport>> {
        None
    }
    fn run(&self, call: AgentCall) -> BoxFuture<'_, AttemptResult>;
    fn resume(&self, call: AgentCall) -> BoxFuture<'_, AttemptResult> {
        self.run(call)
    }
}

/// What a classifier judges: the request, after the card it names. Roles,
/// when set, also asks which role fits: each role's name and when it fits.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Task {
    pub text: String,
    pub roles: Option<BTreeMap<String, String>>,
}

/// A classifier's pick among a task's roles: probs per role when it gives
/// them, else only the role.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RoleAnswer {
    pub role: String,
    pub probs: BTreeMap<String, f64>,
    pub confidence: Option<f64>,
}

/// A classifier's answer: the difficulty, who gave it, and its role pick.
#[derive(Clone, Debug, PartialEq)]
pub struct Classification {
    pub difficulty: Difficulty,
    pub provider: String,
    pub model: String,
    pub confidence: Option<f64>,
    pub latency: Duration,
    pub role: Option<RoleAnswer>,
}

impl From<Difficulty> for Classification {
    fn from(difficulty: Difficulty) -> Self {
        Self {
            difficulty,
            provider: String::new(),
            model: String::new(),
            confidence: None,
            latency: Duration::ZERO,
            role: None,
        }
    }
}

/// Judges how hard a task is before routing. The classifier providers implement it.
pub trait Classifier: Send + Sync {
    fn name(&self) -> String;
    fn classify(&self, task: Task) -> BoxFuture<'_, Result<Classification, Error>>;
}

pub type NowFn = Arc<dyn Fn() -> DateTime<Utc> + Send + Sync>;
pub type InstalledFn = Arc<dyn Fn(&str) -> bool + Send + Sync>;
pub type EnvFn = Arc<dyn Fn(&str) -> String + Send + Sync>;

#[derive(Clone, Default)]
pub struct Deps {
    pub adapters: Vec<Arc<dyn AgentAdapter>>,
    pub table: Table,
    pub store: Option<Arc<dyn Store>>,
    pub now: Option<NowFn>,
    /// Reports whether a binary is available; defaults to a PATH lookup.
    pub installed: Option<InstalledFn>,
    /// Overrides where write-run worktrees are created. When empty they go in
    /// <repo>/.dispatch/worktrees, which keeps them under the repo path that
    /// agent permission rules usually already cover.
    pub worktree_dir: String,
    /// The directory for file-backed sidecars (the catalog prices and live logs).
    pub state_dir: String,
    /// The effective global/project routing policy.
    pub policy: Policy,
    /// Judges a task's difficulty for Auto; None leaves Auto off.
    pub classifier: Option<Arc<dyn Classifier>>,
    /// The roles for the run's directory: built-in, user, and project.
    pub profiles: BTreeMap<String, Profile>,
    /// Reads the environment; defaults to the process environment.
    pub getenv: Option<EnvFn>,
    /// Stands for the working directory; defaults to the process's.
    pub work_dir: String,
    /// Overrides the catalog read from the state directory.
    pub catalog: Option<Catalog>,
}

/// A request. For resumes, `resume` names the earlier run.
#[derive(Clone, Debug, Default)]
pub struct Options {
    /// The other repositories' folders the task spans, each cut a worktree of its own.
    pub with: Vec<String>,
    pub caller: Caller,
    pub project_id: String,
    pub agent_run_id: String,
    pub card_slug: String,
    pub filters: Filters,
    pub tier: String,
    pub prompt: String,
    pub dir: String,
    pub read_only: bool,
    pub timeout: Duration,
    pub no_worktree: bool,
    pub resume: String,
    /// Role: its name for the record, soft agent preferences, its own board,
    /// and its floor tier. `no_profile` (`--role none`) keeps the classifier
    /// from picking one.
    pub profile: String,
    pub prefer: Vec<String>,
    pub avoid: Vec<String>,
    pub role_board: Table,
    pub tier_floor: String,
    pub no_profile: bool,
    /// The part of the project the task touches, resolved by the caller.
    pub area: Area,
    /// The text of the card the run names, which the classifier judges before the prompt.
    pub card_text: String,
    /// An Auto run's classification, so Execute prices the candidates on the
    /// same difficulty Begin recorded.
    pub difficulty: Option<Classification>,
    /// How the classifier chose the run's role, None when it did not.
    pub pick: Option<RolePick>,
}

/// A routed candidate: the router's view plus the adapter, source tier and quota.
#[derive(Clone, Default)]
pub struct Candidate {
    pub adapter: Option<Arc<dyn AgentAdapter>>,
    pub route: router::Candidate,
    pub tier: String,
    pub quota: Option<Quota>,
}

impl std::fmt::Debug for Candidate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Candidate")
            .field("route", &self.route)
            .field("tier", &self.tier)
            .field("quota", &self.quota)
            .finish()
    }
}

impl Candidate {
    pub fn agent(&self) -> &str {
        &self.route.agent
    }
    pub fn model(&self) -> &str {
        &self.route.model
    }
}

/// Fills `o` from the role the request named. Values already set on `o`
/// win over the role. Its tier is a floor on the task's difficulty.
pub fn apply_profile(o: &mut Options, name: &str, p: &Profile) -> Result<(), Error> {
    with_profile_options(o, |po| crate::roles::apply_profile(po, name, p))
}

/// Fills `o` from a role the classifier picked, without changing access.
pub fn apply_picked(o: &mut Options, name: &str, p: &Profile) -> Result<(), Error> {
    with_profile_options(o, |po| crate::roles::apply_picked(po, name, p))
}

fn with_profile_options(
    o: &mut Options,
    f: impl FnOnce(&mut ProfileOptions) -> Result<(), Error>,
) -> Result<(), Error> {
    let mut po = ProfileOptions {
        tier: o.tier.clone(),
        filters: o.filters.clone(),
        tier_floor: o.tier_floor.clone(),
        read_only: o.read_only,
        timeout_ns: i64::try_from(o.timeout.as_nanos()).unwrap_or(i64::MAX),
        prompt: o.prompt.clone(),
        resume: o.resume.clone(),
        profile: o.profile.clone(),
        prefer: o.prefer.clone(),
        avoid: o.avoid.clone(),
        role_board: o.role_board.clone(),
    };
    f(&mut po)?;
    o.tier = po.tier;
    o.filters = po.filters;
    o.tier_floor = po.tier_floor;
    o.read_only = po.read_only;
    o.timeout = Duration::from_nanos(u64::try_from(po.timeout_ns).unwrap_or(0));
    o.prompt = po.prompt;
    o.profile = po.profile;
    o.prefer = po.prefer;
    o.avoid = po.avoid;
    o.role_board = po.role_board;
    Ok(())
}

impl Deps {
    pub fn now(&self) -> DateTime<Utc> {
        self.now.as_ref().map_or_else(Utc::now, |f| f())
    }

    pub(crate) fn env(&self, key: &str) -> String {
        match &self.getenv {
            Some(f) => f(key),
            None => std::env::var(key).unwrap_or_default(),
        }
    }

    pub(crate) fn cwd(&self) -> String {
        if !self.work_dir.is_empty() {
            return self.work_dir.clone();
        }
        std::env::current_dir()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    /// The absolute, cleaned form of `p`.
    pub(crate) fn abs(&self, p: &str) -> String {
        if p.starts_with('/') {
            clean_path(p)
        } else {
            clean_path(&format!("{}/{p}", self.cwd()))
        }
    }

    pub(crate) fn state_dir(&self) -> String {
        if !self.state_dir.is_empty() {
            return self.state_dir.clone();
        }
        self.env("DISPATCH_HOME")
    }

    pub(crate) fn catalog(&self) -> Catalog {
        match &self.catalog {
            Some(c) => c.clone(),
            None => load_catalog(&self.state_dir()),
        }
    }

    pub(crate) fn store(&self) -> Result<&dyn Store, Error> {
        self.store
            .as_deref()
            .ok_or_else(|| msg("no dispatch store"))
    }

    /// Whether `bin` is available to run.
    pub fn is_installed(&self, bin: &str) -> bool {
        match &self.installed {
            Some(f) => f(bin),
            None => look_path(bin, &self.env("PATH")),
        }
    }

    pub(crate) fn adapter(&self, name: &str) -> Option<Arc<dyn AgentAdapter>> {
        self.adapters.iter().find(|a| a.name() == name).cloned()
    }

    pub(crate) fn recursive(&self) -> bool {
        !self.env(DEPTH_ENV).is_empty() || !self.env(NO_DELEGATE_ENV).is_empty()
    }
}

fn look_path(bin: &str, path: &str) -> bool {
    use std::os::unix::fs::PermissionsExt;
    let executable = |p: &std::path::Path| {
        std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    };
    if bin.contains('/') {
        return executable(std::path::Path::new(bin));
    }
    path.split(':').any(|dir| {
        let dir = if dir.is_empty() { "." } else { dir };
        executable(&std::path::Path::new(dir).join(bin))
    })
}

pub(crate) fn fmt_time(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::AutoSi, true)
}

pub(crate) fn parse_time(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|t| t.with_timezone(&Utc))
}

pub(crate) fn non_neg_ms(d: chrono::TimeDelta) -> i64 {
    d.num_milliseconds().max(0)
}

/// Lexical path cleaning like Go's `filepath.Clean` on Unix.
pub(crate) fn clean_path(p: &str) -> String {
    let rooted = p.starts_with('/');
    let mut parts: Vec<&str> = Vec::new();
    for seg in p.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                if parts.last().is_some_and(|l| *l != "..") {
                    parts.pop();
                } else if !rooted {
                    parts.push("..");
                }
            }
            s => parts.push(s),
        }
    }
    let joined = parts.join("/");
    match (rooted, joined.is_empty()) {
        (true, _) => format!("/{joined}"),
        (false, true) => ".".into(),
        (false, false) => joined,
    }
}
