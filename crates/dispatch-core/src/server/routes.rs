//! The core's Connect procedures and the session token every one but
//! Health.Check requires.

use super::{
    Core, RpcError,
    connect::{stream_request, stream_response, unary_request, unary_response},
};
use crate::proto as pb;
use axum::{
    Router,
    body::Bytes,
    extract::State,
    http::HeaderMap,
    response::{IntoResponse, Response},
    routing::post,
};
use std::sync::Arc;
use std::time::Duration;

/// Authenticates a call with the session token and records activity for
/// idle exit. Health.Check answers without it, so a client can tell a live
/// core from a stale core.json.
fn authorize(core: &Core, headers: &HeaderMap) -> Result<(), RpcError> {
    let value = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let ok = value
        .strip_prefix("Bearer ")
        .is_some_and(|t| constant_time_eq(t.as_bytes(), core.token.as_bytes()));
    if !ok {
        return Err(RpcError::unauthenticated(
            "invalid or missing session token",
        ));
    }
    core.touch();
    Ok(())
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

macro_rules! unary {
    ($router:expr, $proc:literal, $method:ident) => {
        $router.route(
            $proc,
            post(
                |State(core): State<Arc<Core>>, headers: HeaderMap, body: Bytes| async move {
                    if let Err(e) = authorize(&core, &headers) {
                        return e.into_response();
                    }
                    match unary_request(&headers, &body) {
                        Ok((codec, req)) => unary_response(codec, core.$method(req).await),
                        Err(e) => e.into_response(),
                    }
                },
            ),
        )
    };
}

async fn check(State(core): State<Arc<Core>>, headers: HeaderMap, body: Bytes) -> Response {
    match unary_request::<pb::CheckRequest>(&headers, &body) {
        Ok((codec, _)) => unary_response(
            codec,
            Ok(pb::CheckResponse {
                status: "ok".into(),
                version: "0.1.0".into(),
                uptime_sec: core.started.elapsed().as_secs_f64(),
                build: core.build.clone(),
                pid: i32::try_from(std::process::id()).unwrap_or(0),
                active_runs: core
                    .store
                    .count_active_runs()
                    .ok()
                    .and_then(|n| i32::try_from(n).ok())
                    .unwrap_or(0),
            }),
        ),
        Err(e) => e.into_response(),
    }
}

async fn shutdown(State(core): State<Arc<Core>>, headers: HeaderMap, body: Bytes) -> Response {
    if let Err(e) = authorize(&core, &headers) {
        return e.into_response();
    }
    match unary_request::<pb::ShutdownRequest>(&headers, &body) {
        Ok((codec, _)) => {
            // Let the response reach the caller before the server stops.
            let c = core.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(100)).await;
                c.stop();
            });
            unary_response(codec, Ok(pb::ShutdownResponse {}))
        }
        Err(e) => e.into_response(),
    }
}

/// The client a call names in `Polterware-Client`, as the desktop app does.
fn client_name(headers: &HeaderMap) -> String {
    headers
        .get("polterware-client")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_owned()
}

/// StartRun reads the client header: the app's own requests record their caller as the app's.
async fn start_run(State(core): State<Arc<Core>>, headers: HeaderMap, body: Bytes) -> Response {
    if let Err(e) = authorize(&core, &headers) {
        return e.into_response();
    }
    let client = client_name(&headers);
    match unary_request::<pb::StartRunRequest>(&headers, &body) {
        Ok((codec, req)) => unary_response(codec, core.start_run(req, &client).await),
        Err(e) => e.into_response(),
    }
}

async fn watch_run(State(core): State<Arc<Core>>, headers: HeaderMap, body: Bytes) -> Response {
    if let Err(e) = authorize(&core, &headers) {
        return e.into_response();
    }
    let (codec, req) = match stream_request::<pb::WatchRunRequest>(&headers, &body) {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    match core.watch_run(req) {
        Ok(messages) => stream_response(codec, messages),
        Err(e) => stream_response(
            codec,
            futures::stream::iter([Err::<pb::WatchRunResponse, _>(e)]),
        ),
    }
}

pub(super) fn router(core: Arc<Core>) -> Router {
    let r = Router::new()
        .route("/dispatch.v1.HealthService/Check", post(check))
        .route("/dispatch.v1.HealthService/Shutdown", post(shutdown))
        .route("/dispatch.v1.DispatchService/WatchRun", post(watch_run))
        .route("/dispatch.v1.DispatchService/StartRun", post(start_run));
    let r = unary!(
        r,
        "/dispatch.v1.DispatchService/ListSections",
        list_sections
    );
    let r = unary!(r, "/dispatch.v1.DispatchService/SaveSection", save_section);
    let r = unary!(
        r,
        "/dispatch.v1.DispatchService/DeleteSection",
        delete_section
    );
    let r = unary!(
        r,
        "/dispatch.v1.DispatchService/RenameConversation",
        rename_conversation
    );
    let r = unary!(
        r,
        "/dispatch.v1.DispatchService/MoveToSection",
        move_to_section
    );
    let r = unary!(r, "/dispatch.v1.DispatchService/ListRuns", list_runs);
    let r = unary!(r, "/dispatch.v1.DispatchService/GetRun", get_run);
    let r = unary!(
        r,
        "/dispatch.v1.DispatchService/ReadRunEvents",
        read_run_events
    );
    let r = unary!(r, "/dispatch.v1.DispatchService/CancelRun", cancel_run);
    let r = unary!(r, "/dispatch.v1.DispatchService/Result", result);
    let r = unary!(
        r,
        "/dispatch.v1.DispatchService/ListWorktrees",
        list_worktrees
    );
    let r = unary!(
        r,
        "/dispatch.v1.DispatchService/ListArtifacts",
        list_artifacts
    );
    let r = unary!(
        r,
        "/dispatch.v1.DispatchService/DeleteArtifact",
        delete_artifact
    );
    let r = unary!(r, "/dispatch.v1.DispatchService/ListQuota", list_quota);
    let r = unary!(r, "/dispatch.v1.DispatchService/ListTiers", list_tiers);
    let r = unary!(r, "/dispatch.v1.DispatchService/ListAgents", list_agents);
    let r = unary!(r, "/dispatch.v1.DispatchService/ListHistory", list_history);
    let r = unary!(r, "/dispatch.v1.DispatchService/CheckTiers", check_tiers);
    let r = unary!(r, "/dispatch.v1.DispatchService/ListModels", list_models);
    let r = unary!(
        r,
        "/dispatch.v1.DispatchService/ListModelSettings",
        list_model_settings
    );
    let r = unary!(
        r,
        "/dispatch.v1.DispatchService/SetModelEnabled",
        set_model_enabled
    );
    let r = unary!(
        r,
        "/dispatch.v1.DispatchService/SetAgentEnabled",
        set_agent_enabled
    );
    let r = unary!(
        r,
        "/dispatch.v1.DispatchService/ReplaceTiers",
        replace_tiers
    );
    let r = unary!(r, "/dispatch.v1.DispatchService/Stats", stats);
    let r = unary!(
        r,
        "/dispatch.v1.DispatchService/GetConversationUsage",
        conversation_usage
    );
    let r = unary!(
        r,
        "/dispatch.v1.DispatchService/GetRouteDecision",
        get_route_decision
    );
    let r = unary!(r, "/dispatch.v1.DispatchService/BuildTiers", build_tiers);
    let r = unary!(r, "/dispatch.v1.DispatchService/Capacity", capacity);
    let r = unary!(r, "/dispatch.v1.DispatchService/Route", route);
    let r = unary!(r, "/dispatch.v1.DispatchService/Classify", classify);
    let r = unary!(
        r,
        "/dispatch.v1.DispatchService/ListModelCatalog",
        list_model_catalog
    );
    let r = unary!(
        r,
        "/dispatch.v1.DispatchService/RefreshModelCatalog",
        refresh_model_catalog
    );
    let r = unary!(
        r,
        "/dispatch.v1.DispatchService/ListModelBenchmarks",
        list_model_benchmarks
    );
    let r = unary!(r, "/dispatch.v1.DispatchService/MergeRun", merge_run);
    let r = unary!(r, "/dispatch.v1.DispatchService/CleanRun", clean_run);
    let r = unary!(r, "/dispatch.v1.DispatchService/DeleteRuns", delete_runs);
    let r = unary!(
        r,
        "/dispatch.v1.DispatchService/ListWorktreeFolders",
        list_worktree_folders
    );
    let r = unary!(
        r,
        "/dispatch.v1.DispatchService/PruneWorktreeFolders",
        prune_worktree_folders
    );
    let r = unary!(r, "/dispatch.v1.DispatchService/RecoverRun", recover_run);
    let r = unary!(r, "/dispatch.v1.DispatchService/Config", config);
    let r = unary!(r, "/dispatch.v1.DispatchService/Subagents", subagents);
    let r = unary!(r, "/dispatch.v1.DispatchService/Skill", skill);
    let r = unary!(
        r,
        "/dispatch.v1.DispatchService/GetAgentSetup",
        get_agent_setup
    );
    let r = unary!(
        r,
        "/dispatch.v1.DispatchService/ApplyAgentSetup",
        apply_agent_setup
    );
    let r = unary!(
        r,
        "/dispatch.v1.WorkspaceService/ListWorkspaces",
        list_workspaces
    );
    let r = unary!(
        r,
        "/dispatch.v1.WorkspaceService/AddWorkspace",
        add_workspace
    );
    let r = unary!(
        r,
        "/dispatch.v1.WorkspaceService/RemoveWorkspace",
        remove_workspace
    );
    let r = unary!(
        r,
        "/dispatch.v1.WorkspaceService/ResolveWorkspace",
        resolve_workspace
    );
    let r = unary!(
        r,
        "/dispatch.v1.WorkspaceService/ListWorkspaceCards",
        list_workspace_cards
    );
    r.with_state(core)
}
