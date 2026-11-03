pub fn effort_of(model: &str) -> String {
    let Some((_, suffix)) = model.rsplit_once('-') else {
        return String::new();
    };
    let suffix = suffix.to_lowercase();
    if matches!(
        suffix.as_str(),
        "none" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max"
    ) {
        suffix
    } else {
        String::new()
    }
}

pub fn provider_of(agent: &str, model: &str) -> String {
    let m = model.to_lowercase();
    let provider = if m.contains("gemini") {
        "google"
    } else if m.contains("claude") {
        "anthropic"
    } else if m.contains("grok") {
        "xai"
    } else if m.contains("composer") {
        "cursor"
    } else if m.contains("gpt-")
        || m.starts_with("gpt")
        || m.contains("codex")
        || ["o1", "o3", "o4"].iter().any(|p| m.starts_with(p))
    {
        "openai"
    } else {
        match agent.to_lowercase().as_str() {
            "codex" => "openai",
            "grok" => "xai",
            "cursor" => "cursor",
            "antigravity" | "gemini" => "google",
            "claude" => "anthropic",
            "qwen" => "alibaba",
            "kimi" => "moonshotai",
            "copilot" => "github-copilot",
            "opencode" | "pi" => m.split_once('/').map_or("", |(p, _)| p),
            _ => "",
        }
    };
    provider.to_owned()
}

pub fn valid_effort(e: &str) -> bool {
    matches!(e, "low" | "medium" | "high" | "xhigh")
}
pub fn base_model(m: &str) -> &str {
    let effort = effort_of(m);
    if effort.is_empty() {
        m
    } else {
        m.strip_suffix(&format!("-{effort}")).unwrap_or(m)
    }
}
pub fn allowed_model(agent: &str, model: &str) -> bool {
    agent == "claude"
        || !["claude", "sonnet", "opus", "haiku", "fable"]
            .iter()
            .any(|p| model.to_lowercase().contains(p))
}
