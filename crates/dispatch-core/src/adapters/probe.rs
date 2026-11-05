use super::Capabilities;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlagCheck {
    #[serde(rename = "flag")]
    pub name: String,
    pub present: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub capability: String,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbeReport {
    pub name: String,
    pub binary: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub version: String,
    pub flags: Vec<FlagCheck>,
    pub capabilities: Capabilities,
    pub status: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
    pub installed: bool,
}
pub fn flag_present(help: &str, flag: &str) -> bool {
    if flag.is_empty() {
        return false;
    }
    let bytes = help.as_bytes();
    for (i, _) in help.match_indices(flag) {
        let left = i == 0
            || matches!(
                bytes[i - 1],
                b' ' | b'\n'
                    | b'\t'
                    | b'\r'
                    | b'['
                    | b']'
                    | b'('
                    | b')'
                    | b','
                    | b'|'
                    | b'='
                    | b':'
                    | b'<'
                    | b'>'
                    | b'"'
                    | b'\''
                    | b'`'
            );
        let end = i + flag.len();
        let right = end == bytes.len()
            || !(bytes[end].is_ascii_alphanumeric() || matches!(bytes[end], b'-' | b'_'));
        if left && right {
            return true;
        }
    }
    false
}

#[derive(Clone, Copy, Debug)]
pub struct ProbeFlag {
    pub name: &'static str,
    pub capability: &'static str,
    pub subcommand: &'static str,
}

fn flag(name: &'static str, capability: &'static str, subcommand: &'static str) -> ProbeFlag {
    ProbeFlag {
        name,
        capability,
        subcommand,
    }
}

pub fn capabilities(name: &str) -> Capabilities {
    let mut c = Capabilities {
        session_resume: true,
        structured_output: true,
        read_only: true,
        file_editing: true,
        usage: matches!(name, "codex" | "claude" | "antigravity"),
        ..Default::default()
    };
    if matches!(name, "codex" | "claude" | "pi") {
        c.effort_mode = "config".into();
        c.efforts = ["low", "medium", "high", "xhigh"]
            .into_iter()
            .map(str::to_owned)
            .collect();
        // Claude Code's `--effort` also takes max.
        if name == "claude" {
            c.efforts.push("max".into());
        }
    } else if name == "antigravity" {
        c.effort_mode = "model-suffix".into();
    }
    c
}

pub fn flags(name: &str) -> Vec<ProbeFlag> {
    let entries: &[(&str, &str, &str)] = match name {
        "codex" => &[
            ("exec", "", ""),
            ("resume", "sessionResume", "exec"),
            ("app-server", "usage", ""),
            ("--json", "structuredOutput", "exec"),
            ("--skip-git-repo-check", "", "exec"),
            ("-o", "", "exec"),
            ("-m", "", "exec"),
            ("-c", "readOnly", "exec"),
        ],
        "claude" => &[
            ("--print", "", ""),
            ("--output-format", "structuredOutput", ""),
            ("--include-partial-messages", "", ""),
            ("--verbose", "", ""),
            ("--permission-mode", "readOnly", ""),
            ("--resume", "sessionResume", ""),
            ("--model", "", ""),
            ("--effort", "", ""),
        ],
        "cursor" => &[
            ("-p", "", ""),
            ("--output-format", "structuredOutput", ""),
            ("--stream-partial-output", "", ""),
            ("--trust", "", ""),
            ("--mode", "readOnly", ""),
            ("--force", "fileEditing", ""),
            ("--sandbox", "", ""),
            ("--resume", "sessionResume", ""),
            ("--model", "", ""),
            ("--workspace", "", ""),
            ("create-chat", "", ""),
        ],
        "grok" => &[
            ("--output-format", "structuredOutput", ""),
            ("--permission-mode", "readOnly", ""),
            ("--resume", "sessionResume", ""),
            ("-s", "", ""),
            ("--model", "", ""),
            ("--cwd", "", ""),
            ("-p", "", ""),
        ],
        "pi" => &[
            ("--mode", "structuredOutput", ""),
            ("--print", "", ""),
            ("--session-id", "sessionResume", ""),
            ("--model", "", ""),
            ("--thinking", "", ""),
            ("--tools", "readOnly", ""),
        ],
        "antigravity" => &[
            ("--stdio", "structuredOutput", "acp"),
            ("--sandbox", "", "acp"),
        ],
        "gemini" | "qwen" => &[
            ("--acp", "structuredOutput", ""),
            ("--approval-mode", "readOnly", ""),
        ],
        "copilot" => &[("--acp", "structuredOutput", "")],
        "kimi" | "opencode" => &[("acp", "structuredOutput", "")],
        _ => &[],
    };
    entries.iter().map(|&(n, c, s)| flag(n, c, s)).collect()
}

async fn output(binary: &str, args: &[&str]) -> (String, Option<String>) {
    use std::os::unix::process::ExitStatusExt;
    use tokio::process::Command;
    let mut cmd = Command::new(binary);
    cmd.args(args)
        .env_clear()
        .kill_on_drop(true)
        .process_group(0);
    for item in crate::sandbox::environ(&[]) {
        if let Some((k, v)) = item.split_once('=') {
            cmd.env(k, v);
        }
    }
    cmd.stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => return (String::new(), Some(e.to_string())),
    };
    let pid = child.id();
    match tokio::time::timeout(std::time::Duration::from_secs(3), child.wait_with_output()).await {
        Err(_) => {
            if let Some(pid) = pid {
                let _ = nix::sys::signal::kill(
                    nix::unistd::Pid::from_raw(-(pid as i32)),
                    nix::sys::signal::Signal::SIGKILL,
                );
            }
            (String::new(), Some("timed out".into()))
        }
        Ok(Err(e)) => (String::new(), Some(e.to_string())),
        Ok(Ok(out)) => {
            let mut bytes = out.stdout;
            bytes.extend(out.stderr);
            bytes.truncate(256 * 1024);
            let err = if out.status.success() {
                None
            } else if let Some(code) = out.status.code() {
                Some(format!("exit status {code}"))
            } else {
                Some(format!(
                    "signal: {}",
                    out.status.signal().unwrap_or_default()
                ))
            };
            (String::from_utf8_lossy(&bytes).into_owned(), err)
        }
    }
}

pub async fn probe_binary(
    name: &str,
    binary: &str,
    caps: Capabilities,
    flags: &[ProbeFlag],
) -> ProbeReport {
    use std::collections::HashMap;
    let mut rep = ProbeReport {
        name: name.into(),
        binary: binary.into(),
        capabilities: caps,
        status: "UNVERIFIED".into(),
        ..Default::default()
    };
    // The CLI is asked for its version and each help it has at once: each answer is a process
    // start, which for a Node CLI takes a few hundred milliseconds, and a run waits for them.
    let mut subcommands: Vec<&str> = Vec::new();
    for f in flags {
        if !f.subcommand.is_empty() && !subcommands.contains(&f.subcommand) {
            subcommands.push(f.subcommand);
        }
    }
    let sub_args: Vec<[&str; 2]> = subcommands.iter().map(|s| [*s, "--help"]).collect();
    let ((version, version_err), (help, help_err), sub_helps) = tokio::join!(
        output(binary, &["--version"]),
        output(binary, &["--help"]),
        futures::future::join_all(sub_args.iter().map(|args| output(binary, args))),
    );
    let mut sub_helps: HashMap<&str, (String, Option<String>)> =
        subcommands.into_iter().zip(sub_helps).collect();
    if version_err
        .as_deref()
        .is_some_and(|e| e.contains("No such file or directory"))
    {
        rep.notes.push(format!("{binary} not on PATH"));
        return rep;
    }
    rep.installed = true;
    if let Ok(re) = regex::Regex::new(r"v?(\d+\.\d+(?:\.\d+)?)")
        && let Some(captures) = re.captures(&version)
        && let Some(v) = captures.get(1)
    {
        rep.version = v.as_str().into();
    }
    let version_ok = !rep.version.is_empty();
    if !version_ok {
        rep.notes.push(format!(
            "{binary} --version: {}",
            version_err.unwrap_or_else(|| "version unknown".into())
        ));
    }
    let mut help_ok = help_err.is_none() && !help.trim().is_empty();
    if !help_ok {
        rep.notes.push(format!(
            "{binary} --help: {}",
            help_err.unwrap_or_else(|| "no response".into())
        ));
    }
    let mut helps = HashMap::from([(String::new(), (help, help_ok))]);
    let mut missing = Vec::new();
    for f in flags {
        if !helps.contains_key(f.subcommand) {
            let (text, err) = sub_helps.remove(f.subcommand).unwrap_or_default();
            let ok = err.is_none() && !text.trim().is_empty();
            if !ok {
                rep.notes
                    .push(format!("{binary} {} --help: no response", f.subcommand));
                help_ok = false;
            }
            helps.insert(f.subcommand.into(), (text, ok));
        }
        if let Some((text, ok)) = helps.get(f.subcommand) {
            let present = *ok && flag_present(text, f.name);
            rep.flags.push(FlagCheck {
                name: f.name.into(),
                present,
                capability: f.capability.into(),
            });
            if *ok && !present {
                missing.push(f.name);
            }
        }
    }
    if !missing.is_empty() {
        rep.notes
            .push(format!("missing flags: {}", missing.join(", ")));
    }
    rep.status = if !version_ok || !help_ok {
        "UNVERIFIED"
    } else if !missing.is_empty() {
        "DEGRADED"
    } else {
        "OK"
    }
    .into();
    if rep.status == "OK" {
        rep.notes.clear();
    }
    for note in &mut rep.notes {
        *note = crate::sandbox::redact(
            &note
                .trim()
                .replace('\n', " ")
                .chars()
                .take(200)
                .collect::<String>(),
        );
    }
    rep
}

pub async fn probe(name: &str) -> ProbeReport {
    probe_binary(
        name,
        &super::binaries::bin(name),
        capabilities(name),
        &flags(name),
    )
    .await
}

/// Every agent CLI found, probed at once, in the order they were found.
pub async fn doctor() -> Vec<ProbeReport> {
    let agents: Vec<String> = super::binaries::detect()
        .into_iter()
        .filter(|d| matches!(d.source.as_str(), "PATH" | "configured"))
        .map(|d| d.agent)
        .collect();
    futures::future::join_all(agents.iter().map(|a| probe(a))).await
}

pub fn usage_source(name: &str) -> Option<&'static str> {
    match name {
        "codex" => Some(super::quota::SOURCE_CODEX_APP_SERVER),
        "antigravity" => Some(super::quota::SOURCE_AGY_USAGE),
        "claude" => Some(super::quota::SOURCE_CLAUDE_STREAM),
        _ => None,
    }
}
