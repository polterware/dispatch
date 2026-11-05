//! The agent CLIs dispatch knows: each one's entry points behind
//! [`AgentAdapter`], the built-in ones and then the ACP agents the user added.

use super::{
    Capabilities, Model, RunResult, StartedHook, acp, binaries, claude_limits, cli, codex,
    probe::{self, ProbeReport},
    quota::{self, Quota, QuotaSample},
};
use crate::{
    Error,
    runner::{AgentAdapter, AgentCall, AttemptError, AttemptResult, ERR_NO_QUOTA},
    sandbox,
};
use chrono::Utc;
use futures::future::BoxFuture;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

/// The models Claude Code takes by id; it has no command that lists them.
pub const CLAUDE_MODELS: [&str; 4] = [
    "claude-fable-5-1",
    "claude-opus-5-5",
    "claude-sonnet-5-5",
    "claude-haiku-5-5",
];

/// The built-in agents, in the order Known lists them.
pub const BUILT_IN: [&str; 11] = [
    "codex",
    "claude",
    "cursor",
    "grok",
    "antigravity",
    "opencode",
    "pi",
    "gemini",
    "qwen",
    "copilot",
    "kimi",
];

/// How an agent's runs are driven.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Driver {
    /// `codex app-server`.
    Codex,
    /// A print-mode CLI with a JSON event stream.
    Cli,
    /// The Agent Client Protocol over stdio.
    Acp,
}

/// One agent CLI.
#[derive(Clone, Debug)]
pub struct Agent {
    name: String,
    driver: Driver,
}

impl Agent {
    pub fn new(name: &str) -> Self {
        let driver = match name {
            "codex" => Driver::Codex,
            "claude" | "cursor" | "grok" | "pi" => Driver::Cli,
            _ => Driver::Acp,
        };
        Self {
            name: name.into(),
            driver,
        }
    }

    async fn attempt(&self, call: AgentCall, resume: bool) -> AttemptResult {
        let mut req = call.request.clone();
        let started = call.started.clone();
        req.started = Some(StartedHook(Arc::new(move |pid| started(pid))));
        let cancel = call.cancel.clone();
        let (result, status) = match self.driver {
            Driver::Codex => {
                if resume && req.resume_session.is_empty() {
                    (RunResult::default(), Err(no_session(&self.name)))
                } else {
                    codex::run_outcome(&req, &call.model, cancel.clone()).await
                }
            }
            Driver::Cli => {
                let r = if resume {
                    cli::resume(&self.name, &req, &call.model, cancel.clone()).await
                } else {
                    cli::run(&self.name, &req, &call.model, cancel.clone()).await
                };
                match r {
                    Ok(r) => (r, Ok(())),
                    Err(e) => (RunResult::default(), Err(e)),
                }
            }
            Driver::Acp => {
                if resume && req.resume_session.is_empty() {
                    (RunResult::default(), Err(no_session(&self.name)))
                } else {
                    let spec = acp::Spec::for_agent(&self.name, req.read_only);
                    acp::run_outcome(&spec, &req, &call.model, cancel.clone()).await
                }
            }
        };
        AttemptResult {
            result,
            error: status.err().map(attempt_error),
        }
    }
}

fn no_session(name: &str) -> Error {
    Error::Message(format!("{name}: resume requires a session"))
}

/// Tells the deadline and an incomplete process cleanup apart, because they
/// end the run instead of falling back.
fn attempt_error(e: Error) -> AttemptError {
    let m = e.to_string();
    if m == "context deadline exceeded" {
        AttemptError::DeadlineExceeded
    } else if m.starts_with("agent process cleanup incomplete") {
        AttemptError::ProcessesAlive
    } else {
        AttemptError::Failed(m)
    }
}

fn no_quota() -> Error {
    Error::Message(ERR_NO_QUOTA.into())
}

fn no_quota_because(e: &Error) -> Error {
    Error::Message(format!("{ERR_NO_QUOTA}: {e}"))
}

/// Runs `args` of `bin` in a sanitized environment, in its own process group,
/// and returns stdout; a failure or a timeout is an error.
async fn command_output(bin: &str, args: &[&str], timeout: Duration) -> Result<Vec<u8>, Error> {
    let mut cmd = Command::new(bin);
    cmd.args(args)
        .env_clear()
        .envs(
            sandbox::environ(&[])
                .iter()
                .filter_map(|e| e.split_once('=')),
        )
        .stdin(Stdio::null())
        .process_group(0)
        .kill_on_drop(true);
    let out = tokio::time::timeout(timeout, cmd.output())
        .await
        .map_err(|_| Error::Message("context deadline exceeded".into()))??;
    if !out.status.success() {
        return Err(Error::Message(format!("{bin}: {}", out.status)));
    }
    Ok(out.stdout)
}

fn to_models(ids: Vec<String>) -> Vec<Model> {
    ids.into_iter()
        .map(|id| Model {
            id,
            tier: String::new(),
        })
        .collect()
}

/// Performs initialize and one request against a short-lived `codex
/// app-server` over stdio and returns the raw result.
async fn app_server_call(method: &str, params: Option<Value>) -> Result<Value, Error> {
    let call = async {
        let mut cmd = Command::new(binaries::bin("codex"));
        cmd.args(["-s", "read-only", "-a", "never", "app-server"])
            .env_clear()
            .envs(
                sandbox::environ(&[])
                    .iter()
                    .filter_map(|e| e.split_once('=')),
            )
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let mut child = cmd.spawn()?;
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| Error::Message("codex app-server: no stdin".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| Error::Message("codex app-server: no stdout".into()))?;
        let mut req = json!({"jsonrpc": "2.0", "id": 2, "method": method});
        if let Some(p) = params {
            req["params"] = p;
        }
        for msg in [
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": {"clientInfo": {"name": "dispatch", "version": "0.1.0"}}}),
            json!({"jsonrpc": "2.0", "method": "initialized"}),
            req,
        ] {
            let mut line = serde_json::to_vec(&msg)?;
            line.push(b'\n');
            stdin.write_all(&line).await?;
        }
        let mut lines = BufReader::new(stdout).lines();
        while let Some(line) = lines.next_line().await? {
            let Ok(m) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if m["id"].as_i64() != Some(2) {
                continue;
            }
            if let Some(e) = m.get("error").filter(|e| !e.is_null()) {
                return Err(Error::Message(format!(
                    "codex app-server: {}",
                    e["message"].as_str().unwrap_or_default()
                )));
            }
            return Ok(m["result"].clone());
        }
        Err(Error::Message(
            "codex app-server closed without a response".into(),
        ))
    };
    tokio::time::timeout(Duration::from_secs(20), call)
        .await
        .map_err(|_| Error::Message("context deadline exceeded".into()))?
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CodexLimits {
    #[serde(default)]
    rate_limits: CodexRateLimits,
}

#[derive(Default, Deserialize)]
struct CodexRateLimits {
    primary: Option<quota::CodexWindow>,
    secondary: Option<quota::CodexWindow>,
}

async fn codex_limits() -> Result<CodexRateLimits, Error> {
    let raw = app_server_call("account/rateLimits/read", None).await?;
    Ok(serde_json::from_value::<CodexLimits>(raw)?.rate_limits)
}

async fn codex_models() -> Result<Vec<Model>, Error> {
    #[derive(Deserialize)]
    struct Listed {
        #[serde(default)]
        id: String,
        #[serde(default)]
        model: String,
    }
    #[derive(Deserialize)]
    struct List {
        #[serde(default)]
        data: Vec<Listed>,
    }
    let raw = app_server_call("model/list", Some(json!({}))).await?;
    let list: List = serde_json::from_value(raw)?;
    // Tier assignment lives in the declarative tier table, not here.
    Ok(list
        .data
        .into_iter()
        .map(|m| Model {
            id: if m.model.is_empty() { m.id } else { m.model },
            tier: String::new(),
        })
        .collect())
}

/// Antigravity's own non-interactive `/usage` report.
async fn agy_usage_raw() -> Result<Vec<u8>, Error> {
    command_output(
        &binaries::bin("antigravity"),
        &["-p", "/usage", "--output-format", "json"],
        Duration::from_secs(90),
    )
    .await
}

impl AgentAdapter for Agent {
    fn name(&self) -> String {
        self.name.clone()
    }

    fn binary(&self) -> String {
        binaries::bin(&self.name)
    }

    fn capabilities(&self) -> Capabilities {
        probe::capabilities(&self.name)
    }

    fn models(&self) -> BoxFuture<'_, Result<Vec<Model>, Error>> {
        Box::pin(async move {
            let list = |args: &'static [&'static str]| async move {
                let out = command_output(&self.binary(), args, Duration::from_secs(60)).await?;
                Ok::<String, Error>(String::from_utf8_lossy(&out).into_owned())
            };
            match self.name.as_str() {
                "codex" => codex_models().await,
                "claude" => Ok(CLAUDE_MODELS
                    .iter()
                    .map(|id| Model {
                        id: (*id).into(),
                        tier: String::new(),
                    })
                    .collect()),
                "cursor" => Ok(to_models(cli::parse_cursor_models(
                    &list(&["models"]).await?,
                ))),
                "grok" => Ok(to_models(cli::parse_grok_models(&list(&["models"]).await?))),
                "pi" => Ok(to_models(cli::parse_pi_models(
                    &list(&["--list-models"]).await?,
                ))),
                name => {
                    let cancel = CancellationToken::new();
                    match tokio::time::timeout(
                        Duration::from_secs(30),
                        acp::models(name, cancel.clone()),
                    )
                    .await
                    {
                        Ok(r) => r,
                        Err(_) => {
                            cancel.cancel();
                            Err(Error::Message("context deadline exceeded".into()))
                        }
                    }
                }
            }
        })
    }

    fn quota(&self) -> BoxFuture<'_, Result<Quota, Error>> {
        Box::pin(async move {
            match self.name.as_str() {
                "codex" => {
                    let l = codex_limits().await.map_err(|e| no_quota_because(&e))?;
                    quota::codex_quota(l.primary.as_ref(), l.secondary.as_ref())
                }
                "claude" => claude_limits::quota(&claude_limits::observed(Utc::now())),
                // Antigravity's quota needs a model: model_quotas resolves it.
                _ => Err(no_quota()),
            }
        })
    }

    fn model_quotas(
        &self,
        models: Vec<String>,
    ) -> Option<BoxFuture<'_, Result<HashMap<String, Quota>, Error>>> {
        if self.name != "antigravity" {
            return None;
        }
        Some(Box::pin(async move {
            let raw = agy_usage_raw().await.map_err(|e| no_quota_because(&e))?;
            let groups = quota::parse_agy_usage(&raw).map_err(|e| no_quota_because(&e))?;
            let mut res = HashMap::new();
            for m in models {
                let g = if m.to_lowercase().contains("gemini") {
                    "Gemini Models"
                } else {
                    "Claude and GPT models"
                };
                if let Some(q) = groups.get(g) {
                    let mut q = q.clone();
                    q.key = format!("antigravity:{g}");
                    res.insert(m, q);
                }
            }
            Ok(res)
        }))
    }

    fn usage_source(&self) -> Option<&'static str> {
        probe::usage_source(&self.name)
    }

    fn usage(&self) -> BoxFuture<'_, Result<Vec<QuotaSample>, Error>> {
        Box::pin(async move {
            match self.name.as_str() {
                "codex" => {
                    let l = codex_limits().await.map_err(|e| no_quota_because(&e))?;
                    quota::codex_samples(l.primary.as_ref(), l.secondary.as_ref())
                }
                "antigravity" => {
                    let raw = agy_usage_raw().await.map_err(|e| no_quota_because(&e))?;
                    quota::parse_agy_samples(&raw)
                }
                // The windows of the last rate_limit_event a run printed that
                // have not reset since; Claude Code has no command that reports them.
                "claude" => {
                    let s = claude_limits::observed(Utc::now());
                    if s.is_empty() { Err(no_quota()) } else { Ok(s) }
                }
                _ => Err(no_quota()),
            }
        })
    }

    fn probe(&self) -> Option<BoxFuture<'_, ProbeReport>> {
        Some(Box::pin(probe::probe(&self.name)))
    }

    fn run(&self, call: AgentCall) -> BoxFuture<'_, AttemptResult> {
        Box::pin(self.attempt(call, false))
    }

    fn resume(&self, call: AgentCall) -> BoxFuture<'_, AttemptResult> {
        Box::pin(self.attempt(call, true))
    }
}

/// The adapters dispatch knows about: the built-in ones, then the ACP agents
/// the user added, by name.
pub fn known() -> Vec<Arc<dyn AgentAdapter>> {
    BUILT_IN
        .iter()
        .map(|n| (*n).to_owned())
        .chain(acp::custom_agents().into_keys())
        .map(|n| Arc::new(Agent::new(&n)) as Arc<dyn AgentAdapter>)
        .collect()
}
