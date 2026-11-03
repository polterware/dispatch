//! Domain types: runs, attempts, events, tiers, agents, quota, worktrees, and the Store trait.
//!
//! Ported from `dispatch/backend/domains/dispatch/types.go`.
//!
//! Times are RFC 3339 strings in UTC; the empty string is Go's zero time.

use crate::Error;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub use crate::pass_model::Area;
pub use crate::route_decision::{DecisionCandidate, RouteDecision};
pub use crate::router::Filters;

pub const STATUS_PENDING: &str = "PENDING";
pub const STATUS_RUNNING: &str = "RUNNING";
pub const STATUS_COMPLETED: &str = "COMPLETED";
pub const STATUS_FAILED: &str = "FAILED";
pub const STATUS_CANCELLED: &str = "CANCELLED";
pub const STATUS_TIMED_OUT: &str = "TIMED_OUT";
pub const STATUS_ORPHANED: &str = "ORPHANED";

pub const TIER_EASY: &str = "easy";
pub const TIER_NORMAL: &str = "normal";
pub const TIER_HARD: &str = "hard";

/// A live output chunk. It is not an Activity entry.
pub const EVENT_OUTPUT: &str = "output";
/// Started, merged, and failed are the only dispatch rows Activity shows.
pub const EVENT_STARTED: &str = "started";
pub const EVENT_MERGED: &str = "merged";
pub const EVENT_FAILED: &str = "failed";

pub const WORKTREE_ACTIVE: &str = "active";
pub const WORKTREE_READY: &str = "ready";
pub const WORKTREE_MERGED: &str = "merged";
pub const WORKTREE_REMOVED: &str = "removed";
pub const WORKTREE_ABANDONED: &str = "abandoned";

pub const ERR_UNKNOWN_RUN: &str = "unknown dispatch run";
pub const ERR_UNKNOWN_WORKTREE: &str = "unknown worktree";

#[derive(Debug, thiserror::Error)]
pub enum DispatchError {
    #[error("unknown dispatch run")]
    UnknownRun,
    #[error("unknown worktree")]
    UnknownWorktree,
}

impl From<DispatchError> for Error {
    fn from(e: DispatchError) -> Self {
        Error::Message(e.to_string())
    }
}

pub fn is_unknown_run(e: &Error) -> bool {
    matches!(e, Error::Message(m) if m == ERR_UNKNOWN_RUN)
}

pub fn is_unknown_worktree(e: &Error) -> bool {
    matches!(e, Error::Message(m) if m == ERR_UNKNOWN_WORKTREE)
}

/// A random 16-byte hex id.
pub fn new_id() -> String {
    let bytes: [u8; 16] = rand::random();
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// One agent try inside a run, including a failed fallback.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Attempt {
    #[serde(rename = "Agent")]
    pub agent: String,
    #[serde(rename = "Model")]
    pub model: String,
    #[serde(rename = "Error")]
    pub error: String,
    #[serde(rename = "ExitClass")]
    pub exit_class: String,
    #[serde(rename = "DurationMs")]
    pub duration_ms: Option<i64>,
}

/// Caller identifies the calling conversation, never the worker's resume session.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Caller {
    pub agent: String,
    pub session_id: String,
    pub conversation_id: String,
    pub label: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Section {
    pub id: String,
    pub name: String,
    pub position: i64,
    pub created_at: String,
    pub items: Vec<SectionItem>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SectionItem {
    pub kind: String,
    pub r#ref: String,
    pub position: i64,
}

/// What a run's agents reported spending, summed over its attempts.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Usage {
    pub input_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub output_tokens: i64,
    pub reasoning_tokens: i64,
    #[serde(rename = "costUsd", skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub cost_source: String,
    #[serde(skip_serializing_if = "is_zero")]
    pub context_tokens: i64,
    #[serde(skip_serializing_if = "is_zero")]
    pub context_window: i64,
}

fn is_zero(v: &i64) -> bool {
    *v == 0
}

/// How the classifier chose a run's role, with its probability per role.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RolePick {
    pub auto: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub classifier: String,
    pub role: String,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub probs: BTreeMap<String, f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
}

/// One delegated execution, linked to an AgentRun by `agent_run_id`.
///
/// `requested_tier` is the caller's tier before classification, `tier` the
/// routing tier, and `execution_tier` the tier of the candidate that ran.
#[derive(Clone, Debug, Default)]
pub struct Run {
    pub conversation_id: String,
    pub caller_agent: String,
    pub id: String,
    pub project_id: String,
    pub status: String,
    pub tier: String,
    pub requested_tier: String,
    pub execution_tier: String,
    pub agent: String,
    pub model: String,
    pub provider: String,
    pub effort: String,
    pub prompt: String,
    pub dir: String,
    pub repo: String,
    pub worktree: String,
    pub branch: String,
    pub base_sha: String,
    pub read_only: bool,
    pub no_worktree: bool,
    pub agent_run_id: String,
    pub card_slug: String,
    /// Routing input resolved by the caller; not stored (the decision log keeps it).
    pub area: Area,
    pub output: String,
    pub error: String,
    pub note: String,
    pub exit_code: i64,
    pub exit_class: String,
    pub timeout_sec: i64,
    pub session_id: String,
    pub resumed_of: String,
    pub parent_run_id: String,
    pub pid: i64,
    pub agent_pid: i64,
    pub proc_start: String,
    pub profile: String,
    pub prefer: Vec<String>,
    pub avoid: Vec<String>,
    /// How the classifier picked the run's role; None when the request named it.
    pub role_pick: Option<RolePick>,
    pub requested: Option<Filters>,
    pub resolved: Option<Filters>,
    pub attempts: Vec<Attempt>,
    pub classifier: String,
    pub difficulty_probs: Vec<f64>,
    pub difficulty: Option<f64>,
    pub files_changed: Option<i64>,
    pub insertions: Option<i64>,
    pub deletions: Option<i64>,
    pub queued_ms: Option<i64>,
    pub run_ms: Option<i64>,
    pub usage: Option<Usage>,
    pub created_at: String,
    pub started_at: String,
    pub finished_at: String,
    pub merged_at: Option<String>,
    pub merge_sha: String,
    /// Read from the stored worktree, not persisted on the run.
    pub worktree_status: String,
}

/// An append-only fact about a run. `seq` is assigned by the store when zero.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RunEvent {
    pub id: String,
    pub run_id: String,
    pub project_id: String,
    pub kind: String,
    pub agent: String,
    pub tier: String,
    pub text: String,
    pub seq: i64,
    pub at: String,
}

/// One row of the declarative difficulty table.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Tier {
    pub name: String,
    pub provider: String,
    pub agent: String,
    pub model: String,
    pub speed: i64,
    pub strength: i64,
    pub position: i64,
}

/// The last probe of an installed CLI. `used_percent` is None when unknown.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Agent {
    pub name: String,
    pub bin: String,
    pub provider: String,
    pub used_percent: Option<f64>,
    pub cooldown_until: Option<String>,
}

/// One official usage sample.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct QuotaSnapshot {
    pub agent: String,
    pub provider: String,
    pub window: String,
    pub source: String,
    pub confidence: String,
    pub used_percent: Option<f64>,
    pub window_minutes: i64,
    pub resets_at: String,
    pub sampled_at: String,
}

/// The core-owned git worktree for one run.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WorktreeInfo {
    pub id: String,
    pub project_id: String,
    pub run_id: String,
    pub card_slug: String,
    pub path: String,
    pub branch: String,
    pub base_sha: String,
    pub status: String,
    pub owner: String,
}

/// The persistence boundary. The core implements it on SQLite; the engine
/// must not open the database itself.
pub trait Store: Send + Sync {
    fn save_run(&self, run: &Run) -> Result<(), Error>;
    /// Fails with "unknown dispatch run" when the id is not stored.
    fn get_run(&self, id: &str) -> Result<Run, Error>;
    /// Newest first. An empty `project_id` lists every workspace's runs, an
    /// empty `conversation_id` does not filter, and `limit` 0 means every row.
    fn list_runs(
        &self,
        project_id: &str,
        limit: i64,
        offset: i64,
        conversation_id: &str,
    ) -> Result<Vec<Run>, Error>;
    fn append_event(&self, event: &RunEvent) -> Result<(), Error>;
    fn list_events(&self, run_id: &str, after_seq: i64, limit: i64)
    -> Result<Vec<RunEvent>, Error>;
    fn list_history(
        &self,
        project_id: &str,
        agent: &str,
        tier: &str,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<RunEvent>, Error>;
    fn save_worktree(&self, info: &WorktreeInfo) -> Result<(), Error>;
    fn list_worktrees(&self, project_id: &str) -> Result<Vec<WorktreeInfo>, Error>;
    fn save_quota(&self, snap: &QuotaSnapshot) -> Result<(), Error>;
    /// An empty `since` does not filter.
    fn list_quota(&self, agent: &str, since: &str) -> Result<Vec<QuotaSnapshot>, Error>;
    fn list_tiers(&self) -> Result<Vec<Tier>, Error>;
    fn replace_tiers(&self, tiers: &[Tier]) -> Result<(), Error>;
    fn upsert_agent(&self, agent: &Agent) -> Result<(), Error>;
    fn list_agents(&self) -> Result<Vec<Agent>, Error>;
    fn set_cooldown(&self, agent: &str, until: &str) -> Result<(), Error>;
    fn clear_cooldown(&self, agent: &str) -> Result<(), Error>;
    /// Stores the routing record of a new run, once per run.
    fn save_route_decision(&self, decision: &RouteDecision) -> Result<(), Error>;
    /// None when the run has no decision.
    fn get_route_decision(&self, run_id: &str) -> Result<Option<RouteDecision>, Error>;
    /// Newest first; an empty `project_id` lists all.
    fn list_route_decisions(&self, project_id: &str) -> Result<Vec<RouteDecision>, Error>;
}
