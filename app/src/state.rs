//! Window state: route, core snapshots, toasts, and preference files.
//!
//! `navigate` and `go_back` only change the route. Callers that need a redraw
//! pass the context: `state.update(cx, |s, cx| { s.navigate(route); s.request_refresh(cx); cx.notify(); })`.

use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::Mutex,
    time::{Duration, Instant},
};

use gpui::{App, AppContext as _, Context, Entity, SharedString};
use serde::{Serialize, de::DeserializeOwned};

use crate::client::{
    CoreClient, CoreError, DispatchRun, DispatchSection, ListRunsRequest, ListSectionsRequest,
    ListWorkspacesRequest, Workspace, core_json_path, resolve_data_dir,
};
pub use crate::ui::{Toast, ToastTone};

const POLL_INTERVAL: Duration = Duration::from_secs(3);
/// The current workspace, kept across launches (`pwdispatch.workspace` in the Electron app).
const WORKSPACE_PREF: &str = "workspace";
/// The sidebar's width (`pwdispatch.shell.sidebar.sizes` in the Electron app).
const SIDEBAR_PREF: &str = "sidebar_sizes";
/// `ResizableSidebar`'s `defaultWidth`, `minWidth`, and `maxWidth` in the Electron app.
pub const SIDEBAR_DEFAULT_WIDTH: f32 = 232.0;
pub const SIDEBAR_MIN_WIDTH: f32 = 184.0;
pub const SIDEBAR_MAX_WIDTH: f32 = 320.0;

/// The stored shape of `ResizableSidebar`: the expanded width and whether it is collapsed.
#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize)]
struct SidebarSizes {
    width: f32,
    collapsed: bool,
}
const RUNS_LIMIT: i32 = 50;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SettingsPage {
    Agents,
    Classifier,
    PickRole,
    RoutingPolicy,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Route {
    Roles,
    RoleBoard(String),
    Intelligence,
    Spend,
    Stats,
    Settings(SettingsPage),
    Run(String),
    Runs,
}

impl Default for Route {
    fn default() -> Self {
        Self::Roles
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConnectionState {
    Connecting,
    Offline { core_json_path: String },
    Online,
}

/// Route plus a back stack. `navigate` does not push when the route is unchanged.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Navigation {
    route: Route,
    history: Vec<Route>,
}

impl Navigation {
    pub fn new(route: Route) -> Self {
        Self {
            route,
            history: Vec::new(),
        }
    }

    pub fn route(&self) -> &Route {
        &self.route
    }

    pub fn navigate(&mut self, route: Route) {
        if self.route == route {
            return;
        }
        // Settings is a dialog over a page: moving between its sections is not history.
        if !(matches!(self.route, Route::Settings(_)) && matches!(route, Route::Settings(_))) {
            self.history.push(self.route.clone());
        }
        self.route = route;
    }

    /// The page drawn under the Settings dialog: the route itself when it is a page, else the
    /// page Settings was opened from.
    pub fn backdrop(&self) -> Route {
        std::iter::once(&self.route)
            .chain(self.history.iter().rev())
            .find(|route| !matches!(route, Route::Settings(_)))
            .cloned()
            .unwrap_or_default()
    }

    /// Closes Settings onto the page it was opened from.
    pub fn close_settings(&mut self) {
        if !matches!(self.route, Route::Settings(_)) {
            return;
        }
        while let Some(previous) = self.history.pop() {
            if !matches!(previous, Route::Settings(_)) {
                self.route = previous;
                return;
            }
        }
        self.route = Route::default();
    }

    pub fn go_back(&mut self) -> bool {
        if let Some(previous) = self.history.pop() {
            self.route = previous;
            true
        } else {
            false
        }
    }
}

#[derive(Clone, Debug)]
pub struct WorkspaceGroup {
    pub id: String,
    pub name: String,
    pub runs: Vec<DispatchRun>,
}

/// Groups runs under workspaces. Empty workspaces are omitted.
/// A run with an empty `project_id` is filed under `"default"` / `"Workspace"`.
pub fn workspace_groups(workspaces: &[Workspace], runs: &[DispatchRun]) -> Vec<WorkspaceGroup> {
    let mut names: HashMap<String, String> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    for workspace in workspaces {
        if workspace.id.trim().is_empty() || names.contains_key(&workspace.id) {
            continue;
        }
        let name = if workspace.name.trim().is_empty() {
            workspace.id.clone()
        } else {
            workspace.name.clone()
        };
        order.push(workspace.id.clone());
        names.insert(workspace.id.clone(), name);
    }

    let mut grouped: HashMap<String, Vec<DispatchRun>> = HashMap::new();
    let mut extra_order: Vec<String> = Vec::new();
    for run in runs {
        let key = if run.project_id.trim().is_empty() {
            "default".to_string()
        } else {
            run.project_id.clone()
        };
        if !grouped.contains_key(&key) && !names.contains_key(&key) {
            extra_order.push(key.clone());
        }
        grouped.entry(key).or_default().push(run.clone());
    }

    let mut out = Vec::new();
    for id in order {
        if let Some(group_runs) = grouped.remove(&id) {
            let name = names.get(&id).cloned().unwrap_or_else(|| id.clone());
            out.push(WorkspaceGroup {
                id,
                name,
                runs: group_runs,
            });
        }
    }
    for id in extra_order {
        if let Some(group_runs) = grouped.remove(&id) {
            let name = if id == "default" {
                "Workspace".to_string()
            } else {
                id.clone()
            };
            out.push(WorkspaceGroup {
                id,
                name,
                runs: group_runs,
            });
        }
    }
    out
}

pub fn sanitize_preference_name(name: &str) -> String {
    let mut out = String::new();
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() {
        out.push('_');
    }
    out
}

/// A preference file. The folder is the core's data folder, which is also the Electron
/// app's userData, whose Chromium `Preferences` file a plain `preferences` folder would
/// collide with on a case-insensitive disk.
pub fn preference_path(data_dir: &Path, name: &str) -> PathBuf {
    data_dir
        .join("gpui-preferences")
        .join(format!("{}.json", sanitize_preference_name(name)))
}

pub fn read_preference<T: DeserializeOwned>(
    data_dir: &Path,
    name: &str,
) -> Result<Option<T>, String> {
    let path = preference_path(data_dir, name);
    if !path.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(&path).map_err(|err| err.to_string())?;
    serde_json::from_str(&raw)
        .map(Some)
        .map_err(|err| err.to_string())
}

pub fn write_preference<T: Serialize>(
    data_dir: &Path,
    name: &str,
    value: &T,
) -> Result<(), String> {
    let path = preference_path(data_dir, name);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| err.to_string())?;
    }
    let raw = serde_json::to_string_pretty(value).map_err(|err| err.to_string())?;
    fs::write(path, raw).map_err(|err| err.to_string())
}

struct Snapshot {
    connection: ConnectionState,
    client: Option<CoreClient>,
    workspaces: Vec<Workspace>,
    linked: bool,
    sections: Vec<DispatchSection>,
    runs: Vec<DispatchRun>,
    reply: Option<(String, String)>,
    last_error: Option<String>,
}

/// How long a core the app started gets to write its `core.json`.
const CORE_START_WAIT: Duration = Duration::from_secs(8);
/// The least time between two cores the app starts, so one that fails is not started again
/// on every poll.
const CORE_START_GAP: Duration = Duration::from_secs(30);
static LAST_CORE_START: Mutex<Option<Instant>> = Mutex::new(None);

/// Starts a core for `data_dir` when none is alive, as any `dispatch` command does: after a
/// restart nothing else has started one yet. The core is its own process, so its runs go on
/// once the app quits. It reports whether a core came up.
fn start_core(data_dir: &Path) -> bool {
    use std::os::unix::{fs::OpenOptionsExt, process::CommandExt};
    use std::process::{Command, Stdio};

    let db = data_dir.join("dispatch.db");
    // A core that is alive but not answering is not replaced from here.
    if dispatch_core::runstate::read_core(&db).is_some() {
        return false;
    }
    {
        let mut last = LAST_CORE_START
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if last.is_some_and(|at| at.elapsed() < CORE_START_GAP) {
            return false;
        }
        *last = Some(Instant::now());
    }
    let Ok(exe) = dispatch_core::runstate::executable() else {
        return false;
    };
    if dispatch_core::runstate::ensure_dir(&db).is_err() {
        return false;
    }
    let Ok(log) = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(dispatch_core::runstate::log_path(&db))
    else {
        return false;
    };
    let Ok(log_err) = log.try_clone() else {
        return false;
    };
    let mut command = Command::new(exe);
    command
        .arg("-db")
        .arg(&db)
        .args(["serve", "--listen", "127.0.0.1:0"])
        .stdin(Stdio::null())
        .stdout(log)
        .stderr(log_err)
        .process_group(0);
    let spawned = command.spawn();
    let Ok(mut child) = spawned else {
        return false;
    };
    // Reaps the core whenever it ends.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    let deadline = Instant::now() + CORE_START_WAIT;
    while Instant::now() < deadline {
        if dispatch_core::runstate::read_core(&db).is_some() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}

/// The core's state, starting a core first when none is running.
fn load_snapshot(data_dir: &Path, selected_run: Option<String>) -> Snapshot {
    let snapshot = read_snapshot(data_dir, selected_run.clone());
    if matches!(snapshot.connection, ConnectionState::Offline { .. }) && start_core(data_dir) {
        return read_snapshot(data_dir, selected_run);
    }
    snapshot
}

fn read_snapshot(data_dir: &Path, selected_run: Option<String>) -> Snapshot {
    let offline_path = core_json_path(data_dir).display().to_string();
    let client = match CoreClient::connect(data_dir) {
        Ok(client) => client,
        Err(_) => {
            return Snapshot {
                connection: ConnectionState::Offline {
                    core_json_path: offline_path,
                },
                client: None,
                workspaces: Vec::new(),
                linked: false,
                sections: Vec::new(),
                runs: Vec::new(),
                reply: None,
                last_error: None,
            };
        }
    };

    let mut snapshot = Snapshot {
        connection: ConnectionState::Online,
        client: Some(client.clone()),
        workspaces: Vec::new(),
        linked: false,
        sections: Vec::new(),
        runs: Vec::new(),
        reply: None,
        last_error: None,
    };

    match client.list_workspaces(&ListWorkspacesRequest::default()) {
        Ok(response) => {
            snapshot.workspaces = response.workspaces;
            snapshot.linked = response.linked;
        }
        Err(CoreError::NotRunning) => {
            return offline(offline_path);
        }
        Err(CoreError::Rpc(message)) => snapshot.last_error = Some(message),
    }

    match client.list_sections(&ListSectionsRequest::default()) {
        Ok(response) => snapshot.sections = response.sections,
        Err(CoreError::NotRunning) => return offline(offline_path),
        Err(CoreError::Rpc(message)) => snapshot.last_error = Some(message),
    }

    let mut runs = Vec::new();
    for workspace in &snapshot.workspaces {
        if workspace.id.trim().is_empty() {
            continue;
        }
        match client.list_runs(&ListRunsRequest {
            project_id: workspace.id.clone(),
            limit: RUNS_LIMIT,
            ..ListRunsRequest::default()
        }) {
            Ok(response) => runs.extend(response.runs),
            Err(CoreError::NotRunning) => return offline(offline_path),
            Err(CoreError::Rpc(message)) => snapshot.last_error = Some(message),
        }
    }
    if runs.is_empty() {
        match client.list_runs(&ListRunsRequest {
            limit: RUNS_LIMIT,
            ..ListRunsRequest::default()
        }) {
            Ok(response) => runs = response.runs,
            Err(CoreError::NotRunning) => return offline(offline_path),
            Err(CoreError::Rpc(message)) => snapshot.last_error = Some(message),
        }
    }
    snapshot.runs = runs;

    if let Some(id) = selected_run {
        match client.run_reply(&id) {
            Ok(text) => snapshot.reply = Some((id, text)),
            Err(CoreError::NotRunning) => return offline(offline_path),
            Err(CoreError::Rpc(_)) => snapshot.reply = Some((id, String::new())),
        }
    }

    snapshot
}

fn offline(core_json_path: String) -> Snapshot {
    Snapshot {
        connection: ConnectionState::Offline { core_json_path },
        client: None,
        workspaces: Vec::new(),
        linked: false,
        sections: Vec::new(),
        runs: Vec::new(),
        reply: None,
        last_error: None,
    }
}

pub struct AppState {
    data_dir: PathBuf,
    navigation: Navigation,
    connection: ConnectionState,
    client: Option<CoreClient>,
    workspaces: Vec<Workspace>,
    /// The workspace the pages read: the one of the last run opened, else the first.
    workspace_id: String,
    /// The sidebar's width, between `SIDEBAR_MIN_WIDTH` and `SIDEBAR_MAX_WIDTH`.
    sidebar_width: f32,
    linked: bool,
    sections: Vec<DispatchSection>,
    runs: Vec<DispatchRun>,
    replies: HashMap<String, String>,
    pending_reply: Option<String>,
    last_error: Option<String>,
    toasts: Vec<Toast>,
    next_toast: u64,
    commander_open: bool,
    generation: u64,
    applied_generation: u64,
    /// Moves on whenever [`Self::runs`] changes, so views can keep what they build from the
    /// runs until it does.
    runs_revision: u64,
}

impl AppState {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let data_dir = resolve_data_dir();
        let workspace_id = read_preference::<String>(&data_dir, WORKSPACE_PREF)
            .ok()
            .flatten()
            .unwrap_or_default();
        let sidebar_width = read_preference::<SidebarSizes>(&data_dir, SIDEBAR_PREF)
            .ok()
            .flatten()
            .map(|sizes| sizes.width)
            .filter(|width| width.is_finite())
            .unwrap_or(SIDEBAR_DEFAULT_WIDTH)
            .clamp(SIDEBAR_MIN_WIDTH, SIDEBAR_MAX_WIDTH);
        let state = Self {
            data_dir,
            navigation: Navigation::new(Route::Roles),
            connection: ConnectionState::Connecting,
            client: None,
            workspaces: Vec::new(),
            workspace_id,
            sidebar_width,
            linked: false,
            sections: Vec::new(),
            runs: Vec::new(),
            replies: HashMap::new(),
            pending_reply: None,
            last_error: None,
            toasts: Vec::new(),
            next_toast: 0,
            commander_open: false,
            generation: 0,
            applied_generation: 0,
            runs_revision: 0,
        };
        Self::spawn_poll(cx);
        state
    }

    pub fn reload_all(&mut self, cx: &mut Context<Self>) {
        self.spawn_load(cx);
    }

    pub fn request_refresh(&mut self, cx: &mut Context<Self>) {
        self.spawn_load(cx);
    }

    pub fn navigate(&mut self, route: Route) {
        if let Route::Run(id) = &route {
            if !self.replies.contains_key(id) {
                self.pending_reply = Some(id.clone());
            }
            let project = self.run(id).map(|run| run.project_id.clone());
            if let Some(project) = project.filter(|project| !project.is_empty()) {
                self.set_workspace(project);
            }
        }
        self.navigation.navigate(route);
    }

    /// The workspace the pages read (`current()` in the Electron app): the remembered one
    /// while it exists, else the first.
    pub fn current_workspace(&self) -> Option<&Workspace> {
        self.workspaces
            .iter()
            .find(|workspace| workspace.id == self.workspace_id)
            .or_else(|| self.workspaces.first())
    }

    pub fn sidebar_width(&self) -> f32 {
        self.sidebar_width
    }

    /// Sets the sidebar's width, clamped; `persist` stores it (at the end of a drag).
    pub fn set_sidebar_width(&mut self, width: f32, persist: bool) {
        self.sidebar_width = width.clamp(SIDEBAR_MIN_WIDTH, SIDEBAR_MAX_WIDTH);
        if persist {
            let sizes = SidebarSizes {
                width: self.sidebar_width,
                collapsed: false,
            };
            let _ = write_preference(&self.data_dir, SIDEBAR_PREF, &sizes);
        }
    }

    pub fn set_workspace(&mut self, id: String) {
        if id != self.workspace_id {
            self.workspace_id = id;
            let _ = write_preference(&self.data_dir, WORKSPACE_PREF, &self.workspace_id);
        }
    }

    pub fn go_back(&mut self) -> bool {
        let moved = self.navigation.go_back();
        if moved
            && let Route::Run(id) = self.navigation.route()
            && !self.replies.contains_key(id)
        {
            self.pending_reply = Some(id.clone());
        }
        moved
    }

    pub fn route(&self) -> &Route {
        self.navigation.route()
    }

    /// The page under the Settings dialog (the route itself outside Settings).
    pub fn backdrop(&self) -> Route {
        self.navigation.backdrop()
    }

    pub fn close_settings(&mut self) {
        self.navigation.close_settings();
    }

    pub fn client(&self) -> Option<CoreClient> {
        self.client.clone()
    }

    pub fn push_toast(&mut self, message: impl Into<SharedString>, tone: ToastTone) -> u64 {
        let message = message.into();
        // The same words already up stay one toast, however often they are said.
        if let Some(shown) = self
            .toasts
            .iter()
            .find(|toast| toast.message == message && toast.tone == tone)
        {
            return shown.id;
        }
        self.next_toast = self.next_toast.wrapping_add(1);
        let id = self.next_toast;
        self.toasts.push(Toast { id, message, tone });
        id
    }

    pub fn dismiss_toast(&mut self, id: u64) {
        self.toasts.retain(|toast| toast.id != id);
    }

    pub fn toasts(&self) -> &[Toast] {
        &self.toasts
    }

    pub fn connection(&self) -> &ConnectionState {
        &self.connection
    }

    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    pub fn workspaces(&self) -> &[Workspace] {
        &self.workspaces
    }

    pub fn workspaces_linked(&self) -> bool {
        self.linked
    }

    pub fn runs(&self) -> &[DispatchRun] {
        &self.runs
    }

    /// The revision of [`Self::runs`]: equal revisions mean equal runs.
    pub fn runs_revision(&self) -> u64 {
        self.runs_revision
    }

    fn set_runs(&mut self, runs: Vec<DispatchRun>) {
        if self.runs != runs {
            self.runs = runs;
            self.runs_revision = self.runs_revision.wrapping_add(1);
        }
    }

    pub fn sections(&self) -> &[DispatchSection] {
        &self.sections
    }

    pub fn workspace_groups(&self) -> Vec<WorkspaceGroup> {
        workspace_groups(&self.workspaces, &self.runs)
    }

    pub fn run(&self, id: &str) -> Option<&DispatchRun> {
        self.runs.iter().find(|run| run.id == id)
    }

    pub fn run_reply(&self, id: &str) -> Option<&str> {
        self.replies.get(id).map(String::as_str)
    }

    pub fn reply_pending(&self, id: &str) -> bool {
        self.pending_reply.as_deref() == Some(id)
    }

    pub fn commander_open(&self) -> bool {
        self.commander_open
    }

    pub fn toggle_commander(&mut self) {
        self.commander_open = !self.commander_open;
    }

    pub fn set_commander_open(&mut self, open: bool) {
        self.commander_open = open;
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    pub fn read_preference<T: DeserializeOwned>(&self, name: &str) -> Result<Option<T>, String> {
        read_preference(&self.data_dir, name)
    }

    pub fn write_preference<T: Serialize>(&self, name: &str, value: &T) -> Result<(), String> {
        write_preference(&self.data_dir, name, value)
    }

    fn spawn_poll(cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                let ticket = match this.update(cx, |state, _| state.begin_load()) {
                    Ok(ticket) => ticket,
                    Err(_) => break,
                };
                let snapshot = cx
                    .background_spawn(async move {
                        let (generation, data_dir, selected) = ticket;
                        (generation, load_snapshot(&data_dir, selected))
                    })
                    .await;
                let alive = this
                    .update(cx, |state, cx| {
                        state.finish_load(snapshot.0, snapshot.1);
                        cx.notify();
                    })
                    .is_ok();
                if !alive {
                    break;
                }
                cx.background_executor().timer(POLL_INTERVAL).await;
            }
        })
        .detach();
    }

    fn spawn_load(&mut self, cx: &mut Context<Self>) {
        let ticket = self.begin_load();
        cx.spawn(async move |this, cx| {
            let (generation, snapshot) = cx
                .background_spawn(async move {
                    let (generation, data_dir, selected) = ticket;
                    (generation, load_snapshot(&data_dir, selected))
                })
                .await;
            let _ = this.update(cx, |state, cx| {
                state.finish_load(generation, snapshot);
                cx.notify();
            });
        })
        .detach();
    }

    fn begin_load(&mut self) -> (u64, PathBuf, Option<String>) {
        self.generation = self.generation.wrapping_add(1);
        let selected = match self.navigation.route() {
            Route::Run(id) => Some(id.clone()),
            _ => None,
        };
        if let Some(id) = &selected
            && !self.replies.contains_key(id)
        {
            self.pending_reply = Some(id.clone());
        }
        (self.generation, self.data_dir.clone(), selected)
    }

    fn finish_load(&mut self, generation: u64, snapshot: Snapshot) {
        if generation < self.applied_generation {
            return;
        }
        self.applied_generation = generation;
        self.last_error = snapshot.last_error;
        match snapshot.connection {
            ConnectionState::Offline { core_json_path } => {
                self.connection = ConnectionState::Offline { core_json_path };
                self.client = None;
                self.workspaces.clear();
                self.linked = false;
                self.sections.clear();
                self.set_runs(Vec::new());
                self.replies.clear();
                self.pending_reply = None;
            }
            ConnectionState::Online => {
                self.connection = ConnectionState::Online;
                if snapshot.client.is_some() {
                    self.client = snapshot.client;
                }
                self.workspaces = snapshot.workspaces;
                self.linked = snapshot.linked;
                self.sections = snapshot.sections;
                self.set_runs(snapshot.runs);
                if let Some((id, reply)) = snapshot.reply {
                    self.replies.insert(id.clone(), reply);
                    if self.pending_reply.as_deref() == Some(id.as_str()) {
                        self.pending_reply = None;
                    }
                }
            }
            ConnectionState::Connecting => {}
        }
    }
}

/// Subscribe a view to [`AppState`] so it repaints when the route or snapshot changes.
pub fn observe<V: 'static>(state: &Entity<AppState>, cx: &mut Context<V>) {
    cx.observe(state, |_, _, cx| cx.notify()).detach();
}

impl AppState {
    pub fn open(cx: &mut App) -> Entity<Self> {
        cx.new(|cx| Self::new(cx))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Navigation, Route, SettingsPage, WorkspaceGroup, preference_path, read_preference,
        sanitize_preference_name, workspace_groups, write_preference,
    };
    use crate::client::{DispatchRun, Workspace};

    #[test]
    fn navigation_skips_duplicates_and_pops_history() {
        let mut navigation = Navigation::new(Route::Roles);
        navigation.navigate(Route::Roles);
        assert_eq!(navigation.route(), &Route::Roles);
        navigation.navigate(Route::Spend);
        navigation.navigate(Route::Settings(SettingsPage::Agents));
        navigation.navigate(Route::Run("run_1".into()));
        assert!(navigation.go_back());
        assert_eq!(navigation.route(), &Route::Settings(SettingsPage::Agents));
        assert!(navigation.go_back());
        assert!(navigation.go_back());
        assert!(!navigation.go_back());
        assert_eq!(navigation.route(), &Route::Roles);
    }

    #[test]
    fn settings_opens_over_a_page_and_closes_back_onto_it() {
        let mut navigation = Navigation::new(Route::Roles);
        navigation.navigate(Route::Spend);
        navigation.navigate(Route::Settings(SettingsPage::Agents));
        navigation.navigate(Route::Settings(SettingsPage::Classifier));
        assert_eq!(navigation.backdrop(), Route::Spend);
        navigation.close_settings();
        assert_eq!(navigation.route(), &Route::Spend);
        assert!(navigation.go_back());
        assert_eq!(navigation.route(), &Route::Roles);
        let mut fresh = Navigation::new(Route::Settings(SettingsPage::Agents));
        assert_eq!(fresh.backdrop(), Route::Roles);
        fresh.close_settings();
        assert_eq!(fresh.route(), &Route::Roles);
    }

    #[test]
    fn groups_runs_by_workspace_and_names_the_default_bucket() {
        let workspaces = vec![Workspace {
            id: "ws_1".into(),
            name: "Polter".into(),
            ..Workspace::default()
        }];
        let runs = vec![
            DispatchRun {
                id: "a".into(),
                project_id: "ws_1".into(),
                ..DispatchRun::default()
            },
            DispatchRun {
                id: "b".into(),
                project_id: String::new(),
                ..DispatchRun::default()
            },
        ];
        let groups: Vec<WorkspaceGroup> = workspace_groups(&workspaces, &runs);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].name, "Polter");
        assert_eq!(groups[0].runs[0].id, "a");
        assert_eq!(groups[1].id, "default");
        assert_eq!(groups[1].name, "Workspace");
    }

    #[test]
    fn preference_files_round_trip_in_an_explicit_directory() {
        assert_eq!(sanitize_preference_name("nav/width"), "nav_width");
        let dir = std::env::temp_dir().join(format!(
            "dispatch-gpui-prefs-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        let value = vec!["roles".to_string(), "spend".to_string()];
        write_preference(&dir, "nav/width", &value).unwrap();
        let path = preference_path(&dir, "nav/width");
        assert!(path.ends_with("gpui-preferences/nav_width.json"));
        let read: Option<Vec<String>> = read_preference(&dir, "nav/width").unwrap();
        assert_eq!(read.unwrap(), value);
        let missing: Option<Vec<String>> = read_preference(&dir, "absent").unwrap();
        assert!(missing.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
