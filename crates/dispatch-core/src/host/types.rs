use crate::{Error, proto::host as pb};
use serde::{Deserialize, Serialize};
use std::{future::Future, pin::Pin, sync::Arc};

pub const ERR_NO_HOST: &str = "polterware is not available";
pub const CLIENT_HEADER: &str = "Polterware-Client";
pub const CLIENT_NAME: &str = "dispatch";

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum HostError {
    #[error("polterware is not available")]
    NoHost,
    #[error("{0}")]
    Remote(String),
}

impl From<HostError> for Error {
    fn from(err: HostError) -> Self {
        Error::Message(err.to_string())
    }
}

pub fn is_no_host(err: &Error) -> bool {
    matches!(err, Error::Message(msg) if msg == ERR_NO_HOST)
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    pub name: String,
}

impl From<pb::HostProject> for Project {
    fn from(p: pb::HostProject) -> Self {
        Self {
            id: p.id,
            name: p.name,
        }
    }
}

/// Settled run metadata from Polterware: directory, area, and card text.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Run {
    pub dir: String,
    pub kinds: Vec<String>,
    pub tech: Vec<String>,
    pub card_text: String,
}

impl From<pb::ResolveRunResponse> for Run {
    fn from(m: pb::ResolveRunResponse) -> Self {
        Self {
            dir: m.dir,
            kinds: m.kinds,
            tech: m.tech,
            card_text: m.card_text,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Card {
    pub slug: String,
    pub title: String,
    pub milestone: String,
    pub stage: String,
}

impl From<pb::HostCard> for Card {
    fn from(c: pb::HostCard) -> Self {
        Self {
            slug: c.slug,
            title: c.title,
            milestone: c.milestone,
            stage: c.stage,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Repository {
    pub name: String,
    pub remote: String,
    pub local_path: String,
}

impl From<pb::HostRepository> for Repository {
    fn from(r: pb::HostRepository) -> Self {
        Self {
            name: r.name,
            remote: r.remote,
            local_path: r.local_path,
        }
    }
}

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
pub type CardsAndRepos = (Vec<Card>, Vec<Repository>);

/// What Dispatch reads from Polterware (`DispatchHostService`).
pub trait Host: Send + Sync {
    fn projects(&self) -> BoxFuture<'_, Result<Vec<Project>, Error>>;

    fn resolve_project<'a>(
        &'a self,
        id: &'a str,
        dir: &'a str,
    ) -> BoxFuture<'a, Result<Project, Error>>;

    fn resolve_run<'a>(
        &'a self,
        project_id: &'a str,
        card_slug: &'a str,
        repo: &'a str,
        dir: &'a str,
    ) -> BoxFuture<'a, Result<Run, Error>>;

    fn cards<'a>(&'a self, project_id: &'a str) -> BoxFuture<'a, Result<CardsAndRepos, Error>>;

    fn start_agent_run<'a>(
        &'a self,
        project_id: &'a str,
        task: &'a str,
        card_slug: &'a str,
    ) -> BoxFuture<'a, Result<String, Error>>;

    fn complete_agent_run<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<(), Error>>;
}

impl<T: Host + ?Sized> Host for Arc<T> {
    fn projects(&self) -> BoxFuture<'_, Result<Vec<Project>, Error>> {
        (**self).projects()
    }

    fn resolve_project<'a>(
        &'a self,
        id: &'a str,
        dir: &'a str,
    ) -> BoxFuture<'a, Result<Project, Error>> {
        (**self).resolve_project(id, dir)
    }

    fn resolve_run<'a>(
        &'a self,
        project_id: &'a str,
        card_slug: &'a str,
        repo: &'a str,
        dir: &'a str,
    ) -> BoxFuture<'a, Result<Run, Error>> {
        (**self).resolve_run(project_id, card_slug, repo, dir)
    }

    fn cards<'a>(&'a self, project_id: &'a str) -> BoxFuture<'a, Result<CardsAndRepos, Error>> {
        (**self).cards(project_id)
    }

    fn start_agent_run<'a>(
        &'a self,
        project_id: &'a str,
        task: &'a str,
        card_slug: &'a str,
    ) -> BoxFuture<'a, Result<String, Error>> {
        (**self).start_agent_run(project_id, task, card_slug)
    }

    fn complete_agent_run<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<(), Error>> {
        (**self).complete_agent_run(id)
    }
}

impl<T: Host + ?Sized> Host for Box<T> {
    fn projects(&self) -> BoxFuture<'_, Result<Vec<Project>, Error>> {
        (**self).projects()
    }

    fn resolve_project<'a>(
        &'a self,
        id: &'a str,
        dir: &'a str,
    ) -> BoxFuture<'a, Result<Project, Error>> {
        (**self).resolve_project(id, dir)
    }

    fn resolve_run<'a>(
        &'a self,
        project_id: &'a str,
        card_slug: &'a str,
        repo: &'a str,
        dir: &'a str,
    ) -> BoxFuture<'a, Result<Run, Error>> {
        (**self).resolve_run(project_id, card_slug, repo, dir)
    }

    fn cards<'a>(&'a self, project_id: &'a str) -> BoxFuture<'a, Result<CardsAndRepos, Error>> {
        (**self).cards(project_id)
    }

    fn start_agent_run<'a>(
        &'a self,
        project_id: &'a str,
        task: &'a str,
        card_slug: &'a str,
    ) -> BoxFuture<'a, Result<String, Error>> {
        (**self).start_agent_run(project_id, task, card_slug)
    }

    fn complete_agent_run<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<(), Error>> {
        (**self).complete_agent_run(id)
    }
}

/// Host implementation when Polterware is absent.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NoHost;

pub use self::NoHost as NoneHost;

impl Host for NoHost {
    fn projects(&self) -> BoxFuture<'_, Result<Vec<Project>, Error>> {
        Box::pin(async { Err(HostError::NoHost.into()) })
    }

    fn resolve_project<'a>(
        &'a self,
        _id: &'a str,
        _dir: &'a str,
    ) -> BoxFuture<'a, Result<Project, Error>> {
        Box::pin(async { Err(HostError::NoHost.into()) })
    }

    fn resolve_run<'a>(
        &'a self,
        _project_id: &'a str,
        _card_slug: &'a str,
        _repo: &'a str,
        _dir: &'a str,
    ) -> BoxFuture<'a, Result<Run, Error>> {
        Box::pin(async { Err(HostError::NoHost.into()) })
    }

    fn cards<'a>(&'a self, _project_id: &'a str) -> BoxFuture<'a, Result<CardsAndRepos, Error>> {
        Box::pin(async { Err(HostError::NoHost.into()) })
    }

    fn start_agent_run<'a>(
        &'a self,
        _project_id: &'a str,
        _task: &'a str,
        _card_slug: &'a str,
    ) -> BoxFuture<'a, Result<String, Error>> {
        Box::pin(async { Err(HostError::NoHost.into()) })
    }

    fn complete_agent_run<'a>(&'a self, _id: &'a str) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async { Err(HostError::NoHost.into()) })
    }
}
