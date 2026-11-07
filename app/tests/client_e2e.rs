//! The app's side of the core, end to end: each call a page makes, with the requests the
//! views build, against a real core in a home and data folder of the test's own, with the
//! fake ACP agent of `crates/dispatch-core/tests/fixtures/e2e`. The app binary is also the
//! `dispatch` command, so it starts the core and sets things up as an agent would.

mod common;

use common::*;
use dispatch_app::client::*;
use serde_json::Value;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

#[test]
fn the_run_page_follows_merges_and_deletes_a_run() {
    let core = Core::new();
    let id = core.start("WRITE a file", false);
    let run = core.wait_for(&id, "COMPLETED");
    assert_eq!(run.agent, "fake");
    assert!(!run.worktree.is_empty() && !run.branch.is_empty());
    assert!(run.files_changed.unwrap_or(0) >= 1, "{run:?}");

    // The page's lists and its reply.
    let project = core.workspace_id();
    let runs = core
        .client
        .list_runs(&ListRunsRequest {
            project_id: project.clone(),
            limit: 50,
            ..ListRunsRequest::default()
        })
        .expect("list runs")
        .runs;
    assert!(runs.iter().any(|r| r.id == id));
    assert_eq!(
        core.client.run_reply(&id).expect("reply").trim(),
        "Fake agent done."
    );
    let events: Vec<_> = core
        .client
        .watch_run(&WatchRunRequest {
            id: id.clone(),
            ..WatchRunRequest::default()
        })
        .expect("watch")
        .collect::<Result<_, _>>()
        .expect("watched events");
    assert!(!events.is_empty(), "an ended run has no events");
    let page = core
        .client
        .read_run_events(&ReadRunEventsRequest {
            id: id.clone(),
            ..ReadRunEventsRequest::default()
        })
        .expect("read events");
    assert!(!page.events.is_empty());
    let decision = core
        .client
        .get_route_decision(&GetRouteDecisionRequest { run_id: id.clone() })
        .expect("route decision");
    assert!(decision.ok, "{}", decision.json);
    let worktrees = core
        .client
        .list_worktrees(&ListWorktreesRequest {
            project_id: project.clone(),
        })
        .expect("worktrees")
        .worktrees;
    assert!(!worktrees.is_empty(), "the run's worktree is not listed");

    // Delete refuses work that is not merged; Merge keeps the worktree, then Delete takes it.
    let refused = core
        .client
        .clean_run(&CleanRunRequest {
            id: id.clone(),
            force: false,
        })
        .map(|r| (r.ok, r.json));
    assert!(
        refused.as_ref().map_or(true, |(ok, _)| !ok),
        "Delete took unmerged work: {refused:?}"
    );
    assert!(Path::new(&run.worktree).exists());
    let merged = core
        .client
        .merge_run(&MergeRunRequest {
            id: id.clone(),
            keep_worktree: true,
        })
        .expect("merge");
    assert!(merged.ok, "{}", merged.json);
    assert!(core.repo.join("e2e.txt").exists());
    assert!(Path::new(&run.worktree).exists(), "Merge took the worktree");
    let deleted = core
        .client
        .clean_run(&CleanRunRequest {
            id: id.clone(),
            force: false,
        })
        .expect("delete");
    assert!(deleted.ok, "{}", deleted.json);
    assert!(!Path::new(&run.worktree).exists());
    assert!(!core.run(&id).merged_at.is_empty());
}

#[test]
fn the_run_page_cancels_and_removes_a_run() {
    let core = Core::new();
    let id = core.start("WRITE then SLOW", false);
    let run = core.wait_for(&id, "RUNNING");
    core.client
        .cancel_run(&CancelRunRequest { id: id.clone() })
        .expect("cancel");
    core.wait_for(&id, "CANCELLED");
    let worktree = core.run(&id).worktree;
    assert!(!worktree.is_empty(), "{run:?}");
    let removed = core
        .client
        .clean_run(&CleanRunRequest {
            id: id.clone(),
            force: true,
        })
        .expect("remove");
    assert!(removed.ok, "{}", removed.json);
    assert!(!Path::new(&worktree).exists());
    assert_eq!(git(&core.repo, &["branch", "--list", "dispatch/*"]), "");
}

#[test]
fn the_run_page_recovers_an_orphan() {
    let core = Core::new();
    let id = core.start("WRITE then SLOW", false);
    core.wait_for(&id, "RUNNING");
    let worktree = core.repo.join(".dispatch/worktrees").join(&id);
    wait_until("the agent wrote its file", || {
        worktree.join("e2e.txt").exists()
    });
    let pid = json(&std::fs::read_to_string(core.data.join("run/core.json")).unwrap())["pid"]
        .as_i64()
        .unwrap();
    assert!(
        Command::new("kill")
            .args(["-9", &pid.to_string()])
            .status()
            .unwrap()
            .success()
    );
    // The app finds no core and starts one, as any command does.
    core.cli(&["runs", "--json"]);
    let client = CoreClient::connect(&core.data).expect("reconnect");
    let deadline = Instant::now() + Duration::from_secs(30);
    let orphan = loop {
        let run = client
            .get_run(&GetRunRequest { id: id.clone() })
            .unwrap()
            .run
            .unwrap();
        if run.status == "ORPHANED" || Instant::now() > deadline {
            break run;
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    assert_eq!(orphan.status, "ORPHANED");
    let recovered = client
        .recover_run(&RecoverRunRequest { id: id.clone() })
        .expect("recover");
    assert!(recovered.ok, "{}", recovered.json);
    let run = client
        .get_run(&GetRunRequest { id: id.clone() })
        .unwrap()
        .run
        .unwrap();
    assert_eq!(run.status, "COMPLETED");
    assert_eq!(run.note, "recovered from orphan; unverified");
}

#[test]
fn the_sidebar_makes_fills_renames_and_removes_sections() {
    let core = Core::new();
    let id = core.start("say hi", true);
    core.wait_for(&id, "COMPLETED");
    let c = &core.client;
    let made = c
        .save_section(&SaveSectionRequest {
            name: "Review".into(),
            ..SaveSectionRequest::default()
        })
        .expect("save")
        .section;
    c.move_to_section(&MoveToSectionRequest {
        kind: "run".into(),
        item_ref: id.clone(),
        section_id: made.id.clone(),
    })
    .expect("move");
    let listed = c
        .list_sections(&ListSectionsRequest::default())
        .expect("list")
        .sections;
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].name, "Review");
    assert_eq!(listed[0].items.len(), 1);
    assert_eq!(listed[0].items[0].item_ref, id);
    c.save_section(&SaveSectionRequest {
        id: made.id.clone(),
        name: "Done".into(),
        position: 0,
    })
    .expect("rename");
    assert_eq!(
        c.list_sections(&ListSectionsRequest::default())
            .unwrap()
            .sections[0]
            .name,
        "Done"
    );
    c.delete_section(&DeleteSectionRequest {
        id: made.id.clone(),
    })
    .expect("delete");
    assert!(
        c.list_sections(&ListSectionsRequest::default())
            .unwrap()
            .sections
            .is_empty()
    );
}

#[test]
fn settings_change_the_policy_hooks_and_skills() {
    let core = Core::new();
    let all = core.config("get", "", &[]);
    assert_eq!(all["discover"], true, "{all}");

    for (key, value, want) in [
        ("priority", "speed", Value::from("speed")),
        ("discover", "false", Value::from(false)),
        ("roles", "false", Value::from(false)),
        ("classifier", "jev", Value::from("jev")),
        // Off is the default, kept as no value.
        ("classifier", "off", Value::from("")),
    ] {
        core.config("set", key, &[value]);
        assert_eq!(core.config("get", key, &[]), want, "{key}");
    }
    let disabled = core
        .client
        .set_model_enabled(&dispatch_app::client::SetModelEnabledRequest {
            provider: "fake".into(),
            model: "fake-fast".into(),
            enabled: false,
        })
        .unwrap();
    assert!(!disabled.enabled);
    let settings = core
        .client
        .list_model_settings(&dispatch_app::client::ListModelSettingsRequest::default())
        .unwrap();
    assert!(settings.providers.iter().any(
        |p| p.provider == "fake" && p.models.iter().any(|m| m.id == "fake-fast" && !m.enabled)
    ));
    core.client
        .set_model_enabled(&dispatch_app::client::SetModelEnabledRequest {
            provider: "fake".into(),
            model: "fake-fast".into(),
            enabled: true,
        })
        .unwrap();
    let disabled = core
        .client
        .set_agent_enabled(&dispatch_app::client::SetAgentEnabledRequest {
            agent: "fake".into(),
            enabled: false,
        })
        .unwrap();
    assert!(!disabled.enabled);
    let agents = core
        .client
        .list_agents(&ListAgentsRequest::default())
        .unwrap()
        .agents;
    assert!(
        agents.iter().any(|a| a.name == "fake" && !a.enabled),
        "{agents:?}"
    );
    core.client
        .set_agent_enabled(&dispatch_app::client::SetAgentEnabledRequest {
            agent: "fake".into(),
            enabled: true,
        })
        .unwrap();
    let agents = core
        .client
        .list_agents(&ListAgentsRequest::default())
        .unwrap()
        .agents;
    assert!(
        agents.iter().any(|a| a.name == "fake" && a.enabled),
        "{agents:?}"
    );
    let keys = core.config("get", "classifier_keys", &[]);
    assert!(keys.get("jev").is_some(), "{keys}");

    // Hooks, per agent, in the core's home.
    let hooks = |setup: &Value, agent: &str| {
        setup["hooks"]
            .as_array()
            .and_then(|h| h.iter().find(|h| h["agent"] == agent))
            .map(|h| h["on"] == true)
    };
    let before = core.setup();
    assert_eq!(hooks(&before, "claude"), Some(false), "{before}");
    core.apply("hook_on", "claude");
    assert_eq!(hooks(&core.setup(), "claude"), Some(true));
    assert!(
        std::fs::read_to_string(core.home.join(".claude/settings.json"))
            .unwrap()
            .contains("SessionStart")
    );
    core.apply("hook_on", "codex");
    assert_eq!(hooks(&core.setup(), "codex"), Some(true));
    core.apply("hook_off", "claude");
    core.apply("hook_off", "codex");
    let after = core.setup();
    assert_eq!(hooks(&after, "claude"), Some(false));
    assert_eq!(hooks(&after, "codex"), Some(false));

    // Skills, by agent.
    for (agent, path) in [
        ("claude", ".claude/skills/dispatch/SKILL.md"),
        ("codex", ".agents/skills/dispatch/SKILL.md"),
    ] {
        let installed = core
            .client
            .skill(&SkillRequest {
                action: "install".into(),
                agent: agent.into(),
                force: false,
            })
            .expect("install skill");
        assert!(installed.ok, "{}", installed.json);
        assert!(core.home.join(path).exists(), "{agent}");
        let removed = core
            .client
            .skill(&SkillRequest {
                action: "remove".into(),
                agent: agent.into(),
                force: false,
            })
            .expect("remove skill");
        assert!(removed.ok, "{}", removed.json);
        assert!(!core.home.join(path).exists(), "{agent}");
    }
}

#[test]
fn roles_and_tiers_are_saved_and_removed() {
    let core = Core::new();
    let roles = |c: &Core| {
        let res = c
            .client
            .subagents(&SubagentsRequest {
                action: "list".into(),
                ..SubagentsRequest::default()
            })
            .expect("list roles");
        assert!(res.ok, "{}", res.json);
        json(&res.json)["roles"].clone()
    };
    assert!(roles(&core).get("explorer").is_some());
    let profile = serde_json::json!({
        "tier": "easy",
        "readonly": true,
        "instructions": "Answer in one word.",
        "when": "end to end checks",
        "tiers": [{"tier": "easy", "agent": "fake", "model": "fake-fast"}],
    });
    let saved = core
        .client
        .subagents(&SubagentsRequest {
            action: "save".into(),
            name: "e2e-checker".into(),
            profile: profile.to_string(),
            ..SubagentsRequest::default()
        })
        .expect("save role");
    assert!(saved.ok, "{}", saved.json);
    let role = roles(&core)["e2e-checker"].clone();
    assert_eq!(role["tier"], "easy", "{role}");
    assert_eq!(role["tiers"][0]["model"], "fake-fast");
    // A model that is off leaves the role's board, and a save from the board keeps it.
    let save = |tiers: serde_json::Value| {
        let mut p = profile.clone();
        p["tiers"] = tiers;
        let res = core
            .client
            .subagents(&SubagentsRequest {
                action: "save".into(),
                name: "e2e-checker".into(),
                profile: p.to_string(),
                ..SubagentsRequest::default()
            })
            .expect("save role");
        assert!(res.ok, "{}", res.json);
    };
    save(serde_json::json!([
        {"tier": "easy", "agent": "fake", "model": "fake-fast"},
        {"tier": "hard", "agent": "codex", "model": "gpt-6.1-sol"},
    ]));
    let board = |core: &Core| {
        roles(core)["e2e-checker"]["tiers"]
            .as_array()
            .unwrap()
            .len()
    };
    assert_eq!(board(&core), 1);
    save(serde_json::json!([{"tier": "easy", "agent": "fake", "model": "fake-fast"}]));
    let openai = |enabled: bool| {
        core.client
            .set_model_enabled(&dispatch_app::client::SetModelEnabledRequest {
                provider: "openai".into(),
                model: String::new(),
                enabled,
            })
            .unwrap();
    };
    openai(true);
    assert_eq!(board(&core), 2);
    openai(false);
    let removed = core
        .client
        .subagents(&SubagentsRequest {
            action: "remove".into(),
            name: "e2e-checker".into(),
            ..SubagentsRequest::default()
        })
        .expect("remove role");
    assert!(removed.ok, "{}", removed.json);
    assert!(roles(&core).get("e2e-checker").is_none());

    // The tier board lists only enabled models: the harness turned off every provider but
    // fake, which the default table never names. Saving the board keeps the hidden rows, and
    // turning their provider back on brings them back.
    let listed = |core: &Core| {
        core.client
            .list_tiers(&ListTiersRequest::default())
            .expect("tiers")
            .tiers
    };
    assert!(listed(&core).is_empty());
    let saved = core
        .client
        .replace_tiers(&ReplaceTiersRequest {
            tiers: vec![dispatch_app::client::DispatchTier {
                name: "easy".into(),
                agent: "fake".into(),
                model: "fake-fast".into(),
                ..Default::default()
            }],
        })
        .expect("replace tiers")
        .tiers;
    assert_eq!(saved.len(), 1);
    core.client
        .set_model_enabled(&dispatch_app::client::SetModelEnabledRequest {
            provider: "openai".into(),
            model: String::new(),
            enabled: true,
        })
        .unwrap();
    let back = listed(&core);
    assert_eq!(back.first().map(|t| t.model.as_str()), Some("fake-fast"));
    assert!(back.iter().any(|t| t.agent == "codex"), "{back:?}");
    core.client
        .check_tiers(&CheckTiersRequest::default())
        .expect("check tiers");
}

#[test]
fn the_report_pages_read_the_core() {
    let core = Core::new();
    let id = core.start("say hi", true);
    core.wait_for(&id, "COMPLETED");
    let project = core.workspace_id();
    let c = &core.client;

    let stats = c
        .stats(&StatsRequest {
            project_id: String::new(),
            by: "model".into(),
            since: String::new(),
            dir: String::new(),
        })
        .expect("stats");
    let row = stats
        .rows
        .iter()
        .find(|r| r.key == "fake-fast")
        .expect("fake-fast in the stats");
    assert_eq!(row.runs, 1);
    assert_eq!(row.input_tokens.0, 100);
    let history = c
        .list_history(&ListHistoryRequest {
            project_id: project.clone(),
            limit: 50,
            ..ListHistoryRequest::default()
        })
        .expect("history")
        .events;
    assert!(!history.is_empty());
    let agents = c
        .list_agents(&ListAgentsRequest::default())
        .expect("agents")
        .agents;
    assert!(agents.iter().any(|a| a.name == "fake"));
    c.list_quota(&ListQuotaRequest::default()).expect("quota");
    c.capacity(&CapacityRequest::default()).expect("capacity");
    c.list_models(&ListModelsRequest::default())
        .expect("models");
    let catalog = c
        .list_model_catalog(&ListModelCatalogRequest::default())
        .expect("catalog");
    assert!(catalog.ok, "{}", catalog.json);
    let benchmarks = c
        .list_model_benchmarks(&ListModelBenchmarksRequest::default())
        .expect("benchmarks");
    assert!(benchmarks.ok, "{}", benchmarks.json);
    let route = c
        .route(&RouteRequest {
            agent: "fake".into(),
            read_only: true,
            prompt: "say hi".into(),
            dir: core.repo.to_string_lossy().into_owned(),
            project_id: project.clone(),
            ..RouteRequest::default()
        })
        .expect("route");
    assert!(route.ok, "{}", route.json);
    assert!(route.json.contains("fake"), "{}", route.json);
    let resolved = c
        .resolve_workspace(&ResolveWorkspaceRequest {
            dir: core.repo.to_string_lossy().into_owned(),
            ..ResolveWorkspaceRequest::default()
        })
        .expect("resolve")
        .workspace;
    assert_eq!(resolved.id, project);

    // The conversation's Usage dialog: the run's stored usage and the calling session's
    // transcript, which the core reads from this home.
    let projects = core.home.join(".claude/projects/-e2e");
    std::fs::create_dir_all(&projects).unwrap();
    std::fs::write(
        projects.join("app-e2e.jsonl"),
        include_str!("../../crates/dispatch-core/tests/fixtures/usage/claude.jsonl"),
    )
    .unwrap();
    let usage = c
        .conversation_usage(&GetConversationUsageRequest {
            conversation_id: "claude:app-e2e".into(),
        })
        .expect("conversation usage");
    assert!(usage.caller_measured, "{}", usage.caller_note);
    assert_eq!(usage.runs.len(), 1);
    assert_eq!(usage.runs[0].agent, "fake");
    assert_eq!(usage.runs[0].input_tokens.0, 100);
    assert_eq!(usage.caller.len(), 2);
    assert_eq!(usage.total.as_ref().map(|t| t.input_tokens.0), Some(122));
}
