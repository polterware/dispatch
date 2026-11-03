#![forbid(unsafe_code)]

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::post,
};
use chrono::Utc;
use dispatch_core::{
    Error,
    host::{
        BoxFuture, CLIENT_HEADER, CLIENT_NAME, Card, CardsAndRepos, Client, ERR_NO_HOST, Host,
        NoHost, NoneHost, Project, Repository, Run, default_db, is_no_host,
    },
    proto::host::{
        CompleteHostAgentRunRequest, CompleteHostAgentRunResponse, HostCard, HostProject,
        HostRepository, ListHostCardsRequest, ListHostCardsResponse, ListHostProjectsResponse,
        ResolveHostProjectRequest, ResolveHostProjectResponse, ResolveRunRequest,
        ResolveRunResponse, StartHostAgentRunRequest, StartHostAgentRunResponse,
    },
    runstate::{self, Core},
};
use nix::{
    sys::signal::{Signal, kill},
    unistd::Pid,
};
use serde_json::json;
use std::{
    env, fs,
    os::unix::fs::PermissionsExt,
    path::Path,
    process::Command,
    sync::{Arc, Mutex},
    time::Duration,
};
use tempfile::TempDir;
use tokio::net::TcpListener;

struct FakeHost {
    project: Project,
}

impl Host for FakeHost {
    fn projects(&self) -> BoxFuture<'_, Result<Vec<Project>, Error>> {
        let p = self.project.clone();
        Box::pin(async move { Ok(vec![p]) })
    }

    fn resolve_project<'a>(
        &'a self,
        _id: &'a str,
        _dir: &'a str,
    ) -> BoxFuture<'a, Result<Project, Error>> {
        let p = self.project.clone();
        Box::pin(async move { Ok(p) })
    }

    fn resolve_run<'a>(
        &'a self,
        _project_id: &'a str,
        _card_slug: &'a str,
        _repo: &'a str,
        dir: &'a str,
    ) -> BoxFuture<'a, Result<Run, Error>> {
        let d = dir.to_owned();
        Box::pin(async move {
            Ok(Run {
                dir: d,
                kinds: vec!["backend".into()],
                tech: vec!["rust".into()],
                card_text: "Port host module".into(),
            })
        })
    }

    fn cards<'a>(&'a self, _project_id: &'a str) -> BoxFuture<'a, Result<CardsAndRepos, Error>> {
        Box::pin(async {
            Ok((
                vec![Card {
                    slug: "c-1".into(),
                    title: "Card 1".into(),
                    milestone: "M1".into(),
                    stage: "doing".into(),
                }],
                vec![Repository {
                    name: "dispatch".into(),
                    remote: "git@example.com:pw/dispatch.git".into(),
                    local_path: "/tmp/dispatch".into(),
                }],
            ))
        })
    }

    fn start_agent_run<'a>(
        &'a self,
        _project_id: &'a str,
        _task: &'a str,
        _card_slug: &'a str,
    ) -> BoxFuture<'a, Result<String, Error>> {
        Box::pin(async { Ok("ar-123".into()) })
    }

    fn complete_agent_run<'a>(&'a self, _id: &'a str) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async { Ok(()) })
    }
}

#[tokio::test]
async fn test_no_host_and_trait_object_dispatch() {
    let none: Box<dyn Host> = Box::new(NoneHost);
    assert!(is_no_host(&none.projects().await.expect_err("projects")));
    assert!(is_no_host(
        &none
            .resolve_project("p1", "/tmp")
            .await
            .expect_err("resolve_project")
    ));
    assert!(is_no_host(
        &none
            .resolve_run("p1", "c1", "r1", "/tmp")
            .await
            .expect_err("resolve_run")
    ));
    assert!(is_no_host(&none.cards("p1").await.expect_err("cards")));
    assert!(is_no_host(
        &none
            .start_agent_run("p1", "task", "c1")
            .await
            .expect_err("start_agent_run")
    ));
    assert_eq!(
        NoHost
            .complete_agent_run("ar-1")
            .await
            .expect_err("complete_agent_run")
            .to_string(),
        ERR_NO_HOST
    );

    let fake: Arc<dyn Host> = Arc::new(FakeHost {
        project: Project {
            id: "pw-1".into(),
            name: "Polterware".into(),
        },
    });
    assert_eq!(fake.projects().await.expect("projects").len(), 1);
    assert_eq!(
        fake.start_agent_run("pw-1", "port", "c-1")
            .await
            .expect("start"),
        "ar-123"
    );
    fake.complete_agent_run("ar-123").await.expect("complete");
}

#[derive(Default)]
struct HostMockState {
    auth_headers: Vec<String>,
    client_headers: Vec<String>,
    completed_ids: Vec<String>,
}

fn record_host_headers(st: &mut HostMockState, headers: &HeaderMap) {
    if let Some(a) = headers.get("authorization").and_then(|v| v.to_str().ok()) {
        st.auth_headers.push(a.to_owned());
    }
    if let Some(c) = headers.get(CLIENT_HEADER).and_then(|v| v.to_str().ok()) {
        st.client_headers.push(c.to_owned());
    }
}

fn build_mock_host_router(state: Arc<Mutex<HostMockState>>) -> Router {
    Router::new()
        .route(
            "/polterware.local.v1.DispatchHostService/ListHostProjects",
            post(
                |State(st): State<Arc<Mutex<HostMockState>>>, headers: HeaderMap| async move {
                    record_host_headers(&mut st.lock().expect("lock"), &headers);
                    Json(ListHostProjectsResponse {
                        projects: vec![HostProject {
                            id: "proj-1".into(),
                            name: "Dispatch GPUI".into(),
                        }],
                    })
                },
            ),
        )
        .route(
            "/polterware.local.v1.DispatchHostService/ResolveHostProject",
            post(
                |State(st): State<Arc<Mutex<HostMockState>>>,
                 headers: HeaderMap,
                 Json(req): Json<ResolveHostProjectRequest>| async move {
                    record_host_headers(&mut st.lock().expect("lock"), &headers);
                    if req.id == "missing" {
                        return (
                            StatusCode::NOT_FOUND,
                            Json(
                                json!({"code": "not_found", "message": "unknown project missing"}),
                            ),
                        )
                            .into_response();
                    }
                    if req.id == "down" {
                        return (
                            StatusCode::SERVICE_UNAVAILABLE,
                            Json(json!({"code": "unavailable", "message": "shutting down"})),
                        )
                            .into_response();
                    }
                    if req.id == "unimpl" {
                        return (
                            StatusCode::NOT_FOUND,
                            Json(json!({"code": "unimplemented", "message": "not implemented"})),
                        )
                            .into_response();
                    }
                    if req.id == "unk" {
                        return (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            Json(json!({"code": "unknown", "message": "mystery"})),
                        )
                            .into_response();
                    }
                    Json(ResolveHostProjectResponse {
                        project: Some(HostProject {
                            id: if req.id.is_empty() {
                                "proj-by-dir".into()
                            } else {
                                req.id
                            },
                            name: "Resolved".into(),
                        }),
                    })
                    .into_response()
                },
            ),
        )
        .route(
            "/polterware.local.v1.DispatchHostService/ResolveRun",
            post(
                |State(st): State<Arc<Mutex<HostMockState>>>,
                 headers: HeaderMap,
                 Json(req): Json<ResolveRunRequest>| async move {
                    record_host_headers(&mut st.lock().expect("lock"), &headers);
                    if req.repo == "ambiguous" {
                        return (
                            StatusCode::BAD_REQUEST,
                            Json(json!({
                                "code": "invalid_argument",
                                "message": "project has multiple repositories; pass --repo"
                            })),
                        )
                            .into_response();
                    }
                    Json(ResolveRunResponse {
                        dir: format!("/repos/{}/{}", req.project_id, req.repo),
                        kinds: vec!["core".into()],
                        tech: vec!["rust".into(), "gpui".into()],
                        card_text: format!("Card {}", req.card_slug),
                    })
                    .into_response()
                },
            ),
        )
        .route(
            "/polterware.local.v1.DispatchHostService/ListHostCards",
            post(
                |State(st): State<Arc<Mutex<HostMockState>>>,
                 headers: HeaderMap,
                 Json(req): Json<ListHostCardsRequest>| async move {
                    record_host_headers(&mut st.lock().expect("lock"), &headers);
                    Json(ListHostCardsResponse {
                        cards: vec![HostCard {
                            slug: format!("{}-card", req.project_id),
                            title: "Port Host".into(),
                            milestone: "M2".into(),
                            stage: "in_progress".into(),
                        }],
                        repositories: vec![HostRepository {
                            name: "dispatch-gpui".into(),
                            remote: "https://example.com/dispatch-gpui.git".into(),
                            local_path: "/work/dispatch-gpui".into(),
                        }],
                    })
                },
            ),
        )
        .route(
            "/polterware.local.v1.DispatchHostService/StartHostAgentRun",
            post(
                |State(st): State<Arc<Mutex<HostMockState>>>,
                 headers: HeaderMap,
                 Json(req): Json<StartHostAgentRunRequest>| async move {
                    record_host_headers(&mut st.lock().expect("lock"), &headers);
                    Json(StartHostAgentRunResponse {
                        agent_run_id: format!("agent-run-{}-{}", req.project_id, req.card_slug),
                    })
                },
            ),
        )
        .route(
            "/polterware.local.v1.DispatchHostService/CompleteHostAgentRun",
            post(
                |State(st): State<Arc<Mutex<HostMockState>>>,
                 headers: HeaderMap,
                 Json(req): Json<CompleteHostAgentRunRequest>| async move {
                    let mut guard = st.lock().expect("lock");
                    record_host_headers(&mut guard, &headers);
                    guard.completed_ids.push(req.agent_run_id);
                    Json(CompleteHostAgentRunResponse {})
                },
            ),
        )
        .with_state(state)
}

#[tokio::test]
async fn test_client_dispatch_host_service_rpcs_headers_and_error_mapping() {
    let state = Arc::new(Mutex::new(HostMockState::default()));
    let app = build_mock_host_router(Arc::clone(&state));

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local_addr");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    let tmp = TempDir::new().expect("tempdir");
    let db_path = tmp.path().join("polterware").join("local.db");
    let core = Core {
        pid: i32::try_from(std::process::id()).expect("pid"),
        endpoint: format!("http://{addr}/"),
        token: "pw-bearer-tok".into(),
        db_path: db_path.to_string_lossy().into_owned(),
        build: "1700000000-10".into(),
        started_at: Utc::now(),
    };
    runstate::write_json(runstate::core_path(&db_path), &core).expect("write core.json");

    let client: Arc<dyn Host> = Arc::new(Client::new(&db_path));

    let projects = client.projects().await.expect("projects");
    assert_eq!(
        projects,
        vec![Project {
            id: "proj-1".into(),
            name: "Dispatch GPUI".into(),
        }]
    );

    let resolved = client
        .resolve_project("", "/work/dispatch-gpui")
        .await
        .expect("resolve_project");
    assert_eq!(resolved.id, "proj-by-dir");

    let run = client
        .resolve_run("proj-1", "host-port", "main", "")
        .await
        .expect("resolve_run");
    assert_eq!(
        run,
        Run {
            dir: "/repos/proj-1/main".into(),
            kinds: vec!["core".into()],
            tech: vec!["rust".into(), "gpui".into()],
            card_text: "Card host-port".into(),
        }
    );

    let (cards, repos) = client.cards("proj-1").await.expect("cards");
    assert_eq!(cards.len(), 1);
    assert_eq!(cards[0].slug, "proj-1-card");
    assert_eq!(repos.len(), 1);
    assert_eq!(repos[0].name, "dispatch-gpui");

    let agent_run_id = client
        .start_agent_run("proj-1", "Port lane host", "c42")
        .await
        .expect("start_agent_run");
    assert_eq!(agent_run_id, "agent-run-proj-1-c42");

    client
        .complete_agent_run(&agent_run_id)
        .await
        .expect("complete_agent_run");

    {
        let guard = state.lock().expect("lock");
        assert_eq!(guard.completed_ids, vec!["agent-run-proj-1-c42"]);
        assert!(!guard.auth_headers.is_empty());
        assert!(
            guard
                .auth_headers
                .iter()
                .all(|h| h == "Bearer pw-bearer-tok")
        );
        assert!(guard.client_headers.iter().all(|h| h == CLIENT_NAME));
    }

    // Domain errors from Polterware preserve the exact remote message without Connect code prefix.
    let not_found = client
        .resolve_project("missing", "")
        .await
        .expect_err("missing project");
    assert!(!is_no_host(&not_found));
    assert_eq!(not_found.to_string(), "unknown project missing");

    let invalid_arg = client
        .resolve_run("proj-1", "c1", "ambiguous", "")
        .await
        .expect_err("ambiguous repo");
    assert!(!is_no_host(&invalid_arg));
    assert_eq!(
        invalid_arg.to_string(),
        "project has multiple repositories; pass --repo"
    );

    // unavailable / unimplemented / unknown map to ERR_NO_HOST.
    for code_id in ["down", "unimpl", "unk"] {
        let err = client
            .resolve_project(code_id, "")
            .await
            .expect_err("unavailable mapping");
        assert!(is_no_host(&err), "id={code_id} err={err}");
    }

    // Dead transport endpoint maps to ERR_NO_HOST.
    let dead_listener = TcpListener::bind("127.0.0.1:0").await.expect("bind dead");
    let dead_addr = dead_listener.local_addr().expect("addr");
    drop(dead_listener);
    let dead_core = Core {
        endpoint: format!("http://{dead_addr}"),
        ..core
    };
    runstate::write_json(runstate::core_path(&db_path), &dead_core).expect("overwrite core.json");
    let transport_err = client.projects().await.expect_err("dead endpoint");
    assert!(is_no_host(&transport_err), "err={transport_err}");
}

fn write_fake_polterware_binary(path: &Path, endpoint: &str) {
    let script = format!(
        r#"#!/bin/sh
DB="$2"
RUN_DIR="$(dirname "$DB")/run"
mkdir -p "$RUN_DIR"
/bin/sleep 30 &
SLEEP_PID=$!
trap 'kill "$SLEEP_PID" 2>/dev/null; rm -f "$RUN_DIR/core.json"; exit 0' TERM INT
cat > "$RUN_DIR/core.json.tmp" <<EOF
{{
  "pid": $$,
  "endpoint": "{endpoint}",
  "token": "autostart-tok",
  "dbPath": "$DB",
  "build": "1700000000-1",
  "startedAt": "2026-04-20T00:00:00Z"
}}
EOF
chmod 600 "$RUN_DIR/core.json.tmp"
mv "$RUN_DIR/core.json.tmp" "$RUN_DIR/core.json"
wait "$SLEEP_PID"
"#
    );
    fs::write(path, script).expect("write fake polterware");
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("chmod +x");
}

#[tokio::test]
async fn test_client_missing_db_and_autostart_when_db_exists() {
    let tmp = TempDir::new().expect("tempdir");

    // 1. When DB file does not exist, Client does not auto-start and returns ERR_NO_HOST.
    let absent_db = tmp.path().join("never-opened").join("local.db");
    let absent_client = Client::new(&absent_db);
    let err = absent_client.projects().await.expect_err("absent db");
    assert!(is_no_host(&err));
    assert!(!absent_db.exists());

    // 2. When DB file exists, Client auto-starts Binary and connects once core.json appears.
    let state = Arc::new(Mutex::new(HostMockState::default()));
    let app = build_mock_host_router(Arc::clone(&state));
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local_addr");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    let existing_db = tmp.path().join("opened").join("local.db");
    fs::create_dir_all(existing_db.parent().expect("parent")).expect("mkdir");
    fs::write(&existing_db, b"").expect("touch db");

    let bin_path = tmp.path().join("fake-polterware");
    write_fake_polterware_binary(&bin_path, &format!("http://{addr}"));

    let client = Client::new(&existing_db)
        .with_binary(&bin_path)
        .with_start_timeout(Duration::from_secs(5));
    let projects = client.projects().await.expect("autostart projects");
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0].id, "proj-1");

    if let Some(spawned) = runstate::read_core(&existing_db) {
        let _ = kill(Pid::from_raw(spawned.pid), Signal::SIGTERM);
    }
}

#[test]
fn test_default_db_env_and_fallback() {
    if env::var("DISPATCH_TEST_SUBPROC_DEFAULT_DB").as_deref() == Ok("1") {
        assert_eq!(default_db(), Path::new("/tmp/custom-polterware/local.db"));
        return;
    }

    let fallback = default_db();
    if env::var("POLTERWARE_CORE_DB")
        .map(|s| s.trim().is_empty())
        .unwrap_or(true)
    {
        assert!(
            fallback.ends_with(Path::new("polterware").join("local.db")),
            "fallback = {}",
            fallback.display()
        );
    }

    let current_test_bin = env::current_exe().expect("current_exe");
    let output = Command::new(current_test_bin)
        .args(["--exact", "test_default_db_env_and_fallback", "--nocapture"])
        .env("DISPATCH_TEST_SUBPROC_DEFAULT_DB", "1")
        .env("POLTERWARE_CORE_DB", "  /tmp/custom-polterware/local.db  ")
        .output()
        .expect("spawn subprocess test");
    assert!(
        output.status.success(),
        "subprocess failed:\n{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
