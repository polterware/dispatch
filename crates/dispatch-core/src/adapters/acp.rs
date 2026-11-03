use crate::Error;
use std::collections::{BTreeMap, HashMap};
use std::sync::{LazyLock, RwLock};

static CUSTOM: LazyLock<RwLock<BTreeMap<String, Vec<String>>>> =
    LazyLock::new(|| RwLock::new(BTreeMap::new()));

pub fn valid_custom_agent(name: &str, command: &[String]) -> Result<(), Error> {
    let mut chars = name.chars();
    if name.len() > 32
        || !chars.next().is_some_and(|c| c.is_ascii_lowercase())
        || !chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err(Error::Message(format!(
            "invalid agent name {name:?} (lowercase letters, digits, and dashes, starting with a letter)"
        )));
    }
    if super::binaries::default_binaries().contains_key(name) {
        return Err(Error::Message(format!("{name} is a built-in agent")));
    }
    if command.first().is_none_or(|s| s.trim().is_empty()) {
        return Err(Error::Message(format!(
            "{name}: the command that starts its ACP server is empty"
        )));
    }
    Ok(())
}
pub fn set_custom_agents(agents: HashMap<String, Vec<String>>) {
    if let Ok(mut custom) = CUSTOM.write() {
        *custom = agents
            .into_iter()
            .filter(|(name, command)| valid_custom_agent(name, command).is_ok())
            .collect();
    }
}
pub fn custom_agents() -> BTreeMap<String, Vec<String>> {
    CUSTOM.read().ok().map(|m| m.clone()).unwrap_or_default()
}

use super::{
    Model, Request, RunResult, binaries, cli,
    livelog::LiveLog,
    model::allowed_model,
    proc::RpcProcess,
    usage::{AcpUsage, Usage, usage_line},
};
use crate::sandbox;
use serde_json::{Value, json};
use std::io::Write;
use std::time::Duration;
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug)]
pub struct Spec {
    pub name: String,
    pub command: Vec<String>,
    pub read_mode: String,
}
impl Spec {
    pub fn for_agent(name: &str, read_only: bool) -> Self {
        let command = match name {
            "gemini" | "qwen" if read_only => {
                vec!["--acp".into(), "--approval-mode".into(), "plan".into()]
            }
            "gemini" | "qwen" | "copilot" => vec!["--acp".into()],
            "kimi" | "opencode" => vec!["acp".into()],
            "antigravity" => vec!["acp".into(), "--sandbox".into()],
            _ => custom_agents()
                .get(name)
                .map(|c| c[1..].to_vec())
                .unwrap_or_default(),
        };
        Self {
            name: name.into(),
            command,
            read_mode: if name == "opencode" {
                "plan".into()
            } else {
                String::new()
            },
        }
    }
}

pub async fn run(
    name: &str,
    req: &Request,
    model: &str,
    cancel: CancellationToken,
) -> Result<RunResult, Error> {
    run_spec(&Spec::for_agent(name, req.read_only), req, model, cancel).await
}
pub async fn resume(
    name: &str,
    req: &Request,
    model: &str,
    cancel: CancellationToken,
) -> Result<RunResult, Error> {
    if req.resume_session.is_empty() {
        return Err(Error::Message(format!("{name}: resume requires a session")));
    }
    run(name, req, model, cancel).await
}
pub async fn run_spec(
    spec: &Spec,
    req: &Request,
    model: &str,
    cancel: CancellationToken,
) -> Result<RunResult, Error> {
    let (result, status) = run_outcome(spec, req, model, cancel).await;
    status.map(|()| result)
}

pub async fn run_outcome(
    spec: &Spec,
    req: &Request,
    model: &str,
    cancel: CancellationToken,
) -> (RunResult, Result<(), Error>) {
    if !allowed_model(&spec.name, model) {
        return (
            RunResult::default(),
            Err(Error::Message(format!(
                "{}: claude models are not routed through this CLI",
                spec.name
            ))),
        );
    }
    let (result, status) = session_outcome(spec, req, model, cancel, false).await;
    (result, status.map(|_| ()))
}

fn values(option: &Value) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(options) = option["options"].as_array() {
        for value in options {
            if let Some(v) = value["value"].as_str() {
                out.push(v.into());
            }
            if let Some(group) = value["options"].as_array() {
                for item in group {
                    if let Some(v) = item["value"].as_str() {
                        out.push(v.into());
                    }
                }
            }
        }
    }
    out
}
fn option<'a>(options: &'a [Value], id: &str, category: &str) -> Option<&'a Value> {
    options
        .iter()
        .find(|o| o["id"] == id || (!category.is_empty() && o["category"] == category))
}
fn call(id: i64, method: &str, params: Value) -> Value {
    json!({"jsonrpc":"2.0", "id":id, "method":method, "params":params})
}

pub async fn session_models(spec: &Spec, cancel: CancellationToken) -> Result<Vec<String>, Error> {
    let dir = std::env::temp_dir().join(format!("dispatch-acp-{}", cli::new_uuid()));
    std::fs::create_dir(&dir)?;
    let req = Request {
        dir: dir.to_string_lossy().into_owned(),
        read_only: true,
        ..Default::default()
    };
    let result = session(spec, &req, "", cancel, true)
        .await
        .map(|(_, models)| models);
    let _ = std::fs::remove_dir_all(dir);
    result
}
type ModelCache = HashMap<String, (std::time::Instant, Vec<Model>)>;
static MODEL_CACHE: LazyLock<std::sync::Mutex<ModelCache>> =
    LazyLock::new(|| std::sync::Mutex::new(HashMap::new()));

pub async fn models(name: &str, cancel: CancellationToken) -> Result<Vec<Model>, Error> {
    if matches!(name, "opencode" | "antigravity") {
        let mut command = Command::new(binaries::bin(name));
        command.arg("models").env_clear().kill_on_drop(true).envs(
            sandbox::environ(&[])
                .iter()
                .filter_map(|e| e.split_once('=')),
        );
        let out = tokio::select! {
            out = command.output() => out?,
            _ = cancel.cancelled() => return Err(Error::Message("context canceled".into())),
        };
        if !out.status.success() {
            return Err(Error::Message(format!("{}", out.status)));
        }
        let text = String::from_utf8_lossy(&out.stdout);
        let ids = if name == "opencode" {
            cli::parse_opencode_models(&text)
        } else {
            cli::parse_agy_models(&text)
        };
        return Ok(ids
            .into_iter()
            .filter(|m| allowed_model(name, m))
            .map(|id| Model {
                id,
                tier: String::new(),
            })
            .collect());
    }
    if let Ok(cache) = MODEL_CACHE.lock()
        && let Some((at, models)) = cache.get(name)
        && at.elapsed() < Duration::from_secs(600)
    {
        return Ok(models.clone());
    }
    let models: Vec<Model> = session_models(&Spec::for_agent(name, true), cancel)
        .await?
        .into_iter()
        .filter(|m| allowed_model(name, m))
        .map(|id| Model {
            id,
            tier: String::new(),
        })
        .collect();
    if let Ok(mut cache) = MODEL_CACHE.lock() {
        cache.insert(name.into(), (std::time::Instant::now(), models.clone()));
    }
    Ok(models)
}

async fn session(
    spec: &Spec,
    req: &Request,
    model: &str,
    cancel: CancellationToken,
    list_only: bool,
) -> Result<(RunResult, Vec<String>), Error> {
    let (result, status) = session_outcome(spec, req, model, cancel, list_only).await;
    status.map(|models| (result, models))
}

async fn session_outcome(
    spec: &Spec,
    req: &Request,
    model: &str,
    cancel: CancellationToken,
    list_only: bool,
) -> (RunResult, Result<Vec<String>, Error>) {
    let mut result = RunResult {
        session_id: req.resume_session.clone(),
        ..Default::default()
    };
    let token = cancel.child_token();
    let duration = if list_only {
        Duration::from_secs(30)
    } else {
        req.timeout
    };
    let timer = if duration.is_zero() {
        None
    } else {
        let token = token.clone();
        Some(tokio::spawn(async move {
            tokio::time::sleep(duration).await;
            token.cancel();
        }))
    };
    let mut command = Command::new(binaries::bin(&spec.name));
    command.args(&spec.command).env_clear();
    let env = if req.env.is_empty() {
        sandbox::environ(&[])
    } else {
        req.env.clone()
    };
    for e in env {
        if let Some((k, v)) = e.split_once('=') {
            command.env(k, v);
        }
    }
    let dir = if req.dir.is_empty() {
        String::new()
    } else {
        std::path::absolute(&req.dir)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|_| req.dir.clone())
    };
    if !dir.is_empty() {
        command.current_dir(&dir);
    }
    let mut log = LiveLog::open(&req.log_path);
    let status = session_inner(
        spec,
        req,
        model,
        token,
        list_only,
        command,
        &dir,
        &mut log,
        &mut result,
    )
    .await;
    if let Some(timer) = timer {
        timer.abort();
    }
    log.close();
    let status = status.map_err(|err| match err {
        Error::Message(message)
            if message == "context canceled" && !cancel.is_cancelled() && !duration.is_zero() =>
        {
            Error::Message("context deadline exceeded".into())
        }
        Error::Message(message) if message.starts_with("stopped early: ") => {
            Error::Message(format!("{}: {message}", spec.name))
        }
        other => other,
    });
    (result, status)
}

#[allow(clippy::too_many_arguments)]
async fn session_inner(
    spec: &Spec,
    req: &Request,
    model: &str,
    token: CancellationToken,
    list_only: bool,
    command: Command,
    dir: &str,
    log: &mut LiveLog,
    result: &mut RunResult,
) -> Result<Vec<String>, Error> {
    let mut process = RpcProcess::start(command, token, log.clone(), req.started.as_ref())?;
    result.exit_code = -1;
    let mut offered = Vec::new();
    let mut setup: Vec<(String, Value, bool)> = Vec::new();
    let mut prompt_id = -1;
    let mut replaying = false;
    let mut current = String::new();
    let mut last = String::new();
    let mut message_id = String::new();
    let mut context: Option<Usage> = None;
    let mut error = String::new();
    let mut stop = String::new();
    let mut completed = false;
    process.send(call(1,"initialize",json!({"protocolVersion":1,"clientCapabilities":{"fs":{"readTextFile":false,"writeTextFile":false},"terminal":false},"clientInfo":{"name":"dispatch","version":"0.1.0"}}))).await?;
    loop {
        let line = match process.next().await {
            Ok(Some(line)) => line,
            Ok(None) => break,
            Err(err) => {
                process.abort().await?;
                return Err(err);
            }
        };
        let Ok(m) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let method = m["method"].as_str().unwrap_or("");
        if let Some(id) = m.get("id").filter(|v| !v.is_null()) {
            if !method.is_empty() {
                if method != "session/request_permission" {
                    process.send(json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":format!("dispatch runs answer no {method} requests")}})).await?;
                } else {
                    let p = &m["params"];
                    let kind = p["toolCall"]["kind"].as_str().unwrap_or("");
                    let title = p["toolCall"]["title"].as_str().unwrap_or("");
                    let allow =
                        !req.read_only || matches!(kind, "read" | "search" | "fetch" | "think");
                    let wanted = if allow {
                        ["allow_once", "allow_always"]
                    } else {
                        ["reject_once", "reject_always"]
                    };
                    let selected = wanted.iter().find_map(|want| {
                        p["options"]
                            .as_array()
                            .and_then(|options| options.iter().find(|o| o["kind"] == *want))
                            .and_then(|o| o["optionId"].as_str())
                            .filter(|s| !s.is_empty())
                    });
                    let outcome = selected.map_or_else(
                        || json!({"outcome":"cancelled"}),
                        |id| json!({"outcome":"selected","optionId":id}),
                    );
                    if !allow {
                        let _ = writeln!(
                            log,
                            "[note] read-only run: refused {}",
                            format!("{kind} {title}").trim()
                        );
                    }
                    process
                        .send(json!({"jsonrpc":"2.0","id":id,"result":{"outcome":outcome}}))
                        .await?;
                }
                continue;
            }
            let Some(id) = id.as_i64() else {
                continue;
            };
            let err = m.get("error").filter(|e| !e.is_null()).map(|e| {
                e["message"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .unwrap_or("request failed")
            });
            if id == 1 {
                if let Some(err) = err {
                    error = format!("initialize: {err}");
                    break;
                }
                let caps = &m["result"]["agentCapabilities"];
                let mut params = json!({"cwd":dir,"mcpServers":[]});
                let method = if !req.resume_session.is_empty() {
                    params["sessionId"] = json!(req.resume_session);
                    if caps["sessionCapabilities"]
                        .get("resume")
                        .is_some_and(|v| !v.is_null())
                    {
                        "session/resume"
                    } else if caps["loadSession"] == true {
                        replaying = true;
                        "session/load"
                    } else {
                        error = "the agent cannot resume a session".into();
                        break;
                    }
                } else {
                    "session/new"
                };
                process.send(call(2, method, params)).await?;
            } else if id == 2 {
                if let Some(err) = err {
                    error = err.into();
                    break;
                }
                replaying = false;
                let data = &m["result"];
                if let Some(id) = data["sessionId"].as_str().filter(|s| !s.is_empty()) {
                    result.session_id = id.into();
                }
                if result.session_id.is_empty() {
                    error = "the agent opened no session".into();
                    break;
                }
                let options = data["configOptions"]
                    .as_array()
                    .map(Vec::as_slice)
                    .unwrap_or(&[]);
                let model_option = option(options, "model", "model");
                if let Some(ms) = data["models"]["availableModels"].as_array() {
                    offered = ms
                        .iter()
                        .map(|m| m["modelId"].as_str().unwrap_or("").to_owned())
                        .collect();
                }
                if offered.is_empty()
                    && let Some(o) = model_option
                {
                    offered = values(o);
                }
                if list_only {
                    completed = true;
                    break;
                }
                let sid = &result.session_id;
                if !model.is_empty() {
                    if data["models"].is_object() && offered.iter().any(|m| m == model) {
                        setup.push((
                            "session/set_model".into(),
                            json!({"sessionId":sid,"modelId":model}),
                            false,
                        ));
                    } else if let Some(o) =
                        model_option.filter(|o| values(o).iter().any(|v| v == model))
                    {
                        setup.push((
                            "session/set_config_option".into(),
                            json!({"sessionId":sid,"configId":o["id"],"value":model}),
                            false,
                        ));
                    } else if !offered.is_empty() {
                        error = format!(
                            "model {model:?} is not offered ({} models listed)",
                            offered.len()
                        );
                        break;
                    } else {
                        setup.push((
                            "session/set_model".into(),
                            json!({"sessionId":sid,"modelId":model}),
                            false,
                        ));
                    }
                }
                if let Some(o) = option(options, "effort", "thought_level")
                    .filter(|o| !req.effort.is_empty() && values(o).contains(&req.effort))
                {
                    setup.push((
                        "session/set_config_option".into(),
                        json!({"sessionId":sid,"configId":o["id"],"value":req.effort}),
                        true,
                    ));
                }
                if req.read_only && !spec.read_mode.is_empty() {
                    let mode = data["modes"]["availableModes"]
                        .as_array()
                        .and_then(|modes| {
                            modes.iter().filter_map(|m| m["id"].as_str()).rfind(|id| {
                                *id == spec.read_mode
                                    || id.ends_with(&format!("#{}", spec.read_mode))
                            })
                        });
                    if let Some(mode) = mode {
                        setup.push((
                            "session/set_mode".into(),
                            json!({"sessionId":sid,"modeId":mode}),
                            false,
                        ));
                    } else if let Some(o) = option(options, "mode", "mode")
                        .filter(|o| values(o).contains(&spec.read_mode))
                    {
                        setup.push((
                            "session/set_config_option".into(),
                            json!({"sessionId":sid,"configId":o["id"],"value":spec.read_mode}),
                            false,
                        ));
                    } else {
                        error = format!(
                            "the agent offers no {} mode for a read-only run",
                            spec.read_mode
                        );
                        break;
                    }
                }
                if let Some((method, params, _)) = setup.first() {
                    process.send(call(3, method, params.clone())).await?;
                } else {
                    prompt_id = 4;
                    process
                        .send(call(
                            prompt_id,
                            "session/prompt",
                            json!({"sessionId":sid,"prompt":[{"type":"text","text":req.prompt}]}),
                        ))
                        .await?;
                }
            } else if id == prompt_id {
                if let Some(err) = err {
                    error = err.into();
                    break;
                }
                stop = m["result"]["stopReason"].as_str().unwrap_or("").into();
                let usage = m["result"]
                    .get("usage")
                    .filter(|v| !v.is_null())
                    .or_else(|| m["result"]["_meta"].get("usage").filter(|v| !v.is_null()));
                if let Some(u) =
                    usage.and_then(|v| serde_json::from_value::<AcpUsage>(v.clone()).ok())
                {
                    let mut u = u.normalize();
                    if let Some(c) = &context {
                        u.context_tokens = c.context_tokens;
                        u.context_window = c.context_window;
                        u.cost_usd = c.cost_usd;
                    }
                    let _ = log.write_all(usage_line(&u).as_bytes());
                    result.usage = Some(u);
                }
                completed = true;
                break;
            } else if id >= 3 {
                let index = (id - 3) as usize;
                if let Some((method, _, optional)) = setup.get(index)
                    && let Some(err) = err
                    && !optional
                {
                    error = format!("{method}: {err}");
                    break;
                }
                if let Some((method, params, _)) = setup.get(index + 1) {
                    process.send(call(id + 1, method, params.clone())).await?;
                } else {
                    prompt_id = 4 + setup.len() as i64;
                    process.send(call(prompt_id,"session/prompt",json!({"sessionId":result.session_id,"prompt":[{"type":"text","text":req.prompt}]}))).await?;
                }
            }
        } else if method == "session/update" && !replaying {
            let u = &m["params"]["update"];
            let kind = u["sessionUpdate"].as_str().unwrap_or("");
            match kind {
                "agent_message_chunk" => {
                    let id = u["messageId"].as_str().unwrap_or("");
                    if !id.is_empty() && id != message_id {
                        if !current.trim().is_empty() {
                            last = current.trim().into();
                        }
                        current.clear();
                        message_id = id.into();
                    }
                    current.push_str(u["content"]["text"].as_str().unwrap_or(""));
                    result.output = if current.trim().is_empty() {
                        last.clone()
                    } else {
                        current.trim().into()
                    };
                }
                "tool_call" => {
                    if !current.trim().is_empty() {
                        last = current.trim().into();
                    }
                    current.clear();
                }
                "usage_update" => {
                    let c = context.get_or_insert_with(Usage::default);
                    c.context_tokens = u["used"].as_i64().unwrap_or(0);
                    c.context_window = u["size"].as_i64().unwrap_or(0);
                    if req.resume_session.is_empty()
                        && u["cost"]["currency"]
                            .as_str()
                            .is_some_and(|s| s.eq_ignore_ascii_case("USD"))
                    {
                        c.cost_usd = u["cost"]["amount"].as_f64().filter(|x| *x > 0.0);
                    }
                    let _ = log.write_all(usage_line(c).as_bytes());
                }
                _ => {}
            }
            if !matches!(
                kind,
                "available_commands_update"
                    | "session_info_update"
                    | "current_mode_update"
                    | "config_option_update"
                    | "user_message_chunk"
                    | "usage_update"
            ) {
                let _ = writeln!(
                    log,
                    "{}",
                    json!({"method":"session/update","params":{"update":u}})
                );
            }
        }
    }
    let (code, stderr, forced, fatal) = process.finish(Duration::from_secs(3)).await?;
    let code = if forced && (completed || list_only) {
        0
    } else {
        code
    };
    result.exit_code = code;
    if !fatal.is_empty() {
        return Err(Error::Message(format!(
            "{}: stopped early: {fatal}",
            spec.name
        )));
    }
    result.output = if current.trim().is_empty() {
        last
    } else {
        current.trim().into()
    };
    if !error.is_empty() {
        return Err(Error::Message(format!("{}: {error}", spec.name)));
    }
    if code != 0 && (!list_only || offered.is_empty()) {
        return Err(Error::Message(format!(
            "{} acp: exit status {code}: {}",
            spec.name,
            stderr.trim()
        )));
    }
    if !completed && !list_only {
        return Err(Error::Message(format!(
            "{} acp ended before the prompt completed: {}",
            spec.name,
            stderr.trim()
        )));
    }
    match stop.as_str() {
        "refusal" => {
            return Err(Error::Message(format!(
                "{}: the agent refused the prompt",
                spec.name
            )));
        }
        "cancelled" => {
            return Err(Error::Message(format!(
                "{}: the prompt was cancelled",
                spec.name
            )));
        }
        _ => {}
    }
    Ok(offered)
}
