#![forbid(unsafe_code)]

use axum::{
    Json, Router,
    body::{Body, Bytes},
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::post,
};
use chrono::Utc;
use dispatch_core::{
    coreclient::{
        self, CLIENT_HEADER, EnvelopeDecoder, build_id, decode_stream_frame, encode_envelope,
        ensure_with_exe, start_with_timeout, stop,
    },
    proto::{
        AddWorkspaceRequest, AddWorkspaceResponse, CheckRequest, CheckResponse, DispatchCaller,
        DispatchRun, GetRunRequest, ListRunsRequest, ListRunsResponse, ListWorkspacesRequest,
        ListWorkspacesResponse, ShutdownRequest, ShutdownResponse, StartRunRequest,
        StartRunResponse, WatchRunRequest, WatchRunResponse, Workspace,
    },
    runstate::{self, Core, pid_alive},
};
use nix::{
    sys::signal::{Signal, kill},
    unistd::Pid,
};
use serde_json::json;
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};
use tempfile::TempDir;
use tokio::net::TcpListener;

#[derive(Default)]
struct RecordedState {
    last_auth: Option<String>,
    last_client: Option<String>,
    last_protocol: Option<String>,
    start_req: Option<StartRunRequest>,
    list_req: Option<ListRunsRequest>,
    watch_req: Option<WatchRunRequest>,
}

fn capture_headers(state: &mut RecordedState, headers: &HeaderMap) {
    state.last_auth = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    state.last_client = headers
        .get(CLIENT_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    state.last_protocol = headers
        .get("connect-protocol-version")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
}

#[tokio::test]
async fn test_dispatch_client_forwards_caller_conversation_and_headers() {
    let shared = Arc::new(Mutex::new(RecordedState::default()));

    let app = Router::new()
        .route(
            "/dispatch.v1.DispatchService/StartRun",
            post(
                |State(st): State<Arc<Mutex<RecordedState>>>,
                 headers: HeaderMap,
                 Json(req): Json<StartRunRequest>| async move {
                    let mut guard = st.lock().expect("lock");
                    capture_headers(&mut guard, &headers);
                    guard.start_req = Some(req.clone());
                    let caller = req.caller.unwrap_or_default();
                    Json(StartRunResponse {
                        run: Some(DispatchRun {
                            id: "run".into(),
                            conversation_id: caller.conversation_id,
                            caller_agent: caller.agent,
                            ..Default::default()
                        }),
                    })
                },
            ),
        )
        .route(
            "/dispatch.v1.DispatchService/ListRuns",
            post(
                |State(st): State<Arc<Mutex<RecordedState>>>,
                 headers: HeaderMap,
                 Json(req): Json<ListRunsRequest>| async move {
                    let mut guard = st.lock().expect("lock");
                    capture_headers(&mut guard, &headers);
                    guard.list_req = Some(req.clone());
                    Json(ListRunsResponse {
                        runs: vec![DispatchRun {
                            id: "run".into(),
                            conversation_id: req.conversation_id,
                            caller_agent: "codex".into(),
                            ..Default::default()
                        }],
                    })
                },
            ),
        )
        .route(
            "/dispatch.v1.DispatchService/GetRun",
            post(|| async {
                (
                    StatusCode::NOT_FOUND,
                    Json(json!({"code": "not_found", "message": "run missing"})),
                )
            }),
        )
        .route(
            "/dispatch.v1.WorkspaceService/ListWorkspaces",
            post(
                |State(st): State<Arc<Mutex<RecordedState>>>, headers: HeaderMap| async move {
                    let mut guard = st.lock().expect("lock");
                    capture_headers(&mut guard, &headers);
                    Json(ListWorkspacesResponse {
                        workspaces: vec![Workspace {
                            id: "ws-1".into(),
                            name: "Main".into(),
                            path: "/tmp/main".into(),
                            ..Default::default()
                        }],
                        linked: true,
                    })
                },
            ),
        )
        .route(
            "/dispatch.v1.WorkspaceService/AddWorkspace",
            post(|Json(req): Json<AddWorkspaceRequest>| async move {
                Json(AddWorkspaceResponse {
                    workspace: Some(Workspace {
                        id: "ws-2".into(),
                        name: req.name,
                        path: req.path,
                        ..Default::default()
                    }),
                })
            }),
        )
        .route(
            "/dispatch.v1.HealthService/Check",
            post(|| async {
                Json(CheckResponse {
                    status: "ok".into(),
                    version: "0.1.0".into(),
                    uptime_sec: 1.5,
                    build: "1700000000-123".into(),
                    pid: 42,
                })
            }),
        )
        .with_state(Arc::clone(&shared));

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local_addr");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    let core = Core {
        pid: i32::try_from(std::process::id()).expect("pid"),
        endpoint: format!("http://{addr}/"),
        token: "secret-core-token".into(),
        db_path: "/tmp/dispatch.db".into(),
        build: "1700000000-123".into(),
        started_at: Utc::now(),
    };

    let clients = coreclient::new(&core, "cli");
    assert_eq!(clients.endpoint(), format!("http://{addr}"));

    let caller = DispatchCaller {
        agent: "codex".into(),
        session_id: "chat".into(),
        conversation_id: "codex:chat".into(),
        label: "Codex".into(),
    };
    let start_res = clients
        .dispatch
        .start_run(&StartRunRequest {
            project_id: "workspace".into(),
            prompt: "Read the code".into(),
            caller: Some(caller.clone()),
            ..Default::default()
        })
        .await
        .expect("start_run");

    {
        let guard = shared.lock().expect("lock");
        assert_eq!(guard.last_auth.as_deref(), Some("Bearer secret-core-token"));
        assert_eq!(guard.last_client.as_deref(), Some("cli"));
        assert_eq!(guard.last_protocol.as_deref(), Some("1"));
        let got_caller = guard
            .start_req
            .as_ref()
            .and_then(|r| r.caller.clone())
            .expect("recorded caller");
        assert_eq!(got_caller, caller);
    }

    let run = start_res.run.expect("run");
    assert_eq!(run.conversation_id, caller.conversation_id);
    assert_eq!(run.caller_agent, caller.agent);

    let list_res = clients
        .dispatch
        .list_runs(&ListRunsRequest {
            project_id: "workspace".into(),
            conversation_id: caller.conversation_id.clone(),
            limit: 2,
            offset: 1,
        })
        .await
        .expect("list_runs");
    assert_eq!(list_res.runs.len(), 1);
    assert_eq!(list_res.runs[0].conversation_id, caller.conversation_id);
    {
        let guard = shared.lock().expect("lock");
        let list_req = guard.list_req.as_ref().expect("recorded list_req");
        assert_eq!(list_req.conversation_id, caller.conversation_id);
        assert_eq!(list_req.limit, 2);
        assert_eq!(list_req.offset, 1);
    }

    // Empty client omits Pwdispatch-Client header.
    let anon = coreclient::new(&core, "");
    let ws = anon
        .workspaces
        .list_workspaces(&ListWorkspacesRequest {})
        .await
        .expect("list_workspaces");
    assert!(ws.linked);
    assert_eq!(ws.workspaces.len(), 1);
    {
        let guard = shared.lock().expect("lock");
        assert_eq!(guard.last_client, None);
    }

    let added = clients
        .workspaces
        .add_workspace(&AddWorkspaceRequest {
            path: "/tmp/repo".into(),
            name: "Repo".into(),
        })
        .await
        .expect("add_workspace");
    assert_eq!(added.workspace.expect("ws").name, "Repo");

    let health = clients.health.check(&CheckRequest {}).await.expect("check");
    assert_eq!(health.status, "ok");

    let err = clients
        .dispatch
        .get_run(&GetRunRequest {
            id: "missing".into(),
        })
        .await
        .expect_err("expected not_found error");
    assert_eq!(err.to_string(), "not_found: run missing");
}

#[tokio::test]
async fn test_watch_run_connect_server_stream_and_error_frame() {
    let shared = Arc::new(Mutex::new(RecordedState::default()));

    let app =
        Router::new()
            .route(
                "/dispatch.v1.DispatchService/WatchRun",
                post(
                    |State(st): State<Arc<Mutex<RecordedState>>>,
                     headers: HeaderMap,
                     body: Bytes| async move {
                        assert_eq!(
                            headers.get("content-type").and_then(|v| v.to_str().ok()),
                            Some("application/connect+json")
                        );
                        let mut dec = EnvelopeDecoder::new();
                        dec.push(&body);
                        let frame = dec.next_frame().expect("request envelope");
                        let req: WatchRunRequest = decode_stream_frame(&frame)
                            .expect("decode request")
                            .expect("request message");
                        {
                            let mut guard = st.lock().expect("lock");
                            capture_headers(&mut guard, &headers);
                            guard.watch_req = Some(req.clone());
                        }

                        let mut wire = Vec::new();
                        if req.id == "fail" {
                            let end_err =
                                br#"{"error":{"code":"aborted","message":"run canceled"}}"#;
                            wire.extend_from_slice(&encode_envelope(0x02, end_err));
                        } else {
                            let m1 = serde_json::to_vec(&WatchRunResponse {
                                seq: 11,
                                kind: "output".into(),
                                text: "line one".into(),
                                at: "2026-04-20T00:00:01Z".into(),
                                ..Default::default()
                            })
                            .expect("m1");
                            let m2 = serde_json::to_vec(&WatchRunResponse {
                                seq: 12,
                                kind: "done".into(),
                                text: "finished".into(),
                                at: "2026-04-20T00:00:02Z".into(),
                                ..Default::default()
                            })
                            .expect("m2");
                            wire.extend_from_slice(&encode_envelope(0x00, &m1));
                            wire.extend_from_slice(&encode_envelope(0x00, &m2));
                            wire.extend_from_slice(&encode_envelope(0x02, b"{}"));
                        }

                        // Split frames into small 7-byte chunks to exercise incremental framing.
                        let chunks: Vec<Result<Bytes, std::io::Error>> = wire
                            .chunks(7)
                            .map(|c| Ok(Bytes::copy_from_slice(c)))
                            .collect();
                        (
                            [("content-type", "application/connect+json")],
                            Body::from_stream(futures::stream::iter(chunks)),
                        )
                            .into_response()
                    },
                ),
            )
            .with_state(Arc::clone(&shared));

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local_addr");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    let core = Core {
        pid: i32::try_from(std::process::id()).expect("pid"),
        endpoint: format!("http://{addr}"),
        token: "stream-tok".into(),
        db_path: "/tmp/stream.db".into(),
        build: "1-1".into(),
        started_at: Utc::now(),
    };

    let clients = coreclient::new(&core, "desktop");
    let mut stream = clients
        .dispatch
        .watch_run(&WatchRunRequest {
            id: "r1".into(),
            after_seq: 10,
            ..Default::default()
        })
        .await
        .expect("open watch_run");

    let ev1 = stream
        .receive()
        .await
        .expect("ev1 ok")
        .expect("ev1 present");
    assert_eq!(ev1.seq, 11);
    assert_eq!(ev1.text, "line one");

    let ev2 = stream
        .receive()
        .await
        .expect("ev2 ok")
        .expect("ev2 present");
    assert_eq!(ev2.seq, 12);
    assert_eq!(ev2.kind, "done");

    assert_eq!(stream.receive().await.expect("eof"), None);
    assert_eq!(stream.receive().await.expect("eof repeat"), None);

    {
        let guard = shared.lock().expect("lock");
        assert_eq!(guard.last_client.as_deref(), Some("desktop"));
        let wreq = guard.watch_req.as_ref().expect("watch_req");
        assert_eq!(wreq.id, "r1");
        assert_eq!(wreq.after_seq, 10);
    }

    let mut err_stream = clients
        .dispatch
        .watch_run(&WatchRunRequest {
            id: "fail".into(),
            after_seq: 0,
            ..Default::default()
        })
        .await
        .expect("open failing watch_run");
    let stream_err = err_stream.receive().await.expect_err("expected stream err");
    assert_eq!(stream_err.to_string(), "aborted: run canceled");
}

fn write_fake_core_script(path: &Path, endpoint: &str) {
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
  "token": "lifecycle-token",
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
    fs::write(path, script).expect("write fake core script");
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("chmod +x");
}

#[derive(Clone)]
struct LifecycleServerState {
    db_path: PathBuf,
    reported_build: Arc<Mutex<String>>,
    shutdown_reasons: Arc<Mutex<Vec<String>>>,
}

#[tokio::test]
async fn test_lifecycle_start_ensure_replace_older_build_and_stop() {
    let tmp = TempDir::new().expect("tempdir");
    let db_path = tmp.path().join("dispatch.db");
    let exe_path = tmp.path().join("fake-dispatch");

    let reported_build = Arc::new(Mutex::new(String::from("1000-1")));
    let shutdown_reasons = Arc::new(Mutex::new(Vec::new()));
    let state = LifecycleServerState {
        db_path: db_path.clone(),
        reported_build: Arc::clone(&reported_build),
        shutdown_reasons: Arc::clone(&shutdown_reasons),
    };

    let app = Router::new()
        .route(
            "/dispatch.v1.HealthService/Check",
            post(|State(st): State<LifecycleServerState>| async move {
                let build = st.reported_build.lock().expect("lock").clone();
                Json(CheckResponse {
                    status: "ok".into(),
                    version: "0.1.0".into(),
                    uptime_sec: 1.0,
                    build,
                    pid: 1,
                })
            }),
        )
        .route(
            "/dispatch.v1.HealthService/Shutdown",
            post(
                |State(st): State<LifecycleServerState>, Json(req): Json<ShutdownRequest>| async move {
                    st.shutdown_reasons.lock().expect("lock").push(req.reason);
                    if let Some(c) = runstate::read_core(&st.db_path) {
                        let _ = kill(Pid::from_raw(c.pid), Signal::SIGTERM);
                    }
                    Json(ShutdownResponse {})
                },
            ),
        )
        .with_state(state);

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local_addr");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    let endpoint = format!("http://{addr}");
    write_fake_core_script(&exe_path, &endpoint);
    let current_exe_build = build_id(&exe_path);

    // 1. start_with_timeout launches the detached core and reads core.json.
    let first = start_with_timeout(&exe_path, &db_path, Duration::from_secs(5))
        .await
        .expect("start first core");
    assert!(pid_alive(first.pid));
    assert_eq!(first.endpoint, endpoint);

    // 2. When HealthService/Check reports an older build ("1000-1"), ensure_with_exe
    // stops the older core and starts a fresh one. Ensure the replacement reports current build.
    let reported_for_shutdown = Arc::clone(&reported_build);
    let first_pid = first.pid;
    // Update reported_build as soon as the first core is stopped so the newly spawned core passes Check.
    let watcher_build = current_exe_build.clone();
    tokio::spawn(async move {
        for _ in 0..100 {
            if !pid_alive(first_pid) {
                *reported_for_shutdown.lock().expect("lock") = watcher_build;
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    });

    let replaced = ensure_with_exe(&exe_path, &db_path)
        .await
        .expect("ensure replaces older build");
    assert_ne!(replaced.pid, first_pid);
    assert!(!pid_alive(first_pid));
    assert!(pid_alive(replaced.pid));
    assert_eq!(*shutdown_reasons.lock().expect("lock"), vec!["stop"]);

    // 3. Calling ensure_with_exe again with matching build reuses the running core PID.
    let reused = ensure_with_exe(&exe_path, &db_path)
        .await
        .expect("ensure reuses current build");
    assert_eq!(reused.pid, replaced.pid);

    // 4. stop shuts down the running core and waits for exit.
    stop(&db_path).await.expect("stop running core");
    assert!(!pid_alive(replaced.pid));

    // 5. Early exit before writing core.json reports exit status and log path.
    let bad_db = tmp.path().join("bad").join("dispatch.db");
    let bad_exe = tmp.path().join("bad-dispatch");
    fs::write(&bad_exe, "#!/bin/sh\nexit 1\n").expect("write bad script");
    fs::set_permissions(&bad_exe, fs::Permissions::from_mode(0o755)).expect("chmod +x");
    let err = start_with_timeout(&bad_exe, &bad_db, Duration::from_secs(3))
        .await
        .expect_err("early exit must error");
    assert!(
        err.to_string()
            .contains("core exited before listening (exit status 1)"),
        "err = {err}"
    );
}
