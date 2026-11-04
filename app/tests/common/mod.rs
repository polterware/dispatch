//! A core of a test's own for the app's tests: a home, data folder, and repository in a
//! temporary folder, the fake ACP agent of `crates/dispatch-core/tests/fixtures/e2e`, and the
//! app binary as the `dispatch` command that starts the core and sets things up as an agent
//! would.
#![allow(dead_code)]

use dispatch_app::client::*;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

pub const FAKE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../crates/dispatch-core/tests/fixtures/e2e/fake-acp.sh"
);

pub fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=e2e",
            "-c",
            "user.email=e2e@example.invalid",
        ])
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("spawn git");
    assert!(out.status.success(), "git {args:?}");
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

pub fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting until {what}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

pub fn json(body: &str) -> Value {
    serde_json::from_str(body).unwrap_or_else(|e| panic!("{e}: {body}"))
}

pub struct Core {
    _dir: tempfile::TempDir,
    pub paths: Paths,
    pub client: CoreClient,
}

impl std::ops::Deref for Core {
    type Target = Paths;
    fn deref(&self) -> &Paths {
        &self.paths
    }
}

impl Core {
    pub fn new() -> Self {
        let dir = tempfile::Builder::new()
            .prefix("dispatch-app-e2e-")
            .tempdir()
            .expect("tempdir");
        let root = std::fs::canonicalize(dir.path()).expect("canonicalize");
        let (home, data, repo) = (root.join("home"), root.join("data"), root.join("repo"));
        for d in [
            &home,
            &data,
            &repo,
            &home.join(".claude"),
            &home.join(".codex"),
        ] {
            std::fs::create_dir_all(d).expect("mkdir");
        }
        git(&repo, &["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("README.md"), "end to end\n").unwrap();
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-qm", "init"]);
        let paths = Paths { home, data, repo };
        paths.cli(&["agents", "add", "fake", "--", FAKE]);
        paths.cli(&["config", "allow", "add", "agents", "fake"]);
        let client = CoreClient::connect(&paths.data).expect("connect to the core");
        Core {
            _dir: dir,
            paths,
            client,
        }
    }
}

/// Where a test's core keeps its home, data, and repository.
pub struct Paths {
    pub home: PathBuf,
    pub data: PathBuf,
    pub repo: PathBuf,
}

impl Paths {
    pub fn command(&self, args: &[&str]) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_dispatch-app"));
        c.args(args)
            .current_dir(&self.repo)
            .env_clear()
            .env("HOME", &self.home)
            .env("PWDISPATCH_DATA_DIR", &self.data)
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .env("SHELL", "/bin/sh")
            .env("TMPDIR", std::env::temp_dir())
            .env("DISPATCH_CALLER_AGENT", "claude")
            .env("DISPATCH_CALLER_SESSION", "app-e2e");
        c
    }

    pub fn cli(&self, args: &[&str]) -> String {
        let out = self.command(args).output().expect("spawn dispatch");
        assert!(
            out.status.success(),
            "dispatch {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }
}

impl Core {
    /// Starts a run on the fake agent the way an agent does, from the repository.
    pub fn start(&self, prompt: &str, read_only: bool) -> String {
        let mut args = vec!["run", "--agent", "fake", "--json"];
        if read_only {
            args.push("--readonly");
        }
        args.push(prompt);
        json(&self.cli(&args))["id"].as_str().unwrap().to_owned()
    }

    pub fn run(&self, id: &str) -> DispatchRun {
        self.client
            .get_run(&GetRunRequest { id: id.into() })
            .expect("get run")
            .run
            .expect("a run")
    }

    pub fn wait_for(&self, id: &str, want: &str) -> DispatchRun {
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            let run = self.run(id);
            if run.status == want {
                return run;
            }
            assert!(
                !["COMPLETED", "FAILED", "CANCELLED", "ORPHANED"].contains(&run.status.as_str()),
                "run {id} ended {}, not {want}: {}",
                run.status,
                run.error
            );
            assert!(
                Instant::now() < deadline,
                "run {id} is still {}",
                run.status
            );
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    pub fn config(&self, action: &str, key: &str, values: &[&str]) -> Value {
        let res = self
            .client
            .config(&ConfigRequest {
                action: action.into(),
                key: key.into(),
                values: values.iter().map(|v| (*v).to_owned()).collect(),
            })
            .expect("config");
        assert!(res.ok, "config {action} {key}: {}", res.json);
        json(&res.json)
    }

    pub fn setup(&self) -> Value {
        json(
            &self
                .client
                .get_agent_setup(&GetAgentSetupRequest::default())
                .expect("agent setup")
                .json,
        )
    }

    pub fn apply(&self, action: &str, agent: &str) {
        self.client
            .apply_agent_setup(&ApplyAgentSetupRequest {
                action: action.into(),
                agent: agent.into(),
            })
            .unwrap_or_else(|e| panic!("apply {action} {agent}: {e:?}"));
    }

    pub fn workspace_id(&self) -> String {
        self.client
            .list_workspaces(&ListWorkspacesRequest::default())
            .expect("workspaces")
            .workspaces
            .into_iter()
            .find(|w| Path::new(&w.path) == self.repo)
            .expect("the repository's workspace")
            .id
    }
}

impl Drop for Core {
    fn drop(&mut self) {
        let _ = self.command(&["stop"]).output();
    }
}
