use serde_json::{Value, json};

use super::Request;

pub fn turn_params(req: &Request, thread_id: &str) -> Value {
    let mut params = json!({"threadId":thread_id,"input":[{"type":"text","text":req.prompt}]});
    if !req.effort.is_empty() {
        params["effort"] = Value::String(req.effort.clone());
    }
    params
}

use super::{
    RunResult, binaries,
    livelog::LiveLog,
    proc::RpcProcess,
    usage::{Usage, usage_line},
};
use crate::{Error, sandbox};
use std::io::Write;
use std::time::Duration;
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

pub fn thread_params(req: &Request, model: &str) -> Value {
    let mut params = json!({"sandbox": if req.read_only { "read-only" } else { "workspace-write" }, "approvalPolicy":"never"});
    if !req.dir.is_empty() {
        params["cwd"] = json!(req.dir);
    }
    if !model.is_empty() {
        params["model"] = json!(model);
    }
    if !req.resume_session.is_empty() {
        params["threadId"] = json!(req.resume_session);
    }
    params
}

pub async fn resume(
    req: &Request,
    model: &str,
    cancel: CancellationToken,
) -> Result<RunResult, Error> {
    if req.resume_session.is_empty() {
        return Err(Error::Message("codex: resume requires a session".into()));
    }
    run(req, model, cancel).await
}

pub async fn run(
    req: &Request,
    model: &str,
    cancel: CancellationToken,
) -> Result<RunResult, Error> {
    let (result, status) = run_outcome(req, model, cancel).await;
    status.map(|()| result)
}

pub async fn run_outcome(
    req: &Request,
    model: &str,
    cancel: CancellationToken,
) -> (RunResult, Result<(), Error>) {
    let mut result = RunResult {
        session_id: req.resume_session.clone(),
        ..Default::default()
    };
    let token = cancel.child_token();
    let timer = if req.timeout.is_zero() {
        None
    } else {
        let token = token.clone();
        let duration = req.timeout;
        Some(tokio::spawn(async move {
            tokio::time::sleep(duration).await;
            token.cancel();
        }))
    };
    let mut command = Command::new(binaries::bin("codex"));
    command.arg("app-server").env_clear();
    if !req.dir.is_empty() {
        command.current_dir(&req.dir);
    }
    let env = if req.env.is_empty() {
        sandbox::environ(&[])
    } else {
        req.env.clone()
    };
    for item in env {
        if let Some((k, v)) = item.split_once('=') {
            command.env(k, v);
        }
    }
    let mut log = LiveLog::open(&req.log_path);
    let status = run_session(req, model, token, command, &mut log, &mut result).await;
    if let Some(timer) = timer {
        timer.abort();
    }
    log.close();
    let status = status.map_err(|err| match err {
        Error::Message(message)
            if message == "context canceled"
                && !cancel.is_cancelled()
                && !req.timeout.is_zero() =>
        {
            Error::Message("context deadline exceeded".into())
        }
        Error::Message(message) if message.starts_with("stopped early: ") => {
            Error::Message(format!("{}: {message}", "codex"))
        }
        other => other,
    });
    (result, status)
}

async fn run_session(
    req: &Request,
    model: &str,
    token: CancellationToken,
    command: Command,
    log: &mut LiveLog,
    result: &mut RunResult,
) -> Result<(), Error> {
    let mut process = RpcProcess::start(command, token, req.started.as_ref())?;
    result.exit_code = -1;
    let mut turn = String::new();
    let mut final_text = String::new();
    let mut error = String::new();
    let mut completed = false;
    process.send(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"clientInfo":{"name":"dispatch","version":"0.1.0"}}})).await?;
    process
        .send(json!({"jsonrpc":"2.0","method":"initialized"}))
        .await?;
    process.send(json!({"jsonrpc":"2.0","id":2,"method":if req.resume_session.is_empty() { "thread/start" } else { "thread/resume" },"params":thread_params(req, model)})).await?;
    loop {
        let line = match process.next().await {
            Ok(Some(line)) => line,
            Ok(None) => break,
            Err(err) => {
                process.abort().await?;
                return Err(err);
            }
        };
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let method = message["method"].as_str().unwrap_or("");
        if let Some(id) = message.get("id").filter(|v| !v.is_null()) {
            if !method.is_empty() {
                process.send(json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"dispatch runs answer no requests"}})).await?;
                continue;
            }
            let id = id.as_i64().unwrap_or(0);
            if let Some(e) = message.get("error").filter(|e| !e.is_null()) {
                if id != 1 {
                    error = e["message"].as_str().unwrap_or("").into();
                    break;
                }
                continue;
            }
            if id == 2 {
                result.session_id = message["result"]["thread"]["id"]
                    .as_str()
                    .unwrap_or("")
                    .into();
                if result.session_id.is_empty() {
                    error = "the app-server started no thread".into();
                    break;
                }
                process.send(json!({"jsonrpc":"2.0","id":3,"method":"turn/start","params":turn_params(req, &result.session_id)})).await?;
            } else if id == 3 && turn.is_empty() {
                turn = message["result"]["turn"]["id"]
                    .as_str()
                    .unwrap_or("")
                    .into();
            }
            continue;
        }
        let params = &message["params"];
        let ours = params["turnId"]
            .as_str()
            .is_none_or(|id| id.is_empty() || turn.is_empty() || id == turn);
        match method {
            "turn/started" if turn.is_empty() => {
                turn = params["turn"]["id"].as_str().unwrap_or("").into()
            }
            "item/completed" if ours && params["item"]["type"] == "agentMessage" => {
                let text = params["item"]["text"].as_str().unwrap_or("");
                if !text.trim().is_empty() {
                    if params["item"]["phase"] == "final_answer" {
                        final_text = text.into();
                    }
                    if final_text.is_empty() {
                        result.output = text.into();
                    } else {
                        result.output = final_text.clone();
                    }
                }
            }
            "thread/tokenUsage/updated" if ours && params["tokenUsage"]["last"].is_object() => {
                let value = &params["tokenUsage"]["last"];
                let num = |key: &str| value[key].as_i64().unwrap_or(0);
                let usage = result.usage.get_or_insert_with(Usage::default);
                let (input, cached, write, output) = (
                    num("inputTokens"),
                    num("cachedInputTokens"),
                    num("cacheWriteInputTokens"),
                    num("outputTokens"),
                );
                usage.input_tokens += (input - cached - write).max(0);
                usage.cache_read_tokens += cached;
                usage.cache_write_tokens += write;
                usage.output_tokens += output;
                usage.reasoning_tokens += num("reasoningOutputTokens");
                usage.context_tokens = input + output;
                usage.context_window = params["tokenUsage"]["modelContextWindow"]
                    .as_i64()
                    .unwrap_or(0);
                let _ = log.write_all(usage_line(usage).as_bytes());
            }
            "error" if !params["willRetry"].as_bool().unwrap_or(false) => {
                if let Some(message) = params["error"]["message"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                {
                    error = message.into();
                }
            }
            _ => {}
        }
        if let Some(line) = log_line(method, params) {
            let _ = log.write_all(line.as_bytes());
        }
        if method == "turn/completed"
            && (turn.is_empty() || params["turn"]["id"].as_str().unwrap_or("") == turn)
        {
            completed = true;
            if let Some(message) = params["turn"]["error"]["message"]
                .as_str()
                .filter(|s| !s.is_empty())
            {
                error = message.into();
            } else if let Some(status) = params["turn"]["status"]
                .as_str()
                .filter(|s| !s.is_empty() && *s != "completed")
                && error.is_empty()
            {
                error = format!("the turn ended {status}");
            }
            break;
        }
    }
    let (code, stderr, _, fatal) = process.finish(Duration::from_secs(5)).await?;
    result.exit_code = code;
    if !fatal.is_empty() {
        return Err(Error::Message(format!("codex: stopped early: {fatal}")));
    }
    if !final_text.is_empty() {
        result.output = final_text;
    }
    if code != 0 {
        return Err(Error::Message(format!(
            "codex app-server: exit status {code}: {}",
            stderr.trim()
        )));
    }
    if !error.is_empty() {
        return Err(Error::Message(format!("codex: {error}")));
    }
    if !completed {
        return Err(Error::Message(format!(
            "codex app-server ended before the turn completed: {}",
            stderr.trim()
        )));
    }
    Ok(())
}

fn log_line(method: &str, params: &Value) -> Option<String> {
    if method.is_empty()
        || matches!(
            method,
            "thread/started"
                | "thread/status/changed"
                | "thread/tokenUsage/updated"
                | "account/updated"
                | "account/rateLimits/updated"
                | "remoteControl/status/changed"
                | "mcpServer/startupStatus/updated"
                | "configWarning"
                | "warning"
                | "serverRequest/resolved"
        )
        || params["item"]["type"] == "userMessage"
    {
        return None;
    }
    let mut params = params.clone();
    if let Some(map) = params.as_object_mut() {
        map.remove("threadId");
        map.remove("turnId");
    }
    Some(format!("{}\n", json!({"method":method,"params":params})))
}
