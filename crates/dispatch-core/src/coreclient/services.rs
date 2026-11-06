use crate::{
    Error,
    coreclient::rpc::{ConnectError, EnvelopeDecoder, RpcTransport, decode_stream_frame},
    proto,
};

/// Typed Connect clients of one Dispatch core.
#[derive(Clone, Debug)]
pub struct Clients {
    pub health: HealthClient,
    pub dispatch: DispatchClient,
    pub workspaces: WorkspaceClient,
}

impl Clients {
    pub(crate) fn from_transport(rpc: RpcTransport) -> Self {
        Self {
            health: HealthClient { rpc: rpc.clone() },
            dispatch: DispatchClient { rpc: rpc.clone() },
            workspaces: WorkspaceClient { rpc },
        }
    }

    pub fn endpoint(&self) -> &str {
        self.health.rpc.endpoint()
    }
}

#[derive(Clone, Debug)]
pub struct HealthClient {
    rpc: RpcTransport,
}

impl HealthClient {
    pub async fn check(&self, req: &proto::CheckRequest) -> Result<proto::CheckResponse, Error> {
        Ok(self
            .rpc
            .unary("dispatch.v1.HealthService/Check", req)
            .await?)
    }

    pub async fn check_connect(
        &self,
        req: &proto::CheckRequest,
    ) -> Result<proto::CheckResponse, ConnectError> {
        self.rpc.unary("dispatch.v1.HealthService/Check", req).await
    }

    pub async fn shutdown(
        &self,
        req: &proto::ShutdownRequest,
    ) -> Result<proto::ShutdownResponse, Error> {
        Ok(self
            .rpc
            .unary("dispatch.v1.HealthService/Shutdown", req)
            .await?)
    }
}

#[derive(Clone, Debug)]
pub struct WorkspaceClient {
    rpc: RpcTransport,
}

impl WorkspaceClient {
    pub async fn list_workspaces(
        &self,
        req: &proto::ListWorkspacesRequest,
    ) -> Result<proto::ListWorkspacesResponse, Error> {
        Ok(self
            .rpc
            .unary("dispatch.v1.WorkspaceService/ListWorkspaces", req)
            .await?)
    }

    pub async fn add_workspace(
        &self,
        req: &proto::AddWorkspaceRequest,
    ) -> Result<proto::AddWorkspaceResponse, Error> {
        Ok(self
            .rpc
            .unary("dispatch.v1.WorkspaceService/AddWorkspace", req)
            .await?)
    }

    pub async fn remove_workspace(
        &self,
        req: &proto::RemoveWorkspaceRequest,
    ) -> Result<proto::RemoveWorkspaceResponse, Error> {
        Ok(self
            .rpc
            .unary("dispatch.v1.WorkspaceService/RemoveWorkspace", req)
            .await?)
    }

    pub async fn resolve_workspace(
        &self,
        req: &proto::ResolveWorkspaceRequest,
    ) -> Result<proto::ResolveWorkspaceResponse, Error> {
        Ok(self
            .rpc
            .unary("dispatch.v1.WorkspaceService/ResolveWorkspace", req)
            .await?)
    }

    pub async fn list_workspace_cards(
        &self,
        req: &proto::ListWorkspaceCardsRequest,
    ) -> Result<proto::ListWorkspaceCardsResponse, Error> {
        Ok(self
            .rpc
            .unary("dispatch.v1.WorkspaceService/ListWorkspaceCards", req)
            .await?)
    }
}

/// Async stream of [`proto::WatchRunResponse`] frames from `DispatchService/WatchRun`.
#[derive(Debug)]
pub struct WatchRunStream {
    response: reqwest::Response,
    decoder: EnvelopeDecoder,
    finished: bool,
}

impl WatchRunStream {
    pub async fn receive(&mut self) -> Result<Option<proto::WatchRunResponse>, Error> {
        if self.finished {
            return Ok(None);
        }
        loop {
            if let Some(frame) = self.decoder.next_frame() {
                match decode_stream_frame::<proto::WatchRunResponse>(&frame) {
                    Ok(Some(msg)) => return Ok(Some(msg)),
                    Ok(None) => {
                        self.finished = true;
                        return Ok(None);
                    }
                    Err(err) => {
                        self.finished = true;
                        return Err(err.into());
                    }
                }
            }
            match self.response.chunk().await {
                Ok(Some(chunk)) => self.decoder.push(&chunk),
                Ok(None) => {
                    self.finished = true;
                    if self.decoder.pending_bytes() > 0 {
                        return Err(Error::Message("truncated connect stream".into()));
                    }
                    return Ok(None);
                }
                Err(err) => {
                    self.finished = true;
                    return Err(Error::Message(err.to_string()));
                }
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct DispatchClient {
    rpc: RpcTransport,
}

macro_rules! unary_rpc {
    ($fn_name:ident, $method:literal, $req:ty, $resp:ty) => {
        pub async fn $fn_name(&self, req: &$req) -> Result<$resp, Error> {
            Ok(self
                .rpc
                .unary(concat!("dispatch.v1.DispatchService/", $method), req)
                .await?)
        }
    };
}

impl DispatchClient {
    unary_rpc!(
        list_sections,
        "ListSections",
        proto::ListSectionsRequest,
        proto::ListSectionsResponse
    );
    unary_rpc!(
        save_section,
        "SaveSection",
        proto::SaveSectionRequest,
        proto::SaveSectionResponse
    );
    unary_rpc!(
        delete_section,
        "DeleteSection",
        proto::DeleteSectionRequest,
        proto::DeleteSectionResponse
    );
    unary_rpc!(
        move_to_section,
        "MoveToSection",
        proto::MoveToSectionRequest,
        proto::MoveToSectionResponse
    );
    unary_rpc!(
        list_runs,
        "ListRuns",
        proto::ListRunsRequest,
        proto::ListRunsResponse
    );
    unary_rpc!(
        get_run,
        "GetRun",
        proto::GetRunRequest,
        proto::GetRunResponse
    );
    unary_rpc!(
        start_run,
        "StartRun",
        proto::StartRunRequest,
        proto::StartRunResponse
    );
    unary_rpc!(
        cancel_run,
        "CancelRun",
        proto::CancelRunRequest,
        proto::CancelRunResponse
    );
    unary_rpc!(
        result,
        "Result",
        proto::ResultRequest,
        proto::ResultResponse
    );
    unary_rpc!(
        read_run_events,
        "ReadRunEvents",
        proto::ReadRunEventsRequest,
        proto::ReadRunEventsResponse
    );
    unary_rpc!(
        list_worktrees,
        "ListWorktrees",
        proto::ListWorktreesRequest,
        proto::ListWorktreesResponse
    );
    unary_rpc!(
        list_quota,
        "ListQuota",
        proto::ListQuotaRequest,
        proto::ListQuotaResponse
    );
    unary_rpc!(
        list_tiers,
        "ListTiers",
        proto::ListTiersRequest,
        proto::ListTiersResponse
    );
    unary_rpc!(
        list_agents,
        "ListAgents",
        proto::ListAgentsRequest,
        proto::ListAgentsResponse
    );
    unary_rpc!(
        list_history,
        "ListHistory",
        proto::ListHistoryRequest,
        proto::ListHistoryResponse
    );
    unary_rpc!(
        check_tiers,
        "CheckTiers",
        proto::CheckTiersRequest,
        proto::CheckTiersResponse
    );
    unary_rpc!(
        list_models,
        "ListModels",
        proto::ListModelsRequest,
        proto::ListModelsResponse
    );
    unary_rpc!(
        list_model_settings,
        "ListModelSettings",
        proto::ListModelSettingsRequest,
        proto::ListModelSettingsResponse
    );
    unary_rpc!(
        set_model_enabled,
        "SetModelEnabled",
        proto::SetModelEnabledRequest,
        proto::SetModelEnabledResponse
    );
    unary_rpc!(
        replace_tiers,
        "ReplaceTiers",
        proto::ReplaceTiersRequest,
        proto::ReplaceTiersResponse
    );
    unary_rpc!(stats, "Stats", proto::StatsRequest, proto::StatsResponse);
    unary_rpc!(
        get_route_decision,
        "GetRouteDecision",
        proto::GetRouteDecisionRequest,
        proto::GetRouteDecisionResponse
    );
    unary_rpc!(
        build_tiers,
        "BuildTiers",
        proto::BuildTiersRequest,
        proto::BuildTiersResponse
    );
    unary_rpc!(
        capacity,
        "Capacity",
        proto::CapacityRequest,
        proto::CapacityResponse
    );
    unary_rpc!(route, "Route", proto::RouteRequest, proto::RouteResponse);
    unary_rpc!(
        classify,
        "Classify",
        proto::ClassifyRequest,
        proto::ClassifyResponse
    );
    unary_rpc!(
        list_model_catalog,
        "ListModelCatalog",
        proto::ListModelCatalogRequest,
        proto::ListModelCatalogResponse
    );
    unary_rpc!(
        refresh_model_catalog,
        "RefreshModelCatalog",
        proto::RefreshModelCatalogRequest,
        proto::RefreshModelCatalogResponse
    );
    unary_rpc!(
        list_model_benchmarks,
        "ListModelBenchmarks",
        proto::ListModelBenchmarksRequest,
        proto::ListModelBenchmarksResponse
    );
    unary_rpc!(
        merge_run,
        "MergeRun",
        proto::MergeRunRequest,
        proto::MergeRunResponse
    );
    unary_rpc!(
        clean_run,
        "CleanRun",
        proto::CleanRunRequest,
        proto::CleanRunResponse
    );
    unary_rpc!(
        rename_conversation,
        "RenameConversation",
        proto::RenameConversationRequest,
        proto::RenameConversationResponse
    );
    unary_rpc!(
        delete_runs,
        "DeleteRuns",
        proto::DeleteRunsRequest,
        proto::DeleteRunsResponse
    );
    unary_rpc!(
        list_worktree_folders,
        "ListWorktreeFolders",
        proto::ListWorktreeFoldersRequest,
        proto::ListWorktreeFoldersResponse
    );
    unary_rpc!(
        prune_worktree_folders,
        "PruneWorktreeFolders",
        proto::PruneWorktreeFoldersRequest,
        proto::PruneWorktreeFoldersResponse
    );
    unary_rpc!(
        recover_run,
        "RecoverRun",
        proto::RecoverRunRequest,
        proto::RecoverRunResponse
    );
    unary_rpc!(
        config,
        "Config",
        proto::ConfigRequest,
        proto::ConfigResponse
    );
    unary_rpc!(
        subagents,
        "Subagents",
        proto::SubagentsRequest,
        proto::SubagentsResponse
    );
    unary_rpc!(skill, "Skill", proto::SkillRequest, proto::SkillResponse);
    unary_rpc!(
        get_agent_setup,
        "GetAgentSetup",
        proto::GetAgentSetupRequest,
        proto::GetAgentSetupResponse
    );
    unary_rpc!(
        apply_agent_setup,
        "ApplyAgentSetup",
        proto::ApplyAgentSetupRequest,
        proto::ApplyAgentSetupResponse
    );

    pub async fn watch_run(&self, req: &proto::WatchRunRequest) -> Result<WatchRunStream, Error> {
        let response = self
            .rpc
            .open_stream("dispatch.v1.DispatchService/WatchRun", req)
            .await?;
        Ok(WatchRunStream {
            response,
            decoder: EnvelopeDecoder::new(),
            finished: false,
        })
    }
}
