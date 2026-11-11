//! End to end with real agents and the Jev classifier. Every test is ignored by default: each
//! spends a little of the agents' quota (the cheapest models, one-line prompts), and the Jev
//! test one classifier request. They run in the user's home, where the agents keep their
//! logins, with a data folder and repositories of their own:
//!
//! cargo test -p dispatch-core --test e2e_live -- --ignored --test-threads=1
//!
//! `DISPATCH_LIVE_MODELS` picks the agents and models (`agent/model,...`), and
//! `DISPATCH_E2E_BIN` another build, such as the installed app's.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant};
use tempfile::TempDir;

/// The cheapest model of each agent this suite was written against.
const MODELS: &str = "codex/gpt-5.6-luna,claude/claude-haiku-5-5,cursor/cursor-grok-4.6-low,opencode/opencode/deepseek-v4.1-flash-free,antigravity/gemini-3.8-flash-low";

fn binary() -> PathBuf {
    std::env::var_os("DISPATCH_E2E_BIN").map_or_else(
        || PathBuf::from(env!("CARGO_BIN_EXE_dispatch")),
        PathBuf::from,
    )
}

/// The agents and models to try, as `(agent, model)`.
fn models() -> Vec<(String, String)> {
    std::env::var("DISPATCH_LIVE_MODELS")
        .unwrap_or_else(|_| MODELS.into())
        .split(',')
        .filter_map(|e| e.trim().split_once('/'))
        .map(|(a, m)| (a.to_owned(), m.to_owned()))
        .collect()
}

fn git(dir: &Path, args: &[&str]) -> String {
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
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

/// A data folder of its own; agents run with the user's home and PATH.
struct Live {
    _dir: TempDir,
    root: PathBuf,
    data: PathBuf,
    session: String,
}

impl Live {
    fn new() -> Self {
        let dir = tempfile::Builder::new()
            .prefix("dispatch-live-")
            .tempdir()
            .expect("tempdir");
        let root = std::fs::canonicalize(dir.path()).expect("canonicalize");
        let data = root.join("data");
        std::fs::create_dir_all(&data).expect("mkdir data");
        let session = format!("live-{}", root.file_name().unwrap().to_string_lossy());
        Live {
            _dir: dir,
            root,
            data,
            session,
        }
    }

    /// A new repository with one commit.
    fn repo(&self, name: &str) -> PathBuf {
        let repo = self.root.join(name);
        std::fs::create_dir_all(&repo).expect("mkdir repo");
        git(&repo, &["init", "-q", "-b", "main"]);
        std::fs::write(
            repo.join("README.md"),
            "A scratch repository for a Dispatch test.\n",
        )
        .expect("write readme");
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-qm", "init"]);
        repo
    }

    fn command(&self, dir: &Path, args: &[&str]) -> Command {
        let mut c = Command::new(binary());
        c.args(args).current_dir(dir);
        // The caller is this test, not the session that runs cargo.
        for key in [
            "CLAUDE_ENV_FILE",
            "CODEX_THREAD_ID",
            "POLTERWARE_SESSION",
            "OPENCODE_SESSION_ID",
            "DISPATCH_CALLER_SESSION",
            "DISPATCH_CALLER_AGENT",
        ] {
            c.env_remove(key);
        }
        c.env("PWDISPATCH_DATA_DIR", &self.data)
            .env("DISPATCH_CALLER_AGENT", "claude")
            .env("DISPATCH_CALLER_SESSION", &self.session);
        c
    }

    fn output(&self, dir: &Path, args: &[&str]) -> Output {
        self.command(dir, args).output().expect("spawn dispatch")
    }

    fn ok(&self, dir: &Path, args: &[&str]) -> String {
        let out = self.output(dir, args);
        assert!(
            out.status.success(),
            "dispatch {args:?}: {}\n{}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    fn json(&self, dir: &Path, args: &[&str]) -> Value {
        let out = self.ok(dir, args);
        serde_json::from_str(&out).unwrap_or_else(|e| panic!("dispatch {args:?}: {e}: {out}"))
    }

    fn start(&self, dir: &Path, args: &[&str]) -> String {
        let mut all = vec!["run", "--json", "--timeout", "4m"];
        all.extend_from_slice(args);
        self.json(dir, &all)["id"]
            .as_str()
            .expect("run id")
            .to_owned()
    }

    /// Waits for the run to end and returns its status.
    fn finish(&self, dir: &Path, id: &str) -> Value {
        let deadline = Instant::now() + Duration::from_secs(300);
        loop {
            let s = self.json(dir, &["status", id, "--json"]);
            let status = s["status"].as_str().unwrap_or_default();
            if !["PENDING", "RUNNING"].contains(&status) {
                return s;
            }
            assert!(Instant::now() < deadline, "run {id} is still {status}");
            std::thread::sleep(Duration::from_millis(500));
        }
    }

    fn reply(&self, dir: &Path, id: &str) -> String {
        self.ok(dir, &["result", id, "--section", "agent"])
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        let _ = self.output(&self.root, &["stop"]).status;
    }
}

/// Spent tokens, for the report.
fn tokens(s: &Value) -> i64 {
    ["input_tokens", "output_tokens"]
        .iter()
        .filter_map(|k| s["usage"][k].as_i64())
        .sum()
}

#[test]
#[ignore = "runs real agents: spends a little quota on their cheapest models"]
fn live_agents_answer_a_read_only_prompt() {
    let live = Live::new();
    // One repository per agent, so they run side by side.
    let runs: Vec<_> = models()
        .into_iter()
        .map(|(agent, model)| {
            let repo = live.repo(&format!("ask-{agent}"));
            let id = live.start(
                &repo,
                &[
                    "--agent",
                    &agent,
                    "--model",
                    &model,
                    "--readonly",
                    "Reply with exactly one word, pong, and do nothing else.",
                ],
            );
            (agent, model, repo, id)
        })
        .collect();
    let mut failed = Vec::new();
    for (agent, model, repo, id) in &runs {
        let s = live.finish(repo, id);
        let reply = live.reply(repo, id);
        eprintln!(
            "{agent}/{model}: {} in {}ms, {} tokens, attempts {}: {:?}",
            s["status"],
            s["run_ms"],
            tokens(&s),
            s["attempts"],
            reply.trim()
        );
        if s["status"] != "COMPLETED" || !reply.to_lowercase().contains("pong") {
            failed.push(format!("{agent}/{model}: {s}"));
        } else {
            assert_eq!(s["agent"], agent.as_str());
            assert_eq!(
                s["model"],
                model.as_str(),
                "the run left the model it was given"
            );
        }
    }
    assert!(
        failed.is_empty(),
        "runs that did not answer:\n{}",
        failed.join("\n")
    );
}

#[test]
#[ignore = "runs a real agent: spends a little quota on its cheapest model"]
fn live_write_run_merges() {
    let live = Live::new();
    let (agent, model) = models().into_iter().next().expect("a model");
    let repo = live.repo("write");
    let id = live.start(
        &repo,
        &[
            "--agent",
            &agent,
            "--model",
            &model,
            "Create a file named hello.txt whose only content is the line: hello from dispatch. Change nothing else.",
        ],
    );
    let s = live.finish(&repo, &id);
    assert_eq!(s["status"], "COMPLETED", "{s}");
    let worktree = PathBuf::from(s["worktree"].as_str().expect("worktree"));
    assert!(worktree.join("hello.txt").exists(), "{s}");
    assert!(!repo.join("hello.txt").exists());
    live.ok(&repo, &["merge", &id]);
    let body = std::fs::read_to_string(repo.join("hello.txt")).expect("merged hello.txt");
    assert!(body.contains("hello from dispatch"), "{body:?}");
    assert!(!worktree.exists());
}

#[test]
#[ignore = "calls TypeSafe's Jev and runs a real agent: one classifier request and a cheap model"]
fn live_jev_classifies_and_auto_routes() {
    let live = Live::new();
    let repo = live.repo("auto");
    live.ok(&repo, &["config", "set", "classifier", "jev"]);
    let keys = live.json(&repo, &["config", "get", "classifier_keys"]);
    assert_ne!(keys["jev"], "missing", "no Jev key: {keys}");

    let answer = live.json(
        &repo,
        &[
            "classify",
            "--json",
            "Rename one local variable in a single file.",
        ],
    );
    eprintln!("classify: {answer}");
    let text = answer.to_string();
    assert!(
        ["easy", "normal", "hard"].iter().any(|t| text.contains(t)),
        "no tier in {answer}"
    );

    // Auto picks among the cheap models only.
    let names: Vec<String> = models().into_iter().map(|(_, model)| model).collect();
    let settings = live.json(&repo, &["models", "--json"]);
    for provider in settings["providers"].as_array().unwrap() {
        let p = provider["provider"].as_str().unwrap();
        for model in provider["models"].as_array().unwrap() {
            let id = model["id"].as_str().unwrap();
            if !names.iter().any(|name| {
                dispatch_core::catalog::catalog_key(name) == dispatch_core::catalog::catalog_key(id)
            }) {
                live.ok(&repo, &["models", "disable", &format!("{p}/{id}")]);
            }
        }
    }
    let route = live.json(
        &repo,
        &[
            "route",
            "--json",
            "--readonly",
            "Say which file in this repository holds its readme.",
        ],
    );
    eprintln!("route: {route}");

    let id = live.start(
        &repo,
        &[
            "--tier",
            "auto",
            "--readonly",
            "Reply with exactly one word, pong, and do nothing else.",
        ],
    );
    let s = live.finish(&repo, &id);
    eprintln!("auto run: {s}");
    assert_eq!(s["status"], "COMPLETED", "{s}");
    assert!(
        names.iter().any(|m| s["model"] == m.as_str()),
        "auto left the allowed models: {s}"
    );
    assert!(
        s["note"]
            .as_str()
            .unwrap_or_default()
            .starts_with("auto: d="),
        "the run was not routed by a difficulty: {s}"
    );
    // The run keeps which classifier judged it in its meta, which `--json` prints whole.
    let meta = live.json(&repo, &["result", &id, "--section", "meta", "--json"]);
    assert_eq!(meta["Classifier"], "jev", "{meta}");
}
