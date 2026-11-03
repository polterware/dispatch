//! Small display helpers shared by the sidebar and the run page.

/// First non-empty line of a prompt, else the run id.
pub fn prompt_first_line(prompt: &str, fallback_id: &str) -> String {
    let first = prompt.split('\n').next().unwrap_or("").trim();
    if first.is_empty() {
        fallback_id.to_string()
    } else {
        first.to_string()
    }
}

pub fn non_empty_or(value: &str, fallback: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        fallback.to_string()
    } else {
        trimmed.to_string()
    }
}

pub fn agent_display_name(agent: &str) -> String {
    match agent.trim().to_ascii_lowercase().as_str() {
        "claude" | "claude-code" => "Claude".to_string(),
        "codex" => "Codex".to_string(),
        "cursor" => "Cursor".to_string(),
        "antigravity" => "Antigravity".to_string(),
        "gemini" => "Gemini".to_string(),
        "grok" => "Grok".to_string(),
        "opencode" => "OpenCode".to_string(),
        "qwen" => "Qwen".to_string(),
        "pi" => "Pi".to_string(),
        "kimi" => "Kimi".to_string(),
        "" => "Agent".to_string(),
        _ => agent.trim().to_string(),
    }
}

pub fn is_active_status(status: &str) -> bool {
    matches!(
        status.trim().to_ascii_lowercase().as_str(),
        "running" | "pending" | "queued" | "starting" | "started" | "active"
    )
}
