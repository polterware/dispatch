//! End to end: the real `dispatch` binary and the core it starts, driven as an agent or a
//! person drives them, with a fake ACP agent (`tests/fixtures/e2e/fake-acp.sh`) in a home,
//! data folder, and repository of each test's own. Nothing here touches the user's data or
//! runs a paid agent. `DISPATCH_E2E_BIN` points the suite at another build, such as the
//! installed app's `Contents/Resources/dispatch`.

use dispatch_core::{coreclient, proto as pb, runstate};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant};
use tempfile::TempDir;

const FAKE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/e2e/fake-acp.sh"
);

fn binary() -> PathBuf {
    std::env::var_os("DISPATCH_E2E_BIN").map_or_else(
        || PathBuf::from(env!("CARGO_BIN_EXE_dispatch")),
        PathBuf::from,
    )
}

/// Runs git in `dir` with no user or system config, so the machine's hooks stay out.
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

fn read(path: impl AsRef<Path>) -> String {
    std::fs::read_to_string(path.as_ref())
        .unwrap_or_else(|e| panic!("read {}: {e}", path.as_ref().display()))
}

/// Waits up to 30 seconds for `done`.
fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting until {what}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn items(v: &Value) -> &[Value] {
    v.as_array().map_or(&[], Vec::as_slice)
}

/// One test's world: its home, data folder, repository, and copy of the fake agent.
struct Env {
    _dir: TempDir,
    root: PathBuf,
    home: PathBuf,
    data: PathBuf,
    repo: PathBuf,
    agent: PathBuf,
    session: String,
}

impl Env {
    fn new() -> Self {
        let dir = tempfile::Builder::new()
            .prefix("dispatch-e2e-")
            .tempdir()
            .expect("tempdir");
        let root = std::fs::canonicalize(dir.path()).expect("canonicalize");
        let (home, data, repo) = (root.join("home"), root.join("data"), root.join("repo"));
        for d in [&home, &data, &repo] {
            std::fs::create_dir_all(d).expect("mkdir");
        }
        // Each test runs its own copy of the agent, so a process left behind is its own,
        // and the copy logs what it reads next to it.
        let agent = root.join("fake-acp.sh");
        let script = read(FAKE).replacen(
            "#!/bin/sh\n",
            &format!(
                "#!/bin/sh\nFAKE_ACP_LOG='{}'\n",
                root.join("agent.log").display()
            ),
            1,
        );
        std::fs::write(&agent, script).expect("write agent");
        std::fs::set_permissions(&agent, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .expect("chmod agent");
        git(&repo, &["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("README.md"), "end to end\n").expect("write readme");
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-qm", "init"]);
        let session = format!("e2e-{}", root.file_name().unwrap().to_string_lossy());
        let env = Env {
            _dir: dir,
            root,
            home,
            data,
            repo,
            agent,
            session,
        };
        let agent = env.agent.to_string_lossy().into_owned();
        env.ok(&["agents", "add", "fake", "--", &agent]);
        // Discovery stays on, but routing can only reach the fake agent's provider.
        let settings = env.json(&["models", "--json"]);
        for provider in settings["providers"].as_array().unwrap() {
            let provider = provider["provider"].as_str().unwrap();
            if provider != "fake" {
                env.ok(&["models", "disable", provider]);
            }
        }
        env
    }

    fn command(&self, args: &[&str]) -> Command {
        self.command_of(&binary(), args)
    }

    /// A command of another program, with this test's environment.
    fn command_of(&self, program: &Path, args: &[&str]) -> Command {
        let mut c = Command::new(program);
        c.args(args)
            .current_dir(&self.repo)
            .env_clear()
            .env("HOME", &self.home)
            .env("PWDISPATCH_DATA_DIR", &self.data)
            .env("PWDISPATCH_CATALOG_REFRESH", "0")
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .env("SHELL", "/bin/sh")
            .env("TMPDIR", std::env::temp_dir())
            .env("DISPATCH_CALLER_AGENT", "claude")
            .env("DISPATCH_CALLER_SESSION", &self.session);
        c
    }

    fn output(&self, args: &[&str]) -> Output {
        self.command(args).output().expect("spawn dispatch")
    }

    /// Runs a command that must succeed and returns its stdout.
    fn ok(&self, args: &[&str]) -> String {
        let out = self.output(args);
        assert!(
            out.status.success(),
            "dispatch {args:?}: {}\n{}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    fn json(&self, args: &[&str]) -> Value {
        let out = self.ok(args);
        serde_json::from_str(&out).unwrap_or_else(|e| panic!("dispatch {args:?}: {e}: {out}"))
    }

    /// Runs a command that must fail and returns its stderr.
    fn fails(&self, args: &[&str]) -> String {
        let out = self.output(args);
        assert!(
            !out.status.success(),
            "dispatch {args:?} succeeded: {}",
            String::from_utf8_lossy(&out.stdout)
        );
        String::from_utf8_lossy(&out.stderr).into_owned()
    }

    /// Starts a run on the fake agent and returns its id.
    fn run(&self, prompt: &str, flags: &[&str]) -> String {
        let mut args = vec!["run", "--agent", "fake", "--json"];
        args.extend_from_slice(flags);
        args.push(prompt);
        let started = self.json(&args);
        assert_eq!(started["status"], "PENDING", "{started}");
        started["id"].as_str().expect("run id").to_owned()
    }

    fn status(&self, id: &str) -> Value {
        self.json(&["status", id, "--json"])
    }

    /// Waits for the run to reach `want`, failing at once when it ends otherwise.
    fn wait_for(&self, id: &str, want: &str) -> Value {
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            let s = self.status(id);
            let status = s["status"].as_str().unwrap_or_default().to_owned();
            if status == want {
                return s;
            }
            let ended = ["COMPLETED", "FAILED", "CANCELLED", "ORPHANED"].contains(&status.as_str());
            assert!(!ended, "run {id} ended {status}, not {want}: {s}");
            assert!(
                Instant::now() < deadline,
                "run {id} is still {status}, not {want}"
            );
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// Whether this test's agent has a process.
    fn agent_alive(&self) -> bool {
        Command::new("pgrep")
            .args(["-f", &self.agent.to_string_lossy()])
            .output()
            .is_ok_and(|o| o.status.success())
    }

    fn agent_log(&self) -> String {
        std::fs::read_to_string(self.root.join("agent.log")).unwrap_or_default()
    }

    fn db(&self) -> PathBuf {
        self.data.join("dispatch.db")
    }

    /// Kills the core with SIGKILL, as a crash or a force quit would.
    fn kill_core(&self) {
        let core = runstate::read_core(self.db()).expect("core.json");
        let killed = Command::new("kill")
            .args(["-9", &core.pid.to_string()])
            .status()
            .expect("spawn kill");
        assert!(killed.success());
        wait_until("the killed core is gone", || {
            !Command::new("kill")
                .args(["-0", &core.pid.to_string()])
                .stderr(std::process::Stdio::null())
                .status()
                .is_ok_and(|s| s.success())
        });
    }

    fn worktrees(&self) -> String {
        git(&self.repo, &["branch", "--list", "dispatch/*"])
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = self.command(&["stop"]).output();
        let _ = Command::new("pkill")
            .args(["-9", "-f", &self.agent.to_string_lossy()])
            .status();
    }
}

#[test]
fn a_read_only_run_answers_and_is_listed() {
    let env = Env::new();
    let id = env.run("say hi", &["--readonly"]);
    let s = env.wait_for(&id, "COMPLETED");
    assert_eq!(s["agent"], "fake");
    assert_eq!(s["model"], "fake-fast");
    assert_eq!(s["attempts"], 1);
    assert_eq!(s["exit_class"], "ok");
    assert_eq!(s["usage"]["input_tokens"], 100);
    assert_eq!(s["usage"]["output_tokens"], 10);
    let conversation = format!("claude:{}", env.session);
    assert_eq!(s["conversation_id"], conversation.as_str());
    assert!(
        s.get("worktree").is_none(),
        "a read-only run got a worktree: {s}"
    );
    assert!(
        env.agent_log().contains("say hi"),
        "the prompt never reached the agent"
    );
    assert_eq!(
        env.ok(&["result", &id, "--section", "agent"]).trim(),
        "Fake agent done."
    );
    assert!(
        env.ok(&["result", &id])
            .starts_with("COMPLETED fake/fake-fast")
    );

    // A short id names the run as the whole one does.
    assert_eq!(env.status(&id[..8])["id"], id.as_str());
    let err = env.fails(&["status", "0000000000", "--json"]);
    assert!(err.contains("not found"), "{err}");

    let all = env.json(&["runs", "--json"]);
    assert!(
        items(&all["runs"])
            .iter()
            .any(|r| r["ID"] == id.as_str() && r["Status"] == "COMPLETED"),
        "{all}"
    );
    let mine = env.json(&["runs", "--conversation", &conversation, "--json"]);
    assert_eq!(items(&mine["runs"]).len(), 1, "{mine}");
    let theirs = env.json(&["runs", "--conversation", "claude:someone-else", "--json"]);
    assert!(items(&theirs["runs"]).is_empty(), "{theirs}");

    let waited: Value =
        serde_json::from_str(&env.ok(&["wait", &id, "--timeout", "10s"])).expect("wait json");
    assert_eq!(waited["status"], "COMPLETED");

    let usage = env.json(&["usage", "--json"]);
    let row = items(&usage["rows"])
        .iter()
        .find(|r| r["key"] == "fake-fast")
        .unwrap_or_else(|| panic!("no fake-fast row: {usage}"));
    assert_eq!(row["runs"], 1);
    assert_eq!(row["input_tokens"], 100);
}

#[test]
fn events_show_a_running_runs_log_and_an_ended_ones() {
    let env = Env::new();
    // A run still working shows what it holds so far: its prompt, and the events the
    // app would draw, without waiting for it to end.
    let slow = env.run("SLOW events", &["--readonly"]);
    env.wait_for(&slow, "RUNNING");
    let live = env.json(&["events", &slow[..8], "--json"]);
    assert_eq!(live["status"], "RUNNING", "{live}");
    let events = items(&live["events"]);
    assert!(!events.is_empty(), "a running run has no events: {live}");
    assert!(
        events.iter().any(|e| e["text"]
            .as_str()
            .is_some_and(|t| t.contains("SLOW events"))),
        "the running run's prompt is missing: {live}"
    );
    assert!(live["more"].is_boolean(), "{live}");
    env.ok(&["cancel", &slow]);
    env.wait_for(&slow, "CANCELLED");

    // A run that ended before anyone watched it still has its agent's log: reading it is
    // a snapshot of what the agent printed, as the app draws it.
    let id = env.run("say hi", &["--readonly"]);
    env.wait_for(&id, "COMPLETED");
    let done = env.json(&["events", &id, "--json"]);
    assert_eq!(done["status"], "COMPLETED", "{done}");
    let events = items(&done["events"]);
    assert!(
        events.iter().any(|e| e["kind"] == "output"
            && e["text"]
                .as_str()
                .is_some_and(|t| t.contains("Fake agent done."))),
        "the ended run's agent log is missing: {done}"
    );
    let first = &events[0];
    assert!(first["seq"].as_i64().is_some_and(|n| n > 0), "{done}");
    assert!(first["offset"].as_i64().is_some(), "{done}");
    assert!(
        env.ok(&["events", &id]).contains("Fake agent done."),
        "the human output is not the log"
    );

    // A page from before an event reads further back, and a huge tail is still bounded.
    let last = events.last().expect("events");
    let earlier = env.json(&[
        "events",
        &id,
        "--before-seq",
        &last["seq"].as_i64().unwrap_or(0).to_string(),
        "--before-offset",
        &last["offset"].as_i64().unwrap_or(0).to_string(),
        "--json",
    ]);
    assert!(earlier["events"].is_array(), "{earlier}");
    assert!(
        env.json(&["events", &id, "--tail-bytes", "1", "--json"])["events"]
            .as_array()
            .is_some(),
        "a one-byte tail has no page"
    );
}

#[test]
fn a_write_run_lands_on_its_branch_and_merges() {
    let env = Env::new();
    let id = env.run("WRITE a file", &[]);
    let s = env.wait_for(&id, "COMPLETED");
    let worktree = PathBuf::from(s["worktree"].as_str().expect("worktree"));
    assert_eq!(s["branch"], format!("dispatch/{id}").as_str());
    assert!(
        worktree.join("e2e.txt").exists(),
        "the agent's file is not in the worktree"
    );
    assert!(
        !env.repo.join("e2e.txt").exists(),
        "the run wrote into the checkout"
    );
    assert!(
        env.ok(&["result", &id, "--section", "files"])
            .contains("files=1"),
        "files section"
    );

    env.ok(&["merge", &id[..8]]);
    assert!(read(env.repo.join("e2e.txt")).starts_with("written by the fake agent"));
    assert!(!worktree.exists(), "merge left the worktree");
    assert_eq!(env.worktrees(), "", "merge left the branch");
    assert!(git(&env.repo, &["log", "-1", "--format=%s"]).contains(&id));
    assert_eq!(git(&env.repo, &["status", "--porcelain"]), "");
    // A clean after the merge finds nothing left and is no error.
    env.ok(&["clean", &id]);
}

/// A run that spans another repository cuts a worktree there on its branch name, runs that
/// repository's link hook with every worktree named, and merges and cleans both as one.
#[test]
fn a_run_with_another_repository_cuts_merges_and_cleans_both() {
    let env = Env::new();
    let lib = env.root.join("lib");
    std::fs::create_dir_all(lib.join(".dispatch/hooks")).expect("mkdir");
    git(&lib, &["init", "-q", "-b", "main"]);
    std::fs::write(lib.join("lib.txt"), "library\n").expect("write");
    let hook = lib.join(".dispatch/hooks/link");
    std::fs::write(
        &hook,
        "#!/bin/sh\nprintf '%s' \"$DISPATCH_LINKED_WORKTREES\" > linked.txt\n",
    )
    .expect("write hook");
    std::fs::set_permissions(&hook, std::os::unix::fs::PermissionsExt::from_mode(0o755))
        .expect("chmod");
    git(&lib, &["add", "-A"]);
    git(&lib, &["commit", "-qm", "init"]);
    let lib_dir = lib.to_string_lossy().into_owned();

    let id = env.run("WRITE a file", &["--with", &lib_dir]);
    let s = env.wait_for(&id, "COMPLETED");
    let linked = s["linked"].as_array().expect("linked repos");
    assert_eq!(linked.len(), 1, "{s}");
    let lib_tree = PathBuf::from(linked[0]["worktree"].as_str().expect("worktree"));
    assert_eq!(linked[0]["branch"], format!("dispatch/{id}").as_str());
    // The hook ran in the library's worktree and saw both worktrees.
    let seen = read(lib_tree.join("linked.txt"));
    assert!(seen.contains("lib=") && seen.contains("repo="), "{seen}");

    env.ok(&["merge", &id[..8]]);
    assert!(read(env.repo.join("e2e.txt")).starts_with("written by the fake agent"));
    assert!(
        read(lib.join("linked.txt")).contains("lib="),
        "the library merged too"
    );
    assert!(!lib_tree.exists(), "merge left the library's worktree");
    assert_eq!(
        git(&lib, &["worktree", "list", "--porcelain"])
            .matches("worktree ")
            .count(),
        1
    );
}

#[test]
fn a_message_to_an_ended_run_continues_its_session_and_worktree() {
    let env = Env::new();
    let first = env.run("WRITE a file", &[]);
    let s = env.wait_for(&first, "COMPLETED");
    let worktree = s["worktree"].as_str().expect("worktree").to_owned();

    let sent = env.json(&["send", &first[..8], "--json", "now fix the test"]);
    assert_eq!(sent["status"], "PENDING", "{sent}");
    let second = sent["id"].as_str().expect("run id").to_owned();
    assert_ne!(second, first);
    let s = env.wait_for(&second, "COMPLETED");
    assert_eq!(s["agent"], "fake");
    assert_eq!(s["model"], "fake-fast");
    assert_eq!(s["worktree"], worktree.as_str());
    assert_eq!(
        s["conversation_id"],
        format!("claude:{}", env.session).as_str()
    );
    let meta = env.json(&["result", &second, "--section", "meta", "--json"]);
    assert_eq!(meta["ResumedOf"], first.as_str(), "{meta}");
    let log = env.agent_log();
    assert!(
        log.contains("session/load") && log.contains("now fix the test"),
        "the follow-up never reached the agent's session: {log}"
    );

    // Nothing to send to: a message needs a run that has ended and a message.
    let err = env.fails(&["send", "0000000000", "hello"]);
    assert!(err.contains("not found"), "{err}");
    let slow = env.run("SLOW", &["--readonly"]);
    env.wait_for(&slow, "RUNNING");
    let err = env.fails(&["send", &slow, "hello"]);
    assert!(err.contains("still"), "{err}");
    env.ok(&["cancel", &slow]);
}

#[test]
fn clean_keeps_unmerged_work_unless_forced() {
    let env = Env::new();
    let id = env.run("WRITE a file", &[]);
    let s = env.wait_for(&id, "COMPLETED");
    let worktree = PathBuf::from(s["worktree"].as_str().expect("worktree"));

    let err = env.fails(&["clean", &id]);
    assert!(err.contains("not merged"), "{err}");
    assert!(worktree.exists(), "a refused clean took the worktree");
    assert_ne!(env.worktrees(), "", "a refused clean took the branch");

    env.ok(&["clean", &id, "--force"]);
    assert!(!worktree.exists());
    assert_eq!(env.worktrees(), "");
    assert!(!env.repo.join("e2e.txt").exists());
    // Nothing is left to merge.
    let merged = env.output(&["merge", &id]);
    assert!(!merged.status.success(), "a cleaned run merged");
}

#[test]
fn a_failing_agent_fails_after_each_model() {
    let env = Env::new();
    let id = env.run("FAIL please", &["--readonly"]);
    let s = env.wait_for(&id, "FAILED");
    assert_eq!(s["attempts"], 2, "both models were not tried: {s}");
    assert!(
        s["error"]
            .as_str()
            .unwrap_or_default()
            .contains("the fake agent failed"),
        "{s}"
    );
    let waited = env.output(&["wait", &id, "--timeout", "10s"]);
    assert!(!waited.status.success(), "wait exits 0 for a failed run");

    // A denied model is never tried.
    env.ok(&["models", "disable", "fake/fake-deep"]);
    let id = env.run("FAIL again", &["--readonly"]);
    let s = env.wait_for(&id, "FAILED");
    assert_eq!(s["attempts"], 1, "{s}");
    assert_eq!(s["model"], "fake-fast");
}

#[test]
fn a_cancelled_run_ends_with_its_agent() {
    let env = Env::new();
    let id = env.run("SLOW answer", &["--readonly"]);
    env.wait_for(&id, "RUNNING");
    wait_until("the agent runs", || env.agent_alive());

    // Runs share a folder: a read-only one, and a writable one in a worktree of its own,
    // start and end while the first still works.
    let reader = env.run("say hi", &["--readonly"]);
    env.wait_for(&reader, "COMPLETED");
    let writer = env.run("say hi", &[]);
    env.wait_for(&writer, "COMPLETED");
    assert_eq!(env.status(&id)["status"], "RUNNING");

    env.ok(&["cancel", &id[..8]]);
    env.wait_for(&id, "CANCELLED");
    wait_until("the cancelled agent is gone", || !env.agent_alive());
}

#[test]
fn a_second_run_editing_one_folder_itself_waits_in_line() {
    let env = Env::new();
    let folder = env.repo.to_string_lossy().into_owned();
    let id = env.run("SLOW answer", &["--worktree", &folder]);
    env.wait_for(&id, "RUNNING");
    // Both would write in the folder itself, with no worktree of their own: the second waits.
    let queued = env.run("SLOW another", &["--worktree", &folder]);
    std::thread::sleep(Duration::from_millis(1500));
    let waiting = env.status(&queued);
    assert_eq!(waiting["status"], "PENDING", "{waiting}");
    assert!(
        waiting["note"]
            .as_str()
            .unwrap_or("")
            .contains("waiting for run"),
        "{waiting}"
    );
    // A read-only run there is fine.
    let reader = env.run("say hi", &["--readonly", "--worktree", &folder]);
    env.wait_for(&reader, "COMPLETED");
    // Cancel ends the wait: the run in line never starts.
    env.ok(&["cancel", &queued[..8]]);
    env.wait_for(&queued, "CANCELLED");
    env.ok(&["cancel", &id[..8]]);
    env.wait_for(&id, "CANCELLED");
}

#[test]
fn a_run_in_place_edits_the_folder_itself() {
    let env = Env::new();
    let id = env.run("WRITE a file", &["--worktree-mode", "in-place"]);
    let s = env.wait_for(&id, "COMPLETED");
    assert!(s["worktree"].as_str().unwrap_or_default().is_empty(), "{s}");
    assert!(read(env.repo.join("e2e.txt")).starts_with("written by the fake agent"));
    assert_eq!(env.worktrees(), "", "an in-place run cut a branch");
}

#[test]
fn a_new_worktree_needs_git_and_a_busy_folder_offers_one() {
    let env = Env::new();
    let plain = env.root.join("plain");
    std::fs::create_dir_all(&plain).expect("mkdir");
    let plain = plain.to_string_lossy().into_owned();
    let err = env.fails(&[
        "run",
        "--agent",
        "fake",
        "--json",
        "--worktree-mode",
        "worktree",
        "--dir",
        &plain,
        "x",
    ]);
    assert!(err.contains("not a git checkout"), "{err}");

    // In place, the checkout takes one writer at a time: another waits in line behind it,
    // naming the run in front, while a run with a worktree of its own goes at once.
    let first = env.run("SLOW answer", &["--worktree-mode", "in-place"]);
    env.wait_for(&first, "RUNNING");
    let queued = env.run("another", &["--worktree-mode", "in-place"]);
    let waiting = env.status(&queued);
    assert_eq!(waiting["status"], "PENDING", "{waiting}");
    assert!(
        waiting["note"]
            .as_str()
            .is_some_and(|n| n.contains(&format!("waiting for run {}", &first[..8]))),
        "{waiting}"
    );
    let second = env.run("WRITE a file", &["--worktree-mode", "worktree"]);
    let s = env.wait_for(&second, "COMPLETED");
    let worktree = PathBuf::from(s["worktree"].as_str().expect("worktree"));
    assert!(worktree.join("e2e.txt").exists(), "{s}");
    env.ok(&["cancel", &first[..8]]);
    env.wait_for(&first, "CANCELLED");
    // The folder is free: the run in line starts by itself and leaves the line's note.
    let done = env.wait_for(&queued, "COMPLETED");
    assert!(
        !done["note"]
            .as_str()
            .unwrap_or("")
            .contains("waiting for run"),
        "{done}"
    );
}

#[test]
fn a_run_that_changed_nothing_leaves_no_worktree_and_prune_clears_the_rest() {
    let env = Env::new();
    let quiet = env.run("say hi", &[]);
    let s = env.wait_for(&quiet, "COMPLETED");
    let gone = PathBuf::from(s["worktree"].as_str().expect("worktree"));
    assert!(!gone.exists(), "the empty run's worktree stayed: {s}");
    assert_eq!(env.worktrees(), "", "the empty run's branch stayed");

    let kept = env.run("WRITE a file", &[]);
    env.wait_for(&kept, "COMPLETED");
    // A worktree no run owns any more.
    let ghost = env.repo.join(".dispatch").join("worktrees").join("ghost");
    git(
        &env.repo,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "dispatch/ghost",
            &ghost.to_string_lossy(),
            "HEAD",
        ],
    );
    let listed = env.json(&["worktrees", "--json"]);
    let rows = listed["worktrees"].as_array().expect("worktrees").clone();
    let row = |id: &str| {
        rows.iter()
            .find(|row| row["run_id"] == id)
            .cloned()
            .unwrap_or_else(|| panic!("no {id} in {listed}"))
    };
    assert_eq!(rows.len(), 2, "{listed}");
    assert_eq!(row("ghost")["prunable"], true);
    assert_eq!(row(&kept)["changes"], true);
    assert_eq!(row(&kept)["prunable"], false);

    let pruned = env.json(&["worktrees", "--prune", "--json"]);
    assert_eq!(
        pruned["removed"].as_array().map(Vec::len),
        Some(1),
        "{pruned}"
    );
    assert_eq!(pruned["removed"][0]["run_id"], "ghost");
    assert!(!ghost.exists(), "prune left the ghost worktree");
    // `+` marks a branch a worktree has checked out.
    assert_eq!(
        env.worktrees().trim().trim_start_matches('+').trim(),
        format!("dispatch/{kept}"),
        "prune took the wrong branches"
    );
}

#[test]
fn a_run_cut_off_by_its_core_is_orphaned_and_recovered() {
    let env = Env::new();
    let id = env.run("WRITE then SLOW", &[]);
    env.wait_for(&id, "RUNNING");
    let worktree = env.repo.join(".dispatch").join("worktrees").join(&id);
    wait_until("the agent wrote its file", || {
        worktree.join("e2e.txt").exists()
    });

    env.kill_core();
    assert!(env.agent_alive(), "the agent went down with the core");
    // The next command starts a core, which finds the run's runner gone and stops its agent.
    let s = env.wait_for(&id, "ORPHANED");
    let said = format!("{} {}", s["error"], s["note"]);
    assert!(said.contains("exited unexpectedly"), "{s}");
    wait_until("the orphan's agent is gone", || !env.agent_alive());

    let recovered = env.json(&["recover", &id[..8]]);
    assert!(
        recovered["diffStat"]
            .as_str()
            .unwrap_or_default()
            .contains("e2e.txt"),
        "{recovered}"
    );
    let s = env.status(&id);
    assert_eq!(s["status"], "COMPLETED");
    assert_eq!(s["note"], "recovered from orphan; unverified");
    assert_eq!(git(&worktree, &["status", "--porcelain"]), "");

    env.ok(&["merge", &id]);
    assert!(read(env.repo.join("e2e.txt")).starts_with("written by the fake agent"));
}

#[test]
fn runs_and_settings_outlive_the_core() {
    let env = Env::new();
    let id = env.run("say hi", &["--readonly"]);
    env.wait_for(&id, "COMPLETED");
    env.ok(&["config", "set", "priority", "speed"]);
    let before = runstate::read_core(env.db()).expect("core.json").pid;

    assert!(env.ok(&["stop"]).contains("stopped"));
    // Any command starts a core again, with everything kept.
    assert_eq!(env.status(&id)["status"], "COMPLETED");
    let after = runstate::read_core(env.db()).expect("core.json").pid;
    assert_ne!(before, after, "stop left the core running");
    assert_eq!(env.json(&["config", "get", "priority"]), "speed");
    let agents = env.json(&["agents", "--json"]);
    assert!(
        items(&agents["agents"]).iter().any(|a| a["Name"] == "fake"),
        "{agents}"
    );
}

#[test]
fn roles_are_saved_used_updated_and_removed() {
    let env = Env::new();
    env.ok(&[
        "roles",
        "save",
        "e2e-checker",
        "--when",
        "end to end checks",
        "--tier",
        "easy",
        "--readonly",
        "--instructions",
        "Answer in one word.",
        "--easy",
        "fake/fake-fast",
        "--timeout",
        "2m",
    ]);
    let roles = env.json(&["roles"]);
    let role = &roles["roles"]["e2e-checker"];
    assert_eq!(role["tier"], "easy", "{roles}");
    assert_eq!(role["readonly"], true);
    assert_eq!(role["timeout"], "2m");
    assert_eq!(role["when"], "end to end checks");
    assert_eq!(role["tiers"][0]["agent"], "fake");
    assert_eq!(role["tiers"][0]["model"], "fake-fast");

    // A run with the role gets its instructions and runs read-only.
    let id = env.run("say hi", &["--role", "e2e-checker"]);
    let s = env.wait_for(&id, "COMPLETED");
    assert!(
        s.get("worktree").is_none(),
        "a read-only role got a worktree: {s}"
    );
    assert!(
        env.agent_log().contains("Answer in one word."),
        "the role's instructions never reached the agent"
    );

    env.ok(&[
        "roles",
        "save",
        "e2e-checker",
        "--tier",
        "normal",
        "--when",
        "updated",
    ]);
    let role = env.json(&["roles"])["roles"]["e2e-checker"].clone();
    assert_eq!(role["tier"], "normal", "{role}");
    assert_eq!(role["when"], "updated");
    assert!(
        role.get("readonly").is_none(),
        "save keeps what it was not given: {role}"
    );

    env.ok(&["roles", "remove", "e2e-checker"]);
    assert!(env.json(&["roles"])["roles"].get("e2e-checker").is_none());
    assert!(
        env.fails(&["roles", "remove", "e2e-checker"])
            .contains("no role")
    );
}

#[test]
fn builtin_roles_are_templates_and_can_be_removed() {
    let env = Env::new();
    let template = |env: &Env, name: &str| {
        env.json(&["roles", "templates"])["templates"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["name"] == name)
            .cloned()
            .unwrap_or_else(|| panic!("no {name} template"))
    };
    assert!(env.json(&["roles"])["roles"].get("explorer").is_some());
    let explorer = template(&env, "explorer");
    assert_eq!(explorer["in_use"], true, "{explorer}");
    assert_eq!(explorer["profile"]["tier"], "easy", "{explorer}");
    assert_eq!(explorer["profile"]["readonly"], true, "{explorer}");

    env.ok(&["roles", "remove", "explorer"]);
    assert!(env.json(&["roles"])["roles"].get("explorer").is_none());
    assert_eq!(template(&env, "explorer")["in_use"], false);
    // A deleted built-in is refused by --role, naming the command that brings it back.
    let refused = env.fails(&["run", "--role", "explorer", "--agent", "fake", "say hi"]);
    assert!(
        refused.contains("dispatch roles add explorer --from-template explorer"),
        "{refused}"
    );

    env.ok(&["roles", "add", "my-explorer", "--from-template", "explorer"]);
    let role = env.json(&["roles"])["roles"]["my-explorer"].clone();
    assert_eq!(role["tier"], "easy", "{role}");
    assert_eq!(role["readonly"], true, "{role}");
    assert!(
        role["when"].as_str().unwrap().contains("Find where"),
        "{role}"
    );
    assert!(
        role["instructions"]
            .as_str()
            .unwrap()
            .contains("Answer from the code"),
        "{role}"
    );

    // Saving the deleted built-in's name makes it the user's own again.
    env.ok(&["roles", "add", "explorer", "--from-template", "explorer"]);
    assert!(env.json(&["roles"])["roles"].get("explorer").is_some());
    assert_eq!(template(&env, "explorer")["in_use"], true);
    // General is the one role that stays.
    assert!(
        env.fails(&["roles", "remove", "general"])
            .contains("cannot be removed")
    );
}

#[test]
fn roles_save_their_columns_and_refuse_a_bad_set() {
    let env = Env::new();
    env.ok(&[
        "roles",
        "save",
        "e2e-columns",
        "--when",
        "end to end checks",
        "--columns",
        "easy,hard",
        "--easy",
        "fake/fake-fast",
    ]);
    let role = env.json(&["roles"])["roles"]["e2e-columns"].clone();
    assert_eq!(
        role["columns"],
        serde_json::json!(["easy", "hard"]),
        "{role}"
    );

    // General and none keep all three; an empty or invalid set is refused.
    assert!(
        env.fails(&["roles", "save", "e2e-columns", "--columns", "easy,wide"])
            .contains("not easy, normal, or hard")
    );
    assert!(
        env.fails(&["roles", "save", "e2e-columns", "--columns", "easy,easy"])
            .contains("twice")
    );
    env.ok(&["roles", "save", "e2e-columns", "--when", "all three"]);
    let role = env.json(&["roles"])["roles"]["e2e-columns"].clone();
    assert!(
        role.get("columns").is_none(),
        "no columns setting keeps all three: {role}"
    );
}

#[test]
fn config_is_set_read_and_global() {
    let env = Env::new();
    env.ok(&["config", "set", "priority", "speed"]);
    assert_eq!(env.json(&["config", "get", "priority"]), "speed");
    assert!(
        env.fails(&["config", "set", "priority", "fastest"])
            .contains("invalid priority")
    );

    env.ok(&["models", "disable", "fake/fake-deep"]);
    let settings = env.json(&["models", "--json"]);
    let fake = settings["providers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["provider"] == "fake")
        .unwrap();
    assert!(
        fake["models"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["id"] == "fake-deep" && m["enabled"] == false)
    );
    env.ok(&["models", "enable", "fake/fake-deep"]);
    env.ok(&["config", "set", "discover", "false"]);
    assert_eq!(env.json(&["config", "get", "discover"]), false);
    env.ok(&["config", "set", "discover", "true"]);

    // The CLI classifier asks the agent and model it is given, and the easy tier's
    // again once that goes back to the default.
    env.ok(&["config", "set", "classifier_agent", "fake/fake-mini"]);
    assert_eq!(
        env.json(&["config", "get", "classifier_cli"]),
        serde_json::json!({"agent": "fake", "model": "fake-mini"})
    );
    env.ok(&["config", "set", "classifier_agent", "default"]);
    assert_eq!(env.json(&["config", "get", "classifier_agent"]), "");
    assert_ne!(
        env.json(&["config", "get", "classifier_cli"])["agent"],
        "fake"
    );

    // The policy is global: there is no repository's own to write, and a file a repository
    // keeps where an older build put one changes neither the policy nor its runs.
    assert!(
        env.fails(&["config", "set", "priority", "quality", "--project"])
            .contains("project")
    );
    let dir = env.repo.join(".dispatch");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("config.json"),
        r#"{"priority":"quality","deny":{"agents":["fake"]}}"#,
    )
    .unwrap();
    assert_eq!(env.json(&["config", "get", "priority"]), "speed");
    let global: Value =
        serde_json::from_str(&read(env.data.join("dispatch").join("config.json"))).unwrap();
    assert_eq!(global["priority"], "speed");
    let id = env.run("say hi", &["--readonly"]);
    env.wait_for(&id, "COMPLETED");
}

#[test]
fn output_into_a_closed_pipe_ends_the_command_quietly() {
    use std::os::unix::process::ExitStatusExt;
    let env = Env::new();
    // The reader is gone before the command writes, as `| head` leaves it once it has enough.
    let (reader, writer) = std::io::pipe().expect("pipe");
    drop(reader);
    let out = env
        .command(&["skill", "show"])
        .stdout(writer)
        .output()
        .expect("spawn dispatch");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.signal(), Some(libc::SIGPIPE), "{stderr}");
    assert!(!stderr.contains("panicked"), "{stderr}");
}

#[test]
fn workspaces_are_added_listed_and_removed() {
    let env = Env::new();
    let other = env.root.join("other");
    std::fs::create_dir_all(&other).unwrap();
    let other_path = other.to_string_lossy().into_owned();
    let added = env.ok(&["workspace", "add", &other_path, "--name", "second"]);
    assert!(added.contains("second"), "{added}");
    // Adding it again is the same workspace.
    assert_eq!(env.ok(&["workspace", "add", &other_path]), added);

    let listed = env.json(&["workspace", "ls", "--json"]);
    let ws = items(&listed["workspaces"])
        .iter()
        .find(|w| w["path"] == other_path.as_str())
        .unwrap_or_else(|| panic!("{listed}"))
        .clone();
    assert_eq!(ws["name"], "second");
    let id = ws["id"].as_str().unwrap();
    // The repository a command ran in is a workspace too.
    assert!(
        items(&listed["workspaces"])
            .iter()
            .any(|w| w["path"] == env.repo.to_string_lossy().as_ref()),
        "{listed}"
    );

    assert!(env.ok(&["workspace", "rm", id]).contains(id));
    let listed = env.json(&["workspace", "ls", "--json"]);
    assert!(!items(&listed["workspaces"]).iter().any(|w| w["id"] == id));
    assert!(
        env.fails(&["workspace", "rm", id])
            .contains("no such workspace")
    );
    let missing = env.root.join("missing").to_string_lossy().into_owned();
    assert!(
        env.fails(&["workspace", "add", &missing])
            .contains("not a folder")
    );
}

#[test]
fn repo_is_workspace_in_repository_words_and_names_the_main_checkout() {
    let env = Env::new();
    // A run makes the repository a workspace; a subfolder of it is the same one.
    env.wait_for(&env.run("say hi", &["--readonly"]), "COMPLETED");
    let sub = env.repo.join("sub");
    std::fs::create_dir_all(&sub).unwrap();
    let added = env.ok(&["repo", "add", &sub.to_string_lossy()]);
    assert!(added.starts_with("repository "), "{added}");
    let listed = env.json(&["repo", "ls", "--json"]);
    let repo = items(&listed["workspaces"])
        .iter()
        .find(|w| added.contains(w["id"].as_str().unwrap_or("-")))
        .unwrap_or_else(|| panic!("{added} not in {listed}"))
        .clone();
    assert_eq!(
        std::fs::canonicalize(repo["path"].as_str().unwrap()).unwrap(),
        std::fs::canonicalize(&env.repo).unwrap(),
        "a subfolder names its repository"
    );
    assert!(env.fails(&["repo", "nope"]).contains("dispatch repo"));
}

#[test]
fn removing_a_repository_deletes_its_runs_and_their_worktrees() {
    let env = Env::new();
    let cut_run = env.run("WRITE a file", &[]);
    let s = env.wait_for(&cut_run, "COMPLETED");
    let cut = PathBuf::from(s["worktree"].as_str().expect("worktree"));
    let read_run = env.run("say hi", &["--readonly"]);
    env.wait_for(&read_run, "COMPLETED");
    let repo = std::fs::canonicalize(&env.repo).unwrap();
    let id_of_repo = || {
        items(&env.json(&["repo", "ls", "--json"])["workspaces"])
            .iter()
            .find(|w| {
                w["path"]
                    .as_str()
                    .and_then(|path| std::fs::canonicalize(path).ok())
                    .is_some_and(|path| path == repo)
            })
            .and_then(|w| w["id"].as_str().map(str::to_owned))
    };
    let id = id_of_repo().expect("the repository is listed");
    let removed = env.ok(&["repo", "rm", &id]);
    assert!(removed.contains("and its 2 runs (1 worktree)"), "{removed}");
    assert!(id_of_repo().is_none(), "the repository is still listed");
    assert!(!cut.exists(), "the run's worktree stayed");
    for run in [&cut_run, &read_run] {
        assert!(
            !env.output(&["status", run]).status.success(),
            "run {run} stayed"
        );
    }
}

#[test]
fn delete_takes_the_worktrees_dispatch_cut_and_keeps_the_users() {
    let env = Env::new();
    let ours = env.run("WRITE a file", &[]);
    let s = env.wait_for(&ours, "COMPLETED");
    let cut = PathBuf::from(s["worktree"].as_str().expect("worktree"));
    assert!(cut.exists());
    let deleted = env.ok(&["delete", &ours]);
    assert!(
        deleted.contains("deleted 1 run and its worktree"),
        "{deleted}"
    );
    assert!(!cut.exists(), "the run's worktree stayed");
    assert!(
        !env.worktrees().contains(&format!("dispatch/{ours}")),
        "the run's branch stayed"
    );
    assert!(
        !env.output(&["status", &ours]).status.success(),
        "the deleted run is still there"
    );

    // A worktree the user cut and handed to the run is theirs.
    let mine = env.root.join("mine");
    git(
        &env.repo,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "mine",
            &mine.to_string_lossy(),
            "HEAD",
        ],
    );
    let theirs = env.run("WRITE a file", &["--worktree", &mine.to_string_lossy()]);
    env.wait_for(&theirs, "COMPLETED");
    let deleted = env.ok(&["delete", &theirs]);
    assert_eq!(deleted.trim(), "deleted 1 run", "{deleted}");
    assert!(mine.exists(), "delete removed the user's worktree");
    assert!(
        git(&env.repo, &["branch", "--list", "mine"]).contains("mine"),
        "delete removed the user's branch"
    );
    assert!(
        env.fails(&["delete"])
            .contains("needs run ids or --conversation")
    );
}

#[test]
fn a_run_started_from_the_app_reads_as_the_apps() {
    let env = Env::new();
    // The app records its conversations' runs with the caller `dispatch`; a delegated run
    // records the agent that ran it, which is what the gallery's App filter reads.
    let out = env
        .command(&["run", "--agent", "fake", "--json", "say hi"])
        .env("DISPATCH_CALLER_AGENT", "dispatch")
        .output()
        .expect("spawn dispatch");
    assert!(
        out.status.success(),
        "dispatch run: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let started: Value = serde_json::from_slice(&out.stdout).expect("run json");
    let id = started["id"].as_str().expect("run id").to_owned();
    let s = env.wait_for(&id, "COMPLETED");
    assert_eq!(s["origin"], "app", "{s}");
    assert_eq!(s["caller_agent"], "dispatch", "{s}");
    assert!(
        s["conversation_id"]
            .as_str()
            .is_some_and(|c| c.starts_with("dispatch:")),
        "{s}"
    );

    let delegated = env.run("say hi", &["--readonly"]);
    let s = env.wait_for(&delegated, "COMPLETED");
    assert_eq!(s["origin"], "delegated", "{s}");
}

#[test]
fn run_images_land_in_artifacts_and_go_with_the_run() {
    let env = Env::new();
    let id = env.run("IMAGE draw a chart", &[]);
    env.wait_for(&id, "COMPLETED");
    wait_until("the run's image is indexed", || {
        env.json(&["artifacts", "--json"])["total"] == 1
    });
    let listed = env.json(&["artifacts", "--json"]);
    let item = &items(&listed["artifacts"])[0];
    assert_eq!(item["runId"], id.as_str(), "{listed}");
    assert_eq!(item["origin"], "delegated");
    assert_eq!(item["kind"], "png");
    assert_eq!(item["width"], 1);
    assert_eq!(item["height"], 1);
    assert_eq!(item["relPath"], "chart.png");
    assert_eq!(item["prompt"], "IMAGE draw a chart");
    let chart = PathBuf::from(item["path"].as_str().expect("path"));
    assert!(
        chart.starts_with(env.data.join("artifacts")),
        "{} is not in the data folder",
        chart.display()
    );
    assert!(chart.is_file());
    // This run is an agent's delegation, not a conversation started in the app.
    let run = env.status(&id);
    assert_eq!(run["origin"], "delegated", "{run}");

    // The filters pick it by its run's prompt and agent, and leave the app's out.
    assert!(items(&env.json(&["artifacts", "--origin", "app", "--json"])["artifacts"]).is_empty());
    let mine = env.json(&["artifacts", "--agent", "fake", "--query", "chart", "--json"]);
    assert_eq!(items(&mine["artifacts"]).len(), 1, "{mine}");
    assert!(items(&env.json(&["artifacts", "--query", "a fox", "--json"])["artifacts"]).is_empty());

    // An image the agent wrote into its worktree is copied before the worktree can go.
    let committed = env.run("COMMITIMAGE draw a shot", &[]);
    let s = env.wait_for(&committed, "COMPLETED");
    let worktree = PathBuf::from(s["worktree"].as_str().expect("worktree"));
    assert!(worktree.join("shot.png").exists());
    wait_until("both images are indexed", || {
        env.json(&["artifacts", "--json"])["total"] == 2
    });
    let listed = env.json(&["artifacts", "--json"]);
    let shot = items(&listed["artifacts"])
        .iter()
        .find(|a| a["runId"] == committed.as_str())
        .unwrap_or_else(|| panic!("the committed image is not indexed: {listed}"))
        .clone();
    assert_eq!(shot["relPath"], "worktree/shot.png");
    let copy = PathBuf::from(shot["path"].as_str().expect("path"));
    assert!(copy.is_file());
    let text = env.ok(&["artifacts"]);
    assert!(text.contains("chart.png"), "{text}");
    assert!(text.contains("delegated"), "{text}");
    assert!(text.contains("IMAGE draw a chart"), "{text}");

    // Deleting the run takes its images with it, rows and copies.
    env.ok(&["delete", &committed]);
    wait_until("the deleted run's image is gone", || !copy.exists());
    let listed = env.json(&["artifacts", "--json"]);
    assert_eq!(listed["total"], 1, "{listed}");
    assert!(
        items(&listed["artifacts"])
            .iter()
            .all(|a| a["runId"] != committed.as_str())
    );

    let refused = env.fails(&["artifacts", "--origin", "nonsense"]);
    assert!(
        refused.contains("origin must be app or delegated"),
        "{refused}"
    );
}

#[test]
fn a_conversation_is_renamed_and_named_by_its_prompt_again() {
    let env = Env::new();
    let named = env.ok(&["rename", "conv-1", "Release", "notes"]);
    assert!(named.contains("renamed \"Release notes\""), "{named}");
    let cleared = env.ok(&["rename", "conv-1"]);
    assert!(
        cleared.contains("named by its first prompt again"),
        "{cleared}"
    );
    assert!(env.fails(&["rename"]).contains("needs a conversation id"));
}

#[test]
fn a_conversations_usage_reads_its_runs_and_calling_session() {
    let env = Env::new();
    // The calling agent's own transcript, in the fake home the core resolves.
    let projects = env.home.join(".claude/projects/-e2e");
    std::fs::create_dir_all(&projects).expect("projects");
    std::fs::write(
        projects.join(format!("{}.jsonl", env.session)),
        include_str!("fixtures/usage/claude.jsonl"),
    )
    .expect("transcript");
    let id = env.run("say hi", &["--readonly"]);
    env.wait_for(&id, "COMPLETED");

    let conversation = format!("claude:{}", env.session);
    let usage = env.json(&["usage", "--conversation", &conversation, "--json"]);
    assert_eq!(usage["conversationId"], conversation);
    assert_eq!(usage["callerMeasured"], true, "{usage}");
    let runs = items(&usage["runs"]);
    assert_eq!(runs.len(), 1, "{usage}");
    assert_eq!(runs[0]["agent"], "fake");
    assert_eq!(runs[0]["runs"], 1);
    // The fake ACP agent reports 100 input and 10 output tokens.
    assert_eq!(runs[0]["inputTokens"], 100);
    // The calling session's two models, the repeated message counted once.
    assert_eq!(items(&usage["caller"]).len(), 2, "{usage}");
    let total = &usage["total"];
    assert_eq!(total["inputTokens"], 122);
    assert_eq!(total["runs"], 1);

    // An agent this build cannot read says so rather than guessing.
    let unknown = env.json(&["usage", "--conversation", "other:abc", "--json"]);
    assert_eq!(unknown["callerMeasured"], false);
    assert!(
        unknown["callerNote"]
            .as_str()
            .is_some_and(|note| note.contains("Not measured")),
        "{unknown}"
    );

    let text = env.ok(&["usage", "--conversation", &conversation]);
    assert!(text.contains("Calling agent"), "{text}");
    assert!(text.contains("API-equivalent"), "{text}");
}

#[test]
fn added_agents_are_listed_at_once() {
    let env = Env::new();
    let names = || -> Vec<String> {
        items(&env.json(&["agents", "--json"])["agents"])
            .iter()
            .filter_map(|a| a["Name"].as_str().map(str::to_owned))
            .collect()
    };
    assert!(names().contains(&"fake".to_owned()));
    env.ok(&["agents", "add", "second", "--", "/bin/cat"]);
    assert!(
        names().contains(&"second".to_owned()),
        "an added agent is missing"
    );
    env.ok(&["agents", "remove", "second"]);
    assert!(
        !names().contains(&"second".to_owned()),
        "a removed agent is listed"
    );
    assert!(
        env.fails(&["agents", "remove", "second"])
            .contains("no added agent")
    );
    assert!(
        env.fails(&["agents", "add", "codex", "--", "/bin/cat"])
            .contains("built-in")
    );
}

#[test]
fn sections_are_made_filled_renamed_and_removed() {
    let env = Env::new();
    let id = env.run("say hi", &["--readonly"]);
    env.wait_for(&id, "COMPLETED");
    let core = runstate::read_core(env.db()).expect("core.json");
    let c = coreclient::new(&core, "e2e");
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let list = || async {
            c.dispatch
                .list_sections(&pb::ListSectionsRequest {})
                .await
                .expect("list sections")
                .sections
        };
        let made = c
            .dispatch
            .save_section(&pb::SaveSectionRequest {
                name: "Review".into(),
                ..Default::default()
            })
            .await
            .expect("save section")
            .section
            .expect("section");
        assert!(!made.id.is_empty());
        let moved = |section_id: String| pb::MoveToSectionRequest {
            kind: "run".into(),
            r#ref: id.clone(),
            section_id,
        };
        c.dispatch
            .move_to_section(&moved(made.id.clone()))
            .await
            .expect("move run");
        let sections = list().await;
        let s = sections
            .iter()
            .find(|s| s.id == made.id)
            .expect("section listed");
        assert_eq!(s.name, "Review");
        assert_eq!(s.items.len(), 1);
        assert_eq!(s.items[0].r#ref, id);

        c.dispatch
            .save_section(&pb::SaveSectionRequest {
                id: made.id.clone(),
                name: "Done".into(),
                position: 0,
            })
            .await
            .expect("rename section");
        assert_eq!(list().await[0].name, "Done");

        // Out of every section, then into one that does not exist.
        c.dispatch
            .move_to_section(&moved(String::new()))
            .await
            .expect("move out");
        assert!(list().await[0].items.is_empty());
        assert!(
            c.dispatch
                .move_to_section(&moved("no-such-section".into()))
                .await
                .is_err()
        );

        c.dispatch
            .delete_section(&pb::DeleteSectionRequest {
                id: made.id.clone(),
            })
            .await
            .expect("delete section");
        assert!(list().await.is_empty());
    });
    // The run itself stays.
    assert_eq!(env.status(&id)["status"], "COMPLETED");
}

#[test]
fn setup_installs_and_removes_skills_and_hooks() {
    let env = Env::new();
    std::fs::create_dir_all(env.home.join(".claude")).unwrap();
    std::fs::create_dir_all(env.home.join(".codex")).unwrap();
    let report = env.json(&["setup", "--json"]);
    for key in ["cli", "skills", "hooks", "agents"] {
        assert!(
            report.get(key).is_some(),
            "setup --json has no {key}: {report}"
        );
    }
    let hook_on = |agent: &str| {
        items(&env.json(&["setup", "--json"])["hooks"])
            .iter()
            .find(|h| h["agent"] == agent)
            .map(|h| h["on"] == true)
    };
    assert_eq!(hook_on("claude"), Some(false));
    assert_eq!(hook_on("codex"), Some(false));

    // Skills: Claude Code's own folder, and the shared one Codex reads.
    let claude_skill = env.home.join(".claude/skills/dispatch/SKILL.md");
    let shared_skill = env.home.join(".agents/skills/dispatch/SKILL.md");
    env.ok(&["setup", "skill", "claude"]);
    assert!(read(&claude_skill).contains("dispatch"));
    env.ok(&["skill", "install", "--agent", "codex"]);
    assert!(shared_skill.exists());
    assert!(
        env.fails(&["skill", "install", "--agent", "codex"])
            .contains("--force"),
        "a second install overwrote the skill"
    );
    env.ok(&["setup", "skill", "--remove", "claude"]);
    assert!(!claude_skill.exists());
    env.ok(&["skill", "remove", "--agent", "codex"]);
    assert!(!shared_skill.exists());

    // Hooks, one agent at a time, and what the hook says while one is on.
    let speaks = |agent: &str| {
        let mut child = env
            .command(&["hook", "--agent", agent])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("spawn hook");
        use std::io::Write;
        child
            .stdin
            .take()
            .unwrap()
            .write_all(br#"{"session_id":"hook-session"}"#)
            .unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success());
        String::from_utf8_lossy(&out.stdout).contains("Dispatch is active")
    };
    assert!(!speaks("claude"), "the hook spoke with every hook off");
    env.ok(&["setup", "hook", "claude", "on"]);
    assert_eq!(hook_on("claude"), Some(true));
    assert_eq!(hook_on("codex"), Some(false));
    assert!(read(env.home.join(".claude/settings.json")).contains("SessionStart"));
    assert!(speaks("claude"));
    env.ok(&["setup", "hook", "codex", "on"]);
    assert!(read(env.home.join(".codex/hooks.json")).contains("hook --agent codex"));
    env.ok(&["setup", "hook", "claude", "off"]);
    assert!(!read(env.home.join(".claude/settings.json")).contains("SessionStart"));
    assert!(speaks("codex"), "codex's hook went quiet with claude's off");
    env.ok(&["setup", "hook", "codex", "off"]);
    assert!(!speaks("codex"));
    assert!(
        env.fails(&["setup", "hook", "claude", "maybe"])
            .contains("usage")
    );
}

/// A core started the way the app starts one, from a background thread of another program:
/// in macOS background mode and with every signal blocked. Such a core once never heard its
/// children exit, so each run waited forever on the first command it ran.
#[cfg(target_os = "macos")]
#[test]
#[allow(unsafe_code)]
fn a_core_started_from_a_background_thread_runs() {
    use std::os::unix::process::CommandExt;
    let env = Env::new();
    env.ok(&["stop"]);
    let db = env.db().to_string_lossy().into_owned();
    let bin = binary().to_string_lossy().into_owned();
    let mut core = env.command_of(
        Path::new("/usr/sbin/taskpolicy"),
        &["-b", &bin, "-db", &db, "serve", "--listen", "127.0.0.1:0"],
    );
    core.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .process_group(0);
    // SAFETY: the hook runs in the child before exec and only changes its signal mask.
    unsafe {
        core.pre_exec(|| {
            let mut all: libc::sigset_t = std::mem::zeroed();
            libc::sigfillset(&mut all);
            libc::pthread_sigmask(libc::SIG_SETMASK, &all, std::ptr::null_mut());
            Ok(())
        });
    }
    let mut child = core.spawn().expect("spawn core");
    let pid = i32::try_from(child.id()).unwrap();
    // This test is the core's parent: it reaps the core, as the app does, so `stop` sees it go.
    let reaper = std::thread::spawn(move || child.wait());
    wait_until("the core is up", || runstate::read_core(env.db()).is_some());

    let started = Instant::now();
    let id = env.run("say hi", &["--readonly"]);
    env.wait_for(&id, "COMPLETED");
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "the run took {:?}",
        started.elapsed()
    );
    assert_eq!(
        runstate::read_core(env.db()).map(|c| c.pid),
        Some(pid),
        "the run went to another core"
    );
    env.ok(&["stop"]);
    let _ = reaper.join();
}

#[test]
fn disabled_agents_leave_agent_lists_and_never_route() {
    let env = Env::new();
    let fake_enabled = |env: &Env| {
        let listed = env.json(&["agents", "--json"]);
        items(&listed["agents"])
            .iter()
            .find(|a| a["Name"] == "fake")
            .is_some_and(|a| a["Enabled"] == true)
    };
    assert!(fake_enabled(&env));
    env.ok(&["agents", "disable", "fake"]);
    assert!(!fake_enabled(&env));
    let refused = env.fails(&["run", "--agent", "fake", "--readonly", "must not run"]);
    assert!(refused.contains("agent fake is disabled"), "{refused}");
    let route = env.output(&["route", "--agent", "fake", "--json", "hello"]);
    assert!(!route.status.success());
    let report: Value = serde_json::from_slice(&route.stdout).unwrap();
    assert!(
        report["candidates"]
            .as_array()
            .is_none_or(|cs| cs.is_empty()),
        "{report}"
    );
    for args in [
        ["route", "--tier", "normal", "--json", "hello"].as_slice(),
        ["route", "--model", "fake-fast", "--json", "hello"].as_slice(),
    ] {
        let out = env.output(args);
        let report: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert!(
            report["candidates"]
                .as_array()
                .is_none_or(|cs| cs.iter().all(|c| c["agent"] != "fake")),
            "{args:?}: {report}"
        );
    }
    env.ok(&["agents", "enable", "fake"]);
    assert!(fake_enabled(&env));
    let id = env.run("hello", &["--readonly"]);
    env.wait_for(&id, "COMPLETED");
}

#[test]
fn disabled_models_and_providers_leave_list_models_and_never_run() {
    let env = Env::new();
    let clients = coreclient::new(&runstate::read_core(env.db()).unwrap(), "e2e");
    let rt = tokio::runtime::Runtime::new().unwrap();
    let models = || {
        rt.block_on(clients.dispatch.list_models(&pb::ListModelsRequest {}))
            .unwrap()
            .models
    };
    assert!(
        models()
            .iter()
            .any(|m| m.agent == "fake" && m.model == "fake-fast")
    );
    env.ok(&["models", "disable", "fake/fake-fast-high"]);
    assert!(
        !models()
            .iter()
            .any(|m| m.agent == "fake" && m.model == "fake-fast")
    );
    let route = env.output(&[
        "route",
        "--agent",
        "fake",
        "--model",
        "fake-fast",
        "--relax",
        "--json",
        "hello",
    ]);
    assert!(!route.status.success());
    let report: Value = serde_json::from_slice(&route.stdout).unwrap();
    assert!(
        report["candidates"]
            .as_array()
            .is_none_or(|cs| cs.is_empty()),
        "{report}"
    );
    env.ok(&["models", "enable", "fake/fake-fast"]);
    assert!(
        models()
            .iter()
            .any(|m| m.agent == "fake" && m.model == "fake-fast")
    );
    env.ok(&["models", "disable", "fake"]);
    assert!(!models().iter().any(|m| m.agent == "fake"));
    let id = env.run("must not invoke the agent", &["--readonly"]);
    let run = env.wait_for(&id, "FAILED");
    assert_eq!(run["attempts"], 0);
    env.ok(&["models", "enable", "fake"]);
    assert!(models().iter().any(|m| m.agent == "fake"));
}

#[test]
fn an_agents_disabled_model_leaves_that_agents_card_alone() {
    let env = Env::new();
    let clients = coreclient::new(&runstate::read_core(env.db()).unwrap(), "e2e");
    let rt = tokio::runtime::Runtime::new().unwrap();
    let models = || {
        rt.block_on(clients.dispatch.list_models(&pb::ListModelsRequest {}))
            .unwrap()
            .models
    };
    let settings = || {
        rt.block_on(
            clients
                .dispatch
                .list_model_settings(&pb::ListModelSettingsRequest {}),
        )
        .unwrap()
    };
    let off = |settings: &pb::ListModelSettingsResponse, id: &str| {
        settings
            .agents
            .iter()
            .any(|a| a.agent == "fake" && a.models.iter().any(|m| m.id == id && !m.enabled))
    };
    assert!(settings().agents.iter().any(|a| {
        a.agent == "fake" && a.models.iter().any(|m| m.id == "fake-fast" && m.enabled)
    }));
    env.ok(&["models", "disable", "fake-fast-high", "--agent", "fake"]);
    assert!(
        !models()
            .iter()
            .any(|m| m.agent == "fake" && m.model == "fake-fast")
    );
    assert!(off(&settings(), "fake-fast"));
    let listed = env.json(&["models", "--json"]);
    assert!(
        listed["agents"]
            .as_array()
            .is_some_and(|agents| agents.iter().any(|a| {
                a["agent"] == "fake"
                    && a["models"].as_array().is_some_and(|ms| {
                        ms.iter()
                            .any(|m| m["id"] == "fake-fast" && m["enabled"] == false)
                    })
            })),
        "{listed}"
    );
    let route = env.output(&[
        "route",
        "--agent",
        "fake",
        "--model",
        "fake-fast",
        "--relax",
        "--json",
        "hello",
    ]);
    assert!(!route.status.success());
    let report: Value = serde_json::from_slice(&route.stdout).unwrap();
    assert!(
        report["candidates"]
            .as_array()
            .is_none_or(|cs| cs.is_empty()),
        "{report}"
    );
    env.ok(&["models", "enable", "fake-fast", "--agent", "fake"]);
    assert!(!off(&settings(), "fake-fast"));
    assert!(
        models()
            .iter()
            .any(|m| m.agent == "fake" && m.model == "fake-fast")
    );
}
