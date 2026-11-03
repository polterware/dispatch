pub fn parse_cursor_models(out: &str) -> Vec<String> {
    out.split('\n')
        .filter_map(|line| {
            let (id, _) = line.split_once(" - ")?;
            let id = id.trim();
            if id.is_empty() || id.contains(' ') {
                None
            } else {
                Some(id.to_owned())
            }
        })
        .collect()
}
pub fn parse_agy_models(out: &str) -> Vec<String> {
    out.split('\n')
        .filter_map(|line| {
            let (id, _) = line.split_once('\t')?;
            let id = id.trim();
            if id.is_empty() {
                None
            } else {
                Some(id.to_owned())
            }
        })
        .collect()
}
pub fn parse_grok_models(out: &str) -> Vec<String> {
    out.split('\n')
        .filter_map(|line| {
            let line = line.trim();
            let line = line
                .strip_prefix("* ")
                .or_else(|| line.strip_prefix("- "))?;
            line.split_whitespace().next().map(str::to_owned)
        })
        .collect()
}
pub fn parse_opencode_models(out: &str) -> Vec<String> {
    out.split('\n')
        .map(str::trim)
        .filter(|line| line.contains('/') && !line.contains(' '))
        .map(str::to_owned)
        .collect()
}
pub fn parse_pi_models(out: &str) -> Vec<String> {
    out.split('\n')
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            let provider = words.next()?;
            let model = words.next()?;
            if provider == "provider" {
                None
            } else {
                Some(format!("{provider}/{model}"))
            }
        })
        .collect()
}

use super::{
    Request, RunResult, binaries, livelog::LiveLog, model::allowed_model, proc::run_command,
    stream, usage,
};
use crate::{Error, sandbox};
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

pub fn new_uuid() -> String {
    let mut b: [u8; 16] = rand::random();
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        b[0],
        b[1],
        b[2],
        b[3],
        b[4],
        b[5],
        b[6],
        b[7],
        b[8],
        b[9],
        b[10],
        b[11],
        b[12],
        b[13],
        b[14],
        b[15]
    )
}

pub fn arguments(
    name: &str,
    req: &Request,
    model: &str,
    session: &str,
) -> Result<Vec<String>, Error> {
    let mut a: Vec<String> = match name {
        "claude" => [
            "-p",
            "--output-format",
            "stream-json",
            "--verbose",
            "--include-partial-messages",
            "--permission-mode",
            if req.read_only { "plan" } else { "acceptEdits" },
        ]
        .into_iter()
        .map(str::to_owned)
        .collect(),
        "pi" => ["--mode", "json", "-p"]
            .into_iter()
            .map(str::to_owned)
            .collect(),
        "cursor" => [
            "-p",
            "--output-format",
            "stream-json",
            "--stream-partial-output",
            "--trust",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect(),
        "grok" => [
            "--output-format",
            "streaming-json",
            "--permission-mode",
            if req.read_only { "plan" } else { "acceptEdits" },
        ]
        .into_iter()
        .map(str::to_owned)
        .collect(),
        _ => return Err(Error::Message(format!("unknown CLI: {name}"))),
    };
    if name == "cursor" {
        a.extend(if req.read_only {
            vec!["--mode".into(), "ask".into()]
        } else {
            vec!["--force".into(), "--sandbox".into(), "enabled".into()]
        });
    }
    if !session.is_empty() && (name != "claude" || !req.resume_session.is_empty()) {
        let flag = match name {
            "pi" => "--session-id",
            "grok" if req.resume_session.is_empty() => "-s",
            _ => "--resume",
        };
        a.extend([flag.into(), session.into()]);
    }
    if !model.is_empty() {
        a.extend(["--model".into(), model.into()]);
    }
    if !req.effort.is_empty() && matches!(name, "pi" | "claude") {
        a.extend([
            if name == "pi" {
                "--thinking"
            } else {
                "--effort"
            }
            .into(),
            req.effort.clone(),
        ]);
    }
    if name == "pi" && req.read_only {
        a.extend(["--tools".into(), "read,grep,find,ls".into()]);
    }
    if !req.dir.is_empty() && matches!(name, "cursor" | "grok") {
        a.extend([
            if name == "cursor" {
                "--workspace"
            } else {
                "--cwd"
            }
            .into(),
            req.dir.clone(),
        ]);
    }
    match name {
        "pi" => a.extend(["--".into(), req.prompt.clone()]),
        "grok" => a.extend(["-p".into(), req.prompt.clone()]),
        "cursor" => a.push(req.prompt.clone()),
        _ => {}
    }
    Ok(a)
}

pub async fn run(
    name: &str,
    req: &Request,
    model: &str,
    cancel: CancellationToken,
) -> Result<RunResult, Error> {
    if !allowed_model(name, model) {
        return Err(Error::Message(format!(
            "{name}: claude models are not routed through this CLI"
        )));
    }
    let session = if !req.resume_session.is_empty() {
        req.resume_session.clone()
    } else if matches!(name, "pi" | "grok") {
        new_uuid()
    } else if name == "cursor" {
        let mut cmd = Command::new(binaries::bin(name));
        cmd.arg("create-chat").env_clear();
        for e in sandbox::environ(&[]) {
            if let Some((k, v)) = e.split_once('=') {
                cmd.env(k, v);
            }
        }
        let out = cmd.output().await?;
        if !out.status.success() {
            return Err(Error::Message(format!(
                "{name}: new session: {}",
                out.status
            )));
        }
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    } else {
        String::new()
    };
    let args = arguments(name, req, model, &session)?;
    let mut cmd = Command::new(binaries::bin(name));
    cmd.args(args).env_clear();
    let env = if req.env.is_empty() {
        sandbox::environ(&[])
    } else {
        req.env.clone()
    };
    for e in env {
        if let Some((k, v)) = e.split_once('=') {
            cmd.env(k, v);
        }
    }
    if matches!(name, "pi" | "claude") && !req.dir.is_empty() {
        cmd.current_dir(&req.dir);
    }
    let token = cancel.child_token();
    let timer = if !req.timeout.is_zero() {
        let t = token.clone();
        let d = req.timeout;
        Some(tokio::spawn(async move {
            tokio::time::sleep(d).await;
            t.cancel();
        }))
    } else {
        None
    };
    let output = run_command(
        cmd,
        (name == "claude").then(|| req.prompt.clone()),
        token,
        LiveLog::open(&req.log_path),
        matches!(name, "claude" | "pi").then_some(name),
    )
    .await;
    if let Some(timer) = timer {
        timer.abort();
    }
    let output = output?;
    if !output.fatal.is_empty() {
        return Err(Error::Message(format!(
            "{name}: stopped early: {}",
            output.fatal
        )));
    }
    let (text, parsed) = match name {
        "claude" => stream::parse_claude_stream(&output.stdout),
        "pi" => stream::parse_pi_stream(&output.stdout),
        "cursor" => stream::parse_cursor_stream(&output.stdout),
        "grok" => stream::parse_grok_stream(&output.stdout),
        _ => (output.stdout.trim().into(), String::new()),
    };
    if output.exit_code != 0 {
        return Err(Error::Message(format!(
            "{name}: exit status {}: {}",
            output.exit_code,
            output.stderr.trim()
        )));
    }
    if text.is_empty() && output.stderr.contains("auto-denied") {
        return Err(Error::Message(format!(
            "{name}: permission denied in headless mode: {}",
            output.stderr.trim()
        )));
    }
    Ok(RunResult {
        output: text,
        exit_code: output.exit_code,
        session_id: if parsed.is_empty() { session } else { parsed },
        usage: match name {
            "claude" => usage::usage_claude(&output.stdout),
            "pi" => usage::usage_pi(&output.stdout),
            "cursor" => usage::usage_cursor(&output.stdout),
            "grok" => usage::usage_grok(&output.stdout),
            _ => None,
        },
    })
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
