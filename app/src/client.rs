//! Typed Connect-JSON client for Dispatch core.
//!
//! Discovers `<data_dir>/run/core.json`, refuses non-loopback endpoints, and
//! never starts the core. Unary calls use `application/json`. [`CoreClient::watch_run`]
//! is a Connect server stream (`application/connect+json` envelopes).

use std::{
    env, fs,
    io::Read,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    time::Duration,
};

use serde::{Deserialize, Deserializer, Serialize, Serializer, de, de::DeserializeOwned};

const MAX_CORE_JSON_BYTES: u64 = 16_384;
const CLIENT_HEADER_VALUE: &str = "dispatch-desktop";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CoreError {
    NotRunning,
    Rpc(String),
}

#[derive(Clone, Debug)]
pub struct CoreConfig {
    pub endpoint: String,
    pub token: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CoreStateFile {
    endpoint: String,
    token: String,
    db_path: String,
    #[serde(default)]
    pid: Option<i32>,
}

/// ProtoJSON int64: a decimal string on the wire, or a JSON number.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProtoI64(pub i64);

impl ProtoI64 {
    pub fn is_zero(&self) -> bool {
        self.0 == 0
    }
}

impl Serialize for ProtoI64 {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0.to_string())
    }
}

impl<'de> Deserialize<'de> for ProtoI64 {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ProtoI64Visitor;

        impl<'de> de::Visitor<'de> for ProtoI64Visitor {
            type Value = ProtoI64;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("an int64 as a string or a number")
            }

            fn visit_i64<E: de::Error>(self, value: i64) -> Result<ProtoI64, E> {
                Ok(ProtoI64(value))
            }

            fn visit_u64<E: de::Error>(self, value: u64) -> Result<ProtoI64, E> {
                i64::try_from(value)
                    .map(ProtoI64)
                    .map_err(de::Error::custom)
            }

            fn visit_f64<E: de::Error>(self, value: f64) -> Result<ProtoI64, E> {
                if value.is_finite() {
                    Ok(ProtoI64(value as i64))
                } else {
                    Err(de::Error::custom("non-finite int64"))
                }
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<ProtoI64, E> {
                if value.is_empty() {
                    return Ok(ProtoI64(0));
                }
                value
                    .parse::<i64>()
                    .map(ProtoI64)
                    .map_err(de::Error::custom)
            }
        }

        deserializer.deserialize_any(ProtoI64Visitor)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DispatchRun {
    pub id: String,
    pub project_id: String,
    pub status: String,
    pub tier: String,
    pub agent: String,
    pub model: String,
    pub provider: String,
    pub effort: String,
    pub prompt: String,
    pub card_slug: String,
    pub agent_run_id: String,
    pub dir: String,
    pub repo: String,
    pub worktree: String,
    pub branch: String,
    pub base_sha: String,
    pub read_only: bool,
    pub exit_code: i32,
    pub exit_class: String,
    pub error: String,
    pub note: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub files_changed: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub insertions: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deletions: Option<i32>,
    pub created_at: String,
    pub started_at: String,
    pub finished_at: String,
    pub merged_at: String,
    pub merge_sha: String,
    pub attempts: Vec<DispatchAttempt>,
    pub resumed_of: String,
    pub parent_run_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub queued_ms: Option<ProtoI64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_ms: Option<ProtoI64>,
    pub priority: String,
    pub usage: DispatchUsage,
    pub worktree_status: String,
    pub conversation_id: String,
    pub caller_agent: String,
    pub resumable: bool,
    /// A writable run that edits its folder itself, with no worktree of its own.
    pub in_place: bool,
    /// The other repositories the run spans, each with a worktree of its own.
    pub linked: Vec<DispatchLinkedRepo>,
}

/// One more repository a run works in, with its worktree of it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DispatchLinkedRepo {
    pub dir: String,
    pub repo: String,
    pub worktree: String,
    pub branch: String,
    pub base_sha: String,
    pub merge_sha: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DispatchUsage {
    pub input_tokens: ProtoI64,
    pub cache_read_tokens: ProtoI64,
    pub cache_write_tokens: ProtoI64,
    pub output_tokens: ProtoI64,
    pub reasoning_tokens: ProtoI64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    pub cost_source: String,
    pub context_tokens: ProtoI64,
    pub context_window: ProtoI64,
    pub cost_partial: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DispatchAttempt {
    pub agent: String,
    pub model: String,
    pub error: String,
    pub exit_class: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<ProtoI64>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub handoff: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DispatchRunEvent {
    pub id: String,
    pub run_id: String,
    pub project_id: String,
    pub kind: String,
    pub agent: String,
    pub tier: String,
    pub text: String,
    pub seq: ProtoI64,
    pub at: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DispatchWorktree {
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

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DispatchQuotaSnapshot {
    pub agent: String,
    pub provider: String,
    pub window: String,
    pub source: String,
    pub confidence: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub used_percent: Option<f64>,
    pub window_minutes: i32,
    pub resets_at: String,
    pub sampled_at: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DispatchTier {
    pub name: String,
    pub provider: String,
    pub agent: String,
    pub model: String,
    pub position: i32,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DispatchAgent {
    pub name: String,
    pub bin: String,
    pub provider: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub used_percent: Option<f64>,
    pub cooldown_until: String,
    pub installed: bool,
    pub enabled: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ListRunsRequest {
    pub project_id: String,
    pub limit: i32,
    pub offset: i32,
    pub conversation_id: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ListRunsResponse {
    pub runs: Vec<DispatchRun>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GetRunRequest {
    pub id: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GetRunResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run: Option<DispatchRun>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct StartRunRequest {
    pub project_id: String,
    pub tier: String,
    pub prompt: String,
    pub card_slug: String,
    pub agent: String,
    pub worktree: String,
    pub dir: String,
    pub read_only: bool,
    pub provider: String,
    pub model: String,
    pub effort: String,
    pub priority: String,
    pub relax: bool,
    pub timeout_sec: i32,
    pub profile: String,
    pub repo: String,
    pub caller: DispatchCaller,
    /// An ended run to send `prompt` to as a follow-up.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub resume: String,
    /// `worktree` (a git worktree of its own) or `in_place` (the folder itself); empty
    /// leaves it to the core (a worktree in a git checkout with commits).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub worktree_mode: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DispatchCaller {
    pub agent: String,
    pub session_id: String,
    pub conversation_id: String,
    pub label: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DispatchSectionItem {
    pub kind: String,
    #[serde(rename = "ref")]
    pub item_ref: String,
    pub position: i32,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DispatchSection {
    pub id: String,
    pub name: String,
    pub position: i32,
    pub created_at: String,
    pub items: Vec<DispatchSectionItem>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ListSectionsRequest {}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ListSectionsResponse {
    pub sections: Vec<DispatchSection>,
    /// The names people gave conversations, over their first message.
    pub conversation_names: Vec<ConversationName>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ConversationName {
    pub conversation_id: String,
    pub name: String,
}

/// A folder under a repository's `.dispatch/worktrees`, as `dispatch worktrees` lists it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct WorktreeFolder {
    pub path: String,
    pub repo: String,
    pub branch: String,
    pub run_id: String,
    /// Empty when the run no longer exists.
    pub run_status: String,
    /// Uncommitted files, or commits the repository's HEAD does not have.
    pub changes: bool,
    /// Clean up would remove it.
    pub prunable: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ListWorktreeFoldersRequest {
    pub dir: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ListWorktreeFoldersResponse {
    pub folders: Vec<WorktreeFolder>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PruneWorktreeFoldersRequest {
    pub dir: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PruneWorktreeFoldersResponse {
    pub removed: Vec<WorktreeFolder>,
    pub kept: Vec<WorktreeFolder>,
}

/// Names a conversation; an empty name gives back its first message's title.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RenameConversationRequest {
    pub conversation_id: String,
    pub name: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RenameConversationResponse {}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SaveSectionRequest {
    pub id: String,
    pub name: String,
    pub position: i32,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SaveSectionResponse {
    pub section: DispatchSection,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DeleteSectionRequest {
    pub id: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DeleteSectionResponse {}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct MoveToSectionRequest {
    pub kind: String,
    #[serde(rename = "ref")]
    pub item_ref: String,
    pub section_id: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct MoveToSectionResponse {}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct StartRunResponse {
    pub run: DispatchRun,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CancelRunRequest {
    pub id: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CancelRunResponse {
    pub run: DispatchRun,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ResultRequest {
    pub id: String,
    pub section: String,
    pub offset: i32,
    pub limit: i32,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ResultResponse {
    pub content: String,
    pub total: i32,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct WatchRunRequest {
    pub id: String,
    pub after_seq: ProtoI64,
    /// With `after_seq` 0, open with only this many of the last bytes; 0 sends everything.
    /// Left off the wire at 0, so a core from before pages still takes the request.
    #[serde(skip_serializing_if = "ProtoI64::is_zero")]
    pub tail_bytes: ProtoI64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct WatchRunResponse {
    pub seq: ProtoI64,
    pub kind: String,
    pub text: String,
    pub at: String,
    /// Where `text` starts in the event's whole text: above 0 when a page edge cut it.
    pub text_offset: ProtoI64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ReadRunEventsRequest {
    pub id: String,
    pub before_seq: ProtoI64,
    pub before_offset: ProtoI64,
    pub max_bytes: ProtoI64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ReadRunEventsResponse {
    pub events: Vec<WatchRunResponse>,
    pub more: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ListWorktreesRequest {
    pub project_id: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ListWorktreesResponse {
    pub worktrees: Vec<DispatchWorktree>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ListQuotaRequest {
    pub agent: String,
    pub since: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ListQuotaResponse {
    pub snapshots: Vec<DispatchQuotaSnapshot>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ListTiersRequest {}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ListTiersResponse {
    pub tiers: Vec<DispatchTier>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ListAgentsRequest {}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ListAgentsResponse {
    pub agents: Vec<DispatchAgent>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ListHistoryRequest {
    pub project_id: String,
    pub agent: String,
    pub tier: String,
    pub limit: i32,
    pub offset: i32,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ListHistoryResponse {
    pub events: Vec<DispatchRunEvent>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DispatchModel {
    pub agent: String,
    pub model: String,
    pub provider: String,
    pub tier: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ListModelsRequest {}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ListModelsResponse {
    pub models: Vec<DispatchModel>,
    pub problems: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DispatchModelSetting {
    pub id: String,
    pub enabled: bool,
    pub agents: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DispatchProviderModels {
    pub provider: String,
    pub enabled: bool,
    pub models: Vec<DispatchModelSetting>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ListModelSettingsRequest {}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ListModelSettingsResponse {
    pub providers: Vec<DispatchProviderModels>,
    pub problems: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SetModelEnabledRequest {
    pub provider: String,
    pub model: String,
    pub enabled: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SetModelEnabledResponse {
    pub provider: String,
    pub model: String,
    pub enabled: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SetAgentEnabledRequest {
    pub agent: String,
    pub enabled: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SetAgentEnabledResponse {
    pub agent: String,
    pub enabled: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ReplaceTiersRequest {
    pub tiers: Vec<DispatchTier>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ReplaceTiersResponse {
    pub tiers: Vec<DispatchTier>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CheckTiersRequest {}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CheckTiersResponse {
    pub problems: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct StatsRequest {
    pub project_id: String,
    pub by: String,
    pub since: String,
    /// A repository's folder: only runs in it or below it.
    pub dir: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DispatchStatsRow {
    pub key: String,
    pub runs: i32,
    pub success_pct: f64,
    pub first_try_pct: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub p50_ms: Option<ProtoI64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub p90_ms: Option<ProtoI64>,
    pub fallbacks: i32,
    pub rate_limits: i32,
    pub orphans: i32,
    pub usage_runs: i32,
    pub input_tokens: ProtoI64,
    pub cache_read_tokens: ProtoI64,
    pub cache_write_tokens: ProtoI64,
    pub output_tokens: ProtoI64,
    pub reasoning_tokens: ProtoI64,
    pub cost_usd: f64,
    pub cost_runs: i32,
    pub live_runs: i32,
    pub partial_cost_runs: i32,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DispatchStatRank {
    pub key: String,
    pub runs: i32,
    pub samples: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub p50_ms: Option<ProtoI64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub success_pct: Option<f64>,
    pub reason: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DispatchStatsRankings {
    pub most_used_model: DispatchStatRank,
    pub most_used_provider: DispatchStatRank,
    pub slowest_model: DispatchStatRank,
    pub most_failed_model: DispatchStatRank,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct StatsResponse {
    pub by: String,
    pub since: String,
    pub in_flight: i32,
    pub rows: Vec<DispatchStatsRow>,
    pub rankings: DispatchStatsRankings,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GetRouteDecisionRequest {
    pub run_id: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GetRouteDecisionResponse {
    pub ok: bool,
    pub json: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct BuildTiersRequest {
    pub apply: bool,
    pub agent: String,
    /// Sent only when set, so a core from before roles had boards still reads the request.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub role: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub judge: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct BuildTiersResponse {
    pub ok: bool,
    pub json: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CapacityRequest {
    pub since: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DispatchCapacityKey {
    pub key: String,
    pub windows: i32,
    pub hit_full: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub consumed_pct: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub leftover_pct: Option<f64>,
    pub leftover: String,
    pub pace: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CapacityResponse {
    pub since: String,
    pub note: String,
    pub keys: Vec<DispatchCapacityKey>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RouteRequest {
    pub tier: String,
    pub provider: String,
    pub model: String,
    pub effort: String,
    pub agent: String,
    pub priority: String,
    pub relax: bool,
    pub read_only: bool,
    pub dir: String,
    pub profile: String,
    pub prompt: String,
    pub project_id: String,
    pub card_slug: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ClassifyRequest {
    pub prompt: String,
    pub dir: String,
    pub read_only: bool,
    pub project_id: String,
    pub card_slug: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct MergeRunRequest {
    pub id: String,
    pub keep_worktree: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CleanRunRequest {
    pub id: String,
    pub force: bool,
}

/// Deletes the runs given and every run of `conversation_id`, whoever started them.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DeleteRunsRequest {
    pub ids: Vec<String>,
    pub conversation_id: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DeleteRunsResponse {
    pub deleted: i32,
    /// `{"ids": [...], "worktrees": [...]}`: the runs deleted and the worktrees discarded.
    pub json: String,
}

impl DeleteRunsResponse {
    /// The ids of the runs the core deleted.
    pub fn ids(&self) -> Vec<String> {
        serde_json::from_str::<serde_json::Value>(&self.json)
            .ok()
            .and_then(|v| {
                v["ids"].as_array().map(|ids| {
                    ids.iter()
                        .filter_map(|id| id.as_str().map(str::to_owned))
                        .collect()
                })
            })
            .unwrap_or_default()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RecoverRunRequest {
    pub id: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ConfigRequest {
    pub action: String,
    pub key: String,
    pub values: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SubagentsRequest {
    pub action: String,
    pub dir: String,
    pub name: String,
    pub profile: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SkillRequest {
    pub action: String,
    pub agent: String,
    pub force: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SkillResponse {
    pub ok: bool,
    pub json: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GetAgentSetupRequest {}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ApplyAgentSetupRequest {
    pub action: String,
    pub agent: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GetAgentSetupResponse {
    pub json: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ApplyAgentSetupResponse {
    pub json: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RouteResponse {
    pub ok: bool,
    pub json: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ListModelCatalogRequest {}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ListModelCatalogResponse {
    pub ok: bool,
    pub json: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ListModelBenchmarksRequest {}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ListModelBenchmarksResponse {
    pub ok: bool,
    pub json: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RefreshModelCatalogRequest {}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RefreshModelCatalogResponse {
    pub ok: bool,
    pub json: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ClassifyResponse {
    pub ok: bool,
    pub json: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct MergeRunResponse {
    pub ok: bool,
    pub json: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CleanRunResponse {
    pub ok: bool,
    pub json: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RecoverRunResponse {
    pub ok: bool,
    pub json: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ConfigResponse {
    pub ok: bool,
    pub json: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SubagentsResponse {
    pub ok: bool,
    pub json: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Workspace {
    pub id: String,
    pub name: String,
    pub path: String,
    pub polterware_project_id: String,
    pub created_at: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ListWorkspacesRequest {}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ListWorkspacesResponse {
    pub workspaces: Vec<Workspace>,
    pub linked: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AddWorkspaceRequest {
    pub path: String,
    pub name: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AddWorkspaceResponse {
    pub workspace: Workspace,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RemoveWorkspaceRequest {
    pub id: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RemoveWorkspaceResponse {
    pub deleted_runs: i32,
    pub removed_worktrees: i32,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ResolveWorkspaceRequest {
    pub id: String,
    pub dir: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ResolveWorkspaceResponse {
    pub workspace: Workspace,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct WorkspaceCard {
    pub slug: String,
    pub title: String,
    pub milestone: String,
    pub stage: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct WorkspaceRepository {
    pub name: String,
    pub remote: String,
    pub local_path: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ListWorkspaceCardsRequest {
    pub workspace_id: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ListWorkspaceCardsResponse {
    pub cards: Vec<WorkspaceCard>,
    pub repositories: Vec<WorkspaceRepository>,
}

/// Resolves `$PWDISPATCH_DATA_DIR` if non-empty, else `~/Library/Application Support/Polterware Dispatch`.
pub fn resolve_data_dir() -> PathBuf {
    if let Ok(override_dir) = env::var("PWDISPATCH_DATA_DIR") {
        let trimmed = override_dir.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    let home = env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home.trim()).join("Library/Application Support/Polterware Dispatch")
}

pub fn core_json_path(data_dir: &Path) -> PathBuf {
    data_dir.join("run").join("core.json")
}

/// Validates that `endpoint` is strictly an IPv4 loopback HTTP origin (`http://127.0.0.1:<port>`).
pub fn validate_loopback_endpoint(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    let without_slash = trimmed.strip_suffix('/').unwrap_or(trimmed);
    let port_str = without_slash.strip_prefix("http://127.0.0.1:")?;
    if port_str.is_empty() || !port_str.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let port: u16 = port_str.parse().ok()?;
    if port == 0 {
        return None;
    }
    Some(format!("http://127.0.0.1:{port}"))
}

fn paths_match(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    match (fs::canonicalize(left), fs::canonicalize(right)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

fn pid_is_alive(pid: i32) -> bool {
    if pid <= 1 {
        return false;
    }
    let rc = unsafe { libc::kill(pid, 0) };
    if rc == 0 {
        return true;
    }
    std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// Reads and validates `<data_dir>/run/core.json` without ever starting the core.
pub fn read_core_config(data_dir: &Path) -> Result<CoreConfig, CoreError> {
    read_core_config_for_uid(data_dir, unsafe { libc::geteuid() })
}

fn read_core_config_for_uid(data_dir: &Path, expected_uid: u32) -> Result<CoreConfig, CoreError> {
    let path = core_json_path(data_dir);
    let meta = fs::symlink_metadata(&path).map_err(|_| CoreError::NotRunning)?;
    if !meta.file_type().is_file() || meta.len() == 0 || meta.len() > MAX_CORE_JSON_BYTES {
        return Err(CoreError::NotRunning);
    }
    if (meta.mode() & 0o077) != 0 || meta.uid() != expected_uid {
        return Err(CoreError::NotRunning);
    }

    let raw = fs::read_to_string(&path).map_err(|_| CoreError::NotRunning)?;
    let parsed: CoreStateFile = serde_json::from_str(&raw).map_err(|_| CoreError::NotRunning)?;

    let endpoint = validate_loopback_endpoint(&parsed.endpoint).ok_or(CoreError::NotRunning)?;
    let token = parsed.token.trim().to_string();
    if token.is_empty() {
        return Err(CoreError::NotRunning);
    }

    let db_trimmed = parsed.db_path.trim();
    if db_trimmed.is_empty() {
        return Err(CoreError::NotRunning);
    }
    let expected_db = data_dir.join("dispatch.db");
    if !paths_match(Path::new(db_trimmed), &expected_db) {
        return Err(CoreError::NotRunning);
    }

    if let Some(pid) = parsed.pid
        && !pid_is_alive(pid)
    {
        return Err(CoreError::NotRunning);
    }

    Ok(CoreConfig { endpoint, token })
}

/// One Connect envelope: a flags byte, a big-endian length, then the payload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Envelope {
    pub flags: u8,
    pub payload: Vec<u8>,
}

pub fn encode_envelope(flags: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(5 + payload.len());
    out.push(flags);
    let len = u32::try_from(payload.len()).unwrap_or(u32::MAX);
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(&payload[..len as usize]);
    out
}

/// Incremental decoder for Connect frames on an HTTP/1.1 body.
#[derive(Clone, Debug, Default)]
pub struct EnvelopeDecoder {
    buf: Vec<u8>,
}

impl EnvelopeDecoder {
    pub fn new() -> Self {
        Self { buf: Vec::new() }
    }

    pub fn push(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    pub fn pending_bytes(&self) -> usize {
        self.buf.len()
    }

    pub fn next_frame(&mut self) -> Result<Option<Envelope>, CoreError> {
        if self.buf.len() < 5 {
            return Ok(None);
        }
        let flags = self.buf[0];
        let len = u32::from_be_bytes([self.buf[1], self.buf[2], self.buf[3], self.buf[4]]) as usize;
        if self.buf.len() < 5 + len {
            return Ok(None);
        }
        let payload = self.buf[5..5 + len].to_vec();
        self.buf.drain(..5 + len);
        Ok(Some(Envelope { flags, payload }))
    }
}

/// Turns one response frame into a watch message.
/// `Ok(None)` is a clean end of stream. Flag `0x02` with an error is `Err`.
pub fn interpret_frame(frame: &Envelope) -> Result<Option<WatchRunResponse>, CoreError> {
    if frame.flags & 0x01 != 0 {
        return Err(CoreError::Rpc(
            "compressed connect frames are not supported".into(),
        ));
    }
    if frame.flags & 0x02 != 0 {
        return match end_stream_error(&frame.payload) {
            Ok(None) => Ok(None),
            Ok(Some(err)) => Err(err),
            Err(err) => Err(err),
        };
    }
    serde_json::from_slice(&frame.payload)
        .map(Some)
        .map_err(|err| CoreError::Rpc(err.to_string()))
}

fn end_stream_error(payload: &[u8]) -> Result<Option<CoreError>, CoreError> {
    if payload.is_empty() {
        return Ok(None);
    }
    let parsed: EndStream = serde_json::from_slice(payload)
        .map_err(|_| CoreError::Rpc("invalid end-of-stream message".into()))?;
    Ok(parsed.error.map(|err| {
        let message = if err.message.trim().is_empty() {
            err.code
        } else {
            err.message
        };
        CoreError::Rpc(if message.trim().is_empty() {
            "stream error".into()
        } else {
            message
        })
    }))
}

#[derive(Deserialize)]
struct EndStream {
    #[serde(default)]
    error: Option<EndStreamError>,
}

#[derive(Deserialize)]
struct EndStreamError {
    #[serde(default)]
    code: String,
    #[serde(default)]
    message: String,
}

#[derive(Deserialize)]
struct ConnectErrorBody {
    #[serde(default)]
    message: String,
}

fn map_transport(err: ureq::Error) -> CoreError {
    match err {
        ureq::Error::Transport(_) => CoreError::NotRunning,
        ureq::Error::Status(status, resp) => {
            let body = resp.into_string().unwrap_or_default();
            if let Ok(parsed) = serde_json::from_str::<ConnectErrorBody>(&body)
                && !parsed.message.trim().is_empty()
            {
                return CoreError::Rpc(parsed.message);
            }
            CoreError::Rpc(format!("HTTP {status}"))
        }
    }
}

/// Blocking Connect-JSON client. Clone is cheap; call it from the background executor.
/// How long a call that answers from the core's own data may take to come back: short, so a
/// core that stopped answering does not hold a page.
const QUICK_READ: Duration = Duration::from_secs(4);
/// How long a call that waits on something outside the core may take: an agent CLI listing
/// its models (20 s at most each), the CLI classifier (60 s), Jev or OpenAI, Polterware, or
/// git. A core that has just started asks every CLI before the catalog's first answer.
const SLOW_READ: Duration = Duration::from_secs(90);

/// The calls that get [`SLOW_READ`].
const SLOW_PROCEDURES: &[&str] = &[
    "dispatch.v1.DispatchService/ApplyAgentSetup",
    "dispatch.v1.DispatchService/BuildTiers",
    "dispatch.v1.DispatchService/Capacity",
    "dispatch.v1.DispatchService/CheckTiers",
    "dispatch.v1.DispatchService/Classify",
    "dispatch.v1.DispatchService/CleanRun",
    // It discards each run's worktree with git.
    "dispatch.v1.DispatchService/DeleteRuns",
    "dispatch.v1.DispatchService/ListAgents",
    "dispatch.v1.DispatchService/ListModelBenchmarks",
    "dispatch.v1.DispatchService/ListModelCatalog",
    "dispatch.v1.DispatchService/ListModelSettings",
    "dispatch.v1.DispatchService/ListModels",
    "dispatch.v1.DispatchService/ListQuota",
    // Both ask git about every worktree folder.
    "dispatch.v1.DispatchService/ListWorktreeFolders",
    "dispatch.v1.DispatchService/MergeRun",
    "dispatch.v1.DispatchService/PruneWorktreeFolders",
    "dispatch.v1.DispatchService/RecoverRun",
    "dispatch.v1.DispatchService/RefreshModelCatalog",
    "dispatch.v1.DispatchService/ReplaceTiers",
    "dispatch.v1.DispatchService/Route",
    "dispatch.v1.DispatchService/StartRun",
    "dispatch.v1.DispatchService/Subagents",
    "dispatch.v1.WorkspaceService/AddWorkspace",
    "dispatch.v1.WorkspaceService/ListWorkspaceCards",
    "dispatch.v1.WorkspaceService/ResolveWorkspace",
];

/// The longest `procedure` may take to answer.
pub(crate) fn read_timeout(procedure: &str) -> Duration {
    if SLOW_PROCEDURES.contains(&procedure) {
        SLOW_READ
    } else {
        QUICK_READ
    }
}

/// How many calls [`each_at_once`] makes at the same time.
const MAX_AT_ONCE: usize = 8;

/// Makes two calls at once and returns both answers. A call blocks its thread until the core
/// answers, so a page that needs two answers would otherwise wait for their sum.
pub fn at_once<A: Send, B: Send>(
    first: impl FnOnce() -> A + Send,
    second: impl FnOnce() -> B + Send,
) -> (A, B) {
    std::thread::scope(|scope| {
        let second = scope.spawn(second);
        let first = first();
        (first, joined(second))
    })
}

/// Makes `call` for every item, [`MAX_AT_ONCE`] at a time, and returns the answers in the
/// items' order.
pub fn each_at_once<T: Sync, R: Send>(items: &[T], call: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let call = &call;
    std::thread::scope(|scope| {
        let mut answers = Vec::with_capacity(items.len());
        for chunk in items.chunks(MAX_AT_ONCE) {
            let calls: Vec<_> = chunk
                .iter()
                .map(|item| scope.spawn(move || call(item)))
                .collect();
            answers.extend(calls.into_iter().map(joined));
        }
        answers
    })
}

/// A scoped thread's answer; a panic in it goes on in the caller, as it would have there.
fn joined<R>(handle: std::thread::ScopedJoinHandle<'_, R>) -> R {
    handle
        .join()
        .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
}

#[derive(Clone)]
pub struct CoreClient {
    config: CoreConfig,
    agent: ureq::Agent,
    slow_agent: ureq::Agent,
    stream_agent: ureq::Agent,
}

impl CoreClient {
    pub fn connect(data_dir: &Path) -> Result<Self, CoreError> {
        let config = read_core_config(data_dir)?;
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(2))
            .timeout_read(QUICK_READ)
            .timeout_write(Duration::from_secs(4))
            .build();
        let slow_agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(2))
            .timeout_read(SLOW_READ)
            .timeout_write(Duration::from_secs(4))
            .build();
        let stream_agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(2))
            .timeout_read(Duration::from_secs(600))
            .timeout_write(Duration::from_secs(4))
            .build();
        Ok(Self {
            config,
            agent,
            slow_agent,
            stream_agent,
        })
    }

    pub fn endpoint(&self) -> &str {
        &self.config.endpoint
    }

    fn rpc<Req: Serialize, Resp: DeserializeOwned>(
        &self,
        procedure: &str,
        request: &Req,
    ) -> Result<Resp, CoreError> {
        let url = format!("{}/{}", self.config.endpoint, procedure);
        let payload =
            serde_json::to_string(request).map_err(|err| CoreError::Rpc(err.to_string()))?;
        let agent = if read_timeout(procedure) == SLOW_READ {
            &self.slow_agent
        } else {
            &self.agent
        };
        let response = agent
            .post(&url)
            .set("Content-Type", "application/json")
            .set("Authorization", &format!("Bearer {}", self.config.token))
            .set("Polterware-Client", CLIENT_HEADER_VALUE)
            .send_string(&payload);
        match response {
            Ok(resp) => {
                let text = resp
                    .into_string()
                    .map_err(|err| CoreError::Rpc(err.to_string()))?;
                serde_json::from_str(&text).map_err(|err| CoreError::Rpc(err.to_string()))
            }
            Err(err) => Err(map_transport(err)),
        }
    }

    pub fn list_sections(
        &self,
        request: &ListSectionsRequest,
    ) -> Result<ListSectionsResponse, CoreError> {
        self.rpc("dispatch.v1.DispatchService/ListSections", request)
    }

    pub fn rename_conversation(
        &self,
        request: &RenameConversationRequest,
    ) -> Result<RenameConversationResponse, CoreError> {
        self.rpc("dispatch.v1.DispatchService/RenameConversation", request)
    }

    pub fn save_section(
        &self,
        request: &SaveSectionRequest,
    ) -> Result<SaveSectionResponse, CoreError> {
        self.rpc("dispatch.v1.DispatchService/SaveSection", request)
    }

    pub fn delete_section(
        &self,
        request: &DeleteSectionRequest,
    ) -> Result<DeleteSectionResponse, CoreError> {
        self.rpc("dispatch.v1.DispatchService/DeleteSection", request)
    }

    pub fn move_to_section(
        &self,
        request: &MoveToSectionRequest,
    ) -> Result<MoveToSectionResponse, CoreError> {
        self.rpc("dispatch.v1.DispatchService/MoveToSection", request)
    }

    pub fn list_runs(&self, request: &ListRunsRequest) -> Result<ListRunsResponse, CoreError> {
        self.rpc("dispatch.v1.DispatchService/ListRuns", request)
    }

    pub fn get_run(&self, request: &GetRunRequest) -> Result<GetRunResponse, CoreError> {
        self.rpc("dispatch.v1.DispatchService/GetRun", request)
    }

    pub fn start_run(&self, request: &StartRunRequest) -> Result<StartRunResponse, CoreError> {
        self.rpc("dispatch.v1.DispatchService/StartRun", request)
    }

    pub fn cancel_run(&self, request: &CancelRunRequest) -> Result<CancelRunResponse, CoreError> {
        self.rpc("dispatch.v1.DispatchService/CancelRun", request)
    }

    pub fn result(&self, request: &ResultRequest) -> Result<ResultResponse, CoreError> {
        self.rpc("dispatch.v1.DispatchService/Result", request)
    }

    /// The page of a run's event text before a point, for a transcript opened at its end.
    pub fn read_run_events(
        &self,
        request: &ReadRunEventsRequest,
    ) -> Result<ReadRunEventsResponse, CoreError> {
        self.rpc("dispatch.v1.DispatchService/ReadRunEvents", request)
    }

    pub fn watch_run(&self, request: &WatchRunRequest) -> Result<WatchRunIter, CoreError> {
        let url = format!(
            "{}/dispatch.v1.DispatchService/WatchRun",
            self.config.endpoint
        );
        let payload = serde_json::to_vec(request).map_err(|err| CoreError::Rpc(err.to_string()))?;
        let body = encode_envelope(0, &payload);
        let response = self
            .stream_agent
            .post(&url)
            .set("Content-Type", "application/connect+json")
            .set("Connect-Protocol-Version", "1")
            .set("Authorization", &format!("Bearer {}", self.config.token))
            .set("Polterware-Client", CLIENT_HEADER_VALUE)
            .send_bytes(&body);
        match response {
            Ok(resp) => Ok(WatchRunIter {
                reader: Box::new(resp.into_reader()),
                decoder: EnvelopeDecoder::new(),
                finished: false,
            }),
            Err(err) => Err(map_transport(err)),
        }
    }

    pub fn list_worktrees(
        &self,
        request: &ListWorktreesRequest,
    ) -> Result<ListWorktreesResponse, CoreError> {
        self.rpc("dispatch.v1.DispatchService/ListWorktrees", request)
    }

    pub fn list_quota(&self, request: &ListQuotaRequest) -> Result<ListQuotaResponse, CoreError> {
        self.rpc("dispatch.v1.DispatchService/ListQuota", request)
    }

    pub fn list_tiers(&self, request: &ListTiersRequest) -> Result<ListTiersResponse, CoreError> {
        self.rpc("dispatch.v1.DispatchService/ListTiers", request)
    }

    pub fn list_agents(
        &self,
        request: &ListAgentsRequest,
    ) -> Result<ListAgentsResponse, CoreError> {
        self.rpc("dispatch.v1.DispatchService/ListAgents", request)
    }

    pub fn list_history(
        &self,
        request: &ListHistoryRequest,
    ) -> Result<ListHistoryResponse, CoreError> {
        self.rpc("dispatch.v1.DispatchService/ListHistory", request)
    }

    pub fn check_tiers(
        &self,
        request: &CheckTiersRequest,
    ) -> Result<CheckTiersResponse, CoreError> {
        self.rpc("dispatch.v1.DispatchService/CheckTiers", request)
    }

    pub fn list_models(
        &self,
        request: &ListModelsRequest,
    ) -> Result<ListModelsResponse, CoreError> {
        self.rpc("dispatch.v1.DispatchService/ListModels", request)
    }

    pub fn list_model_settings(
        &self,
        request: &ListModelSettingsRequest,
    ) -> Result<ListModelSettingsResponse, CoreError> {
        self.rpc("dispatch.v1.DispatchService/ListModelSettings", request)
    }

    pub fn set_model_enabled(
        &self,
        request: &SetModelEnabledRequest,
    ) -> Result<SetModelEnabledResponse, CoreError> {
        self.rpc("dispatch.v1.DispatchService/SetModelEnabled", request)
    }

    pub fn set_agent_enabled(
        &self,
        request: &SetAgentEnabledRequest,
    ) -> Result<SetAgentEnabledResponse, CoreError> {
        self.rpc("dispatch.v1.DispatchService/SetAgentEnabled", request)
    }

    pub fn replace_tiers(
        &self,
        request: &ReplaceTiersRequest,
    ) -> Result<ReplaceTiersResponse, CoreError> {
        self.rpc("dispatch.v1.DispatchService/ReplaceTiers", request)
    }

    pub fn stats(&self, request: &StatsRequest) -> Result<StatsResponse, CoreError> {
        self.rpc("dispatch.v1.DispatchService/Stats", request)
    }

    pub fn get_route_decision(
        &self,
        request: &GetRouteDecisionRequest,
    ) -> Result<GetRouteDecisionResponse, CoreError> {
        self.rpc("dispatch.v1.DispatchService/GetRouteDecision", request)
    }

    pub fn build_tiers(
        &self,
        request: &BuildTiersRequest,
    ) -> Result<BuildTiersResponse, CoreError> {
        self.rpc("dispatch.v1.DispatchService/BuildTiers", request)
    }

    pub fn capacity(&self, request: &CapacityRequest) -> Result<CapacityResponse, CoreError> {
        self.rpc("dispatch.v1.DispatchService/Capacity", request)
    }

    pub fn route(&self, request: &RouteRequest) -> Result<RouteResponse, CoreError> {
        self.rpc("dispatch.v1.DispatchService/Route", request)
    }

    pub fn classify(&self, request: &ClassifyRequest) -> Result<ClassifyResponse, CoreError> {
        self.rpc("dispatch.v1.DispatchService/Classify", request)
    }

    pub fn list_model_catalog(
        &self,
        request: &ListModelCatalogRequest,
    ) -> Result<ListModelCatalogResponse, CoreError> {
        self.rpc("dispatch.v1.DispatchService/ListModelCatalog", request)
    }

    pub fn refresh_model_catalog(
        &self,
        request: &RefreshModelCatalogRequest,
    ) -> Result<RefreshModelCatalogResponse, CoreError> {
        self.rpc("dispatch.v1.DispatchService/RefreshModelCatalog", request)
    }

    pub fn list_model_benchmarks(
        &self,
        request: &ListModelBenchmarksRequest,
    ) -> Result<ListModelBenchmarksResponse, CoreError> {
        self.rpc("dispatch.v1.DispatchService/ListModelBenchmarks", request)
    }

    pub fn merge_run(&self, request: &MergeRunRequest) -> Result<MergeRunResponse, CoreError> {
        self.rpc("dispatch.v1.DispatchService/MergeRun", request)
    }

    pub fn clean_run(&self, request: &CleanRunRequest) -> Result<CleanRunResponse, CoreError> {
        self.rpc("dispatch.v1.DispatchService/CleanRun", request)
    }

    pub fn delete_runs(
        &self,
        request: &DeleteRunsRequest,
    ) -> Result<DeleteRunsResponse, CoreError> {
        self.rpc("dispatch.v1.DispatchService/DeleteRuns", request)
    }

    pub fn list_worktree_folders(
        &self,
        request: &ListWorktreeFoldersRequest,
    ) -> Result<ListWorktreeFoldersResponse, CoreError> {
        self.rpc("dispatch.v1.DispatchService/ListWorktreeFolders", request)
    }

    pub fn prune_worktree_folders(
        &self,
        request: &PruneWorktreeFoldersRequest,
    ) -> Result<PruneWorktreeFoldersResponse, CoreError> {
        self.rpc("dispatch.v1.DispatchService/PruneWorktreeFolders", request)
    }

    pub fn recover_run(
        &self,
        request: &RecoverRunRequest,
    ) -> Result<RecoverRunResponse, CoreError> {
        self.rpc("dispatch.v1.DispatchService/RecoverRun", request)
    }

    pub fn config(&self, request: &ConfigRequest) -> Result<ConfigResponse, CoreError> {
        self.rpc("dispatch.v1.DispatchService/Config", request)
    }

    pub fn subagents(&self, request: &SubagentsRequest) -> Result<SubagentsResponse, CoreError> {
        self.rpc("dispatch.v1.DispatchService/Subagents", request)
    }

    pub fn skill(&self, request: &SkillRequest) -> Result<SkillResponse, CoreError> {
        self.rpc("dispatch.v1.DispatchService/Skill", request)
    }

    pub fn get_agent_setup(
        &self,
        request: &GetAgentSetupRequest,
    ) -> Result<GetAgentSetupResponse, CoreError> {
        self.rpc("dispatch.v1.DispatchService/GetAgentSetup", request)
    }

    pub fn apply_agent_setup(
        &self,
        request: &ApplyAgentSetupRequest,
    ) -> Result<ApplyAgentSetupResponse, CoreError> {
        self.rpc("dispatch.v1.DispatchService/ApplyAgentSetup", request)
    }

    pub fn list_workspaces(
        &self,
        request: &ListWorkspacesRequest,
    ) -> Result<ListWorkspacesResponse, CoreError> {
        self.rpc("dispatch.v1.WorkspaceService/ListWorkspaces", request)
    }

    pub fn add_workspace(
        &self,
        request: &AddWorkspaceRequest,
    ) -> Result<AddWorkspaceResponse, CoreError> {
        self.rpc("dispatch.v1.WorkspaceService/AddWorkspace", request)
    }

    pub fn remove_workspace(
        &self,
        request: &RemoveWorkspaceRequest,
    ) -> Result<RemoveWorkspaceResponse, CoreError> {
        self.rpc("dispatch.v1.WorkspaceService/RemoveWorkspace", request)
    }

    pub fn resolve_workspace(
        &self,
        request: &ResolveWorkspaceRequest,
    ) -> Result<ResolveWorkspaceResponse, CoreError> {
        self.rpc("dispatch.v1.WorkspaceService/ResolveWorkspace", request)
    }

    pub fn list_workspace_cards(
        &self,
        request: &ListWorkspaceCardsRequest,
    ) -> Result<ListWorkspaceCardsResponse, CoreError> {
        self.rpc("dispatch.v1.WorkspaceService/ListWorkspaceCards", request)
    }

    /// Result section `agent`, else `output`, with ANSI stripped.
    /// A missing section (`Rpc`) is an empty string. `NotRunning` propagates.
    pub fn run_reply(&self, id: &str) -> Result<String, CoreError> {
        let agent = match self.result(&ResultRequest {
            id: id.to_string(),
            section: "agent".to_string(),
            ..ResultRequest::default()
        }) {
            Ok(response) => response.content,
            Err(CoreError::Rpc(_)) => String::new(),
            Err(err) => return Err(err),
        };
        let raw = if agent.trim().is_empty() {
            match self.result(&ResultRequest {
                id: id.to_string(),
                section: "output".to_string(),
                ..ResultRequest::default()
            }) {
                Ok(response) => response.content,
                Err(CoreError::Rpc(_)) => String::new(),
                Err(err) => return Err(err),
            }
        } else {
            agent
        };
        Ok(strip_ansi(&raw).trim_end().to_string())
    }
}

/// Iterator of [`WatchRunResponse`] frames from a Connect server stream.
pub struct WatchRunIter {
    reader: Box<dyn Read + Send>,
    decoder: EnvelopeDecoder,
    finished: bool,
}

impl Iterator for WatchRunIter {
    type Item = Result<WatchRunResponse, CoreError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.finished {
            return None;
        }
        loop {
            match self.decoder.next_frame() {
                Ok(Some(frame)) => match interpret_frame(&frame) {
                    Ok(Some(message)) => return Some(Ok(message)),
                    Ok(None) => {
                        self.finished = true;
                        return None;
                    }
                    Err(err) => {
                        self.finished = true;
                        return Some(Err(err));
                    }
                },
                Ok(None) => {
                    let mut buf = [0u8; 8192];
                    match self.reader.read(&mut buf) {
                        Ok(0) => {
                            self.finished = true;
                            if self.decoder.pending_bytes() > 0 {
                                return Some(Err(CoreError::Rpc(
                                    "truncated connect stream".into(),
                                )));
                            }
                            return None;
                        }
                        Ok(n) => self.decoder.push(&buf[..n]),
                        Err(err) => {
                            self.finished = true;
                            return Some(Err(CoreError::Rpc(err.to_string())));
                        }
                    }
                }
                Err(err) => {
                    self.finished = true;
                    return Some(Err(err));
                }
            }
        }
    }
}

/// Strips ANSI CSI/OSC terminal escape sequences and resolves carriage-return overwrites.
pub fn strip_ansi(input: &str) -> String {
    let mut cleaned = String::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i] == 0x1b {
            i += 1;
            if i >= bytes.len() {
                break;
            }
            match bytes[i] {
                b'[' => {
                    i += 1;
                    while i < bytes.len() {
                        let b = bytes[i];
                        i += 1;
                        if (0x40..=0x7e).contains(&b) {
                            break;
                        }
                    }
                }
                b']' => {
                    i += 1;
                    while i < bytes.len() {
                        if bytes[i] == 0x07 {
                            i += 1;
                            break;
                        }
                        if bytes[i] == 0x1b && i + 1 < bytes.len() && bytes[i + 1] == b'\\' {
                            i += 2;
                            break;
                        }
                        i += 1;
                    }
                }
                _ => {
                    i += 1;
                }
            }
        } else {
            let ch = input[i..].chars().next().unwrap_or('\0');
            cleaned.push(ch);
            i += ch.len_utf8();
        }
    }

    cleaned
        .split('\n')
        .map(|line| {
            let trimmed = line.strip_suffix('\r').unwrap_or(line);
            match trimmed.rfind('\r') {
                Some(idx) => &trimmed[idx + 1..],
                None => trimmed,
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use std::{fs, os::unix::fs::PermissionsExt};

    use super::{
        ConversationName, CoreError, DeleteRunsResponse, DispatchRun, EnvelopeDecoder,
        GetRunResponse, ListRunsResponse, ListSectionsResponse, MAX_AT_ONCE, ProtoI64, QUICK_READ,
        RenameConversationRequest, ResultResponse, SLOW_PROCEDURES, SLOW_READ, WatchRunResponse,
        at_once, each_at_once, encode_envelope, end_stream_error, interpret_frame,
        read_core_config, read_timeout, strip_ansi, validate_loopback_endpoint,
    };

    #[test]
    fn a_delete_reads_back_the_runs_it_removed() {
        let response = DeleteRunsResponse {
            deleted: 2,
            json: r#"{"ids":["a1","a2"],"worktrees":["/w/a1"]}"#.into(),
        };
        assert_eq!(response.ids(), ["a1", "a2"]);
        assert!(DeleteRunsResponse::default().ids().is_empty());
        assert_eq!(
            super::read_timeout("dispatch.v1.DispatchService/DeleteRuns"),
            SLOW_READ
        );
    }

    #[test]
    fn a_section_list_reads_the_names_given_to_conversations() {
        let response: ListSectionsResponse = serde_json::from_str(
            r#"{"sections":[],"conversationNames":[{"conversationId":"c1","name":"Release notes"}]}"#,
        )
        .unwrap();
        assert_eq!(
            response.conversation_names,
            [ConversationName {
                conversation_id: "c1".into(),
                name: "Release notes".into(),
            }]
        );
        let request = serde_json::to_value(RenameConversationRequest {
            conversation_id: "c1".into(),
            name: String::new(),
        })
        .unwrap();
        assert_eq!(
            request,
            serde_json::json!({"conversationId": "c1", "name": ""})
        );
    }

    #[test]
    fn two_calls_run_at_the_same_time() {
        use std::sync::mpsc::channel;
        use std::time::Duration;
        // Each call waits for the other's word: in turn, the first would never hear it.
        let (to_second, from_first) = channel();
        let (to_first, from_second) = channel();
        let (first, second) = at_once(
            move || {
                to_second.send(()).unwrap();
                from_second.recv_timeout(Duration::from_secs(5)).is_ok()
            },
            move || {
                to_first.send(()).unwrap();
                from_first.recv_timeout(Duration::from_secs(5)).is_ok()
            },
        );
        assert!(first && second);
    }

    #[test]
    fn many_calls_run_a_few_at_a_time_and_answer_in_order() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::time::Duration;
        let (running, most) = (AtomicUsize::new(0), AtomicUsize::new(0));
        let items: Vec<usize> = (0..20).collect();
        let answers = each_at_once(&items, |item| {
            let now = running.fetch_add(1, Ordering::SeqCst) + 1;
            most.fetch_max(now, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(30));
            running.fetch_sub(1, Ordering::SeqCst);
            item * 2
        });
        assert_eq!(
            answers,
            items.iter().map(|item| item * 2).collect::<Vec<_>>()
        );
        let most = most.load(Ordering::SeqCst);
        assert!(most > 1 && most <= MAX_AT_ONCE, "{most} at once");
    }

    #[test]
    fn calls_that_wait_on_the_agents_get_the_long_read() {
        // A core that has just started asks every agent CLI before the catalog answers, which
        // took 7 s once: past the quick read, so Intelligence showed no models.
        assert_eq!(
            read_timeout("dispatch.v1.DispatchService/ListModelCatalog"),
            SLOW_READ
        );
        assert_eq!(
            read_timeout("dispatch.v1.DispatchService/Classify"),
            SLOW_READ
        );
        assert_eq!(
            read_timeout("dispatch.v1.DispatchService/ListRuns"),
            QUICK_READ
        );
        // Every slow call is one the client makes.
        let source = include_str!("client.rs");
        for procedure in SLOW_PROCEDURES {
            assert!(
                source.contains(&format!("self.rpc(\"{procedure}\"")),
                "{procedure} is not called"
            );
        }
    }

    #[test]
    fn validates_loopback_endpoints() {
        assert_eq!(
            validate_loopback_endpoint("http://127.0.0.1:51234"),
            Some("http://127.0.0.1:51234".to_string())
        );
        assert_eq!(
            validate_loopback_endpoint(" http://127.0.0.1:8080/ "),
            Some("http://127.0.0.1:8080".to_string())
        );

        assert_eq!(validate_loopback_endpoint("https://127.0.0.1:8080"), None);
        assert_eq!(validate_loopback_endpoint("http://localhost:8080"), None);
        assert_eq!(validate_loopback_endpoint("http://0.0.0.0:8080"), None);
        assert_eq!(validate_loopback_endpoint("http://127.0.0.1:0"), None);
        assert_eq!(validate_loopback_endpoint("http://127.0.0.1:99999"), None);
        assert_eq!(
            validate_loopback_endpoint("http://127.0.0.1:8080/dispatch"),
            None
        );
        assert_eq!(
            validate_loopback_endpoint("http://user@127.0.0.1:8080"),
            None
        );
    }

    #[test]
    fn enforces_core_json_permissions_and_schema() {
        let unique = format!(
            "dispatch-gpui-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        );
        let dir = std::env::temp_dir().join(unique);
        let run_dir = dir.join("run");
        fs::create_dir_all(&run_dir).expect("create temp run dir");

        let db_path = dir.join("dispatch.db");
        let core_path = run_dir.join("core.json");
        let payload = serde_json::json!({
            "endpoint": "http://127.0.0.1:43123",
            "token": "secret-token",
            "dbPath": db_path.to_string_lossy(),
            "pid": std::process::id(),
        });
        fs::write(&core_path, payload.to_string()).expect("write core.json");

        fs::set_permissions(&core_path, fs::Permissions::from_mode(0o644)).expect("chmod 0644");
        assert_eq!(read_core_config(&dir).unwrap_err(), CoreError::NotRunning);

        fs::set_permissions(&core_path, fs::Permissions::from_mode(0o600)).expect("chmod 0600");
        let cfg = read_core_config(&dir).expect("valid 0600 core.json");
        assert_eq!(cfg.endpoint, "http://127.0.0.1:43123");
        assert_eq!(cfg.token, "secret-token");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn deserializes_protojson_camel_case_and_strips_ansi() {
        let raw = r#"{
            "runs": [{
                "id": "run_01J",
                "projectId": "ws_1",
                "status": "completed",
                "tier": "hard",
                "agent": "claude",
                "model": "claude-opus-4-6",
                "prompt": "Measure binary size\nSecond line",
                "extraIgnoredField": 123
            }]
        }"#;
        let parsed: ListRunsResponse = serde_json::from_str(raw).expect("valid protojson");
        assert_eq!(parsed.runs.len(), 1);
        assert_eq!(parsed.runs[0].project_id, "ws_1");
        assert_eq!(parsed.runs[0].agent, "claude");

        let res: ResultResponse =
            serde_json::from_str(r#"{"content":"\u001b[32mDone.\u001b[0m\n","total":6}"#)
                .expect("valid result json");
        assert_eq!(strip_ansi(&res.content), "Done.\n");
    }

    #[test]
    fn envelopes_round_trip_including_end_of_stream_error() {
        let payload = br#"{"seq":"4","kind":"note","text":"hi"}"#;
        let bytes = encode_envelope(0, payload);
        let mut decoder = EnvelopeDecoder::new();
        decoder.push(&bytes[..3]);
        assert!(decoder.next_frame().unwrap().is_none());
        decoder.push(&bytes[3..]);
        let frame = decoder.next_frame().unwrap().unwrap();
        assert_eq!(frame.flags, 0);
        let message = interpret_frame(&frame).unwrap().unwrap();
        assert_eq!(message.seq, ProtoI64(4));
        assert_eq!(message.kind, "note");
        assert_eq!(message.text, "hi");

        let end = encode_envelope(
            0x02,
            br#"{"error":{"code":"unavailable","message":"gone"}}"#,
        );
        decoder.push(&end);
        let end_frame = decoder.next_frame().unwrap().unwrap();
        assert_eq!(end_frame.flags & 0x02, 0x02);
        let err = interpret_frame(&end_frame).unwrap_err();
        assert_eq!(err, CoreError::Rpc("gone".into()));

        let clean = encode_envelope(0x02, b"{}");
        decoder.push(&clean);
        let clean_frame = decoder.next_frame().unwrap().unwrap();
        assert!(interpret_frame(&clean_frame).unwrap().is_none());
        assert!(end_stream_error(b"{}").unwrap().is_none());

        let compressed = encode_envelope(0x01, b"{}");
        decoder.push(&compressed);
        let compressed_frame = decoder.next_frame().unwrap().unwrap();
        assert!(interpret_frame(&compressed_frame).is_err());
    }

    #[test]
    fn decodes_int64_as_string_or_number() {
        let raw = r#"{
            "id": "run_1",
            "projectId": "ws_1",
            "queuedMs": "15",
            "runMs": 20,
            "usage": { "inputTokens": "3", "costUsd": 1.5 },
            "attempts": [{ "durationMs": "9", "agent": "claude" }]
        }"#;
        let run: DispatchRun = serde_json::from_str(raw).unwrap();
        assert_eq!(run.project_id, "ws_1");
        assert_eq!(run.queued_ms, Some(ProtoI64(15)));
        assert_eq!(run.run_ms, Some(ProtoI64(20)));
        assert_eq!(run.usage.input_tokens, ProtoI64(3));
        assert_eq!(run.usage.cost_usd, Some(1.5));
        assert_eq!(run.attempts[0].duration_ms, Some(ProtoI64(9)));
        assert_eq!(run.attempts[0].agent, "claude");

        let missing: GetRunResponse = serde_json::from_str("{}").unwrap();
        assert!(missing.run.is_none());

        let event: WatchRunResponse = serde_json::from_str(r#"{"seq":7,"kind":"token"}"#).unwrap();
        assert_eq!(event.seq, ProtoI64(7));
        assert_eq!(event.kind, "token");
    }
}
