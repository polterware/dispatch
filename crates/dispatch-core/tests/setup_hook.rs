use std::fs;
use std::path::Path;

use dispatch_core::hook::*;
use serde_json::Value;

fn read_json(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).expect("read settings")).expect("valid json")
}

#[test]
fn install_creates_entry() {
    let dir = tempfile::tempdir().unwrap();
    let settings = dir.path().join("settings.json");
    let bin = "/usr/local/bin/dispatch";

    assert!(!is_installed(&settings).unwrap());
    install(&settings, bin).unwrap();
    assert!(is_installed(&settings).unwrap());

    let root = read_json(&settings);
    let session_start = root["hooks"]["SessionStart"].as_array().unwrap();
    assert_eq!(session_start.len(), 1);
    let inner = session_start[0]["hooks"].as_array().unwrap();
    assert_eq!(inner.len(), 1);
    assert_eq!(inner[0]["command"], format!("{bin} hook"));
    assert_eq!(inner[0]["type"], "command");
}

#[test]
fn install_writes_go_json() {
    let dir = tempfile::tempdir().unwrap();
    let settings = dir.path().join("settings.json");
    fs::write(&settings, r#"{"z": "<a&b>", "a": [], "m": {}}"#).unwrap();
    install(&settings, "/usr/local/bin/dispatch").unwrap();
    let want = "{\n  \"a\": [],\n  \"hooks\": {\n    \"SessionStart\": [\n      {\n        \"hooks\": [\n          {\n            \"command\": \"/usr/local/bin/dispatch hook\",\n            \"timeout\": 10,\n            \"type\": \"command\"\n          }\n        ]\n      }\n    ]\n  },\n  \"m\": {},\n  \"z\": \"\\u003ca\\u0026b\\u003e\"\n}\n";
    assert_eq!(fs::read_to_string(&settings).unwrap(), want);
}

#[test]
fn install_twice_does_not_duplicate() {
    let dir = tempfile::tempdir().unwrap();
    let settings = dir.path().join("settings.json");
    install(&settings, "/usr/local/bin/dispatch").unwrap();
    install(&settings, "/usr/local/bin/dispatch").unwrap();
    let root = read_json(&settings);
    assert_eq!(root["hooks"]["SessionStart"].as_array().unwrap().len(), 1);
}

#[test]
fn install_updates_moved_binary() {
    let dir = tempfile::tempdir().unwrap();
    let settings = dir.path().join("settings.json");
    install(&settings, "/path1/dispatch").unwrap();
    install(&settings, "/path2/dispatch").unwrap();
    let root = read_json(&settings);
    let session_start = root["hooks"]["SessionStart"].as_array().unwrap();
    assert_eq!(session_start.len(), 1);
    assert_eq!(
        session_start[0]["hooks"][0]["command"],
        "/path2/dispatch hook"
    );
}

#[test]
fn preserves_other_hooks_and_keys() {
    let dir = tempfile::tempdir().unwrap();
    let settings = dir.path().join("settings.json");
    let initial = r#"{
  "theme": "dark",
  "tabSize": 4,
  "hooks": {
    "PreToolUse": [
      {
        "type": "command",
        "command": "echo pre_tool"
      }
    ],
    "SessionStart": [
      {
        "matcher": "startup",
        "hooks": [
          {
            "type": "command",
            "command": "echo custom_startup",
            "timeout": 5
          }
        ]
      }
    ]
  }
}"#;
    fs::write(&settings, initial).unwrap();
    let bin = "/usr/local/bin/dispatch";
    install(&settings, bin).unwrap();

    let root = read_json(&settings);
    assert_eq!(root["theme"], "dark");
    assert!(root["tabSize"].is_u64() && root["tabSize"] == 4);
    let hooks = root["hooks"].as_object().unwrap();
    assert!(hooks.contains_key("PreToolUse"));
    let session_start = hooks["SessionStart"].as_array().unwrap();
    assert_eq!(session_start.len(), 2);
    assert_eq!(session_start[0]["matcher"], "startup");
    assert_eq!(
        session_start[1]["hooks"][0]["command"],
        format!("{bin} hook")
    );
}

#[test]
fn uninstall_removes_only_ours() {
    let dir = tempfile::tempdir().unwrap();
    let settings = dir.path().join("settings.json");
    let initial = r#"{
  "theme": "dark",
  "hooks": {
    "PreToolUse": [
      {
        "type": "command",
        "command": "echo pre_tool"
      }
    ],
    "SessionStart": [
      {
        "matcher": "startup",
        "hooks": [
          {
            "type": "command",
            "command": "echo custom_startup"
          }
        ]
      },
      {
        "hooks": [
          {
            "type": "command",
            "command": "/usr/local/bin/dispatch dispatch hook",
            "timeout": 10
          }
        ]
      }
    ]
  }
}"#;
    fs::write(&settings, initial).unwrap();
    uninstall(&settings).unwrap();
    assert!(!is_installed(&settings).unwrap());

    let root = read_json(&settings);
    assert_eq!(root["theme"], "dark");
    let hooks = root["hooks"].as_object().unwrap();
    assert!(hooks.contains_key("PreToolUse"));
    let session_start = hooks["SessionStart"].as_array().unwrap();
    assert_eq!(session_start.len(), 1);
    assert_eq!(session_start[0]["matcher"], "startup");
}

#[test]
fn uninstall_cleans_empty_session_start() {
    let dir = tempfile::tempdir().unwrap();
    let settings = dir.path().join("settings.json");
    install(&settings, "/bin/dispatch").unwrap();
    uninstall(&settings).unwrap();
    assert!(!is_installed(&settings).unwrap());
    let root = read_json(&settings);
    assert!(root.get("hooks").is_none(), "{root}");
}

#[test]
fn toggle_enabled_flag() {
    let dir = tempfile::tempdir().unwrap();
    assert!(!is_enabled(dir.path()));
    set_enabled(dir.path(), true).unwrap();
    assert!(is_enabled(dir.path()));
    assert!(dir.path().join("enabled").exists());
    set_enabled(dir.path(), false).unwrap();
    assert!(!is_enabled(dir.path()));
    assert!(!dir.path().join("enabled").exists());
}

#[test]
fn context_block_is_short_and_mentions_tiers() {
    let block = context_block();
    assert!(!block.is_empty());
    assert!(block.chars().count() <= 400);
    assert!(block.contains("dispatch"));
    assert!(block.contains("easy|normal|hard"));
}

#[test]
fn settings_path_and_enabled_path() {
    let dir = tempfile::tempdir().unwrap();
    let custom = dir.path().join("custom-claude");
    // The only test in this binary that reads the environment.
    unsafe { std::env::set_var("CLAUDE_CONFIG_DIR", &custom) };
    assert_eq!(settings_path(Path::new("")), custom.join("settings.json"));
    unsafe { std::env::remove_var("CLAUDE_CONFIG_DIR") };
    assert_eq!(enabled_path(dir.path()), dir.path().join("enabled"));
}

#[test]
fn hook_command_quotes_unsafe_paths() {
    let cases = [
        (
            "/home/u/.local/bin/dispatch",
            "/home/u/.local/bin/dispatch hook",
        ),
        ("/Users/a b/bin/dispatch", "'/Users/a b/bin/dispatch' hook"),
        ("/tmp/it's/dispatch", r"'/tmp/it'\''s/dispatch' hook"),
    ];
    for (input, want) in cases {
        let got = hook_command(input);
        assert_eq!(got, want, "hook_command({input:?})");
        assert!(
            is_hook_command(&got),
            "{got:?} not recognized as hook command"
        );
    }
}

#[test]
fn every_agents_command_is_ours() {
    for agent in ["codex", "cursor", "gemini", "qwen", "copilot", "grok"] {
        let got = agent_hook_command("/Users/a b/bin/dispatch", agent);
        assert_eq!(
            got,
            format!("'/Users/a b/bin/dispatch' hook --agent {agent}")
        );
        assert!(is_hook_command(&got), "{got:?}");
    }
    assert_eq!(
        agent_hook_command("/bin/dispatch", "claude"),
        "/bin/dispatch hook",
        "Claude Code keeps the bare command"
    );
    assert!(!is_hook_command("/bin/other hook --agent cursor"));
    assert!(!is_hook_command("/bin/dispatch hook --agent"));
}

#[test]
fn the_context_line_comes_in_each_agents_shape() {
    for agent in ["claude", "codex"] {
        assert_eq!(context_output(agent, Default::default()), CONTEXT_MESSAGE);
    }
    let parse = |out: String| -> Value { serde_json::from_str(out.trim_end()).unwrap() };
    for agent in ["gemini", "qwen", "grok"] {
        let out = parse(context_output(agent, Default::default()));
        assert_eq!(out["hookSpecificOutput"]["hookEventName"], "SessionStart");
        assert_eq!(
            out["hookSpecificOutput"]["additionalContext"],
            CONTEXT_MESSAGE
        );
    }
    assert_eq!(
        parse(context_output("copilot", Default::default()))["additionalContext"],
        CONTEXT_MESSAGE
    );
    let mut env = serde_json::Map::new();
    env.insert("DISPATCH_CALLER_SESSION".into(), "c1".into());
    let cursor = parse(context_output("cursor", env));
    assert_eq!(cursor["additional_context"], CONTEXT_MESSAGE);
    assert_eq!(cursor["env"]["DISPATCH_CALLER_SESSION"], "c1");
    assert!(
        parse(context_output("cursor", Default::default()))
            .get("env")
            .is_none()
    );
}
