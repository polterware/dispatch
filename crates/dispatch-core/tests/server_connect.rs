#![forbid(unsafe_code)]

//! The core's Connect server end to end: the session token, both codecs, and
//! the errors and shutdown the CLI, the app, and Polterware rely on.

use dispatch_core::{
    coreclient,
    host::NoHost,
    proto::{
        AddWorkspaceRequest, CheckRequest, CheckResponse, DeleteArtifactRequest, DeleteRunsRequest,
        GetRunRequest, ListArtifactsRequest, ListRunsRequest, ListSectionsRequest,
        ListWorkspacesRequest, ReadRunEventsRequest, RenameConversationRequest, ShutdownRequest,
        WatchRunRequest,
    },
    runstate,
    server::{self, Core},
    store::Store,
    types::{self, Store as _},
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
    start_with(Store::new(":memory:").unwrap()).await
}

async fn start_with(store: Store) -> Running {
    let dir = tempfile::tempdir().unwrap();
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

    // A folder inside an added one gets its own workspace when added itself.
    let inner = folder.path().join("inner");
    std::fs::create_dir(&inner).unwrap();
    let nested = c
        .workspaces
        .add_workspace(&AddWorkspaceRequest {
            path: inner.to_string_lossy().into_owned(),
            name: String::new(),
        })
        .await
        .unwrap()
        .workspace
        .unwrap();
    assert_ne!(nested.id, added.id);
    assert_eq!(nested.name, "inner");
    let again = c
        .workspaces
        .add_workspace(&AddWorkspaceRequest {
            path: format!("{}/", inner.to_string_lossy()),
            name: String::new(),
        })
        .await
        .unwrap()
        .workspace
        .unwrap();
    assert_eq!(again.id, nested.id, "adding the same folder again keeps it");
    let listed = c
        .workspaces
        .list_workspaces(&ListWorkspacesRequest {})
        .await
        .unwrap();
    assert_eq!(listed.workspaces.len(), 2);

    // A repository is its main checkout: a subfolder and a worktree of it name it.
    let repo = folder.path().join("repo");
    std::fs::create_dir_all(repo.join("src")).unwrap();
    let git = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(&repo)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    git(&["init", "-q"]);
    git(&[
        "-c",
        "user.name=t",
        "-c",
        "user.email=t@t",
        "commit",
        "-q",
        "--allow-empty",
        "-m",
        "first",
    ]);
    let tree = folder.path().join("tree");
    git(&["worktree", "add", "-q", tree.to_str().unwrap()]);
    let add = |path: std::path::PathBuf| {
        let c = &c;
        async move {
            c.workspaces
                .add_workspace(&AddWorkspaceRequest {
                    path: path.to_string_lossy().into_owned(),
                    name: String::new(),
                })
                .await
                .unwrap()
                .workspace
                .unwrap()
        }
    };
    let by_root = add(repo.clone()).await;
    assert_eq!(by_root.name, "repo");
    assert_eq!(add(repo.join("src")).await.id, by_root.id);
    assert_eq!(add(tree).await.id, by_root.id);

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

#[tokio::test]
async fn a_watch_opens_at_the_end_and_reads_back_page_by_page() {
    let store = Store::new(":memory:").unwrap();
    store
        .save_run(&types::Run {
            id: "run".into(),
            project_id: "p".into(),
            status: types::STATUS_COMPLETED.into(),
            ..Default::default()
        })
        .unwrap();
    let event = |kind: &str, text: &str| types::RunEvent {
        run_id: "run".into(),
        kind: kind.into(),
        text: text.into(),
        ..Default::default()
    };
    store
        .append_event(&event(types::EVENT_STARTED, "go"))
        .unwrap();
    store
        .append_event(&event(types::EVENT_OUTPUT, "first line\nsecond line\n"))
        .unwrap();
    store
        .append_event(&event(types::EVENT_MERGED, "into main"))
        .unwrap();
    let r = start_with(store).await;
    let c = clients(&r);

    // The tail: the merged line whole, then the last 12 bytes of the output before it.
    let mut stream = c
        .dispatch
        .watch_run(&WatchRunRequest {
            id: "run".into(),
            after_seq: 0,
            tail_bytes: 21,
        })
        .await
        .unwrap();
    let mut tail = Vec::new();
    while let Some(event) = stream.receive().await.unwrap() {
        tail.push(event);
    }
    let read: Vec<_> = tail
        .iter()
        .map(|e| (e.seq, e.text.as_str(), e.text_offset))
        .collect();
    assert_eq!(read, [(2, "second line\n", 11), (3, "into main", 0)]);

    // Back from where the tail starts: the rest of the output and the started line.
    let earlier = c
        .dispatch
        .read_run_events(&ReadRunEventsRequest {
            id: "run".into(),
            before_seq: 2,
            before_offset: 11,
            max_bytes: 0,
        })
        .await
        .unwrap();
    let read: Vec<_> = earlier
        .events
        .iter()
        .map(|e| (e.seq, e.text.as_str(), e.text_offset))
        .collect();
    assert_eq!(read, [(1, "go", 0), (2, "first line\n", 0)]);
    assert!(!earlier.more);

    // A watch with no tail still sends every event.
    let mut stream = c
        .dispatch
        .watch_run(&WatchRunRequest {
            id: "run".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    let mut all = 0;
    while stream.receive().await.unwrap().is_some() {
        all += 1;
    }
    assert_eq!(all, 3);
}

#[tokio::test]
async fn deletes_a_conversation_and_its_runs_but_not_a_working_one() {
    let store = Store::new(":memory:").unwrap();
    let run = |id: &str, project: &str, conversation: &str, status: &str| types::Run {
        id: id.into(),
        project_id: project.into(),
        conversation_id: conversation.into(),
        status: status.into(),
        ..Default::default()
    };
    // One conversation across two workspaces, started by another agent's chat.
    store
        .save_run(&run("a1", "p", "codex:chat", types::STATUS_COMPLETED))
        .unwrap();
    store
        .save_run(&run("a2", "q", "codex:chat", types::STATUS_FAILED))
        .unwrap();
    store
        .save_run(&run("b1", "p", "claude:other", types::STATUS_COMPLETED))
        .unwrap();
    // Still working: its runner (this test) is alive, so the core's boot check keeps it.
    store
        .save_run(&types::Run {
            pid: i64::from(std::process::id()),
            ..run("live", "p", "claude:busy", types::STATUS_RUNNING)
        })
        .unwrap();
    store
        .append_event(&types::RunEvent {
            run_id: "a1".into(),
            kind: types::EVENT_STARTED.into(),
            text: "go".into(),
            ..Default::default()
        })
        .unwrap();
    let r = start_with(store).await;
    let c = clients(&r);

    let out = c
        .dispatch
        .delete_runs(&DeleteRunsRequest {
            conversation_id: "codex:chat".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(out.deleted, 2);
    for id in ["a1", "a2"] {
        assert!(r.core.store.get_run(id).is_err(), "{id} survived");
    }
    assert!(r.core.store.get_run("b1").is_ok());

    // A single run by id.
    let out = c
        .dispatch
        .delete_runs(&DeleteRunsRequest {
            ids: vec!["b1".into()],
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(out.deleted, 1);

    // A run that still works is refused, and kept.
    let err = c
        .dispatch
        .delete_runs(&DeleteRunsRequest {
            conversation_id: "claude:busy".into(),
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("cancel it before deleting"),
        "{err}"
    );
    assert!(r.core.store.get_run("live").is_ok());
}

#[tokio::test]
async fn lists_artifacts_by_origin_and_deletes_their_copies() {
    let store = Store::new(":memory:").unwrap();
    let run = |id: &str, caller: &str, conversation: &str, prompt: &str| types::Run {
        id: id.into(),
        project_id: "p".into(),
        conversation_id: conversation.into(),
        caller_agent: caller.into(),
        prompt: prompt.into(),
        status: types::STATUS_COMPLETED.into(),
        ..Default::default()
    };
    store
        .save_run(&run("r1", types::CALLER_APP, "dispatch:one", "draw a fox"))
        .unwrap();
    store
        .save_run(&run("r2", "claude", "claude:two", "chart the week"))
        .unwrap();
    let r = start_with(store).await;
    let base = r._dir.path().join("artifacts");
    for (run, caller, conversation, rel) in [
        ("r1", types::CALLER_APP, "dispatch:one", "out/fox.png"),
        ("r2", "claude", "claude:two", "worktree/chart.png"),
    ] {
        let path = base.join(run).join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"png").unwrap();
        r.core
            .store
            .save_artifact(&types::Artifact {
                id: format!("a-{run}"),
                run_id: run.into(),
                project_id: "p".into(),
                conversation_id: conversation.into(),
                caller_agent: caller.into(),
                agent: "fake".into(),
                model: "fake-fast".into(),
                repo: "/repo".into(),
                rel_path: rel.into(),
                kind: "png".into(),
                bytes: 3,
                width: 2,
                height: 3,
                created_at: "2026-11-10T10:00:00Z".into(),
                prompt: String::new(),
            })
            .unwrap();
    }
    let c = clients(&r);
    let all = c
        .dispatch
        .list_artifacts(&ListArtifactsRequest::default())
        .await
        .unwrap();
    assert_eq!(all.total, 2);
    // Newest first; the row written last comes first.
    let first = &all.artifacts[0];
    assert_eq!(first.run_id, "r2");
    assert_eq!(first.origin, "delegated");
    assert_eq!(first.prompt, "chart the week");
    assert!(
        first.path.ends_with("r2/worktree/chart.png"),
        "{}",
        first.path
    );
    assert_eq!(first.width, 2);
    assert_eq!(all.artifacts[1].origin, "app");

    let app = c
        .dispatch
        .list_artifacts(&ListArtifactsRequest {
            origin: "app".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(app.total, 1);
    assert_eq!(app.artifacts[0].run_id, "r1");
    let by_prompt = c
        .dispatch
        .list_artifacts(&ListArtifactsRequest {
            query: "FOX".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(by_prompt.total, 1);
    let paged = c
        .dispatch
        .list_artifacts(&ListArtifactsRequest {
            limit: 1,
            offset: 1,
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(paged.total, 2);
    assert_eq!(paged.artifacts.len(), 1);

    let err = c
        .dispatch
        .list_artifacts(&ListArtifactsRequest {
            origin: "elsewhere".into(),
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert!(err.to_string().contains("origin must be"), "{err}");

    c.dispatch
        .delete_artifact(&DeleteArtifactRequest { id: "a-r1".into() })
        .await
        .unwrap();
    assert!(!base.join("r1/out/fox.png").exists());
    assert!(!base.join("r1/out").exists(), "the emptied folder stayed");
    let left = c
        .dispatch
        .list_artifacts(&ListArtifactsRequest::default())
        .await
        .unwrap();
    assert_eq!(left.total, 1);
    let err = c
        .dispatch
        .delete_artifact(&DeleteArtifactRequest { id: "a-r1".into() })
        .await
        .unwrap_err();
    assert!(err.to_string().starts_with("not_found: "), "{err}");
}

#[tokio::test]
async fn names_a_conversation_and_forgets_it_with_the_conversation() {
    let store = Store::new(":memory:").unwrap();
    store
        .save_run(&types::Run {
            id: "c1".into(),
            project_id: "p".into(),
            conversation_id: "codex:chat".into(),
            status: types::STATUS_COMPLETED.into(),
            ..Default::default()
        })
        .unwrap();
    let r = start_with(store).await;
    let c = clients(&r);
    let names = |c: &dispatch_core::coreclient::Clients| {
        let c = c.clone();
        async move {
            c.dispatch
                .list_sections(&ListSectionsRequest::default())
                .await
                .unwrap()
                .conversation_names
                .into_iter()
                .map(|n| (n.conversation_id, n.name))
                .collect::<Vec<_>>()
        }
    };
    let rename = |name: &str| RenameConversationRequest {
        conversation_id: "codex:chat".into(),
        name: name.into(),
    };
    c.dispatch
        .rename_conversation(&rename("  Buttons  "))
        .await
        .unwrap();
    c.dispatch
        .rename_conversation(&rename("Round buttons"))
        .await
        .unwrap();
    assert_eq!(
        names(&c).await,
        [("codex:chat".to_string(), "Round buttons".to_string())]
    );
    // An empty name gives the conversation back its first message.
    c.dispatch.rename_conversation(&rename("")).await.unwrap();
    assert!(names(&c).await.is_empty());
    c.dispatch
        .rename_conversation(&rename("Again"))
        .await
        .unwrap();
    c.dispatch
        .delete_runs(&DeleteRunsRequest {
            conversation_id: "codex:chat".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(names(&c).await.is_empty());
}
