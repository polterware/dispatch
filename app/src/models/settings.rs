//! Pure logic for Settings: agent setup, classifier keys, and routing policy.
//!
//! Parsers follow `agent-setup.tsx`, `policy-page.tsx`, and `mapDispatchAgentSetup`.
//! Key material is never copied into a display string.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkillRow {
    pub agent: String,
    pub path: String,
    pub state: InstallState,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkillFolder {
    pub path: String,
    pub agents: Vec<String>,
    pub state: InstallState,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HookSetup {
    pub on: bool,
    pub settings: String,
    pub command: String,
    pub current: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenCodeSetup {
    pub path: String,
    pub config: String,
    pub state: InstallState,
    pub on: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentSetup {
    pub cli: CliInstall,
    pub skills: Vec<SkillRow>,
    pub hook: HookSetup,
    pub opencode: OpenCodeSetup,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuleSet {
    pub providers: Vec<String>,
    pub models: Vec<String>,
    pub agents: Vec<String>,
}

impl Default for RuleSet {
    fn default() -> Self {
        Self {
            providers: Vec::new(),
            models: Vec::new(),
            agents: Vec::new(),
        }
    }
}

impl RuleSet {
    pub fn get(&self, dimension: Dimension) -> &[String] {
        match dimension {
            Dimension::Providers => &self.providers,
            Dimension::Models => &self.models,
            Dimension::Agents => &self.agents,
        }
    }

    pub fn get_mut(&mut self, dimension: Dimension) -> &mut Vec<String> {
        match dimension {
            Dimension::Providers => &mut self.providers,
            Dimension::Models => &mut self.models,
            Dimension::Agents => &mut self.agents,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Policy {
    pub priority: String,
    pub discover: bool,
    pub roles: bool,
    pub classifier: String,
    pub allow: RuleSet,
    pub deny: RuleSet,
}

impl Policy {
    pub fn classifier_on(&self) -> bool {
        !self.classifier.is_empty()
    }

    pub fn rules(&self, side: RuleSide) -> &RuleSet {
        match side {
            RuleSide::Allow => &self.allow,
            RuleSide::Deny => &self.deny,
        }
    }

    pub fn rules_mut(&mut self, side: RuleSide) -> &mut RuleSet {
        match side {
            RuleSide::Allow => &mut self.allow,
            RuleSide::Deny => &mut self.deny,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuleSide {
    Allow,
    Deny,
}

impl RuleSide {
    pub fn title(self) -> &'static str {
        match self {
            Self::Allow => "Allow",
            Self::Deny => "Deny",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::Allow => "When a list has values, runs use only those. Empty allows any.",
            Self::Deny => "Runs never use these, whatever the allow lists say.",
        }
    }

    pub fn empty_label(self) -> &'static str {
        match self {
            Self::Allow => "Any",
            Self::Deny => "None",
        }
    }

    pub fn action(self, op: &str) -> String {
        let side = match self {
            Self::Allow => "allow",
            Self::Deny => "deny",
        };
        format!("{side}-{op}")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dimension {
    Providers,
    Models,
    Agents,
}

impl Dimension {
    pub const ALL: [Self; 3] = [Self::Providers, Self::Models, Self::Agents];

    pub fn key(self) -> &'static str {
        match self {
            Self::Providers => "providers",
            Self::Models => "models",
            Self::Agents => "agents",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Providers => "Providers",
            Self::Models => "Models",
            Self::Agents => "Agents",
        }
    }
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
pub struct AutoCandidate {
    pub p_pass: f64,
    pub effective_cost: f64,
    pub quota_multiplier: f64,
    pub qualified: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RouteCandidateView {
    pub agent: String,
    pub model: String,
    pub reason: String,
    pub auto: Option<AutoCandidate>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RouteView {
    pub candidates: Vec<RouteCandidateView>,
    pub eliminated: Vec<String>,
    pub reason: String,
    pub difficulty: Option<f64>,
    pub role: Option<String>,
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
        detail: "Asks the easy tier's model through your installed CLI. No key, but each run spends a few seconds and some of your quota.",
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

/// Lists a shared skills folder once, with every agent that reads it, in first-seen order.
pub fn skill_folders(skills: &[SkillRow]) -> Vec<SkillFolder> {
    let mut folders: Vec<SkillFolder> = Vec::new();
    for skill in skills {
        if let Some(folder) = folders.iter_mut().find(|item| item.path == skill.path) {
            folder.agents.push(skill.agent.clone());
        } else {
            folders.push(SkillFolder {
                path: skill.path.clone(),
                agents: vec![skill.agent.clone()],
                state: skill.state,
            });
        }
    }
    folders
}

pub fn skills_need_install(skills: &[SkillRow]) -> bool {
    skills
        .iter()
        .any(|skill| skill.state != InstallState::Current)
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

pub fn opencode_description(item: &OpenCodeSetup) -> &str {
    if item.state == InstallState::Missing {
        &item.config
    } else {
        &item.path
    }
}

pub fn parse_agent_setup(json: &str) -> AgentSetup {
    let root = json_object(json);
    let cli = record(root.get("cli"));
    let hook = record(root.get("hook"));
    let opencode = record(root.get("opencode"));
    let skills = root
        .get("skills")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    let row = record_of(item);
                    SkillRow {
                        agent: text(&row, "agent"),
                        path: text(&row, "path"),
                        state: InstallState::from_wire(&text(&row, "state")),
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    AgentSetup {
        cli: CliInstall {
            path: text(&cli, "path"),
            state: CliState::from_wire(&text(&cli, "state")),
            target: text(&cli, "target"),
            binary: text(&cli, "binary"),
        },
        skills,
        hook: HookSetup {
            on: flag_true(&hook, "on"),
            settings: text(&hook, "settings"),
            command: text(&hook, "command"),
            current: flag_true(&hook, "current"),
        },
        opencode: OpenCodeSetup {
            path: text(&opencode, "path"),
            config: text(&opencode, "config"),
            state: InstallState::from_wire(&text(&opencode, "state")),
            on: flag_not_false(&opencode, "on"),
        },
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
        allow: parse_rules(body.get("allow")),
        deny: parse_rules(body.get("deny")),
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
    if status == "keychain" {
        "Paste a new key"
    } else {
        "Paste a key"
    }
}

pub fn key_uses_replace(status: &str) -> bool {
    status == "keychain"
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
        "keychain" => KeyChip {
            label: "Keychain",
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

pub fn route_role_line(body: &Value) -> Option<String> {
    let root = record_of(body);
    if let Some(pick) = root.get("rolePick").map(record_of) {
        if let Some(role) = pick.get("role").and_then(Value::as_str) {
            if role == "general" {
                return Some("General: no role fits clearly".to_string());
            }
            let probability = pick
                .get("probs")
                .and_then(Value::as_object)
                .and_then(|probs| probs.get(role))
                .and_then(Value::as_f64);
            let mut line = format!("Role {role}, picked");
            if let Some(probability) = probability {
                line.push_str(&format!(" (P {})", fixed2(probability)));
            }
            return Some(line);
        }
    }
    root.get("role")
        .and_then(Value::as_str)
        .filter(|role| !role.is_empty())
        .map(|role| format!("Role {role}"))
}

pub fn parse_route(json: &str) -> RouteView {
    let body = json_value(json);
    let root = record_of(&body);
    let candidates = root
        .get("candidates")
        .and_then(Value::as_array)
        .map(|items| items.iter().map(parse_candidate).collect())
        .unwrap_or_default();
    let eliminated = root
        .get("eliminated")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    RouteView {
        candidates,
        eliminated,
        reason: root
            .get("reason")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        difficulty: root.get("difficulty").and_then(Value::as_f64),
        role: route_role_line(&body),
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

pub fn policy_options(
    dimension: Dimension,
    agents: &[String],
    models: &[ListedModel],
) -> Vec<String> {
    match dimension {
        Dimension::Agents => agents
            .iter()
            .filter(|name| !name.is_empty())
            .cloned()
            .collect(),
        Dimension::Models => unique_sorted(models.iter().map(|model| model.model.as_str())),
        Dimension::Providers => unique_sorted(models.iter().map(|model| model.provider.as_str())),
    }
}

pub fn token_delta(previous: &[String], next: &[String]) -> (Vec<String>, Vec<String>) {
    let added = next
        .iter()
        .filter(|value| !previous.iter().any(|item| item == *value))
        .cloned()
        .collect();
    let removed = previous
        .iter()
        .filter(|value| !next.iter().any(|item| item == *value))
        .cloned()
        .collect();
    (added, removed)
}

pub fn with_token(values: &[String], token: &str) -> Vec<String> {
    let mut next = values.to_vec();
    if !token.is_empty() && !next.iter().any(|item| item == token) {
        next.push(token.to_string());
    }
    next
}

pub fn without_token(values: &[String], token: &str) -> Vec<String> {
    values
        .iter()
        .filter(|item| item.as_str() != token)
        .cloned()
        .collect()
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

pub fn route_tier_options(classifier_on: bool) -> Vec<(&'static str, &'static str)> {
    let mut tiers = Vec::new();
    if classifier_on {
        tiers.push(("auto", "Auto"));
    }
    tiers.extend([("easy", "Easy"), ("normal", "Normal"), ("hard", "Hard")]);
    tiers
}

pub fn route_tier_label(value: &str) -> &'static str {
    match value {
        "auto" => "Auto",
        "easy" => "Easy",
        "hard" => "Hard",
        _ => "Normal",
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

fn parse_candidate(value: &Value) -> RouteCandidateView {
    let row = record_of(value);
    let auto = row.get("auto").filter(|item| item.is_object()).map(|item| {
        let auto = record_of(item);
        AutoCandidate {
            p_pass: number_or(auto.get("pPass"), 0.0),
            effective_cost: number_or(auto.get("effectiveCost"), 0.0),
            quota_multiplier: number_or(auto.get("quotaMultiplier"), 1.0),
            qualified: matches!(auto.get("qualified"), Some(Value::Bool(true))),
        }
    });
    RouteCandidateView {
        agent: field_string(&row, "Agent", "agent"),
        model: field_string(&row, "Model", "model"),
        reason: row
            .get("reason")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        auto,
    }
}

fn parse_rules(value: Option<&Value>) -> RuleSet {
    let Some(value) = value else {
        return RuleSet::default();
    };
    if !value.is_object() {
        return RuleSet::default();
    }
    let map = record_of(value);
    RuleSet {
        providers: string_list(map.get("providers")),
        models: string_list(map.get("models")),
        agents: string_list(map.get("agents")),
    }
}

fn string_list(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn unique_sorted<'a>(values: impl Iterator<Item = &'a str>) -> Vec<String> {
    let mut items: Vec<String> = values
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect();
    items.sort();
    items.dedup();
    items
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
        AgentSetup, CliModel, CliState, HookSetup, InstallState, OpenCodeSetup, SkillRow,
        classifier_trailing, fixed2, folder_agents_label, key_chip, parse_agent_setup,
        parse_classify, parse_key_status_value, parse_policy, parse_role_answer, parse_route,
        role_answer_line, skill_folders, token_delta,
    };
    use serde_json::json;

    #[test]
    fn skill_folders_list_a_shared_folder_once() {
        let folders = skill_folders(&[
            SkillRow {
                agent: "claude".into(),
                path: "/h/.claude/skills/dispatch/SKILL.md".into(),
                state: InstallState::Outdated,
            },
            SkillRow {
                agent: "codex".into(),
                path: "/h/.agents/skills/dispatch/SKILL.md".into(),
                state: InstallState::Current,
            },
            SkillRow {
                agent: "grok".into(),
                path: "/h/.agents/skills/dispatch/SKILL.md".into(),
                state: InstallState::Current,
            },
        ]);
        assert_eq!(folders.len(), 2);
        assert_eq!(folders[0].path, "/h/.claude/skills/dispatch/SKILL.md");
        assert_eq!(folders[0].agents, vec!["claude".to_string()]);
        assert_eq!(folders[0].state, InstallState::Outdated);
        assert_eq!(
            folders[1].agents,
            vec!["codex".to_string(), "grok".to_string()]
        );
        assert_eq!(folders[1].state, InstallState::Current);
        assert_eq!(folder_agents_label(&folders[1].agents), "Codex, Grok");
        assert_eq!(folder_agents_label(&folders[0].agents), "Claude Code");
    }

    #[test]
    fn agent_setup_reads_unknown_states_as_missing() {
        let view = parse_agent_setup(
            r#"{"cli":{"path":"/h/.local/bin/dispatch","state":"other","target":"/h/.local/bin/dispatch","binary":"/app/dispatch"},"skills":[{"agent":"codex","path":"/h/.agents/skills/dispatch/SKILL.md","state":"outdated"},{"agent":"claude","path":"/c","state":"weird"}],"hook":{"on":true,"settings":"/h/.claude/settings.json","command":"dispatch hook","current":false}}"#,
        );
        assert_eq!(view.cli.state, CliState::Other);
        assert_eq!(
            view.skills
                .iter()
                .map(|skill| skill.state)
                .collect::<Vec<_>>(),
            vec![InstallState::Outdated, InstallState::Missing]
        );
        assert_eq!(
            view.hook,
            HookSetup {
                on: true,
                settings: "/h/.claude/settings.json".into(),
                command: "dispatch hook".into(),
                current: false,
            }
        );
    }

    #[test]
    fn opencode_plugin_stays_on_unless_it_says_off() {
        let view = parse_agent_setup(
            r#"{"opencode":{"path":"/s/opencode-plugin","config":"/h/.config/opencode/opencode.json","state":"outdated","on":false}}"#,
        );
        assert_eq!(
            view.opencode,
            OpenCodeSetup {
                path: "/s/opencode-plugin".into(),
                config: "/h/.config/opencode/opencode.json".into(),
                state: InstallState::Outdated,
                on: false,
            }
        );
        assert_eq!(
            parse_agent_setup("{}").opencode,
            OpenCodeSetup {
                path: String::new(),
                config: String::new(),
                state: InstallState::Missing,
                on: true,
            }
        );
        let empty = parse_agent_setup("{}");
        assert_eq!(empty.cli.state, CliState::Missing);
        assert!(empty.skills.is_empty());
        assert!(!empty.hook.on);
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
            classifier_trailing("openai", "off", "keychain", Some(&cli)),
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
        let status = parse_key_status_value(
            r#"{"key":"jev","status":"keychain","value":"sk-secret-value"}"#,
        );
        assert_eq!(status, "keychain");
        assert!(!status.contains("sk-secret"));
        for secret in ["sk-live", "sk-typesafe", "super-secret-key", ""] {
            let chip = key_chip(secret);
            assert!(matches!(chip.label, "Keychain" | "Environment" | "Not set"));
            assert_ne!(chip.label, secret);
        }
        assert_eq!(key_chip("keychain").label, "Keychain");
        assert_eq!(key_chip("env").label, "Environment");
        assert_eq!(key_chip("missing").label, "Not set");
        assert!(!key_chip("missing").ok);
    }

    #[test]
    fn route_report_reads_candidates_cost_and_the_role_pick() {
        let route = parse_route(
            r#"{"reason":"auto","difficulty":1.25,"candidates":[{"Agent":"codex","Model":"gpt","reason":"table","auto":{"pPass":0.91,"effectiveCost":0.2,"quotaMultiplier":1.5,"qualified":true}}],"eliminated":["no quota"],"rolePick":{"role":"reviewer","probs":{"reviewer":0.8}}}"#,
        );
        assert_eq!(route.reason, "auto");
        assert_eq!(route.difficulty.map(fixed2).as_deref(), Some("1.25"));
        assert_eq!(route.eliminated, vec!["no quota".to_string()]);
        assert_eq!(
            route.role.as_deref(),
            Some("Role reviewer, picked (P 0.80)")
        );
        let candidate = &route.candidates[0];
        assert_eq!(candidate.agent, "codex");
        assert_eq!(candidate.model, "gpt");
        let auto = candidate.auto.as_ref().unwrap();
        assert!((auto.p_pass - 0.91).abs() < 1e-9);
        assert!(auto.qualified);
        let named = parse_route(r#"{"role":"implementer","candidates":[]}"#);
        assert_eq!(named.role.as_deref(), Some("Role implementer"));
        let general = parse_route(r#"{"rolePick":{"role":"general"}}"#);
        assert_eq!(
            general.role.as_deref(),
            Some("General: no role fits clearly")
        );
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

    #[test]
    fn token_delta_splits_adds_and_removes() {
        let (added, removed) = token_delta(
            &["codex".into(), "claude".into()],
            &["claude".into(), "grok".into()],
        );
        assert_eq!(added, vec!["grok".to_string()]);
        assert_eq!(removed, vec!["codex".to_string()]);
    }
}
