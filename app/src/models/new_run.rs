//! Pure logic for the new conversation page (⌘N): the routes its header offers, what the
//! header says for each, where the run works and whether it gets a worktree, and the run a
//! message starts.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::client::{DispatchModel, DispatchTier, StartRunRequest, Workspace, WorkspaceRepository};
use crate::format::agent_display_name;

/// The preference that remembers, per workspace, the folder and mode last picked.
pub const PLACE_PREFERENCE: &str = "new-run-place";

/// Whether a new conversation's run edits its folder or a git worktree of its own.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaceMode {
    /// The folder itself: what the person sees is what the agent edits.
    #[default]
    InPlace,
    /// A git worktree and branch of its own, so runs go in parallel and merge back.
    Worktree,
}

impl PlaceMode {
    pub const ALL: [PlaceMode; 2] = [PlaceMode::InPlace, PlaceMode::Worktree];

    pub fn label(self) -> &'static str {
        match self {
            PlaceMode::InPlace => "This checkout",
            PlaceMode::Worktree => "Worktree",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            PlaceMode::InPlace => "The agent edits the repository's checkout itself",
            PlaceMode::Worktree => "A git worktree and branch of its own, to merge back",
        }
    }

    /// `StartRunRequest.worktree_mode`.
    pub fn wire(self) -> &'static str {
        match self {
            PlaceMode::InPlace => "in_place",
            PlaceMode::Worktree => "worktree",
        }
    }

    /// The picker's value.
    pub fn value(self) -> &'static str {
        match self {
            PlaceMode::InPlace => "mode:in_place",
            PlaceMode::Worktree => "mode:worktree",
        }
    }

    pub fn from_value(value: &str) -> Option<PlaceMode> {
        PlaceMode::ALL
            .into_iter()
            .find(|mode| mode.value() == value)
    }
}

/// Where a new conversation's run works: a workspace, a folder in it, and the mode.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RunPlace {
    pub workspace_id: String,
    /// Empty leaves the folder to the core: a Polterware project's only repository.
    pub dir: String,
    pub mode: PlaceMode,
}

/// A folder the header's folder picker offers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FolderChoice {
    pub workspace_id: String,
    pub dir: String,
    pub label: String,
    /// The workspace it is in, for a Polterware project's repositories.
    pub group: String,
}

impl FolderChoice {
    /// The picker's value: the folder, unique across workspaces.
    pub fn value(&self) -> String {
        format!("dir:{}", self.dir)
    }
}

/// The repositories a run can work in, one list as Claude Code's: each linked project's
/// cloned repositories (`repos`, by workspace id) and each standalone workspace's folder,
/// kept only when it is a git checkout on disk, once per folder.
pub fn folder_choices(
    workspaces: &[Workspace],
    repos: &HashMap<String, Vec<WorkspaceRepository>>,
) -> Vec<FolderChoice> {
    let mut seen = std::collections::HashSet::new();
    folder_candidates(workspaces, repos)
        .into_iter()
        .filter(|folder| std::path::Path::new(&folder.dir).join(".git").exists())
        .filter(|folder| {
            let key = std::fs::canonicalize(&folder.dir)
                .map(|path| path.to_string_lossy().into_owned())
                .unwrap_or_else(|_| folder.dir.clone());
            seen.insert(key)
        })
        .collect()
}

/// Every folder the workspaces name, before the ones that are not repositories go.
fn folder_candidates(
    workspaces: &[Workspace],
    repos: &HashMap<String, Vec<WorkspaceRepository>>,
) -> Vec<FolderChoice> {
    let mut folders = Vec::new();
    for workspace in workspaces {
        if workspace.polterware_project_id.is_empty() {
            if workspace.path.trim().is_empty() {
                continue;
            }
            folders.push(FolderChoice {
                workspace_id: workspace.id.clone(),
                dir: workspace.path.clone(),
                label: folder_label(&workspace.name, &workspace.path),
                group: "Repositories".into(),
            });
            continue;
        }
        for repo in repos.get(&workspace.id).into_iter().flatten() {
            if repo.local_path.trim().is_empty() {
                continue;
            }
            folders.push(FolderChoice {
                workspace_id: workspace.id.clone(),
                dir: repo.local_path.clone(),
                label: folder_label(&repo.name, &repo.local_path),
                group: workspace.name.clone(),
            });
        }
    }
    folders
}

/// A folder's name: the one it was given, else its last path component.
fn folder_label(name: &str, path: &str) -> String {
    let name = name.trim();
    if !name.is_empty() {
        return name.to_string();
    }
    path.trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(path)
        .to_string()
}

/// Where a new conversation starts: the folder and mode last picked in the `current`
/// workspace, else its folder (a project's first repository) in This folder.
pub fn default_place(
    current: Option<&Workspace>,
    remembered: &HashMap<String, RunPlace>,
    folders: &[FolderChoice],
) -> Option<RunPlace> {
    let workspace = current?;
    if let Some(place) = remembered.get(&workspace.id)
        && (place.dir.is_empty()
            || folders.is_empty()
            || folders.iter().any(|folder| folder.dir == place.dir))
    {
        return Some(place.clone());
    }
    let dir = if workspace.polterware_project_id.is_empty() {
        // A folder that is not a repository (a home folder) is no place: the first
        // repository is.
        if !folders.is_empty() && !folders.iter().any(|folder| folder.dir == workspace.path) {
            let first = &folders[0];
            return Some(RunPlace {
                workspace_id: first.workspace_id.clone(),
                dir: first.dir.clone(),
                mode: PlaceMode::InPlace,
            });
        }
        workspace.path.clone()
    } else {
        folders
            .iter()
            .find(|folder| folder.workspace_id == workspace.id)
            .map(|folder| folder.dir.clone())
            .unwrap_or_default()
    };
    Some(RunPlace {
        workspace_id: workspace.id.clone(),
        dir,
        mode: PlaceMode::InPlace,
    })
}

/// What the folder pill says for `place`.
pub fn place_label(place: &RunPlace, folders: &[FolderChoice], workspaces: &[Workspace]) -> String {
    if let Some(folder) = folders.iter().find(|folder| folder.dir == place.dir) {
        return folder.label.clone();
    }
    if !place.dir.is_empty() {
        return folder_label("", &place.dir);
    }
    workspaces
        .iter()
        .find(|workspace| workspace.id == place.workspace_id)
        .map(|workspace| workspace.name.clone())
        .unwrap_or_else(|| "Repository".into())
}

/// The folder picker's last row, which asks for a repository to add rather than picking one.
pub const ADD_REPOSITORY: &str = "add-repository";

/// The composer's Image pill: on, the run asks for images and routes to the agents that
/// make them (`StartRunRequest.images`).
pub const IMAGE_PILL: &str = "Image";

/// The Image pill's tooltip, which says what the mode does either way.
pub fn image_hint(on: bool) -> &'static str {
    if on {
        "The run asks for images and routes only to agents that make them"
    } else {
        "Ask for images: routes only to agents that make them"
    }
}

/// Whether a refusal to start says another run is editing the folder in place and offers a
/// new worktree (the core's `IN_PLACE_BUSY_HINT`), so Worktree can retry it.
pub fn offers_worktree(message: &str) -> bool {
    message.contains("--worktree-mode worktree")
}

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
    /// Why the row is shown but cannot be picked (Auto with no classifier).
    pub disabled: Option<String>,
}

/// Why Auto cannot be picked while Settings › Classifier names none.
pub const AUTO_NEEDS_CLASSIFIER: &str =
    "Auto needs a classifier to judge the task: choose one in Settings › Classifier";

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

/// The picker's routes: Auto (disabled, saying why, with no classifier), each tier with what
/// it tries first, then every model the agents list, under its agent.
pub fn route_choices(
    auto: bool,
    tiers: &[DispatchTier],
    models: &[DispatchModel],
) -> Vec<RouteChoice> {
    let mut choices = Vec::new();
    choices.push(RouteChoice {
        value: AUTO.to_string(),
        label: "Auto".to_string(),
        group: "By tier".to_string(),
        hint: if auto {
            "The classifier judges the task, then the cheapest model likely to pass"
        } else {
            "No classifier set"
        }
        .to_string(),
        agent: None,
        disabled: (!auto).then(|| AUTO_NEEDS_CLASSIFIER.to_string()),
    });
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
            disabled: None,
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
            disabled: None,
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

/// The run a new conversation's first message starts, on route `value`, in `place`, asking
/// for images when `images`.
pub fn start_request(value: &str, prompt: &str, place: &RunPlace, images: bool) -> StartRunRequest {
    let mut request = StartRunRequest {
        project_id: place.workspace_id.clone(),
        prompt: prompt.to_string(),
        dir: place.dir.clone(),
        worktree_mode: place.mode.wire().to_string(),
        images,
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
            efforts: Vec::new(),
        }
    }

    #[test]
    fn auto_leads_the_routes_and_needs_a_classifier() {
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
        assert!(choices.iter().all(|c| c.disabled.is_none()));
        let plain = route_choices(false, &[], &[]);
        let values: Vec<&str> = plain.iter().map(|c| c.value.as_str()).collect();
        assert_eq!(values, [AUTO, "tier:easy", "tier:normal", "tier:hard"]);
        assert_eq!(plain[0].disabled.as_deref(), Some(AUTO_NEEDS_CLASSIFIER));
        assert!(plain[1..].iter().all(|c| c.disabled.is_none()));
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
        let place = RunPlace {
            workspace_id: "ws-1".into(),
            ..RunPlace::default()
        };
        let request = start_request(&value, "hi", &place, false);
        assert_eq!(
            (
                request.agent.as_str(),
                request.model.as_str(),
                request.tier.as_str()
            ),
            ("codex", "gpt-6-luna", "")
        );
        assert_eq!(request.project_id, "ws-1");
        assert_eq!(start_request(AUTO, "hi", &place, false).tier, "auto");
    }

    fn workspace(id: &str, name: &str, path: &str, project: &str) -> Workspace {
        Workspace {
            id: id.into(),
            name: name.into(),
            path: path.into(),
            polterware_project_id: project.into(),
            ..Workspace::default()
        }
    }

    fn repo(name: &str, path: &str) -> WorkspaceRepository {
        WorkspaceRepository {
            name: name.into(),
            local_path: path.into(),
            ..WorkspaceRepository::default()
        }
    }

    #[test]
    fn a_new_conversation_starts_in_its_workspace_folder_itself() {
        let root = tempfile::tempdir().unwrap();
        let dir = |name: &str, git: bool| -> String {
            let path = root.path().join(name);
            std::fs::create_dir_all(if git { path.join(".git") } else { path.clone() }).unwrap();
            path.to_string_lossy().into_owned()
        };
        let (app, core, web, plain) = (
            dir("app", true),
            dir("core", true),
            dir("web", true),
            dir("plain", false),
        );
        let folder = workspace("ws-a", "app", &app, "");
        let loose = workspace("ws-b", "plain", &plain, "");
        // The same repository added again as a folder is listed once.
        let again = workspace("ws-c", "web again", &web, "");
        let project = workspace("proj-1", "Polterware", "", "pw-1");
        let repos = HashMap::from([(
            "proj-1".to_string(),
            vec![repo("core", &core), repo("web", &web), repo("gone", "")],
        )]);
        let all = [folder.clone(), loose.clone(), again, project.clone()];
        let folders = folder_choices(&all, &repos);
        let dirs: Vec<&str> = folders.iter().map(|folder| folder.dir.as_str()).collect();
        // Repositories only, each once: the plain folder is not one.
        assert_eq!(dirs, [app.as_str(), web.as_str(), core.as_str()]);
        // A workspace that is not a repository starts on the first one.
        let none = HashMap::new();
        assert_eq!(
            default_place(Some(&loose), &none, &folders).unwrap().dir,
            app
        );

        let none = HashMap::new();
        let place = default_place(Some(&folder), &none, &folders).unwrap();
        assert_eq!(
            place,
            RunPlace {
                workspace_id: "ws-a".into(),
                dir: app.clone(),
                mode: PlaceMode::InPlace,
            }
        );
        // A project starts in its first repository.
        assert_eq!(
            default_place(Some(&project), &none, &folders).unwrap().dir,
            core
        );
        assert_eq!(default_place(None, &none, &folders), None);

        // The last pick in that workspace wins, while its folder is still offered.
        let picked = RunPlace {
            workspace_id: "proj-1".into(),
            dir: web.clone(),
            mode: PlaceMode::Worktree,
        };
        let remembered = HashMap::from([("proj-1".to_string(), picked.clone())]);
        assert_eq!(
            default_place(Some(&project), &remembered, &folders),
            Some(picked)
        );
        let stale = HashMap::from([(
            "proj-1".to_string(),
            RunPlace {
                dir: "/code/removed".into(),
                ..RunPlace::default()
            },
        )]);
        assert_eq!(
            default_place(Some(&project), &stale, &folders).unwrap().dir,
            core
        );
    }

    #[test]
    fn the_request_carries_the_folder_and_its_mode() {
        let place = RunPlace {
            workspace_id: "proj-1".into(),
            dir: "/code/web".into(),
            mode: PlaceMode::Worktree,
        };
        let request = start_request(NORMAL, "fix it", &place, false);
        assert_eq!(
            (
                request.project_id.as_str(),
                request.dir.as_str(),
                request.worktree_mode.as_str(),
                request.tier.as_str(),
            ),
            ("proj-1", "/code/web", "worktree", "normal")
        );
        let here = RunPlace {
            mode: PlaceMode::InPlace,
            ..place
        };
        assert_eq!(
            start_request(NORMAL, "x", &here, false).worktree_mode,
            "in_place"
        );
        assert_eq!(
            PlaceMode::from_value(PlaceMode::Worktree.value()),
            Some(PlaceMode::Worktree)
        );
    }

    #[test]
    fn an_image_conversation_asks_for_images() {
        let place = RunPlace::default();
        assert!(!start_request(NORMAL, "draw a chart", &place, false).images);
        assert!(start_request(NORMAL, "draw a chart", &place, true).images);
        assert_eq!(IMAGE_PILL, "Image");
        assert!(image_hint(true).contains("asks for images"));
        assert!(image_hint(false).contains("Ask for images"));
    }

    #[test]
    fn a_busy_folder_refusal_offers_a_worktree() {
        assert!(offers_worktree(
            "run abc is editing /code/app itself, with no worktree of its own: start this one in a new worktree (--worktree-mode worktree), read-only, or once that run ends"
        ));
        assert!(!offers_worktree("prompt is required"));
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
