use crate::{
    Error,
    coreclient::rpc::{ConnectError, RpcTransport},
    host::types::{
        BoxFuture, CLIENT_HEADER, CLIENT_NAME, Card, CardsAndRepos, Host, HostError, Project,
        Repository, Run,
    },
    proto::host as pb,
    runstate::{self, Core},
};
use std::{
    env, fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{
    process::Command,
    sync::Mutex,
    time::{Instant, sleep},
};

const HTTP_TIMEOUT: Duration = Duration::from_secs(10);
const DEFAULT_START_TIMEOUT: Duration = Duration::from_secs(10);

/// Returns Polterware's database path: `POLTERWARE_CORE_DB`, else `<user config dir>/polterware/local.db`.
pub fn default_db() -> PathBuf {
    if let Ok(db) = env::var("POLTERWARE_CORE_DB") {
        let trimmed = db.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    let dir = dirs::config_dir().unwrap_or_else(|| {
        let home = env::var_os("HOME")
            .map(PathBuf::from)
            .or_else(dirs::home_dir)
            .unwrap_or_default();
        home.join(".config")
    });
    dir.join("polterware").join("local.db")
}

/// Typed Connect-JSON unary client for `polterware.local.v1.DispatchHostService`.
#[derive(Clone, Debug)]
pub struct DispatchHostServiceClient {
    rpc: RpcTransport,
}

impl DispatchHostServiceClient {
    pub fn new(endpoint: &str, token: &str) -> Self {
        Self {
            rpc: RpcTransport::new(
                endpoint,
                token,
                CLIENT_HEADER,
                CLIENT_NAME,
                Some(HTTP_TIMEOUT),
            ),
        }
    }

    pub fn endpoint(&self) -> &str {
        self.rpc.endpoint()
    }

    pub async fn list_host_projects(
        &self,
        req: &pb::ListHostProjectsRequest,
    ) -> Result<pb::ListHostProjectsResponse, ConnectError> {
        self.rpc
            .unary(
                "polterware.local.v1.DispatchHostService/ListHostProjects",
                req,
            )
            .await
    }

    pub async fn resolve_host_project(
        &self,
        req: &pb::ResolveHostProjectRequest,
    ) -> Result<pb::ResolveHostProjectResponse, ConnectError> {
        self.rpc
            .unary(
                "polterware.local.v1.DispatchHostService/ResolveHostProject",
                req,
            )
            .await
    }

    pub async fn resolve_run(
        &self,
        req: &pb::ResolveRunRequest,
    ) -> Result<pb::ResolveRunResponse, ConnectError> {
        self.rpc
            .unary("polterware.local.v1.DispatchHostService/ResolveRun", req)
            .await
    }

    pub async fn list_host_cards(
        &self,
        req: &pb::ListHostCardsRequest,
    ) -> Result<pb::ListHostCardsResponse, ConnectError> {
        self.rpc
            .unary("polterware.local.v1.DispatchHostService/ListHostCards", req)
            .await
    }

    pub async fn start_host_agent_run(
        &self,
        req: &pb::StartHostAgentRunRequest,
    ) -> Result<pb::StartHostAgentRunResponse, ConnectError> {
        self.rpc
            .unary(
                "polterware.local.v1.DispatchHostService/StartHostAgentRun",
                req,
            )
            .await
    }

    pub async fn complete_host_agent_run(
        &self,
        req: &pb::CompleteHostAgentRunRequest,
    ) -> Result<pb::CompleteHostAgentRunResponse, ConnectError> {
        self.rpc
            .unary(
                "polterware.local.v1.DispatchHostService/CompleteHostAgentRun",
                req,
            )
            .await
    }
}

/// Maps transport failures to [`HostError::NoHost`] and preserves Polterware's own RPC errors.
fn unavailable(err: ConnectError) -> Error {
    if !err.transport
        && !matches!(
            err.code.as_str(),
            "" | "unavailable" | "unimplemented" | "unknown"
        )
    {
        return HostError::Remote(err.message).into();
    }
    HostError::NoHost.into()
}

fn is_executable(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|m| m.is_file() && (m.permissions().mode() & 0o111) != 0)
}

fn look_path(name: &str) -> Option<PathBuf> {
    let p = Path::new(name);
    if p.components().count() > 1 {
        return is_executable(p).then(|| p.to_path_buf());
    }
    let path_var = env::var_os("PATH")?;
    for dir in env::split_paths(&path_var) {
        let candidate = dir.join(name);
        if is_executable(&candidate) {
            return Some(candidate);
        }
    }
    None
}

/// Attaches to Polterware's local core for `db_path` on each call.
#[derive(Debug)]
pub struct Client {
    pub db_path: PathBuf,
    pub binary: PathBuf,
    start_timeout: Duration,
    mu: Mutex<()>,
}

impl Default for Client {
    fn default() -> Self {
        Self::new(default_db())
    }
}

impl Client {
    pub fn new(db_path: impl Into<PathBuf>) -> Self {
        Self {
            db_path: db_path.into(),
            binary: PathBuf::new(),
            start_timeout: DEFAULT_START_TIMEOUT,
            mu: Mutex::new(()),
        }
    }

    pub fn with_binary(mut self, binary: impl Into<PathBuf>) -> Self {
        self.binary = binary.into();
        self
    }

    pub fn with_start_timeout(mut self, timeout: Duration) -> Self {
        self.start_timeout = timeout;
        self
    }

    async fn service(&self) -> Result<DispatchHostServiceClient, Error> {
        let _guard = self.mu.lock().await;
        let core = match runstate::read_core(&self.db_path) {
            Some(c) => c,
            None => self.start().await?,
        };
        Ok(DispatchHostServiceClient::new(&core.endpoint, &core.token))
    }

    async fn start(&self) -> Result<Core, Error> {
        if fs::metadata(&self.db_path).is_err() {
            return Err(HostError::NoHost.into());
        }
        let bin = if self.binary.as_os_str().is_empty() {
            look_path("polterware").ok_or(HostError::NoHost)?
        } else {
            self.binary.clone()
        };

        let mut child = Command::new(&bin)
            .arg("-db")
            .arg(&self.db_path)
            .args(["serve", "--listen", "127.0.0.1:0"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()
            .map_err(|_| HostError::NoHost)?;

        tokio::spawn(async move {
            let _ = child.wait().await;
        });

        let deadline = Instant::now() + self.start_timeout;
        while Instant::now() < deadline {
            if let Some(core) = runstate::read_core(&self.db_path) {
                return Ok(core);
            }
            sleep(Duration::from_millis(100)).await;
        }
        Err(HostError::NoHost.into())
    }

    pub async fn projects(&self) -> Result<Vec<Project>, Error> {
        let svc = self.service().await?;
        let res = svc
            .list_host_projects(&pb::ListHostProjectsRequest {})
            .await
            .map_err(unavailable)?;
        Ok(res.projects.into_iter().map(Project::from).collect())
    }

    pub async fn resolve_project(&self, id: &str, dir: &str) -> Result<Project, Error> {
        let svc = self.service().await?;
        let res = svc
            .resolve_host_project(&pb::ResolveHostProjectRequest {
                id: id.to_owned(),
                dir: dir.to_owned(),
            })
            .await
            .map_err(unavailable)?;
        Ok(res.project.map(Project::from).unwrap_or_default())
    }

    pub async fn resolve_run(
        &self,
        project_id: &str,
        card_slug: &str,
        repo: &str,
        dir: &str,
    ) -> Result<Run, Error> {
        let svc = self.service().await?;
        let res = svc
            .resolve_run(&pb::ResolveRunRequest {
                project_id: project_id.to_owned(),
                card_slug: card_slug.to_owned(),
                repo: repo.to_owned(),
                dir: dir.to_owned(),
            })
            .await
            .map_err(unavailable)?;
        Ok(Run::from(res))
    }

    pub async fn cards(&self, project_id: &str) -> Result<(Vec<Card>, Vec<Repository>), Error> {
        let svc = self.service().await?;
        let res = svc
            .list_host_cards(&pb::ListHostCardsRequest {
                project_id: project_id.to_owned(),
            })
            .await
            .map_err(unavailable)?;
        let cards = res.cards.into_iter().map(Card::from).collect();
        let repos = res.repositories.into_iter().map(Repository::from).collect();
        Ok((cards, repos))
    }

    pub async fn start_agent_run(
        &self,
        project_id: &str,
        task: &str,
        card_slug: &str,
    ) -> Result<String, Error> {
        let svc = self.service().await?;
        let res = svc
            .start_host_agent_run(&pb::StartHostAgentRunRequest {
                project_id: project_id.to_owned(),
                task: task.to_owned(),
                card_slug: card_slug.to_owned(),
            })
            .await
            .map_err(unavailable)?;
        Ok(res.agent_run_id)
    }

    pub async fn complete_agent_run(&self, id: &str) -> Result<(), Error> {
        let svc = self.service().await?;
        svc.complete_host_agent_run(&pb::CompleteHostAgentRunRequest {
            agent_run_id: id.to_owned(),
        })
        .await
        .map_err(unavailable)?;
        Ok(())
    }
}

impl Host for Client {
    fn projects(&self) -> BoxFuture<'_, Result<Vec<Project>, Error>> {
        Box::pin(Client::projects(self))
    }

    fn resolve_project<'a>(
        &'a self,
        id: &'a str,
        dir: &'a str,
    ) -> BoxFuture<'a, Result<Project, Error>> {
        Box::pin(Client::resolve_project(self, id, dir))
    }

    fn resolve_run<'a>(
        &'a self,
        project_id: &'a str,
        card_slug: &'a str,
        repo: &'a str,
        dir: &'a str,
    ) -> BoxFuture<'a, Result<Run, Error>> {
        Box::pin(Client::resolve_run(self, project_id, card_slug, repo, dir))
    }

    fn cards<'a>(&'a self, project_id: &'a str) -> BoxFuture<'a, Result<CardsAndRepos, Error>> {
        Box::pin(Client::cards(self, project_id))
    }

    fn start_agent_run<'a>(
        &'a self,
        project_id: &'a str,
        task: &'a str,
        card_slug: &'a str,
    ) -> BoxFuture<'a, Result<String, Error>> {
        Box::pin(Client::start_agent_run(self, project_id, task, card_slug))
    }

    fn complete_agent_run<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(Client::complete_agent_run(self, id))
    }
}
