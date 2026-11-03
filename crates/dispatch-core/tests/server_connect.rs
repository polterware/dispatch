#![forbid(unsafe_code)]

//! The core's Connect server end to end: the session token, both codecs, and
//! the errors and shutdown the CLI, the app, and Polterware rely on.

use dispatch_core::{
    coreclient,
    host::NoHost,
    proto::{
        AddWorkspaceRequest, CheckRequest, CheckResponse, GetRunRequest, ListRunsRequest,
        ListWorkspacesRequest, ShutdownRequest,
    },
    runstate,
    server::{self, Core},
    store::Store,
};
use prost::Message;
use std::sync::Arc;
use std::time::Duration;

struct Running {
    core: Arc<Core>,
    endpoint: String,
    token: String,
    _dir: tempfile::TempDir,
}

async fn start() -> Running {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(":memory:").unwrap();
    let token = server::mint_token();
    let core = Core::new(
        Arc::new(store),
        token.clone(),
        Arc::new(NoHost),
        dir.path().to_path_buf(),
        "1-1".into(),
    );
    let (listener, endpoint) = server::listen("127.0.0.1:0").await.unwrap();
    tokio::spawn(server::serve(listener, core.clone()));
    Running {
        core,
        endpoint,
        token,
        _dir: dir,
    }
}

fn clients(r: &Running) -> coreclient::Clients {
    coreclient::new(
        &runstate::Core {
            endpoint: r.endpoint.clone(),
            token: r.token.clone(),
            ..runstate::Core::default()
        },
        "test",
    )
}

#[tokio::test]
async fn serves_json_with_the_session_token() {
    let r = start().await;
    let c = clients(&r);

    let health = c.health.check(&CheckRequest {}).await.unwrap();
    assert_eq!(health.status, "ok");
    assert_eq!(health.build, "1-1");

    let folder = tempfile::tempdir().unwrap();
    let added = c
        .workspaces
        .add_workspace(&AddWorkspaceRequest {
            path: folder.path().to_string_lossy().into_owned(),
            name: "Repo".into(),
        })
        .await
        .unwrap()
        .workspace
        .unwrap();
    assert!(added.id.starts_with("ws-"));
    let listed = c
        .workspaces
        .list_workspaces(&ListWorkspacesRequest {})
        .await
        .unwrap();
    assert!(!listed.linked);
    assert_eq!(listed.workspaces.len(), 1);

    let runs = c
        .dispatch
        .list_runs(&ListRunsRequest {
            project_id: added.id.clone(),
            ..ListRunsRequest::default()
        })
        .await
        .unwrap();
    assert!(runs.runs.is_empty());

    let err = c
        .dispatch
        .get_run(&GetRunRequest {
            id: "missing".into(),
        })
        .await
        .unwrap_err();
    assert!(err.to_string().starts_with("not_found: "), "{err}");
}

#[tokio::test]
async fn refuses_calls_without_the_token_except_check() {
    let r = start().await;
    let http = reqwest::Client::new();
    let post = |path: &str, token: Option<&str>| {
        let mut req = http
            .post(format!("{}/dispatch.v1.{path}", r.endpoint))
            .header("content-type", "application/json")
            .body("{}");
        if let Some(t) = token {
            req = req.bearer_auth(t);
        }
        req.send()
    };

    let res = post("DispatchService/ListRuns", None).await.unwrap();
    assert_eq!(res.status(), 401);
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["code"], "unauthenticated");

    let res = post("DispatchService/ListRuns", Some("wrong"))
        .await
        .unwrap();
    assert_eq!(res.status(), 401);

    let res = post("HealthService/Check", None).await.unwrap();
    assert_eq!(res.status(), 200);
}

#[tokio::test]
async fn speaks_binary_proto_as_connect_node_does() {
    let r = start().await;
    let res = reqwest::Client::new()
        .post(format!("{}/dispatch.v1.HealthService/Check", r.endpoint))
        .header("content-type", "application/proto")
        .body(CheckRequest {}.encode_to_vec())
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(
        res.headers()["content-type"].to_str().unwrap(),
        "application/proto"
    );
    let health = CheckResponse::decode(res.bytes().await.unwrap()).unwrap();
    assert_eq!(health.status, "ok");
}

#[tokio::test]
async fn shutdown_stops_the_core() {
    let r = start().await;
    clients(&r)
        .health
        .shutdown(&ShutdownRequest {
            reason: "test".into(),
        })
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), r.core.done().cancelled())
        .await
        .expect("core stopped");
}
