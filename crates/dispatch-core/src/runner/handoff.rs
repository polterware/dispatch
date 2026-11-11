use crate::{
    sandbox,
    types::{Attempt, Run},
    worktree,
};
use serde_json::{Map, Value};

/// Instruction prepended to the hand-off when the next candidate continues a
/// run after a usage-limit failure.
pub const HANDOFF_INSTRUCTION: &str = "A previous model stopped at its usage limit partway through this task. Its work is in this worktree. Continue from the hand-off below; do not redo finished work.";

/// Relative path in the worktree where the hand-off is written when `.dispatch/`
/// is already excluded there.
pub const HANDOFF_FILE: &str = ".dispatch/handoff.md";

/// Maximum character count kept for the failed attempt's last assistant messages.
pub const HANDOFF_MAX_CHARS: usize = 4_000;

/// Usage-limit percentage at which a running attempt stops cleanly so the model
/// can write its own hand-off before the account is exhausted.
pub const HANDOFF_NEAR_LIMIT_PERCENT: f64 = 95.0;

/// Default interval between live quota polls while an attempt runs.
pub const HANDOFF_QUOTA_POLL: std::time::Duration = std::time::Duration::from_secs(60);

/// Maximum time allowed for the resumed session to write its hand-off.
pub const HANDOFF_RESUME_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(180);

/// Exit class recorded when an attempt is stopped at [`HANDOFF_NEAR_LIMIT_PERCENT`].
pub const EXIT_NEAR_LIMIT: &str = "near_limit";

/// Prompt sent when resuming the stopped session to write its own hand-off.
pub const HANDOFF_MODEL_PROMPT: &str = "Your account is at 95% of its usage limit, so Dispatch is switching this task to the next model. Do not run tools or edit files. Reply with a concise hand-off in under 300 words covering: what is done, what is left, files touched, key decisions, and the immediate next step.";

/// Maximum character count kept for the failed attempt's todo/plan section.
const HANDOFF_PLAN_MAX_CHARS: usize = 2_000;

/// Reports whether an attempt exit class represents a usage or credit limit.
pub fn is_limit_class(exit_class: &str) -> bool {
    matches!(exit_class.trim(), "rate_limit" | "credits")
}

/// Bounds and redacts the model's own hand-off reply, returning `None` when
/// empty or when the reply is itself a usage-limit notice.
pub fn model_handoff(reply: &str) -> Option<String> {
    let trimmed = reply.trim();
    if trimmed.is_empty() || super::summary::limit_notice(trimmed).is_some() {
        return None;
    }
    let bounded = tail_chars(trimmed, HANDOFF_MAX_CHARS);
    let redacted = sandbox::redact(bounded.trim());
    (!redacted.trim().is_empty()).then_some(redacted)
}

/// Builds the prompt sent to the next candidate when continuing from a hand-off.
pub fn prompt_with_handoff(handoff: &str) -> String {
    format!("{HANDOFF_INSTRUCTION}\n\n{}", handoff.trim())
}

/// Builds the hand-off markdown for a usage-limit switch without making a model call.
pub fn build_handoff(
    rec: &Run,
    attempt: &Attempt,
    dir: &str,
    log_slice: &str,
    result_output: &str,
) -> String {
    let mut out = String::new();

    out.push_str("## Original task\n");
    out.push_str(rec.prompt.trim());
    out.push_str("\n\n");

    let agent_model = match (attempt.agent.trim(), attempt.model.trim()) {
        ("", "") => "agent".to_owned(),
        (a, "") => a.to_owned(),
        ("", m) => m.to_owned(),
        (a, m) => format!("{a}/{m}"),
    };
    let class = if attempt.exit_class.trim().is_empty() {
        "rate_limit"
    } else {
        attempt.exit_class.trim()
    };
    out.push_str("## Switched model\n");
    out.push_str(&format!("- Stopped: {agent_model} ({class})\n"));
    if !attempt.error.trim().is_empty() {
        out.push_str(&format!("- Limit: {}\n", attempt.error.trim()));
    }
    out.push('\n');

    let wt_dir = if !rec.worktree.is_empty() {
        rec.worktree.as_str()
    } else {
        dir
    };
    let base = if !rec.base_sha.is_empty() {
        rec.base_sha.as_str()
    } else {
        "HEAD"
    };
    out.push_str("## Worktree changes\n");
    let changes = if !rec.read_only && !wt_dir.is_empty() {
        worktree::file_changes(wt_dir, base).unwrap_or_default()
    } else {
        Vec::new()
    };
    if changes.is_empty() {
        out.push_str("No files changed.\n");
    } else {
        for change in &changes {
            if change.binary {
                out.push_str(&format!("- {} (binary)\n", change.path));
            } else {
                out.push_str(&format!(
                    "- {} (+{} -{})\n",
                    change.path, change.insertions, change.deletions
                ));
            }
        }
    }

    let digest = extract_stream_digest(log_slice, result_output, &attempt.error);
    if !digest.plan.is_empty() {
        out.push_str("\n## Todo / plan\n");
        out.push_str(&digest.plan);
        out.push('\n');
    }
    if !digest.messages.is_empty() {
        out.push_str("\n## Last assistant messages\n");
        out.push_str(&digest.messages);
        out.push('\n');
    }

    sandbox::redact(out.trim())
}

/// Writes `.dispatch/handoff.md` inside the worktree only when `.dispatch/` is
/// already excluded there.
pub fn write_worktree_handoff(rec: &Run, dir: &str, handoff: &str) {
    if rec.read_only || handoff.trim().is_empty() {
        return;
    }
    let wt = if !rec.worktree.is_empty() {
        rec.worktree.as_str()
    } else {
        dir
    };
    if wt.is_empty() || !worktree::is_excluded(wt, ".dispatch/") {
        return;
    }
    let path = std::path::Path::new(wt).join(HANDOFF_FILE);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, handoff);
}

#[derive(Default)]
struct StreamDigest {
    plan: String,
    messages: String,
}

fn extract_stream_digest(log_slice: &str, result_output: &str, limit_err: &str) -> StreamDigest {
    let mut plan = String::new();
    let mut plan_deltas = String::new();
    let mut messages: Vec<String> = Vec::new();
    let mut current_delta = String::new();
    let mut saw_json = false;

    let feed_source = if !log_slice.trim().is_empty() {
        log_slice
    } else {
        result_output
    };

    for raw_line in feed_source.lines() {
        let line = raw_line.trim();
        if !line.starts_with('{') {
            continue;
        }
        let Ok(Value::Object(ev)) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        saw_json = true;
        read_json_event(
            &ev,
            &mut plan,
            &mut plan_deltas,
            &mut messages,
            &mut current_delta,
        );
    }
    flush_delta(&mut messages, &mut current_delta);
    if plan.is_empty() && !plan_deltas.trim().is_empty() {
        plan = plan_deltas.trim().to_owned();
    }

    // If the log had no assistant messages (e.g., an adapter or fake only returned
    // `result_output` or plain text), inspect `result_output` / plain text.
    if messages.is_empty() && !result_output.trim().is_empty() {
        let out_trimmed = result_output.trim();
        let out_has_json = out_trimmed.lines().any(|l| {
            l.trim_start().starts_with('{') && serde_json::from_str::<Value>(l.trim()).is_ok()
        });
        if out_has_json && !log_slice.trim().is_empty() {
            for raw_line in out_trimmed.lines() {
                let line = raw_line.trim();
                if let Ok(Value::Object(ev)) = serde_json::from_str::<Value>(line) {
                    read_json_event(
                        &ev,
                        &mut plan,
                        &mut plan_deltas,
                        &mut messages,
                        &mut current_delta,
                    );
                }
            }
            flush_delta(&mut messages, &mut current_delta);
        } else if !out_has_json {
            let cleaned = clean_plain_messages(out_trimmed, limit_err);
            if !cleaned.is_empty() {
                messages.push(cleaned);
            }
        }
    } else if !saw_json && messages.is_empty() && !log_slice.trim().is_empty() {
        let cleaned = clean_plain_messages(log_slice.trim(), limit_err);
        if !cleaned.is_empty() {
            messages.push(cleaned);
        }
    }

    let limit_trimmed = limit_err.trim();
    messages.retain(|m| {
        let t = m.trim();
        !t.is_empty() && t != limit_trimmed && super::summary::limit_notice(t).is_none()
    });

    if plan.is_empty() {
        plan = extract_markdown_checklist(&messages.join("\n"));
        if plan.is_empty() && !saw_json {
            plan = extract_markdown_checklist(log_slice);
        }
    }

    let joined_messages = messages.join("\n\n");
    StreamDigest {
        plan: tail_chars(plan.trim(), HANDOFF_PLAN_MAX_CHARS),
        messages: tail_chars(joined_messages.trim(), HANDOFF_MAX_CHARS),
    }
}

fn flush_delta(messages: &mut Vec<String>, current_delta: &mut String) {
    let trimmed = current_delta.trim();
    if !trimmed.is_empty() && messages.last().map(|s| s.trim()) != Some(trimmed) {
        messages.push(trimmed.to_owned());
    }
    current_delta.clear();
}

fn push_complete_message(messages: &mut Vec<String>, current_delta: &mut String, text: &str) {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return;
    }
    // If deltas built up the same message, replace the delta buffer instead of duplicating.
    current_delta.clear();
    if messages.last().map(|s| s.trim()) != Some(trimmed) {
        messages.push(trimmed.to_owned());
    }
}

fn read_json_event(
    ev: &Map<String, Value>,
    plan: &mut String,
    plan_deltas: &mut String,
    messages: &mut Vec<String>,
    current_delta: &mut String,
) {
    if ev.contains_key("dispatch") {
        return;
    }
    if let Some(found) = extract_todos_from_map(ev) {
        *plan = found;
    }

    if let Some(method) = ev.get("method").and_then(Value::as_str) {
        let params = obj(ev.get("params"));
        if method == "session/update" {
            let update = obj(params.get("update"));
            if let Some(found) = extract_todos_from_map(update) {
                *plan = found;
            }
            match str_field(update, "sessionUpdate") {
                "agent_message_chunk" => {
                    let chunk = str_field(obj(update.get("content")), "text");
                    current_delta.push_str(chunk);
                }
                "plan" => {
                    if let Some(found) = format_todo_array(arr(update.get("entries"))) {
                        *plan = found;
                    }
                }
                _ => {}
            }
            return;
        }
        match method {
            "item/agentMessage/delta" => {
                current_delta.push_str(str_field(params, "delta"));
            }
            "item/plan/delta" => {
                plan_deltas.push_str(str_field(params, "delta"));
            }
            "item/started" | "item/completed" => {
                let item = obj(params.get("item"));
                let item_type = str_field(item, "type");
                if item_type == "agentMessage" && method == "item/completed" {
                    push_complete_message(messages, current_delta, str_field(item, "text"));
                } else if item_type == "plan" {
                    let text = item_plan_text(item);
                    if !text.is_empty() {
                        *plan = text;
                        plan_deltas.clear();
                    }
                } else if let Some(found) = extract_todos_from_map(item) {
                    *plan = found;
                }
            }
            _ => {}
        }
        return;
    }

    let type_name = str_field(ev, "type");
    match type_name {
        "stream_event" => {
            let event = obj(ev.get("event"));
            if str_field(event, "type") == "content_block_delta" {
                let delta = obj(event.get("delta"));
                if str_field(delta, "type") == "text_delta" {
                    current_delta.push_str(str_field(delta, "text"));
                }
            }
        }
        "assistant" => {
            let content = arr(obj(ev.get("message")).get("content"));
            let mut text_parts = String::new();
            for part in content.iter().filter_map(Value::as_object) {
                match str_field(part, "type") {
                    "text" => text_parts.push_str(str_field(part, "text")),
                    "tool_use" => {
                        if let Some(found) = extract_todos_from_map(obj(part.get("input"))) {
                            *plan = found;
                        }
                    }
                    _ => {}
                }
            }
            if ev.contains_key("timestamp_ms") {
                current_delta.push_str(&text_parts);
            } else {
                push_complete_message(messages, current_delta, &text_parts);
            }
        }
        "item.started" | "item.completed" => {
            let item = obj(ev.get("item"));
            let kind = {
                let k = str_field(item, "type");
                if k.is_empty() {
                    str_field(item, "item_type")
                } else {
                    k
                }
            };
            if (kind == "agent_message" || kind == "assistant_message")
                && type_name == "item.completed"
            {
                push_complete_message(messages, current_delta, str_field(item, "text"));
            } else if matches!(kind, "plan" | "todo_list" | "todo") {
                if let Some(found) = extract_todos_from_map(item) {
                    *plan = found;
                } else {
                    let t = item_plan_text(item);
                    if !t.is_empty() {
                        *plan = t;
                    }
                }
            }
        }
        "message_update" => {
            let event = obj(ev.get("assistantMessageEvent"));
            match str_field(event, "type") {
                "text_delta" => current_delta.push_str(str_field(event, "delta")),
                "text_end" => {
                    push_complete_message(messages, current_delta, str_field(event, "content"))
                }
                _ => {}
            }
        }
        "message_end" => {
            let msg = obj(ev.get("message"));
            if str_field(msg, "stopReason") != "error" {
                let mut text_parts = String::new();
                for part in arr(msg.get("content")).iter().filter_map(Value::as_object) {
                    if str_field(part, "type") == "text" {
                        text_parts.push_str(str_field(part, "text"));
                    }
                }
                push_complete_message(messages, current_delta, &text_parts);
            }
        }
        "text" => {
            if let Some(data) = ev.get("data").and_then(Value::as_str) {
                current_delta.push_str(data);
            } else if let Some(part) = ev.get("part").and_then(Value::as_object) {
                push_complete_message(messages, current_delta, str_field(part, "text"));
            }
        }
        "result" => {
            if ev.get("is_error") != Some(&Value::Bool(true)) {
                let res = str_field(ev, "result");
                if !res.is_empty() && messages.is_empty() && current_delta.trim().is_empty() {
                    push_complete_message(messages, current_delta, res);
                }
            }
        }
        _ => {}
    }
}

fn item_plan_text(item: &Map<String, Value>) -> String {
    let direct = str_field(item, "text").trim();
    if !direct.is_empty() {
        return direct.to_owned();
    }
    let parts = {
        let s = arr(item.get("summary"));
        if s.is_empty() {
            arr(item.get("content"))
        } else {
            s
        }
    };
    parts
        .iter()
        .filter_map(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn extract_todos_from_map(map: &Map<String, Value>) -> Option<String> {
    for key in ["todos", "plan", "entries", "items"] {
        if let Some(Value::Array(items)) = map.get(key)
            && let Some(formatted) = format_todo_array(items)
        {
            return Some(formatted);
        }
    }
    for sub in [
        "input",
        "args",
        "arguments",
        "rawInput",
        "state",
        "part",
        "tool_call",
    ] {
        if let Some(Value::Object(inner)) = map.get(sub)
            && let Some(found) = extract_todos_from_map(inner)
        {
            return Some(found);
        }
        if let Some(Value::String(raw_json)) = map.get(sub)
            && raw_json.trim_start().starts_with('{')
            && let Ok(Value::Object(parsed)) = serde_json::from_str::<Value>(raw_json)
            && let Some(found) = extract_todos_from_map(&parsed)
        {
            return Some(found);
        }
    }
    None
}

fn format_todo_array(items: &[Value]) -> Option<String> {
    if items.is_empty() {
        return None;
    }
    let mut lines = Vec::new();
    for item in items {
        let Some(obj) = item.as_object() else {
            continue;
        };
        let text = ["content", "step", "text", "title", "description"]
            .into_iter()
            .map(|k| str_field(obj, k).trim())
            .find(|s| !s.is_empty())?;
        let status = str_field(obj, "status");
        let completed = obj.get("completed") == Some(&Value::Bool(true))
            || status.eq_ignore_ascii_case("completed")
            || status.eq_ignore_ascii_case("done");
        let in_progress = status.eq_ignore_ascii_case("in_progress")
            || status.eq_ignore_ascii_case("in-progress");
        let mark = if completed {
            "- [x]"
        } else if in_progress {
            "- [~]"
        } else {
            "- [ ]"
        };
        lines.push(format!("{mark} {text}"));
    }
    (!lines.is_empty()).then(|| lines.join("\n"))
}

fn clean_plain_messages(text: &str, limit_err: &str) -> String {
    let limit_trimmed = limit_err.trim();
    let mut kept = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            if kept.last().is_some_and(|l: &&str| !l.is_empty()) {
                kept.push("");
            }
            continue;
        }
        if trimmed == limit_trimmed
            || super::summary::limit_notice(trimmed).is_some()
            || (trimmed.starts_with("── dispatch:") && trimmed.ends_with("──"))
            || trimmed.starts_with("$ ")
            || trimmed.starts_with("› ")
        {
            continue;
        }
        kept.push(line.trim_end());
    }
    kept.join("\n").trim().to_owned()
}

fn extract_markdown_checklist(text: &str) -> String {
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|line| {
            line.starts_with("- [ ] ")
                || line.starts_with("- [x] ")
                || line.starts_with("- [X] ")
                || line.starts_with("- [~] ")
                || line.starts_with("* [ ] ")
                || line.starts_with("* [x] ")
        })
        .collect();
    lines.join("\n")
}

fn tail_chars(text: &str, max_chars: usize) -> String {
    let count = text.chars().count();
    if count <= max_chars {
        return text.to_owned();
    }
    let skip = count - max_chars;
    let tail: String = text.chars().skip(skip).collect();
    format!("…{}", tail.trim_start())
}

fn obj(value: Option<&Value>) -> &Map<String, Value> {
    static EMPTY: std::sync::OnceLock<Map<String, Value>> = std::sync::OnceLock::new();
    value
        .and_then(Value::as_object)
        .unwrap_or_else(|| EMPTY.get_or_init(Map::new))
}

fn arr(value: Option<&Value>) -> &[Value] {
    value
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

fn str_field<'a>(map: &'a Map<String, Value>, key: &str) -> &'a str {
    map.get(key).and_then(Value::as_str).unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn extracts_assistant_messages_and_todos_without_raw_transcript() {
        let log = [
            json!({"type": "assistant", "message": {"content": [{"type": "thinking", "thinking": "secret internal reasoning"}]}}).to_string(),
            json!({"type": "item.completed", "item": {"id": "c1", "type": "command_execution", "command": "cat huge_raw_transcript.txt", "aggregated_output": "RAW_TRANSCRIPT_BODY_LINE_1\nRAW_TRANSCRIPT_BODY_LINE_2"}}).to_string(),
            json!({"type": "assistant", "message": {"content": [{"type": "tool_use", "id": "t1", "name": "TodoWrite", "input": {"todos": [
                {"content": "Edit extra.txt", "status": "completed"},
                {"content": "Verify build", "status": "in_progress"}
            ]}}]}}).to_string(),
            json!({"type": "assistant", "message": {"content": [{"type": "text", "text": "Updated extra.txt with the initial changes."}]}}).to_string(),
        ]
        .join("\n");

        let digest = extract_stream_digest(&log, "", "429 rate limit exceeded");
        assert_eq!(digest.plan, "- [x] Edit extra.txt\n- [~] Verify build");
        assert_eq!(
            digest.messages,
            "Updated extra.txt with the initial changes."
        );
        assert!(!digest.messages.contains("RAW_TRANSCRIPT_BODY"));
        assert!(!digest.messages.contains("secret internal reasoning"));
    }

    #[test]
    fn bounds_long_messages_to_tail_limit() {
        let long = "abcd".repeat(1500);
        let bounded = tail_chars(&long, HANDOFF_MAX_CHARS);
        assert!(bounded.starts_with('…'));
        assert!(bounded.chars().count() <= HANDOFF_MAX_CHARS + 1);
    }

    #[test]
    fn model_handoff_bounds_and_rejects_limit_notice() {
        assert!(model_handoff("   ").is_none());
        assert!(model_handoff("You've hit your usage limit. Try again in 2h.").is_none());
        let ok = model_handoff("Done: parser. Left: tests. Next: cargo test.").unwrap();
        assert!(ok.contains("Done: parser."));
    }
}
