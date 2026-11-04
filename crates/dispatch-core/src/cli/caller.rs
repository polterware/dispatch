//! Which conversation started a run: the calling agent's session, read from
//! the environment its hook or CLI sets, else from the process ancestry.

use super::state_dir;
use crate::{Error, adapters::binaries::default_binaries, config};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

const ENV_CALLER_SESSION: &str = "DISPATCH_CALLER_SESSION";
const ENV_CALLER_AGENT: &str = "DISPATCH_CALLER_AGENT";

// Polterware's hook sets these for an agent session, so a run started from it
// joins that session's AgentRun.
const ENV_SESSION: &str = "POLTERWARE_SESSION";
const ENV_AGENT_LABEL: &str = "POLTERWARE_AGENT_LABEL";

const MAX_HOOK_INPUT: u64 = 1 << 20;

/// The calling conversation; empty when none is known.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Caller {
    pub agent: String,
    pub session_id: String,
    pub conversation_id: String,
    pub label: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Process {
    pub parent: i32,
    /// Start time in Unix seconds; zero when unknown.
    pub started: i64,
    pub args: Vec<String>,
}

pub(crate) fn resolve(db_path: &Path) -> Caller {
    let policy = config::load(&config::policy_config_path(&state_dir(db_path))).unwrap_or_default();
    let deadline = Instant::now() + Duration::from_secs(3);
    let lookup = |pid: i32| {
        if Instant::now() >= deadline {
            return None;
        }
        lookup_process(pid)
    };
    let parent = i32::try_from(std::os::unix::process::parent_id()).unwrap_or(0);
    resolve_caller(
        &|k| std::env::var(k).unwrap_or_default(),
        parent,
        &lookup,
        &policy.binaries,
        &policy.acp_agents,
    )
}

fn caller(agent: &str, session: &str) -> Caller {
    Caller {
        agent: agent.into(),
        session_id: session.into(),
        conversation_id: format!("{agent}:{session}"),
        label: agent.into(),
    }
}

pub(crate) fn resolve_caller(
    getenv: &dyn Fn(&str) -> String,
    parent: i32,
    lookup: &dyn Fn(i32) -> Option<Process>,
    binaries_cfg: &std::collections::BTreeMap<String, String>,
    acp_agents: &std::collections::BTreeMap<String, Vec<String>>,
) -> Caller {
    for (agent_key, session_key) in [
        (ENV_CALLER_AGENT, ENV_CALLER_SESSION),
        (ENV_AGENT_LABEL, ENV_SESSION),
    ] {
        let agent = getenv(agent_key).trim().to_owned();
        let session = getenv(session_key).trim().to_owned();
        if !agent.is_empty() && !session.is_empty() {
            return caller(&agent, &session);
        }
    }
    // Agents that export their own session id to the shells they run.
    for (agent, key) in [
        ("codex", "CODEX_THREAD_ID"),
        ("codex", "CODEX_SESSION_ID"),
        ("opencode", "OPENCODE_SESSION_ID"),
    ] {
        let session = getenv(key).trim().to_owned();
        if !session.is_empty() {
            return caller(agent, &session);
        }
    }
    let mut binaries: HashMap<String, String> = HashMap::from([("agent".into(), "cursor".into())]);
    let mut add = |agent: &str, args: &[String]| {
        let binary = caller_binary(args);
        if !binary.is_empty() {
            binaries.insert(binary, agent.to_owned());
        }
    };
    let mut defaults: Vec<(String, String)> = default_binaries().into_iter().collect();
    defaults.sort();
    for (agent, binary) in &defaults {
        add(agent, std::slice::from_ref(binary));
    }
    // Ordered maps make duplicate configured basenames deterministic.
    for (agent, binary) in binaries_cfg {
        add(agent, std::slice::from_ref(binary));
    }
    for (agent, command) in acp_agents {
        add(agent, command);
    }
    let mut seen = std::collections::HashSet::new();
    let mut pid = parent;
    while pid > 1 && seen.len() < 128 && seen.insert(pid) {
        let Some(process) = lookup(pid) else {
            break;
        };
        if let Some(agent) = binaries.get(&caller_binary(&process.args))
            && process.started != 0
        {
            return caller(agent, &format!("pid-{pid}-{}", process.started));
        }
        pid = process.parent;
    }
    Caller::default()
}

pub(crate) fn caller_binary(args: &[String]) -> String {
    let base = |s: &str| {
        Path::new(s)
            .file_name()
            .map_or_else(|| s.to_owned(), |n| n.to_string_lossy().into_owned())
    };
    let Some(first) = args.first() else {
        return String::new();
    };
    let binary = base(first);
    if matches!(
        binary.as_str(),
        "node" | "bun" | "deno" | "python" | "python3"
    ) {
        return args.get(1).map(|a| base(a)).unwrap_or_default();
    }
    binary
}

fn ps(pid: i32, format: &str) -> Option<String> {
    let out = Command::new("ps")
        .args(["-o", format, "-p", &pid.to_string()])
        .env("LC_ALL", "C")
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

fn lookup_process(pid: i32) -> Option<Process> {
    let mut process = parse_process(&ps(pid, "ppid=,lstart=,command=")?)?;
    // ps prints argv unquoted, so an executable under a path with spaces
    // ("Application Support") splits; comm holds the whole path on its line.
    if let Some(comm) = ps(pid, "comm=") {
        process.args = with_executable(process.args, comm.trim());
    }
    Some(process)
}

/// Puts the executable path back together as argv[0] when the unquoted
/// command line split it.
pub(crate) fn with_executable(args: Vec<String>, exe: &str) -> Vec<String> {
    if exe.is_empty() || args.is_empty() || !exe.contains(' ') {
        return args;
    }
    let joined = args.join(" ");
    let Some(rest) = joined.strip_prefix(exe) else {
        return args;
    };
    let mut out = vec![exe.to_owned()];
    out.extend(caller_args(rest));
    out
}

pub(crate) fn parse_process(output: &str) -> Option<Process> {
    let fields: Vec<&str> = output.split_whitespace().collect();
    if fields.len() < 7 {
        return None;
    }
    let parent = fields[0].parse().ok()?;
    let started =
        chrono::NaiveDateTime::parse_from_str(&fields[1..6].join(" "), "%a %b %e %H:%M:%S %Y")
            .ok()?
            .and_local_timezone(chrono::Local)
            .earliest()?
            .timestamp();
    Some(Process {
        parent,
        started,
        args: caller_args(&fields[6..].join(" ")),
    })
}

/// Reads quoted command words without evaluating any shell expressions.
pub(crate) fn caller_args(command: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut word = String::new();
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for c in command.chars() {
        if escaped {
            word.push(c);
            escaped = false;
        } else if c == '\\' && quote != Some('\'') {
            escaped = true;
        } else if let Some(q) = quote {
            if c == q {
                quote = None;
            } else {
                word.push(c);
            }
        } else if c == '\'' || c == '"' {
            quote = Some(c);
        } else if c == ' ' || c == '\t' {
            if !word.is_empty() {
                out.push(std::mem::take(&mut word));
            }
        } else {
            word.push(c);
        }
    }
    if escaped {
        word.push('\\');
    }
    if !word.is_empty() {
        out.push(word);
    }
    out
}

/// The caller name agent's runs carry: Claude Code's is `claude-code`, as the
/// sessions it starts have always been named.
pub(crate) fn hook_caller_agent(agent: &str) -> &str {
    match agent {
        "" | "claude" => "claude-code",
        agent => agent,
    }
}

/// Reads the session a SessionStart hook gets on stdin (`session_id`, or
/// Cursor's `conversation_id`) and, when the agent gives the hook an env file
/// (`$CLAUDE_ENV_FILE`), exports it there, so the runs that session starts
/// join it. It returns the session, empty when there is none.
pub(crate) fn record_hook_caller(
    input: &mut dyn Read,
    terminal: bool,
    env_file: &str,
    agent: &str,
) -> Result<String, Error> {
    if terminal {
        return Ok(String::new());
    }
    let mut data = Vec::new();
    if input
        .take(MAX_HOOK_INPUT + 1)
        .read_to_end(&mut data)
        .is_err()
        || data.len() as u64 > MAX_HOOK_INPUT
    {
        return Ok(String::new());
    }
    let session = serde_json::from_slice::<serde_json::Value>(&data)
        .ok()
        .and_then(|v| {
            ["session_id", "conversation_id", "sessionId"]
                .iter()
                .find_map(|key| v.get(key)?.as_str().map(str::to_owned))
        })
        .unwrap_or_default();
    let session = session.trim().to_owned();
    if session.is_empty() || env_file.is_empty() {
        return Ok(session);
    }
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .mode(0o600)
        .open(env_file)?;
    let quote = |v: &str| format!("'{}'", v.replace('\'', "'\\''"));
    write!(
        file,
        "export {ENV_CALLER_SESSION}={}\nexport {ENV_CALLER_AGENT}={}\n",
        quote(&session),
        quote(hook_caller_agent(agent))
    )?;
    Ok(session)
}

/// The caller variables for agent's session, for an agent that sets them
/// from the hook's answer (Cursor's `env`).
pub(crate) fn hook_caller_env(
    session: &str,
    agent: &str,
) -> serde_json::Map<String, serde_json::Value> {
    let mut env = serde_json::Map::new();
    if !session.is_empty() {
        env.insert(ENV_CALLER_SESSION.into(), session.into());
        env.insert(ENV_CALLER_AGENT.into(), hook_caller_agent(agent).into());
    }
    env
}

pub(crate) fn record_hook_stdin(agent: &str) -> (String, Result<(), Error>) {
    match record_hook_caller(
        &mut std::io::stdin().lock(),
        super::stdin_is_terminal(),
        &std::env::var("CLAUDE_ENV_FILE").unwrap_or_default(),
        agent,
    ) {
        Ok(session) => (session, Ok(())),
        Err(e) => (String::new(), Err(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> String + 'a {
        move |k| {
            pairs
                .iter()
                .find(|(key, _)| *key == k)
                .map(|(_, v)| (*v).to_owned())
                .unwrap_or_default()
        }
    }

    #[test]
    fn explicit_caller_wins() {
        let getenv = env(&[
            (ENV_CALLER_AGENT, "claude-code"),
            (ENV_CALLER_SESSION, "s1"),
            ("CODEX_THREAD_ID", "t1"),
        ]);
        let c = resolve_caller(&getenv, 0, &|_| None, &BTreeMap::new(), &BTreeMap::new());
        assert_eq!(c.conversation_id, "claude-code:s1");
    }

    #[test]
    fn codex_thread_is_a_caller() {
        let getenv = env(&[("CODEX_THREAD_ID", "t1")]);
        let c = resolve_caller(&getenv, 0, &|_| None, &BTreeMap::new(), &BTreeMap::new());
        assert_eq!(c, caller("codex", "t1"));
    }

    #[test]
    fn ancestry_finds_the_agent_binary() {
        let getenv = env(&[]);
        let lookup = |pid: i32| match pid {
            10 => Some(Process {
                parent: 9,
                started: 0,
                args: vec!["/bin/zsh".into()],
            }),
            9 => Some(Process {
                parent: 1,
                started: 1_700_000_000,
                args: vec!["node".into(), "/usr/local/bin/claude".into()],
            }),
            _ => None,
        };
        let c = resolve_caller(&getenv, 10, &lookup, &BTreeMap::new(), &BTreeMap::new());
        assert_eq!(c.conversation_id, "claude:pid-9-1700000000");
    }

    #[test]
    fn reads_quoted_words_and_split_executables() {
        assert_eq!(
            caller_args(r#"a 'b c' "d\"e" f\ g"#),
            ["a", "b c", "d\"e", "f g"]
        );
        let args = caller_args("/Users/x/Library/Application Support/bin/agent --x");
        assert_eq!(
            with_executable(args, "/Users/x/Library/Application Support/bin/agent"),
            ["/Users/x/Library/Application Support/bin/agent", "--x"]
        );
        let p = parse_process("  412 Mon Jan  2 15:04:05 2006 /usr/bin/codex exec\n").unwrap();
        assert_eq!(p.parent, 412);
        assert_ne!(p.started, 0);
        assert_eq!(p.args, ["/usr/bin/codex", "exec"]);
    }

    #[test]
    fn hook_exports_the_session() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("env");
        let mut input: &[u8] = br#"{"session_id":"it's"}"#;
        let session = record_hook_caller(&mut input, false, &file.to_string_lossy(), "claude");
        assert_eq!(session.unwrap(), "it's");
        let body = std::fs::read_to_string(&file).unwrap();
        assert_eq!(
            body,
            "export DISPATCH_CALLER_SESSION='it'\\''s'\nexport DISPATCH_CALLER_AGENT='claude-code'\n"
        );
    }

    #[test]
    fn hook_reads_cursors_conversation_and_names_its_agent() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("env");
        let mut input: &[u8] = br#"{"conversation_id":"c1","workspace_roots":["/w"]}"#;
        let session = record_hook_caller(&mut input, false, &file.to_string_lossy(), "grok");
        assert_eq!(session.unwrap(), "c1");
        let body = std::fs::read_to_string(&file).unwrap();
        assert!(
            body.ends_with("export DISPATCH_CALLER_AGENT='grok'\n"),
            "{body}"
        );
        let env = hook_caller_env("c1", "cursor");
        assert_eq!(env["DISPATCH_CALLER_SESSION"], "c1");
        assert_eq!(env["DISPATCH_CALLER_AGENT"], "cursor");
        assert!(hook_caller_env("", "cursor").is_empty());
    }
}
