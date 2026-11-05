//! Pure logic for the new conversation page (⌘N): the routes its header offers, what the
//! header says for each, and the run a message starts.

use crate::client::{DispatchModel, DispatchTier, StartRunRequest};
use crate::format::agent_display_name;

/// The route Auto: the classifier judges the task and the cheapest model likely to pass runs.
pub const AUTO: &str = "tier:auto";
/// The route with no classifier, as a run with no tier takes it.
pub const NORMAL: &str = "tier:normal";
/// The tiers offered when the tier table names none.
const TIERS: [&str; 3] = ["easy", "normal", "hard"];

/// What a new conversation is routed by.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RoutePick {
    /// A tier, `auto` among them.
    Tier(String),
    /// One agent's model.
    Model { agent: String, model: String },
}

/// One route the header's picker offers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RouteChoice {
    pub value: String,
    pub label: String,
    pub group: String,
    pub hint: String,
    /// The agent whose bot marks the row; a tier has none.
    pub agent: Option<String>,
}

/// The route a new conversation starts on: Auto when there is a classifier, else the
/// normal tier.
pub fn default_pick(auto: bool) -> &'static str {
    if auto { AUTO } else { NORMAL }
}

/// The value of a model route.
pub fn model_value(agent: &str, model: &str) -> String {
    format!("model:{agent}\u{0}{model}")
}

pub fn parse_pick(value: &str) -> RoutePick {
    if let Some(rest) = value.strip_prefix("model:")
        && let Some((agent, model)) = rest.split_once('\u{0}')
    {
        return RoutePick::Model {
            agent: agent.to_string(),
            model: model.to_string(),
        };
    }
    RoutePick::Tier(
        value
            .strip_prefix("tier:")
            .filter(|tier| !tier.is_empty())
            .unwrap_or("normal")
            .to_string(),
    )
}

fn title_case(word: &str) -> String {
    let mut chars = word.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().collect::<String>() + chars.as_str())
        .unwrap_or_default()
}

/// The picker's routes: Auto (with a classifier), each tier with what it tries first, then
/// every model the agents list, under its agent.
pub fn route_choices(
    auto: bool,
    tiers: &[DispatchTier],
    models: &[DispatchModel],
) -> Vec<RouteChoice> {
    let mut choices = Vec::new();
    if auto {
        choices.push(RouteChoice {
            value: AUTO.to_string(),
            label: "Auto".to_string(),
            group: "By tier".to_string(),
            hint: "The classifier judges the task, then the cheapest model likely to pass"
                .to_string(),
            agent: None,
        });
    }
    let mut names: Vec<&str> = Vec::new();
    for tier in tiers {
        if !names.contains(&tier.name.as_str()) {
            names.push(&tier.name);
        }
    }
    if names.is_empty() {
        names = TIERS.to_vec();
    }
    for name in names {
        let mut entries: Vec<&DispatchTier> =
            tiers.iter().filter(|tier| tier.name == name).collect();
        entries.sort_by_key(|tier| tier.position);
        let hint = match entries.split_first() {
            Some((first, [])) => format!("{} / {}", first.agent, first.model),
            Some((first, rest)) => format!(
                "{} / {}, then {} fallback{}",
                first.agent,
                first.model,
                rest.len(),
                if rest.len() == 1 { "" } else { "s" }
            ),
            None => "The tier table picks".to_string(),
        };
        choices.push(RouteChoice {
            value: format!("tier:{name}"),
            label: title_case(name),
            group: "By tier".to_string(),
            hint,
            agent: None,
        });
    }
    let mut seen: Vec<(String, String)> = Vec::new();
    for model in models {
        let key = (model.agent.clone(), model.model.clone());
        if model.agent.is_empty() || model.model.is_empty() || seen.contains(&key) {
            continue;
        }
        seen.push(key);
        let hint = [model.provider.as_str(), model.tier.as_str()]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(" / ");
        choices.push(RouteChoice {
            value: model_value(&model.agent, &model.model),
            label: model.model.clone(),
            group: agent_display_name(&model.agent),
            hint,
            agent: Some(model.agent.clone()),
        });
    }
    choices
}

/// What the header pill shows for a route: the agent whose bot leads it (none for a tier,
/// which shows the Polterware ghost), its name, and the model after it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RouteLabel {
    pub agent: Option<String>,
    pub name: String,
    pub model: String,
}

pub fn pick_label(value: &str) -> RouteLabel {
    match parse_pick(value) {
        RoutePick::Tier(tier) => RouteLabel {
            agent: None,
            name: title_case(&tier),
            model: String::new(),
        },
        RoutePick::Model { agent, model } => RouteLabel {
            name: agent_display_name(&agent),
            agent: Some(agent),
            model,
        },
    }
}

/// A run's header: its agent and model once it has them, else the route it was started on
/// (`started`, while its route is still being picked), else Auto.
pub fn run_label(agent: &str, model: &str, started: Option<&str>) -> RouteLabel {
    if !agent.trim().is_empty() {
        return RouteLabel {
            agent: Some(agent.to_string()),
            name: agent_display_name(agent),
            model: model.trim().to_string(),
        };
    }
    pick_label(started.unwrap_or(AUTO))
}

/// The run a new conversation's first message starts in workspace `project_id`.
pub fn start_request(value: &str, prompt: &str, project_id: &str) -> StartRunRequest {
    let mut request = StartRunRequest {
        project_id: project_id.to_string(),
        prompt: prompt.to_string(),
        ..StartRunRequest::default()
    };
    match parse_pick(value) {
        RoutePick::Tier(tier) => request.tier = tier,
        RoutePick::Model { agent, model } => {
            request.agent = agent;
            request.model = model;
        }
    }
    request
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tier(name: &str, agent: &str, model: &str, position: i32) -> DispatchTier {
        DispatchTier {
            name: name.into(),
            agent: agent.into(),
            model: model.into(),
            position,
            ..DispatchTier::default()
        }
    }

    fn model(agent: &str, model: &str) -> DispatchModel {
        DispatchModel {
            agent: agent.into(),
            model: model.into(),
            provider: "openai".into(),
            tier: String::new(),
        }
    }

    #[test]
    fn auto_leads_the_routes_only_with_a_classifier() {
        let tiers = [
            tier("easy", "codex", "gpt-6-luna", 1),
            tier("easy", "claude", "claude-haiku-5-5", 2),
        ];
        let models = [model("codex", "gpt-6-luna"), model("codex", "gpt-6-luna")];
        let choices = route_choices(true, &tiers, &models);
        let values: Vec<&str> = choices.iter().map(|c| c.value.as_str()).collect();
        assert_eq!(values, [AUTO, "tier:easy", "model:codex\u{0}gpt-6-luna"]);
        assert_eq!(choices[1].label, "Easy");
        assert_eq!(choices[1].hint, "codex / gpt-6-luna, then 1 fallback");
        assert_eq!(choices[2].agent.as_deref(), Some("codex"));
        let plain = route_choices(false, &[], &[]);
        let values: Vec<&str> = plain.iter().map(|c| c.value.as_str()).collect();
        assert_eq!(values, ["tier:easy", "tier:normal", "tier:hard"]);
        assert_eq!(default_pick(true), AUTO);
        assert_eq!(default_pick(false), NORMAL);
    }

    #[test]
    fn a_pick_names_its_route_and_starts_its_run() {
        assert_eq!(pick_label(AUTO).name, "Auto");
        assert_eq!(pick_label(AUTO).agent, None);
        let value = model_value("codex", "gpt-6-luna");
        let label = pick_label(&value);
        assert_eq!(label.agent.as_deref(), Some("codex"));
        assert_eq!(label.model, "gpt-6-luna");
        let request = start_request(&value, "hi", "ws-1");
        assert_eq!(
            (
                request.agent.as_str(),
                request.model.as_str(),
                request.tier.as_str()
            ),
            ("codex", "gpt-6-luna", "")
        );
        assert_eq!(request.project_id, "ws-1");
        assert_eq!(start_request(AUTO, "hi", "ws-1").tier, "auto");
    }

    #[test]
    fn a_run_shows_its_route_until_it_has_an_agent() {
        assert_eq!(run_label("", "", None).name, "Auto");
        let picked = model_value("claude", "claude-haiku-5-5");
        assert_eq!(run_label("", "", Some(&picked)).model, "claude-haiku-5-5");
        let routed = run_label("codex", "gpt-6-luna", Some(AUTO));
        assert_eq!(routed.agent.as_deref(), Some("codex"));
        assert_eq!(routed.model, "gpt-6-luna");
    }
}
