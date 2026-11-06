//! WorkspaceService, the custom sections, and how a run settles in a
//! workspace through Polterware.

use super::{Core, RpcError, RpcResult, convert};
use crate::{
    Error,
    host::{self, Run as HostRun},
    pass_model::Area,
    proto as pb,
    store::{self, Workspace},
    types::Section,
};
use sha2::{Digest, Sha256};
use std::path::Path;
use std::time::Duration;

/// Bounds one read of Polterware, so a slow host never holds a run.
pub(crate) const HOST_TIMEOUT: Duration = Duration::from_secs(5);

async fn with_host<T>(f: impl std::future::Future<Output = Result<T, Error>>) -> Result<T, Error> {
    tokio::time::timeout(HOST_TIMEOUT, f)
        .await
        .map_err(|_| Error::Message("context deadline exceeded".into()))?
}

/// Resolves a leading ~ to the home folder.
pub(crate) fn expand_path(path: &str) -> String {
    let path = path.trim();
    if (path == "~" || path.starts_with("~/"))
        && let Some(home) = dirs::home_dir()
    {
        let rest = path.trim_start_matches('~').trim_start_matches('/');
        return home
            .join(rest)
            .to_string_lossy()
            .trim_end_matches('/')
            .to_owned();
    }
    path.to_owned()
}

pub(crate) fn run_area(run: &HostRun) -> Area {
    Area::new(&run.kinds.join(","), &run.tech.join(","))
}

fn section_error(e: Error) -> RpcError {
    if store::is_unknown_section(&e) {
        RpcError::not_found(e.to_string())
    } else if store::is_invalid_section(&e) {
        RpcError::invalid(e.to_string())
    } else {
        RpcError::internal(e)
    }
}

impl Core {
    /// The workspace with `id`. A Polterware project Dispatch has not seen
    /// yet becomes a linked workspace with the project's id.
    pub(crate) async fn workspace(&self, id: &str) -> Result<Workspace, Error> {
        match self.store.get_workspace(id) {
            Err(e) if store::is_no_workspace(&e) => {}
            other => return other,
        }
        let project = with_host(self.host.resolve_project(id, "")).await;
        let Ok(project) = project
            .map_err(|_| ())
            .and_then(|p| if p.id == id { Ok(p) } else { Err(()) })
        else {
            return Err(Error::Message(format!("unknown workspace {id}")));
        };
        let ws = Workspace {
            id: project.id.clone(),
            name: project.name,
            polterware_project_id: project.id,
            ..Workspace::default()
        };
        self.store.save_workspace(&ws)?;
        self.store.get_workspace(&ws.id)
    }

    /// The workspace of `dir`, as coding harnesses (OpenCode, Claude Code)
    /// take a project: the Polterware project whose repository is cloned
    /// there; else the git repository holding `dir`, named by its main
    /// checkout so a subfolder and every worktree of it are one workspace;
    /// else `dir` itself. A folder added before holds only its own path, so
    /// a home folder added as a workspace never takes in the repositories
    /// under it.
    async fn workspace_at(&self, dir: &str, name: &str) -> Result<Workspace, Error> {
        let dir = super::runs::absolute(&expand_path(dir));
        if let Ok(project) = with_host(self.host.resolve_project("", &dir)).await
            && !project.id.is_empty()
        {
            return match self.store.get_workspace(&project.id) {
                Err(e) if store::is_no_workspace(&e) => {
                    let ws = Workspace {
                        id: project.id.clone(),
                        name: project.name,
                        polterware_project_id: project.id,
                        ..Workspace::default()
                    };
                    self.store.save_workspace(&ws)?;
                    self.store.get_workspace(&ws.id)
                }
                other => other,
            };
        }
        let main = crate::worktree::main_root(&dir);
        let dir = if main.is_empty() { dir } else { main };
        if let Some(ws) = self
            .store
            .list_workspaces()?
            .into_iter()
            .find(|ws| !ws.path.is_empty() && clean_dir(&ws.path) == clean_dir(&dir))
        {
            return Ok(ws);
        }
        if !Path::new(&dir).is_dir() {
            return Err(Error::Message(format!("not a folder: {dir}")));
        }
        let name = if name.trim().is_empty() {
            Path::new(&dir)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| dir.clone())
        } else {
            name.to_owned()
        };
        let sum = Sha256::digest(dir.as_bytes());
        let id: String = sum[..6].iter().map(|b| format!("{b:02x}")).collect();
        let ws = Workspace {
            id: format!("ws-{id}"),
            name,
            path: dir,
            ..Workspace::default()
        };
        self.store.save_workspace(&ws)?;
        self.store.get_workspace(&ws.id)
    }

    /// Settles a run in `ws`: a linked workspace asks Polterware for the
    /// directory (the request's, else the repository's clone, else the
    /// card's component repository, else the only one), the area, and the
    /// card text; a standalone one, or Polterware being away, runs in `dir`
    /// or the folder.
    pub(crate) async fn host_run(
        &self,
        ws: &Workspace,
        card_slug: &str,
        repo: &str,
        dir: &str,
    ) -> Result<HostRun, Error> {
        if ws.polterware_project_id.is_empty() {
            let dir = if dir.is_empty() {
                ws.path.as_str()
            } else {
                dir
            };
            if dir.is_empty() {
                return Err(Error::Message("the run needs a directory".into()));
            }
            return Ok(HostRun {
                dir: dir.into(),
                ..HostRun::default()
            });
        }
        let resolved =
            with_host(
                self.host
                    .resolve_run(&ws.polterware_project_id, card_slug, repo, dir),
            )
            .await;
        match resolved {
            Err(e) if host::is_no_host(&e) => {
                if dir.is_empty() {
                    return Err(Error::Message(
                        "Polterware is not running: pass the run's directory".into(),
                    ));
                }
                Ok(HostRun {
                    dir: dir.into(),
                    ..HostRun::default()
                })
            }
            Err(e) => Err(e),
            Ok(mut run) => {
                if run.dir.is_empty() {
                    run.dir = dir.into();
                }
                Ok(run)
            }
        }
    }

    /// The area and card text of a dry run (Route, Classify); an unknown
    /// workspace or an absent Polterware gives none.
    pub(crate) async fn run_hints(
        &self,
        workspace_id: &str,
        card_slug: &str,
        dir: &str,
    ) -> HostRun {
        let id = workspace_id.trim();
        if id.is_empty() {
            return HostRun::default();
        }
        let Ok(ws) = self.store.get_workspace(id) else {
            return HostRun::default();
        };
        if ws.polterware_project_id.is_empty() {
            return HostRun::default();
        }
        self.host_run(&ws, card_slug.trim(), "", dir)
            .await
            .unwrap_or_default()
    }

    /// Lists Polterware's projects (kept as linked workspaces) and the
    /// folders added on their own.
    pub(crate) async fn list_workspaces(
        &self,
        _: pb::ListWorkspacesRequest,
    ) -> RpcResult<pb::ListWorkspacesResponse> {
        let projects = with_host(self.host.projects()).await;
        let linked = projects.is_ok();
        let projects = projects.unwrap_or_default();
        for p in &projects {
            match self.store.get_workspace(&p.id) {
                Err(e) if store::is_no_workspace(&e) => {
                    self.save_linked(&p.id, &p.name, String::new())?;
                }
                Ok(ws) if ws.name != p.name => {
                    self.save_linked(&p.id, &p.name, ws.created_at)?;
                }
                Ok(_) => {}
                Err(e) => return Err(RpcError::internal(e)),
            }
        }
        let items = self.store.list_workspaces().map_err(RpcError::internal)?;
        Ok(pb::ListWorkspacesResponse {
            linked,
            workspaces: items
                .iter()
                // A project Polterware no longer lists (archived) is hidden while it answers.
                .filter(|ws| {
                    !(linked
                        && !ws.polterware_project_id.is_empty()
                        && !projects.iter().any(|p| p.id == ws.polterware_project_id))
                })
                .map(convert::workspace)
                .collect(),
        })
    }

    fn save_linked(&self, id: &str, name: &str, created_at: String) -> RpcResult<()> {
        self.store
            .save_workspace(&Workspace {
                id: id.into(),
                name: name.into(),
                polterware_project_id: id.into(),
                created_at,
                ..Workspace::default()
            })
            .map_err(RpcError::internal)
    }

    pub(crate) async fn add_workspace(
        &self,
        m: pb::AddWorkspaceRequest,
    ) -> RpcResult<pb::AddWorkspaceResponse> {
        let path = m.path.trim();
        if path.is_empty() {
            return Err(RpcError::invalid("path is required"));
        }
        let ws = self
            .workspace_at(path, &m.name)
            .await
            .map_err(|e| RpcError::invalid(e.to_string()))?;
        Ok(pb::AddWorkspaceResponse {
            workspace: Some(convert::workspace(&ws)),
        })
    }

    /// Forgets a standalone folder; its runs stay in the store.
    pub(crate) async fn remove_workspace(
        &self,
        m: pb::RemoveWorkspaceRequest,
    ) -> RpcResult<pb::RemoveWorkspaceResponse> {
        let ws = self
            .store
            .get_workspace(m.id.trim())
            .map_err(|e| RpcError::invalid(e.to_string()))?;
        if !ws.polterware_project_id.is_empty() {
            return Err(RpcError::invalid(
                "a Polterware project's workspace follows the project",
            ));
        }
        self.store
            .delete_workspace(&ws.id)
            .map_err(RpcError::internal)?;
        Ok(pb::RemoveWorkspaceResponse {})
    }

    pub(crate) async fn resolve_workspace(
        &self,
        m: pb::ResolveWorkspaceRequest,
    ) -> RpcResult<pb::ResolveWorkspaceResponse> {
        let id = m.id.trim();
        let ws = if !id.is_empty() {
            self.workspace(id).await
        } else {
            let dir = m.dir.trim();
            if dir.is_empty() {
                return Err(RpcError::invalid("id or dir is required"));
            }
            self.workspace_at(dir, "").await
        }
        .map_err(|e| RpcError::invalid(e.to_string()))?;
        Ok(pb::ResolveWorkspaceResponse {
            workspace: Some(convert::workspace(&ws)),
        })
    }

    /// A linked workspace's open cards and repositories; a standalone
    /// workspace, or an absent Polterware, has none.
    pub(crate) async fn list_workspace_cards(
        &self,
        m: pb::ListWorkspaceCardsRequest,
    ) -> RpcResult<pb::ListWorkspaceCardsResponse> {
        let mut out = pb::ListWorkspaceCardsResponse::default();
        let Ok(ws) = self.store.get_workspace(m.workspace_id.trim()) else {
            return Ok(out);
        };
        if ws.polterware_project_id.is_empty() {
            return Ok(out);
        }
        let Ok((cards, repos)) = with_host(self.host.cards(&ws.polterware_project_id)).await else {
            return Ok(out);
        };
        out.cards = cards
            .into_iter()
            .map(|c| pb::WorkspaceCard {
                slug: c.slug,
                title: c.title,
                milestone: c.milestone,
                stage: c.stage,
            })
            .collect();
        out.repositories = repos
            .into_iter()
            .map(|r| pb::WorkspaceRepository {
                name: r.name,
                remote: r.remote,
                local_path: r.local_path,
            })
            .collect();
        Ok(out)
    }

    pub(crate) async fn list_sections(
        &self,
        _: pb::ListSectionsRequest,
    ) -> RpcResult<pb::ListSectionsResponse> {
        let sections = self.store.list_sections().map_err(section_error)?;
        let names = self
            .store
            .conversation_names()
            .map_err(RpcError::internal)?;
        Ok(pb::ListSectionsResponse {
            sections: sections.iter().map(convert::section).collect(),
            conversation_names: names
                .into_iter()
                .map(|(conversation_id, name)| pb::ConversationName {
                    conversation_id,
                    name,
                })
                .collect(),
        })
    }

    pub(crate) async fn rename_conversation(
        &self,
        m: pb::RenameConversationRequest,
    ) -> RpcResult<pb::RenameConversationResponse> {
        self.store
            .rename_conversation(&m.conversation_id, &m.name)
            .map_err(section_error)?;
        Ok(pb::RenameConversationResponse {})
    }

    pub(crate) async fn save_section(
        &self,
        m: pb::SaveSectionRequest,
    ) -> RpcResult<pb::SaveSectionResponse> {
        let section = self
            .store
            .save_section(&Section {
                id: m.id.trim().into(),
                name: m.name,
                position: m.position.into(),
                ..Section::default()
            })
            .map_err(section_error)?;
        Ok(pb::SaveSectionResponse {
            section: Some(convert::section(&section)),
        })
    }

    pub(crate) async fn delete_section(
        &self,
        m: pb::DeleteSectionRequest,
    ) -> RpcResult<pb::DeleteSectionResponse> {
        self.store
            .delete_section(m.id.trim())
            .map_err(section_error)?;
        Ok(pb::DeleteSectionResponse {})
    }

    pub(crate) async fn move_to_section(
        &self,
        m: pb::MoveToSectionRequest,
    ) -> RpcResult<pb::MoveToSectionResponse> {
        self.store
            .move_to_section(m.kind.trim(), m.r#ref.trim(), m.section_id.trim())
            .map_err(section_error)?;
        Ok(pb::MoveToSectionResponse {})
    }
}

/// `dir` without trailing slashes, to compare a workspace's folder with another.
fn clean_dir(dir: &str) -> &str {
    let trimmed = dir.trim_end_matches('/');
    if trimmed.is_empty() { "/" } else { trimmed }
}
