//! Domain types: runs, attempts, events, tiers, agents, quota, worktrees, and the Store trait.
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

/// The caller agent a run started from the desktop app records; a delegating agent records its
/// own name (claude, codex, ...).
pub const CALLER_APP: &str = "dispatch";
/// Where a run was started: the app's conversations, or an agent's delegation.
pub const ORIGIN_APP: &str = "app";
pub const ORIGIN_DELEGATED: &str = "delegated";

/// A run's origin, read off its caller agent: the app records [`CALLER_APP`] for the
/// conversations it starts. A run from an older app, which sent no caller, has no way to say
/// where it came from and reads as delegated.
pub fn origin(caller_agent: &str) -> &'static str {
    if caller_agent.trim() == CALLER_APP {
        ORIGIN_APP
    } else {
        ORIGIN_DELEGATED
    }
}

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
    #[serde(rename = "Handoff", default, skip_serializing_if = "String::is_empty")]
    pub handoff: String,
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
    /// Some attempt could not be priced, so `cost_usd` covers only the ones that could.
    #[serde(rename = "costPartial", skip_serializing_if = "std::ops::Not::not")]
    pub cost_partial: bool,
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
    /// The other repositories the task spans, each with a worktree of its own on a branch
    /// named for the run: merged, cleaned, and reverted with the run's own as one.
    pub linked: Vec<LinkedRepo>,
}

impl LinkedRepo {
    /// The worktree's top folder, which `worktree` may sit below.
    pub fn worktree_top(&self) -> String {
        let root = crate::worktree::root(&self.worktree);
        if root.is_empty() {
            self.worktree.clone()
        } else {
            root
        }
    }
}

/// One more repository a run works in besides its own.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct LinkedRepo {
    /// The folder asked for, inside the repository.
    pub dir: String,
    /// The repository's top folder.
    pub repo: String,
    /// The run's worktree of it, and the folder there the agent works in.
    pub worktree: String,
    pub branch: String,
    pub base_sha: String,
    /// Set once merged.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub merge_sha: String,
}

/// One image a run made, copied into its folder under the data directory's artifacts so a
/// cleaned worktree keeps it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Artifact {
    pub id: String,
    pub run_id: String,
    pub project_id: String,
    pub conversation_id: String,
    pub caller_agent: String,
    pub agent: String,
    pub model: String,
    /// The run's repository: its checkout, else its folder.
    pub repo: String,
    /// The copy's path under `<data dir>/artifacts/<run id>/`.
    pub rel_path: String,
    /// png, jpeg, webp, or gif, read from the file's header.
    pub kind: String,
    pub bytes: i64,
    pub width: i32,
    pub height: i32,
    pub created_at: String,
    /// The making run's prompt, read with the row and never stored on it.
    pub prompt: String,
}

impl Artifact {
    pub fn origin(&self) -> &'static str {
        origin(&self.caller_agent)
    }
}

/// What a gallery lists: every field empty matches every artifact, and `limit` 0 is every row.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ArtifactFilter {
    /// [`ORIGIN_APP`], [`ORIGIN_DELEGATED`], or empty.
    pub origin: String,
    pub repo: String,
    pub agent: String,
    pub conversation_id: String,
    /// The images of one run only.
    pub run_id: String,
    /// Searched in the run's prompt, case-insensitively.
    pub query: String,
    pub limit: i64,
    pub offset: i64,
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

/// Where one event sits in a run's text: its seq, its kind, and how many bytes its text has.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EventSize {
    pub seq: i64,
    pub kind: String,
    pub bytes: u64,
}

fn is_continuation(byte: u8) -> bool {
    byte & 0b1100_0000 == 0b1000_0000
}

/// Cuts `wanted` bytes off the front of `window` to whole characters: the cut's start moves on
/// past the bytes that continue a character begun before it, and its end moves back onto the
/// start of a character the cut would split. `window` may hold one byte past `wanted`, which
/// tells whether the end splits one. Returns the text and how many bytes its start moved on.
pub fn cut_utf8(window: &[u8], wanted: usize) -> (String, usize) {
    let wanted = wanted.min(window.len());
    let mut start = 0;
    while start < wanted && is_continuation(window[start]) {
        start += 1;
    }
    let mut end = wanted;
    while end > start && end < window.len() && is_continuation(window[end]) {
        end -= 1;
    }
    (
        String::from_utf8_lossy(&window[start..end]).into_owned(),
        start,
    )
}

/// One row of the declarative difficulty table.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Tier {
    pub name: String,
    pub provider: String,
    pub agent: String,
    pub model: String,
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

/// A disabled provider (`model` is empty) or a disabled base model under `provider`, with no
/// `agent` (the legacy setting for every agent), or one agent's disabled model (its id the CLI
/// lists, without effort, in `model`, and `provider` empty).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisabledModel {
    pub provider: String,
    pub model: String,
    #[serde(default)]
    pub agent: String,
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
    /// Every run ever saved, newest first, as Spend and Stats count them: a deleted run stays
    /// here with what it used, but no prompt, output, or worktree. An empty `project_id` or
    /// `dir` does not filter; a `dir` keeps runs in that folder or below it.
    fn list_run_history(&self, project_id: &str, dir: &str) -> Result<Vec<Run>, Error> {
        let dir = dir.trim_end_matches('/');
        Ok(self
            .list_runs(project_id, 0, 0, "")?
            .into_iter()
            .filter(|r| {
                dir.is_empty()
                    || r.repo == dir
                    || r.dir == dir
                    || r.dir.starts_with(&format!("{dir}/"))
            })
            .collect())
    }
    fn append_event(&self, event: &RunEvent) -> Result<(), Error>;
    fn list_events(&self, run_id: &str, after_seq: i64, limit: i64)
    -> Result<Vec<RunEvent>, Error>;
    /// The run's events oldest first, by size alone, so a page of their text is planned
    /// without reading it.
    fn event_sizes(&self, run_id: &str) -> Result<Vec<EventSize>, Error> {
        Ok(self
            .list_events(run_id, 0, 0)?
            .into_iter()
            .map(|e| EventSize {
                seq: e.seq,
                kind: e.kind,
                bytes: e.text.len() as u64,
            })
            .collect())
    }
    /// Event `seq` with its text cut to the bytes `start..end`, both moved inward to whole
    /// characters, and where its text now starts. None when the run has no such event.
    fn event_slice(
        &self,
        run_id: &str,
        seq: i64,
        start: u64,
        end: u64,
    ) -> Result<Option<(RunEvent, u64)>, Error> {
        let Some(mut event) = self
            .list_events(run_id, seq - 1, 1)?
            .into_iter()
            .find(|e| e.seq == seq)
        else {
            return Ok(None);
        };
        let bytes = event.text.as_bytes();
        let from = usize::try_from(start)
            .unwrap_or(usize::MAX)
            .min(bytes.len());
        let to = usize::try_from(end)
            .unwrap_or(usize::MAX)
            .clamp(from, bytes.len());
        let window = &bytes[from..(to + 1).min(bytes.len())];
        let (text, moved) = cut_utf8(window, to - from);
        event.text = text;
        Ok(Some((event, (from + moved) as u64)))
    }
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
    /// Disabled providers (`model` is empty) and base models (legacy, no `agent`), and one
    /// agent's disabled models (`agent` set, `model` the id its CLI lists without effort).
    fn list_disabled_models(&self) -> Result<Vec<DisabledModel>, Error> {
        Ok(Vec::new())
    }
    /// Enables or disables a provider (`model` is empty) or a base model under `provider`.
    fn set_model_enabled(
        &self,
        _provider: &str,
        _model: &str,
        _enabled: bool,
    ) -> Result<(), Error> {
        Ok(())
    }
    /// Enables or disables one agent's model, as its CLI lists it.
    fn set_agent_model_enabled(
        &self,
        _agent: &str,
        _model: &str,
        _enabled: bool,
    ) -> Result<(), Error> {
        Ok(())
    }
    /// The agents turned off in Settings > Agents.
    fn list_disabled_agents(&self) -> Result<Vec<String>, Error> {
        Ok(Vec::new())
    }
    /// Enables or disables an agent.
    fn set_agent_enabled(&self, _agent: &str, _enabled: bool) -> Result<(), Error> {
        Ok(())
    }
    /// Indexes one image a run made, newest first for [`Store::list_artifacts`].
    fn save_artifact(&self, _artifact: &Artifact) -> Result<(), Error> {
        Ok(())
    }
    /// The artifacts matching `filter`, newest first, with how many match in all.
    fn list_artifacts(&self, _filter: &ArtifactFilter) -> Result<(Vec<Artifact>, i64), Error> {
        Ok((Vec::new(), 0))
    }
    /// Forgets an artifact and answers what it was, for its copy to go with it.
    fn delete_artifact(&self, _id: &str) -> Result<Option<Artifact>, Error> {
        Ok(None)
    }
}
