use super::Deps;
use crate::{Error, types::Run};
use futures::future::BoxFuture;
use std::time::Duration;

/// Bounds one call to Polterware, so a slow host never holds a run.
pub const HOST_TIMEOUT: Duration = Duration::from_secs(5);

/// The AgentRun calls a linked workspace makes. The host client implements it.
pub trait AgentRunHost: Send + Sync {
    fn start_agent_run<'a>(
        &'a self,
        project_id: &'a str,
        task: &'a str,
        card_slug: &'a str,
    ) -> BoxFuture<'a, Result<String, Error>>;
    fn complete_agent_run<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<(), Error>>;
}

impl AgentRunHost for crate::host::Client {
    fn start_agent_run<'a>(
        &'a self,
        project_id: &'a str,
        task: &'a str,
        card_slug: &'a str,
    ) -> BoxFuture<'a, Result<String, Error>> {
        Box::pin(crate::host::Client::start_agent_run(
            self, project_id, task, card_slug,
        ))
    }

    fn complete_agent_run<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(crate::host::Client::complete_agent_run(self, id))
    }
}

/// The one-line task an AgentRun shows for a delegation.
pub fn dispatch_run_task(prompt: &str) -> String {
    let first = prompt.trim().split('\n').next().unwrap_or_default();
    let mut line = first.trim();
    if line.len() > 160 {
        let mut end = 160;
        while !line.is_char_boundary(end) {
            end -= 1;
        }
        line = &line[..end];
    }
    if line.is_empty() {
        return "delegated run".into();
    }
    line.into()
}

impl Deps {
    /// A delegation in a linked workspace is also an audited AgentRun: the
    /// agent it launches writes under it, so Activity shows one history.
    pub async fn link_agent_run(
        &self,
        host: &dyn AgentRunHost,
        host_project_id: &str,
        run: &mut Run,
    ) {
        if host_project_id.is_empty() {
            return;
        }
        let task = dispatch_run_task(&run.prompt);
        let started = tokio::time::timeout(
            HOST_TIMEOUT,
            host.start_agent_run(host_project_id, &task, &run.card_slug),
        )
        .await;
        if let Ok(Ok(id)) = started
            && !id.is_empty()
        {
            run.agent_run_id = id;
            if let Some(st) = self.store.as_deref() {
                let _ = st.save_run(run);
            }
        }
    }
}

/// Closes the AgentRun a delegation started.
pub async fn complete_linked_agent_run(host: &dyn AgentRunHost, run: &Run) {
    if run.agent_run_id.is_empty() {
        return;
    }
    let _ = tokio::time::timeout(HOST_TIMEOUT, host.complete_agent_run(&run.agent_run_id)).await;
}
