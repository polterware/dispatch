//! Agent CLI adapter types and stream helpers.

pub mod acp;
pub mod binaries;
pub mod claude_limits;
pub mod cli;
pub mod codex;
pub mod failfast;
pub mod livelog;
pub mod model;
pub mod probe;
pub mod proc;
pub mod quota;
pub mod stream;
pub mod usage;

use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Capabilities {
    pub effort_mode: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub efforts: Vec<String>,
    pub session_resume: bool,
    pub structured_output: bool,
    pub read_only: bool,
    pub file_editing: bool,
    pub usage: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Request {
    pub effort: String,
    pub prompt: String,
    pub tier: String,
    pub dir: String,
    pub read_only: bool,
    pub timeout: Duration,
    pub resume_session: String,
    pub env: Vec<String>,
    pub log_path: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RunResult {
    pub output: String,
    pub exit_code: i32,
    pub session_id: String,
    pub usage: Option<usage::Usage>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Model {
    pub id: String,
    pub tier: String,
}

pub fn valid_tier(tier: &str) -> bool {
    matches!(tier, "easy" | "normal" | "hard")
}
