//! The calling agent's own session: how much it used, read from that agent's local
//! transcript. Only numbers and model names leave this file; a session's prompts, replies,
//! commands, and attachment text are never read into a result, logged, or returned.
//!
//! Claude Code keeps one JSONL per session under `~/.claude/projects/<project>/`; Codex
//! keeps a rollout JSONL per thread under `~/.codex/sessions/`; OpenCode keeps its sessions
//! in a SQLite database under `~/.local/share/opencode`. Any other agent is reported as not
//! measured. The session id is validated before it names a file, so it can never leave the
//! folder its agent keeps.

use super::{CallerUsage, Slice};
use crate::{
    adapters::usage::Usage as AdapterUsage,
    catalog::{Catalog, catalog_key},
    reports::spend,
};
use rusqlite::{Connection, OpenFlags};
use serde_json::Value;
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    time::SystemTime,
};

/// The longest session id a transcript path may be built from.
const MAX_SESSION: usize = 128;

/// The deepest under `~/.codex/sessions` a rollout file lives (year/month/day/file).
const CODEX_DEPTH: usize = 6;

/// Measures the calling agent's own session inside `conversation_id` (`agent:session`).
pub fn measure(conversation_id: &str, catalog: &Catalog) -> CallerUsage {
    let Some((agent, session)) = conversation_id.split_once(':') else {
        let agent = conversation_id.trim();
        return CallerUsage {
            agent: agent.to_string(),
            measured: false,
            note: format!("Not measured for {agent}"),
            ..CallerUsage::default()
        };
    };
    let (agent, session) = (agent.trim(), session.trim());
    if !valid_session(session) {
        return CallerUsage {
            agent: agent.to_string(),
            session: session.to_string(),
            measured: false,
            note: format!("Not measured for {agent}: invalid session id"),
            ..CallerUsage::default()
        };
    }
    let mut out = CallerUsage {
        agent: agent.to_string(),
        session: session.to_string(),
        ..CallerUsage::default()
    };
    if !matches!(agent, "claude" | "claude-code" | "codex" | "opencode") {
        out.note = format!("Not measured for {agent}");
        return out;
    }
    let Some(home) = home() else {
        out.note = format!("Not measured for {agent}: no home folder");
        return out;
    };
    let (slices, note) = match agent {
        "claude" | "claude-code" => claude(&home, agent, session, catalog),
        "codex" => codex(&home, agent, session, catalog),
        _ => opencode(&home, agent, session),
    };
    out.slices = slices;
    out.measured = !out.slices.is_empty();
    out.note = note.unwrap_or_default();
    out
}

/// Whether `session` can name a transcript file: only letters, digits, `_`, and `-`, so it
/// can never climb out of the folder its agent keeps.
pub(crate) fn valid_session(session: &str) -> bool {
    !session.is_empty()
        && session.len() <= MAX_SESSION
        && session
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
        .or_else(dirs::home_dir)
}

/// The Claude Code session's transcript under `~/.claude/projects`, or
/// `$CLAUDE_CONFIG_DIR/projects` when that names an absolute folder.
fn claude_projects(home: &Path) -> PathBuf {
    std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .unwrap_or_else(|| home.join(".claude"))
        .join("projects")
}

fn claude(
    home: &Path,
    agent: &str,
    session: &str,
    catalog: &Catalog,
) -> (Vec<Slice>, Option<String>) {
    let folder = claude_projects(home);
    let file = format!("{session}.jsonl");
    let mut files = Vec::new();
    if let Ok(entries) = fs::read_dir(&folder) {
        for entry in entries.flatten() {
            let path = entry.path().join(&file);
            if path.is_file() {
                files.push(path);
            }
        }
    }
    let mut slices = Vec::new();
    for path in files {
        if let Ok(body) = fs::read_to_string(&path) {
            slices.extend(claude_slices(agent, &body, catalog));
        }
    }
    let note = slices
        .is_empty()
        .then(|| format!("Not measured for {agent}: no usage in the session transcript"));
    (super::sort_and_merge(slices), note)
}

/// Sums each assistant message's usage by model, counting a message id or request id once:
/// the CLI's transcript repeats the same message while it streams.
pub(crate) fn claude_slices(agent: &str, body: &str, catalog: &Catalog) -> Vec<Slice> {
    let mut seen = HashSet::new();
    let mut slices = Vec::new();
    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(event) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if event["type"] != "assistant" || !event["message"]["usage"].is_object() {
            continue;
        }
        let message = &event["message"];
        let key = [
            message["id"].as_str(),
            event["requestId"].as_str(),
            event["uuid"].as_str(),
        ]
        .into_iter()
        .flatten()
        .find(|id| !id.is_empty());
        if let Some(key) = key
            && !seen.insert(key.to_string())
        {
            continue;
        }
        let usage = &message["usage"];
        let model = message["model"].as_str().unwrap_or("").trim().to_string();
        let adapter = AdapterUsage {
            input_tokens: num(usage, "input_tokens"),
            cache_read_tokens: num(usage, "cache_read_input_tokens"),
            cache_write_tokens: num(usage, "cache_creation_input_tokens"),
            output_tokens: num(usage, "output_tokens"),
            reasoning_tokens: num(&usage["output_tokens_details"], "thinking_tokens"),
            context_tokens: context(usage),
            ..AdapterUsage::default()
        };
        slices.push(slice(agent, &model, &adapter, Some(catalog)));
    }
    slices
}

fn codex(
    home: &Path,
    agent: &str,
    session: &str,
    catalog: &Catalog,
) -> (Vec<Slice>, Option<String>) {
    let root = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .unwrap_or_else(|| home.join(".codex"))
        .join("sessions");
    let file = newest(&root, CODEX_DEPTH, &|name| {
        name.starts_with("rollout-") && name.contains(session)
    });
    let slices = file
        .and_then(|path| fs::read_to_string(path).ok())
        .map(|body| codex_slices(agent, &body, catalog))
        .unwrap_or_default();
    let note = slices
        .is_empty()
        .then(|| format!("Not measured for {agent}: no usage in the session transcript"));
    (slices, note)
}

/// Reads a Codex rollout's last cumulative token count. Codex counts cached and written
/// tokens inside `input_tokens`, so they are taken out of the input before they are priced.
pub(crate) fn codex_slices(agent: &str, body: &str, catalog: &Catalog) -> Vec<Slice> {
    let mut model = String::new();
    let mut tokens = None;
    let mut window = 0;
    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(event) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let payload = &event["payload"];
        if event["type"] == "turn_context" {
            if let Some(name) = payload["model"].as_str().filter(|m| !m.trim().is_empty()) {
                model = name.trim().to_string();
            }
        } else if event["type"] == "event_msg"
            && payload["type"] == "token_count"
            && payload["info"]["total_token_usage"].is_object()
        {
            tokens = Some(payload["info"]["total_token_usage"].clone());
            window = payload["info"]["model_context_window"]
                .as_i64()
                .unwrap_or(0);
        }
    }
    let Some(tokens) = tokens else {
        return Vec::new();
    };
    let (input, cached, write, output) = (
        num(&tokens, "input_tokens"),
        num(&tokens, "cached_input_tokens"),
        num(&tokens, "cache_write_input_tokens"),
        num(&tokens, "output_tokens"),
    );
    let adapter = AdapterUsage {
        input_tokens: (input - cached - write).max(0),
        cache_read_tokens: cached,
        cache_write_tokens: write,
        output_tokens: output,
        reasoning_tokens: num(&tokens, "reasoning_output_tokens"),
        context_tokens: input + output,
        context_window: window,
        ..AdapterUsage::default()
    };
    vec![slice(agent, &model, &adapter, Some(catalog))]
}

/// OpenCode's sessions are rows in its SQLite database, one assistant row per model call
/// with its tokens and the cost the CLI itself computed.
fn opencode(home: &Path, agent: &str, session: &str) -> (Vec<Slice>, Option<String>) {
    let data = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .unwrap_or_else(|| home.join(".local/share"));
    let path = data.join("opencode").join("opencode.db");
    if !path.is_file() {
        return (
            Vec::new(),
            Some(format!(
                "Not measured for {agent}: no OpenCode database found"
            )),
        );
    }
    let slices = read_opencode(&path, agent, session).unwrap_or_default();
    let note = slices
        .is_empty()
        .then(|| format!("Not measured for {agent}: no usage in the session"));
    (super::sort_and_merge(slices), note)
}

/// Reads one OpenCode session's assistant messages, read-only. Any failure is no result,
/// never a guess.
fn read_opencode(path: &Path, agent: &str, session: &str) -> Option<Vec<Slice>> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?;
    let mut stmt = conn
        .prepare("SELECT data FROM session_message WHERE session_id = ?1 AND type = 'assistant'")
        .ok()?;
    let rows = stmt
        .query_map([session], |row| row.get::<_, String>(0))
        .ok()?;
    let mut slices = Vec::new();
    for data in rows.flatten() {
        let Ok(message) = serde_json::from_str::<Value>(&data) else {
            continue;
        };
        let id = message["model"]["id"].as_str().unwrap_or("").trim();
        if id.is_empty() {
            continue;
        }
        let provider = message["model"]["providerID"].as_str().unwrap_or("");
        let model = if provider.trim().is_empty() {
            id.to_string()
        } else {
            format!("{}/{id}", provider.trim())
        };
        let tokens = &message["tokens"];
        let cache = &tokens["cache"];
        let adapter = AdapterUsage {
            input_tokens: num(tokens, "input"),
            cache_read_tokens: num(cache, "read"),
            cache_write_tokens: num(cache, "write"),
            output_tokens: num(tokens, "output"),
            reasoning_tokens: num(tokens, "reasoning"),
            context_tokens: num(tokens, "input") + num(cache, "read") + num(cache, "write"),
            ..AdapterUsage::default()
        };
        let mut slice = slice(agent, &model, &adapter, None);
        // OpenCode reports its own price for every call.
        slice.cost_usd = message["cost"].as_f64().filter(|cost| cost.is_finite());
        slices.push(slice);
    }
    Some(slices)
}

/// One message or session's slice: its tokens and, when the catalog (or the agent) prices
/// `model`, its cost.
fn slice(agent: &str, model: &str, usage: &AdapterUsage, catalog: Option<&Catalog>) -> Slice {
    let cost = catalog.and_then(|catalog| {
        (!model.is_empty())
            .then(|| spend::usage_cost(catalog, &catalog_key(model), usage))
            .flatten()
            .map(|(cost, _)| cost)
    });
    Slice {
        agent: agent.to_string(),
        model: model.to_string(),
        input_tokens: usage.input_tokens,
        cache_read_tokens: usage.cache_read_tokens,
        cache_write_tokens: usage.cache_write_tokens,
        output_tokens: usage.output_tokens,
        reasoning_tokens: usage.reasoning_tokens,
        cost_usd: cost,
        ..Slice::default()
    }
}

fn num(value: &Value, key: &str) -> i64 {
    value.get(key).and_then(Value::as_i64).unwrap_or(0)
}

fn context(usage: &Value) -> i64 {
    num(usage, "input_tokens")
        + num(usage, "cache_read_input_tokens")
        + num(usage, "cache_creation_input_tokens")
        + num(usage, "output_tokens")
}

/// The most recently changed file under `root` (searching `depth` levels) whose name passes.
fn newest(root: &Path, depth: usize, accept: &dyn Fn(&str) -> bool) -> Option<PathBuf> {
    let mut best: Option<(SystemTime, PathBuf)> = None;
    collect_newest(root, depth, accept, &mut best);
    best.map(|(_, path)| path)
}

fn collect_newest(
    root: &Path,
    depth: usize,
    accept: &dyn Fn(&str) -> bool,
    best: &mut Option<(SystemTime, PathBuf)>,
) {
    if depth == 0 {
        return;
    }
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            collect_newest(&entry.path(), depth - 1, accept, best);
        } else if kind.is_file() && accept(&name) {
            let changed = entry
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH);
            if best.as_ref().is_none_or(|(at, _)| changed > *at) {
                *best = Some((changed, entry.path()));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::Catalog;

    fn catalog() -> Catalog {
        Catalog::embedded().unwrap_or_default()
    }

    #[test]
    fn a_claude_transcript_counts_each_message_once_by_model() {
        let body = include_str!("../../tests/fixtures/usage/claude.jsonl");
        let slices = super::super::sort_and_merge(claude_slices("claude-code", body, &catalog()));
        // Two models, and the repeated message counted once.
        assert_eq!(slices.len(), 2);
        let fast = slices
            .iter()
            .find(|s| s.model == "claude-sonnet-5-5")
            .expect("sonnet");
        assert_eq!((fast.input_tokens, fast.output_tokens), (12, 5));
        assert_eq!((fast.cache_read_tokens, fast.cache_write_tokens), (40, 8));
        assert_eq!(fast.reasoning_tokens, 3);
        let heavy = slices
            .iter()
            .find(|s| s.model == "claude-opus-5-5")
            .expect("opus");
        assert_eq!(heavy.input_tokens, 10);
        // A malformed line is skipped, not fatal.
        assert!(slices.iter().all(|s| s.runs == 0));
    }

    #[test]
    fn a_codex_rollout_reads_its_last_total_and_takes_cached_out_of_input() {
        let body = include_str!("../../tests/fixtures/usage/codex.jsonl");
        let slices = codex_slices("codex", body, &catalog());
        assert_eq!(slices.len(), 1);
        let only = &slices[0];
        assert_eq!(only.model, "gpt-6.1-sol");
        assert_eq!(only.input_tokens, 44884);
        assert_eq!(only.cache_read_tokens, 36480);
        assert_eq!(only.cache_write_tokens, 0);
        assert_eq!(only.output_tokens, 86);
        assert_eq!(only.reasoning_tokens, 14);
    }

    #[test]
    fn a_session_id_never_leaves_its_folder() {
        assert!(valid_session("abc-123_XYZ"));
        assert!(!valid_session("../../etc/passwd"));
        assert!(!valid_session("a/b"));
        assert!(!valid_session(""));
        assert!(!valid_session(&"a".repeat(129)));
    }
}
