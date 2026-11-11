//! Pure logic for Settings: agent setup, classifier keys, and routing policy.
//!
//! Parsers follow `agent-setup.tsx`, `policy-page.tsx`, and `mapDispatchAgentSetup`.
//! Key material is never copied into a display string.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use crate::client::{DispatchAgent, DispatchAgentModels, DispatchModelSetting};

pub const SAMPLE_TASK: &str = "Add pagination to the runs list API and its table, with tests";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstallState {
    Current,
    Outdated,
    Missing,
}

impl InstallState {
    pub fn from_wire(value: &str) -> Self {
        match value {
            "current" => Self::Current,
            "outdated" => Self::Outdated,
            _ => Self::Missing,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Current => "Current",
            Self::Outdated => "Outdated",
            Self::Missing => "Not installed",
        }
    }

    pub fn action_label(self) -> Option<&'static str> {
        match self {
            Self::Outdated => Some("Update"),
            Self::Missing => Some("Install"),
            Self::Current => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CliState {
    Linked,
    Other,
    Missing,
}

impl CliState {
    pub fn from_wire(value: &str) -> Self {
        match value {
            "linked" => Self::Linked,
            "other" => Self::Other,
            _ => Self::Missing,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Linked => "Linked",
            Self::Other => "Another copy",
            Self::Missing => "Not installed",
        }
    }

    pub fn action_label(self) -> Option<&'static str> {
        match self {
            Self::Other => Some("Replace"),
            Self::Missing => Some("Install"),
            Self::Linked => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CliInstall {
    pub path: String,
    pub state: CliState,
    pub target: String,
    pub binary: String,
}

/// The dispatch skill in one skills folder, and the agents that read it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkillRow {
    pub dir: String,
    pub path: String,
    pub agents: Vec<String>,
    pub state: InstallState,
    /// An agent's own folder the shared one already reaches, listed only while it holds a
    /// second copy, to remove it.
    pub extra: bool,
}

/// One agent's SessionStart hook.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HookSetup {
    pub agent: String,
    pub on: bool,
    pub settings: String,
    pub command: String,
    pub current: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentSetup {
    pub cli: CliInstall,
    pub skills: Vec<SkillRow>,
    pub hooks: Vec<HookSetup>,
    /// The agents whose config folder is on this machine.
    pub agents: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Policy {
    pub priority: String,
    pub discover: bool,
    pub roles: bool,
    pub classifier: String,
}

impl Policy {
    pub fn classifier_on(&self) -> bool {
        !self.classifier.is_empty()
    }

    /// Why picking a role would do nothing with this classifier, or `None` when it works.
    pub fn pick_blocked(&self) -> Option<&'static str> {
        if !self.classifier_on() {
            Some(
                "No classifier is set, so nothing picks a role yet: a run gets one only from --role. Choose a classifier in Classifier.",
            )
        } else if self.classifier == "openai" {
            Some(
                "OpenAI Decisions judges a task's difficulty but cannot pick a role, so Auto runs stay on General. Jev and the agent CLI model can pick one.",
            )
        } else {
            None
        }
    }
}

/// One installed agent's card on Settings > Models: every model its CLI lists, flat.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentModelGroup {
    pub agent: String,
    pub name: String,
    pub enabled: bool,
    pub models: Vec<DispatchModelSetting>,
}

/// Groups every installed agent's models for Settings > Models, one flat list per agent,
/// sorted by model name.
///
/// The agents keep the order `ListAgents` gives. An agent turned off reads its models off,
/// without changing the state each of them keeps: turning it back on restores them as they
/// were stored. With a query, a card whose agent and models hold nothing is left out.
pub fn agent_model_groups(
    agents: &[DispatchAgent],
    settings: &[DispatchAgentModels],
    query: &str,
) -> Vec<AgentModelGroup> {
    let q = query.trim().to_lowercase();
    agents
        .iter()
        .filter(|agent| agent.installed && !agent.name.is_empty())
        .filter_map(|agent| {
            let mut models = settings
                .iter()
                .find(|entry| entry.agent.eq_ignore_ascii_case(&agent.name))
                .map(|entry| entry.models.clone())
                .unwrap_or_default();
            models.sort_by(|a, b| {
                a.id.to_lowercase()
                    .cmp(&b.id.to_lowercase())
                    .then_with(|| a.id.cmp(&b.id))
            });
            let agent_matches = q.is_empty()
                || agent.name.to_lowercase().contains(&q)
                || agent_label(&agent.name).to_lowercase().contains(&q)
                || crate::format::agent_display_name(&agent.name)
                    .to_lowercase()
                    .contains(&q);
            if !agent_matches {
                models.retain(|model| model.id.to_lowercase().contains(&q));
            }
            if !agent_matches && models.is_empty() {
                return None;
            }
            let enabled = agent.enabled;
            if !enabled {
                for model in &mut models {
                    model.enabled = false;
                }
            }
            Some(AgentModelGroup {
                agent: agent.name.clone(),
                name: crate::format::agent_display_name(&agent.name),
                enabled,
                models,
            })
        })
        .collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListedModel {
    pub agent: String,
    pub model: String,
    pub provider: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CliModel {
    pub agent: String,
    pub model: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RoleAnswer {
    pub role: String,
    pub probs: Vec<(String, f64)>,
    pub unsupported: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ClassifyView {
    pub provider: String,
    pub model: String,
    pub tier: String,
    pub d: f64,
    pub probs: Vec<f64>,
    pub latency_ms: f64,
    pub role: Option<RoleAnswer>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClassifierChoice {
    pub value: &'static str,
    pub radio: &'static str,
    pub label: &'static str,
    pub detail: &'static str,
}

pub static CLASSIFIERS: [ClassifierChoice; 4] = [
    ClassifierChoice {
        value: "",
        radio: "off",
        label: "Off",
        detail: "Routes follow the tier table in order. Auto is unavailable.",
    },
    ClassifierChoice {
        value: "jev",
        radio: "jev",
        label: "Jev (TypeSafe)",
        detail: "Sends the task text, or the card it names, to TypeSafe: about 0.2 s and a fraction of a cent per run.",
    },
    ClassifierChoice {
        value: "openai",
        radio: "openai",
        label: "OpenAI Decisions",
        detail: "The same question, asked of OpenAI's Decisions API.",
    },
    ClassifierChoice {
        value: "cli",
        radio: "cli",
        label: "Agent CLI model",
        detail: "Asks a model through an installed agent CLI: the easy tier's first, or the one picked here. No key, but each run spends a few seconds and some of your quota.",
    },
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyedClassifier {
    pub provider: &'static str,
    pub env: &'static str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyChip {
    pub label: &'static str,
    pub ok: bool,
}

pub fn agent_label(agent: &str) -> String {
    match agent {
        "claude" => "Claude Code".to_string(),
        "cursor" => "Cursor".to_string(),
        "codex" => "Codex".to_string(),
        "grok" => "Grok".to_string(),
        "antigravity" => "Antigravity".to_string(),
        "opencode" => "OpenCode".to_string(),
        "gemini" => "Gemini CLI".to_string(),
        "qwen" => "Qwen Code".to_string(),
        "pi" => "Pi".to_string(),
        "kimi" => "Kimi Code".to_string(),
        "copilot" => "Copilot".to_string(),
        other => other.to_string(),
    }
}

pub fn folder_agents_label(agents: &[String]) -> String {
    agents
        .iter()
        .map(|agent| agent_label(agent))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The agents of a skills folder to name: those on this machine, or every reader when none
/// of them is.
pub fn skill_readers<'a>(skill: &'a SkillRow, present: &[String]) -> Vec<&'a String> {
    let here: Vec<&String> = skill
        .agents
        .iter()
        .filter(|agent| present.contains(agent))
        .collect();
    if here.is_empty() {
        skill.agents.iter().collect()
    } else {
        here
    }
}

/// Whether a folder Dispatch installs into lacks the current skill.
pub fn skills_need_install(skills: &[SkillRow]) -> bool {
    skills
        .iter()
        .any(|skill| !skill.extra && skill.state != InstallState::Current)
}

pub fn cli_description(cli: &CliInstall) -> String {
    if cli.state == CliState::Other {
        format!("{} is another copy ({}).", cli.path, cli.target)
    } else {
        cli.path.clone()
    }
}

pub fn hook_points_elsewhere(hook: &HookSetup) -> bool {
    hook.on && !hook.current
}

/// What an agent asks of its hook, said under the file it lives in.
pub fn hook_note(agent: &str) -> Option<&'static str> {
    match agent {
        "codex" => Some("Codex asks you to trust it once in /hooks."),
        "grok" => Some("Grok also runs Claude Code's and Cursor's hooks; it reads the line once."),
        _ => None,
    }
}

pub fn parse_agent_setup(json: &str) -> AgentSetup {
    let root = json_object(json);
    let cli = record(root.get("cli"));
    let items = |key: &str| -> Vec<Map<String, Value>> {
        root.get(key)
            .and_then(Value::as_array)
            .map(|items| items.iter().map(record_of).collect())
            .unwrap_or_default()
    };
    let strings = |row: &Map<String, Value>, key: &str| -> Vec<String> {
        row.get(key)
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    };
    let mut skills: Vec<SkillRow> = Vec::new();
    for row in items("skills") {
        let path = text(&row, "path");
        let state = InstallState::from_wire(&text(&row, "state"));
        if row.contains_key("agents") {
            skills.push(SkillRow {
                dir: text(&row, "dir"),
                path,
                agents: strings(&row, "agents"),
                state,
                extra: flag_true(&row, "extra"),
            });
            continue;
        }
        // A core from before skills folders lists one row per agent: a folder's agents
        // come together, in first-seen order.
        let agent = text(&row, "agent");
        if let Some(folder) = skills.iter_mut().find(|folder| folder.path == path) {
            folder.agents.push(agent);
            continue;
        }
        let dir = std::path::Path::new(&path)
            .parent()
            .and_then(std::path::Path::parent)
            .map(|dir| dir.to_string_lossy().into_owned())
            .unwrap_or_default();
        skills.push(SkillRow {
            dir,
            path,
            agents: vec![agent],
            state,
            extra: false,
        });
    }
    let hook = |row: &Map<String, Value>, agent: String| HookSetup {
        agent,
        on: flag_true(row, "on"),
        settings: text(row, "settings"),
        command: text(row, "command"),
        current: flag_true(row, "current"),
    };
    let mut hooks: Vec<HookSetup> = items("hooks")
        .iter()
        .map(|row| hook(row, text(row, "agent")))
        .collect();
    if hooks.is_empty()
        && let Some(Value::Object(old)) = root.get("hook")
    {
        // A core from before per-agent hooks reports Claude Code's alone.
        hooks.push(hook(old, "claude".to_string()));
    }
    AgentSetup {
        cli: CliInstall {
            path: text(&cli, "path"),
            state: CliState::from_wire(&text(&cli, "state")),
            target: text(&cli, "target"),
            binary: text(&cli, "binary"),
        },
        skills,
        hooks,
        agents: strings(&root, "agents"),
    }
}

pub fn parse_policy(json: &str) -> Policy {
    let body = json_object(json);
    Policy {
        priority: body
            .get("priority")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        discover: flag_not_false(&body, "discover"),
        roles: flag_not_false(&body, "roles"),
        classifier: body
            .get("classifier")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
    }
}

pub fn radio_value(classifier: &str) -> &'static str {
    match classifier {
        "jev" => "jev",
        "openai" => "openai",
        "cli" => "cli",
        _ => "off",
    }
}

pub fn keyed_classifier(value: &str) -> Option<KeyedClassifier> {
    match value {
        "jev" => Some(KeyedClassifier {
            provider: "TypeSafe",
            env: "TYPESAFE_API_KEY",
        }),
        "openai" => Some(KeyedClassifier {
            provider: "OpenAI",
            env: "OPENAI_API_KEY",
        }),
        _ => None,
    }
}

pub fn env_key_sentence(env: &str) -> String {
    format!("{env} is set, and it wins over a saved key.")
}

pub fn key_placeholder(status: &str) -> &'static str {
    if status == "saved" {
        "Paste a new key"
    } else {
        "Paste a key"
    }
}

pub fn key_uses_replace(status: &str) -> bool {
    status == "saved"
}

/// Test is disabled only while the core reports the key missing.
pub fn key_test_enabled(status: &str) -> bool {
    status != "missing"
}

pub fn draft_ready(draft: &str) -> bool {
    !draft.trim().is_empty()
}

pub fn trimmed_draft(draft: &str) -> String {
    draft.trim().to_string()
}

/// Chip copy for a key source. The returned label is never the stored secret.
pub fn key_chip(status: &str) -> KeyChip {
    match status {
        "saved" => KeyChip {
            label: "Saved",
            ok: true,
        },
        "env" => KeyChip {
            label: "Environment",
            ok: true,
        },
        _ => KeyChip {
            label: "Not set",
            ok: false,
        },
    }
}

/// Status from `key-set` / `key-clear`. Other fields, including any `value`, are dropped.
pub fn parse_key_status_value(json: &str) -> String {
    let body = json_object(json);
    match body.get("status") {
        Some(Value::String(status)) => status.clone(),
        _ => String::new(),
    }
}

pub fn parse_key_map(json: &str) -> BTreeMap<String, String> {
    json_object(json)
        .into_iter()
        .filter_map(|(key, value)| value.as_str().map(|status| (key, status.to_string())))
        .collect()
}

pub fn parse_cli_model(json: &str) -> CliModel {
    let body = json_object(json);
    CliModel {
        agent: text(&body, "agent"),
        model: text(&body, "model"),
    }
}

pub fn classifier_trailing(
    value: &str,
    picked_radio: &str,
    status: &str,
    cli: Option<&CliModel>,
) -> Option<String> {
    let radio = radio_value(value);
    if radio == picked_radio {
        return None;
    }
    if keyed_classifier(value).is_some() {
        if status == "missing" {
            return Some("Needs a key".to_string());
        }
        return None;
    }
    if value == "cli" {
        let agent = cli.map(|item| item.agent.as_str()).unwrap_or("");
        if agent.is_empty() {
            return None;
        }
        let model = cli.map(|item| item.model.as_str()).unwrap_or("");
        if model.is_empty() {
            Some(agent.to_string())
        } else {
            Some(format!("{agent} · {model}"))
        }
    } else {
        None
    }
}

pub fn role_answer_line(answer: &RoleAnswer) -> String {
    if answer.unsupported {
        return "This classifier does not pick a role: runs stay general.".to_string();
    }
    let mut probs = answer.probs.clone();
    probs.sort_by(|left, right| {
        right
            .1
            .partial_cmp(&left.1)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let probs_text = probs
        .iter()
        .map(|(name, probability)| format!("{name} {}", fixed2(*probability)))
        .collect::<Vec<_>>()
        .join(" · ");
    let head = if answer.role == "general" {
        "General: no role fits clearly".to_string()
    } else {
        format!("Picks {}", answer.role)
    };
    if probs_text.is_empty() {
        head
    } else {
        format!("{head} · {probs_text}")
    }
}

pub fn parse_role_answer(value: &Value) -> RoleAnswer {
    let body = record_of(value);
    let probs = body
        .get("probs")
        .and_then(Value::as_object)
        .map(|entries| {
            let mut pairs = Vec::new();
            for (name, probability) in entries {
                if let Some(number) = probability.as_f64() {
                    pairs.push((name.clone(), number));
                }
            }
            pairs
        })
        .unwrap_or_default();
    let role = body
        .get("role")
        .and_then(Value::as_str)
        .unwrap_or("general")
        .to_string();
    RoleAnswer {
        role,
        probs,
        unsupported: matches!(body.get("unsupported"), Some(Value::Bool(true))),
    }
}

pub fn parse_classify(json: &str) -> ClassifyView {
    let body = json_object(json);
    let difficulty = record(body.get("difficulty"));
    let probs = difficulty
        .get("probs")
        .and_then(Value::as_array)
        .and_then(|items| {
            if items.len() != 3 {
                return None;
            }
            let numbers: Vec<f64> = items.iter().filter_map(Value::as_f64).collect();
            if numbers.len() == 3 {
                Some(numbers)
            } else {
                None
            }
        })
        .unwrap_or_default();
    let role = body
        .get("role")
        .filter(|value| !value.is_null() && value.as_bool() != Some(false))
        .map(parse_role_answer);
    ClassifyView {
        provider: text(&difficulty, "provider"),
        model: text(&difficulty, "model"),
        tier: text(&difficulty, "tier"),
        d: number_or(difficulty.get("d"), 0.0),
        probs,
        latency_ms: number_or(difficulty.get("latencyMs"), 0.0),
        role,
    }
}

/// The agent that offers `model`: the first listed model with that name.
pub fn model_agent<'a>(models: &'a [ListedModel], model: &str) -> Option<&'a str> {
    models
        .iter()
        .find(|item| item.model == model)
        .map(|item| item.agent.as_str())
}

pub fn priority_label(value: &str) -> &'static str {
    match value {
        "speed" => "Speed",
        "quality" => "Quality",
        _ => "Balanced",
    }
}

pub fn priority_value(value: &str) -> &'static str {
    match value {
        "speed" => "speed",
        "quality" => "quality",
        _ => "balanced",
    }
}

pub fn fixed2(value: f64) -> String {
    format!("{value:.2}")
}

pub fn latency_text(value: f64) -> String {
    if value.is_finite() && value.fract() == 0.0 {
        format!("{} ms", value as i64)
    } else if value.is_finite() {
        format!("{value} ms")
    } else {
        "0 ms".to_string()
    }
}

pub fn prob_line(probs: &[f64]) -> Option<String> {
    if probs.len() != 3 {
        return None;
    }
    Some(format!(
        "Easy {} · Normal {} · Hard {}",
        fixed2(probs[0]),
        fixed2(probs[1]),
        fixed2(probs[2])
    ))
}

pub fn classify_meta(view: &ClassifyView) -> String {
    let model = if view.model.is_empty() {
        String::new()
    } else {
        format!(" · {}", view.model)
    };
    format!(
        "{}{} · {}",
        view.provider,
        model,
        latency_text(view.latency_ms)
    )
}

pub fn bool_wire(value: bool) -> String {
    if value { "true" } else { "false" }.to_string()
}

fn json_value(json: &str) -> Value {
    serde_json::from_str(json).unwrap_or(Value::Null)
}

fn json_object(json: &str) -> Map<String, Value> {
    match json_value(json) {
        Value::Object(map) => map,
        _ => Map::new(),
    }
}

fn record(value: Option<&Value>) -> Map<String, Value> {
    value.map(record_of).unwrap_or_default()
}

fn record_of(value: &Value) -> Map<String, Value> {
    match value {
        Value::Object(map) => map.clone(),
        _ => Map::new(),
    }
}

fn text(map: &Map<String, Value>, key: &str) -> String {
    map.get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

fn field_string(map: &Map<String, Value>, pascal: &str, camel: &str) -> String {
    map.get(pascal)
        .or_else(|| map.get(camel))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

fn flag_true(map: &Map<String, Value>, key: &str) -> bool {
    matches!(map.get(key), Some(Value::Bool(true)))
}

fn flag_not_false(map: &Map<String, Value>, key: &str) -> bool {
    !matches!(map.get(key), Some(Value::Bool(false)))
}

fn number_or(value: Option<&Value>, default: f64) -> f64 {
    value.and_then(Value::as_f64).unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use super::{
        AgentSetup, CliModel, CliState, HookSetup, InstallState, ListedModel, agent_model_groups,
        classifier_trailing, fixed2, folder_agents_label, hook_note, hook_points_elsewhere,
        key_chip, model_agent, parse_agent_setup, parse_classify, parse_key_status_value,
        parse_policy, parse_role_answer, role_answer_line, skill_readers, skills_need_install,
    };
    use crate::client::{DispatchAgent, DispatchAgentModels, DispatchModelSetting};
    use serde_json::json;

    #[test]
    fn a_model_leads_with_the_first_agent_that_offers_it() {
        let listed = |agent: &str, model: &str| ListedModel {
            agent: agent.to_string(),
            model: model.to_string(),
            provider: String::new(),
        };
        let models = [
            listed("cursor", "grok-4.5"),
            listed("codex", "gpt-6"),
            listed("opencode", "gpt-6"),
        ];
        assert_eq!(model_agent(&models, "gpt-6"), Some("codex"));
        assert_eq!(model_agent(&models, "grok-4.5"), Some("cursor"));
        assert_eq!(model_agent(&models, "missing"), None);
    }

    #[test]
    fn says_when_no_classifier_can_pick_a_role() {
        let blocked = |classifier: &str| {
            parse_policy(&json!({ "classifier": classifier }).to_string()).pick_blocked()
        };
        assert!(blocked("").unwrap().contains("No classifier is set"));
        assert!(blocked("openai").unwrap().contains("stay on General"));
        assert_eq!(blocked("jev"), None);
    }

    #[test]
    fn a_skills_folder_names_the_agents_here_that_read_it() {
        let view = parse_agent_setup(
            r#"{"agents":["codex","grok"],"skills":[{"dir":"/h/.agents/skills","path":"/h/.agents/skills/dispatch/SKILL.md","agents":["codex","cursor","grok"],"state":"current"},{"dir":"/h/.cursor/skills","path":"/h/.cursor/skills/dispatch/SKILL.md","agents":["cursor"],"state":"outdated","extra":true}]}"#,
        );
        let shared = &view.skills[0];
        let extra = &view.skills[1];
        assert!(!shared.extra && extra.extra);
        assert_eq!(extra.state, InstallState::Outdated);
        let names = |skill| {
            let readers: Vec<String> = skill_readers(skill, &view.agents)
                .into_iter()
                .cloned()
                .collect();
            folder_agents_label(&readers)
        };
        assert_eq!(names(shared), "Codex, Grok", "only the agents here");
        assert_eq!(names(extra), "Cursor", "every reader when none is here");
        assert!(
            !skills_need_install(&view.skills),
            "a second copy never asks to be installed"
        );
    }

    #[test]
    fn agent_setup_reads_an_older_cores_rows() {
        let view = parse_agent_setup(
            r#"{"cli":{"path":"/h/.local/bin/dispatch","state":"other","target":"/h/.local/bin/dispatch","binary":"/app/dispatch"},"skills":[{"agent":"codex","path":"/h/.agents/skills/dispatch/SKILL.md","state":"outdated"},{"agent":"grok","path":"/h/.agents/skills/dispatch/SKILL.md","state":"outdated"},{"agent":"claude","path":"/c/dispatch/SKILL.md","state":"weird"}],"hook":{"on":true,"settings":"/h/.claude/settings.json","command":"dispatch hook","current":false}}"#,
        );
        assert_eq!(view.cli.state, CliState::Other);
        assert_eq!(
            view.skills
                .iter()
                .map(|skill| (skill.dir.as_str(), skill.agents.len(), skill.state))
                .collect::<Vec<_>>(),
            vec![
                ("/h/.agents/skills", 2, InstallState::Outdated),
                ("/c", 1, InstallState::Missing)
            ]
        );
        assert_eq!(
            view.hooks,
            vec![HookSetup {
                agent: "claude".into(),
                on: true,
                settings: "/h/.claude/settings.json".into(),
                command: "dispatch hook".into(),
                current: false,
            }]
        );
        assert!(hook_points_elsewhere(&view.hooks[0]));
    }

    #[test]
    fn agent_setup_reads_a_hook_per_agent() {
        let view = parse_agent_setup(
            r#"{"hooks":[{"agent":"claude","on":false,"settings":"/h/.claude/settings.json","current":false},{"agent":"codex","on":true,"settings":"/h/.codex/hooks.json","command":"/a/dispatch hook --agent codex","current":true}]}"#,
        );
        let agents: Vec<(&str, bool)> = view
            .hooks
            .iter()
            .map(|hook| (hook.agent.as_str(), hook.on))
            .collect();
        assert_eq!(agents, vec![("claude", false), ("codex", true)]);
        assert!(hook_note(&view.hooks[1].agent).is_some_and(|note| note.contains("/hooks")));
        assert_eq!(hook_note(&view.hooks[0].agent), None);
    }

    #[test]
    fn agent_setup_reads_an_empty_report() {
        let empty = parse_agent_setup("{}");
        assert_eq!(empty.cli.state, CliState::Missing);
        assert!(empty.skills.is_empty());
        assert!(empty.hooks.is_empty());
        assert!(empty.agents.is_empty());
        let _typed: AgentSetup = empty;
    }

    #[test]
    fn role_answer_line_reads_a_pick_most_likely_first() {
        let picked = parse_role_answer(&json!({
            "role": "reviewer",
            "probs": { "general": 0.1, "reviewer": 0.9 },
            "unsupported": false
        }));
        assert_eq!(
            role_answer_line(&picked),
            "Picks reviewer · reviewer 0.90 · general 0.10"
        );
        let general = parse_role_answer(&json!({ "role": "general", "probs": {} }));
        assert_eq!(role_answer_line(&general), "General: no role fits clearly");
        let unsupported = parse_role_answer(&json!({ "unsupported": true }));
        assert!(role_answer_line(&unsupported).contains("does not pick"));
    }

    #[test]
    fn discovery_and_roles_default_on_unless_explicitly_off() {
        for json in ["{}", r#"{"discover":true}"#, r#"{"roles":true}"#] {
            let policy = parse_policy(json);
            assert!(policy.discover, "{json}");
            assert!(policy.roles, "{json}");
        }
        let off = parse_policy(
            r#"{"discover":false,"roles":false,"priority":"speed","classifier":"jev"}"#,
        );
        assert!(!off.discover);
        assert!(!off.roles);
        assert_eq!(off.priority, "speed");
        assert_eq!(off.classifier, "jev");
        assert!(parse_policy(r#"{"discover":"false"}"#).discover);
    }

    #[test]
    fn classifier_trailing_matches_the_unpicked_rows() {
        let cli = CliModel {
            agent: "codex".into(),
            model: "gpt-5-codex-mini".into(),
        };
        assert_eq!(classifier_trailing("", "off", "missing", Some(&cli)), None);
        assert_eq!(
            classifier_trailing("jev", "off", "missing", Some(&cli)).as_deref(),
            Some("Needs a key")
        );
        assert_eq!(
            classifier_trailing("openai", "off", "saved", Some(&cli)),
            None
        );
        assert_eq!(
            classifier_trailing("cli", "off", "missing", Some(&cli)).as_deref(),
            Some("codex · gpt-5-codex-mini")
        );
        assert_eq!(
            classifier_trailing("jev", "jev", "missing", Some(&cli)),
            None
        );
        assert_eq!(
            classifier_trailing("cli", "cli", "missing", Some(&cli)),
            None
        );
        assert_eq!(classifier_trailing("cli", "off", "missing", None), None);
    }

    #[test]
    fn key_status_never_keeps_or_displays_the_secret() {
        let status =
            parse_key_status_value(r#"{"key":"jev","status":"saved","value":"sk-secret-value"}"#);
        assert_eq!(status, "saved");
        assert!(!status.contains("sk-secret"));
        for secret in ["sk-live", "sk-typesafe", "super-secret-key", ""] {
            let chip = key_chip(secret);
            assert!(matches!(chip.label, "Saved" | "Environment" | "Not set"));
            assert_ne!(chip.label, secret);
        }
        assert_eq!(key_chip("saved").label, "Saved");
        assert_eq!(key_chip("env").label, "Environment");
        assert_eq!(key_chip("missing").label, "Not set");
        assert!(!key_chip("missing").ok);
    }

    #[test]
    fn classify_report_reads_probs_and_an_unsupported_role() {
        let view = parse_classify(
            r#"{"difficulty":{"provider":"jev","model":"","tier":"normal","d":-1.8,"probs":[0.2,0.6,0.2],"latencyMs":210},"role":{"role":"general","probs":{},"unsupported":true}}"#,
        );
        assert_eq!(view.provider, "jev");
        assert_eq!(view.tier, "normal");
        assert_eq!(fixed2(view.d), "-1.80");
        assert_eq!(view.probs, vec![0.2, 0.6, 0.2]);
        assert_eq!(view.latency_ms, 210.0);
        assert!(view.role.as_ref().unwrap().unsupported);
        let bare = parse_classify(r#"{"difficulty":{"tier":"easy"}}"#);
        assert!(bare.probs.is_empty());
        assert!(bare.role.is_none());
    }

    fn agent(name: &str, installed: bool, enabled: bool) -> DispatchAgent {
        DispatchAgent {
            name: name.into(),
            installed,
            enabled,
            ..Default::default()
        }
    }

    fn model(id: &str, enabled: bool) -> DispatchModelSetting {
        DispatchModelSetting {
            id: id.into(),
            enabled,
            agents: Vec::new(),
        }
    }

    fn settings(
        agent: &str,
        enabled: bool,
        models: Vec<DispatchModelSetting>,
    ) -> DispatchAgentModels {
        DispatchAgentModels {
            agent: agent.into(),
            enabled,
            models,
        }
    }

    #[test]
    fn an_agents_card_lists_its_models_flat_and_sorted() {
        let agents = [agent("fake", true, true), agent("claude", false, true)];
        let settings = vec![
            settings(
                "fake",
                true,
                vec![model("fake-slow", true), model("fake-fast", false)],
            ),
            settings("claude", true, vec![model("claude-opus", true)]),
        ];
        let groups = agent_model_groups(&agents, &settings, "");
        assert_eq!(groups.len(), 1, "an agent not installed has no card");
        assert_eq!(groups[0].agent, "fake");
        assert_eq!(groups[0].name, "fake");
        assert!(groups[0].enabled);
        assert_eq!(
            groups[0]
                .models
                .iter()
                .map(|model| (model.id.as_str(), model.enabled))
                .collect::<Vec<_>>(),
            vec![("fake-fast", false), ("fake-slow", true)]
        );
    }

    #[test]
    fn an_agent_turned_off_reads_its_rows_off_and_keeps_their_stored_state() {
        let agents = [agent("fake", true, false)];
        let settings = vec![settings("fake", true, vec![model("fake-fast", true)])];
        let groups = agent_model_groups(&agents, &settings, "");
        assert!(!groups[0].enabled);
        assert!(!groups[0].models[0].enabled);
        assert!(
            settings[0].models[0].enabled,
            "the model keeps its stored state"
        );
    }

    #[test]
    fn the_same_model_keeps_a_switch_of_its_own_in_each_agents_card() {
        let agents = [agent("a", true, true), agent("b", true, true)];
        let settings = vec![
            settings("a", true, vec![model("shared", true)]),
            settings("b", true, vec![model("shared", false)]),
        ];
        let groups = agent_model_groups(&agents, &settings, "");
        assert!(groups[0].models[0].enabled);
        assert!(!groups[1].models[0].enabled);
    }

    #[test]
    fn a_search_leaves_out_the_cards_that_hold_nothing() {
        let agents = [agent("fake", true, true), agent("claude", true, true)];
        let settings = vec![
            settings("fake", true, vec![model("fake-fast", true)]),
            settings("claude", true, vec![model("claude-opus", true)]),
        ];
        let by_model = agent_model_groups(&agents, &settings, "opus");
        assert_eq!(by_model.len(), 1);
        assert_eq!(by_model[0].agent, "claude");
        assert_eq!(by_model[0].models.len(), 1);

        let by_agent = agent_model_groups(&agents, &settings, "fake");
        assert_eq!(by_agent.len(), 1);
        assert_eq!(by_agent[0].agent, "fake");
        assert_eq!(by_agent[0].models.len(), 1);

        assert!(agent_model_groups(&agents, &settings, "nothing").is_empty());
    }
}
