use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use dispatch_core::setup::*;
use dispatch_core::skill;
use serde_json::Value;

static ENV: Mutex<()> = Mutex::new(());

struct Fixture {
    _env: MutexGuard<'static, ()>,
    _home: tempfile::TempDir,
    _bin: tempfile::TempDir,
    home: PathBuf,
    binary: String,
    state: PathBuf,
}

// fixture clears CLAUDE_CONFIG_DIR, CODEX_HOME, and XDG_CONFIG_HOME while it
// lives, so the tests never touch the real agent folders.
fn fixture() -> Fixture {
    let env = ENV.lock().unwrap_or_else(|e| e.into_inner());
    unsafe {
        std::env::remove_var("CLAUDE_CONFIG_DIR");
        std::env::remove_var("CODEX_HOME");
        std::env::remove_var("XDG_CONFIG_HOME");
    }
    let home = tempfile::tempdir().unwrap();
    let bin = tempfile::tempdir().unwrap();
    let binary = bin.path().join("dispatch");
    fs::write(&binary, "new").unwrap();
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).unwrap();
    Fixture {
        home: home.path().to_path_buf(),
        binary: binary.to_string_lossy().into_owned(),
        state: home.path().join("state"),
        _env: env,
        _home: home,
        _bin: bin,
    }
}

// skill_state is the state of the skill in the folder Dispatch installs into
// for agent.
fn skill_state(r: &Report, agent: &str) -> String {
    r.skills
        .iter()
        .find(|s| !s.extra && s.agents.iter().any(|a| a == agent))
        .map(|s| s.state.clone())
        .unwrap_or_default()
}

fn hook_of<'a>(r: &'a Report, agent: &str) -> Option<&'a Hook> {
    r.hooks.iter().find(|h| h.agent == agent)
}

#[test]
fn read_starts_missing() {
    let f = fixture();
    let r = read(&f.home, &f.binary, &f.state);
    assert!(
        r.cli.state == MISSING && r.hooks.is_empty() && r.agents.is_empty(),
        "report = {r:?}"
    );
    let dirs: Vec<&str> = r.skills.iter().map(|s| s.dir.as_str()).collect();
    let home = f.home.to_string_lossy();
    assert_eq!(
        dirs,
        [
            format!("{home}/.agents/skills"),
            format!("{home}/.claude/skills"),
            format!("{home}/.gemini/config/skills"),
        ],
        "the shared folder, Claude Code's, and Antigravity's"
    );
    for s in &r.skills {
        assert!(s.state == MISSING && !s.extra, "{s:?}");
    }
}

#[test]
fn link_cli_replaces_an_older_copy() {
    let f = fixture();
    let path = cli_path(&f.home);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, "old").unwrap();
    assert_eq!(read(&f.home, &f.binary, &f.state).cli.state, OTHER);
    link_cli(&f.home, &f.binary).unwrap();
    assert_eq!(read(&f.home, &f.binary, &f.state).cli.state, LINKED);
}

#[test]
fn install_skill_overwrites_an_outdated_copy() {
    let f = fixture();
    let dir = skill::resolve(&f.home, "codex").unwrap().dir;
    assert_eq!(dir, f.home.join(".agents").join("skills"));
    fs::create_dir_all(dir.join(skill::NAME)).unwrap();
    fs::write(dir.join(skill::NAME).join("SKILL.md"), "old").unwrap();
    let r = read(&f.home, &f.binary, &f.state);
    assert!(
        skill_state(&r, "codex") == OUTDATED && skill_state(&r, "cursor") == OUTDATED,
        "codex and cursor share a folder: {:?}",
        r.skills
    );
    install_skill(&f.home, "codex").unwrap();
    let r = read(&f.home, &f.binary, &f.state);
    assert!(
        skill_state(&r, "grok") == CURRENT && skill_state(&r, "claude") == MISSING,
        "skills = {:?}",
        r.skills
    );
    assert!(
        install_skill(&f.home, "nope").is_err(),
        "an unknown agent must be refused"
    );
    install_skill(&f.home, "").unwrap();
    let r = read(&f.home, &f.binary, &f.state);
    assert!(
        r.skills.iter().all(|s| s.state == CURRENT),
        "{:?}",
        r.skills
    );
}

#[test]
fn an_agents_own_folder_shows_only_while_it_holds_a_copy() {
    let f = fixture();
    let own = f.home.join(".cursor").join("skills");
    skill::install(&own, false).unwrap();
    let r = read(&f.home, &f.binary, &f.state);
    let extra = r.skills.iter().find(|s| s.extra).expect("the second copy");
    assert!(
        extra.dir == own.to_string_lossy() && extra.agents == ["cursor"],
        "{extra:?}"
    );
    let removed = remove_skill(&f.home, &extra.dir).unwrap();
    assert_eq!(removed, [own.join(skill::NAME).join("SKILL.md")]);
    assert!(!own.join(skill::NAME).exists(), "its folder goes too");
    let r = read(&f.home, &f.binary, &f.state);
    assert!(r.skills.iter().all(|s| !s.extra), "{:?}", r.skills);
}

#[test]
fn remove_skill_keeps_what_is_not_dispatchs() {
    let f = fixture();
    let shared = f.home.join(".agents").join("skills");
    skill::install(&shared, false).unwrap();
    fs::write(shared.join(skill::NAME).join("notes.md"), "mine").unwrap();
    remove_skill(&f.home, "codex").unwrap();
    assert!(
        shared.join(skill::NAME).join("notes.md").exists()
            && !shared.join(skill::NAME).join("SKILL.md").exists(),
        "only SKILL.md goes"
    );

    // A linked skill folder: the link goes, never what it points at.
    let claude = f.home.join(".claude").join("skills");
    fs::create_dir_all(&claude).unwrap();
    let target = f.home.join("elsewhere");
    skill::install(&target, false).unwrap();
    std::os::unix::fs::symlink(target.join(skill::NAME), claude.join(skill::NAME)).unwrap();
    remove_skill(&f.home, "claude").unwrap();
    assert!(
        fs::symlink_metadata(claude.join(skill::NAME)).is_err(),
        "link removed"
    );
    assert!(skill::is_installed(&target), "the linked skill stays");
    assert!(
        remove_skill(&f.home, "claude").unwrap().is_empty(),
        "removing twice"
    );
}

#[test]
fn set_hook_on_and_off() {
    let f = fixture();
    set_hook(&f.home, &f.binary, &f.state, "", true).unwrap();
    let r = read(&f.home, &f.binary, &f.state);
    let h = hook_of(&r, "claude").expect("a registered hook is listed");
    assert!(h.on && h.current, "hook on = {h:?}");
    set_hook(&f.home, &f.binary, &f.state, "claude", false).unwrap();
    let r = read(&f.home, &f.binary, &f.state);
    let h = hook_of(&r, "claude").expect("~/.claude is there now");
    assert!(!h.on && h.command.is_empty(), "hook off = {h:?}");
    assert!(
        set_hook(&f.home, &f.binary, &f.state, "pi", true).is_err(),
        "pi has no session hook"
    );
}

#[test]
fn hooks_are_listed_for_the_agents_on_this_machine() {
    let f = fixture();
    fs::create_dir_all(f.home.join(".codex")).unwrap();
    fs::create_dir_all(f.home.join(".grok")).unwrap();
    let r = read(&f.home, &f.binary, &f.state);
    let agents: Vec<&str> = r.hooks.iter().map(|h| h.agent.as_str()).collect();
    assert_eq!(agents, ["codex", "grok"]);
    assert!(r.hooks.iter().all(|h| !h.on));
    assert_eq!(r.agents, ["codex", "grok"]);
}

#[test]
fn each_agent_gets_its_hook_in_its_own_shape() {
    let f = fixture();
    let (home, bin, state) = (&f.home, f.binary.as_str(), &f.state);
    let cursor = home.join(".cursor").join("hooks.json");
    fs::create_dir_all(cursor.parent().unwrap()).unwrap();
    fs::write(&cursor, "{\n  \"hooks\": {},\n  \"version\": 1\n}\n").unwrap();
    let gemini = home.join(".gemini").join("settings.json");
    fs::create_dir_all(gemini.parent().unwrap()).unwrap();
    fs::write(&gemini, r#"{"general": {"vimMode": true}}"#).unwrap();
    for agent in [
        "claude", "codex", "cursor", "gemini", "qwen", "copilot", "grok",
    ] {
        set_hook(home, bin, state, agent, true).unwrap();
    }
    let json =
        |p: PathBuf| -> Value { serde_json::from_str(&fs::read_to_string(p).unwrap()).unwrap() };
    let cmd = |agent: &str| format!("{bin} hook --agent {agent}");

    let codex = json(home.join(".codex/hooks.json"));
    assert_eq!(
        codex["hooks"]["SessionStart"][0]["hooks"][0]["command"],
        cmd("codex")
    );
    assert_eq!(codex["hooks"]["SessionStart"][0]["hooks"][0]["timeout"], 10);

    let c = json(cursor.clone());
    assert_eq!(c["version"], 1);
    assert_eq!(c["hooks"]["sessionStart"][0]["command"], cmd("cursor"));
    assert!(c["hooks"]["sessionStart"][0].get("type").is_none());

    let g = json(gemini.clone());
    assert_eq!(
        g["general"]["vimMode"], true,
        "the rest of the settings stays"
    );
    assert_eq!(
        g["hooks"]["SessionStart"][0]["hooks"][0]["timeout"], 10000,
        "milliseconds"
    );

    let copilot = home.join(".copilot/hooks/dispatch.json");
    let cp = json(copilot.clone());
    assert_eq!(cp["version"], 1);
    assert_eq!(cp["hooks"]["sessionStart"][0]["bash"], cmd("copilot"));
    assert_eq!(cp["hooks"]["sessionStart"][0]["timeoutSec"], 10);

    let grok = home.join(".grok/hooks/dispatch.json");
    assert_eq!(
        json(grok.clone())["hooks"]["SessionStart"][0]["hooks"][0]["command"],
        cmd("grok")
    );

    let claude = json(home.join(".claude/settings.json"));
    assert_eq!(
        claude["hooks"]["SessionStart"][0]["hooks"][0]["command"],
        format!("{bin} hook")
    );

    let r = read(home, bin, state);
    assert_eq!(r.hooks.len(), 7, "{:?}", r.hooks);
    assert!(r.hooks.iter().all(|h| h.on && h.current), "{:?}", r.hooks);

    // Off: the shared files keep the rest, Dispatch's own files go, and the
    // enabled flag goes with the last hook.
    for agent in ["claude", "codex", "cursor", "gemini", "qwen", "copilot"] {
        set_hook(home, bin, state, agent, false).unwrap();
        assert!(dispatch_core::hook::is_enabled(state), "grok's is still on");
    }
    assert_eq!(json(cursor)["version"], 1);
    assert_eq!(json(gemini.clone())["general"]["vimMode"], true);
    assert!(json(gemini).get("hooks").is_none());
    assert!(!copilot.exists(), "Dispatch's own file is removed");
    set_hook(home, bin, state, "grok", false).unwrap();
    assert!(!grok.exists());
    assert!(
        !dispatch_core::hook::is_enabled(state),
        "the last hook takes the flag"
    );
}

#[test]
fn under_grok_one_hook_speaks() {
    use dispatch_core::hook::quiet_under_grok;
    let f = fixture();
    let (home, bin, state) = (&f.home, f.binary.as_str(), &f.state);
    set_hook(home, bin, state, "cursor", true).unwrap();
    assert!(
        !quiet_under_grok(home, "cursor"),
        "Cursor's is the only one"
    );
    set_hook(home, bin, state, "claude", true).unwrap();
    assert!(quiet_under_grok(home, "cursor") && !quiet_under_grok(home, "claude"));
    set_hook(home, bin, state, "grok", true).unwrap();
    assert!(quiet_under_grok(home, "claude") && quiet_under_grok(home, "cursor"));
    assert!(!quiet_under_grok(home, "grok") && !quiet_under_grok(home, "codex"));
}

#[test]
fn legacy_opencode_plugin_is_removed() {
    let f = fixture();
    let (home, state) = (&f.home, &f.state);
    let config = home.join(".config").join("opencode").join("opencode.json");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    let dir = opencode_plugin_dir(state);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("server.js"), "export default {}\n").unwrap();
    let switch = state.join("opencode-plugin.json");
    fs::write(&switch, "{\n  \"enabled\": true\n}\n").unwrap();
    let ours = serde_json::to_string(&dir.to_string_lossy().as_ref()).unwrap();
    let src = format!(
        "{{\n  // plugins\n  \"plugins\": [\n    \"acme\", /* theirs */\n    {ours}\n  ],\n  \"model\": \"a/b\"\n}}\n"
    );
    fs::write(&config, &src).unwrap();

    assert!(remove_legacy_opencode_plugin(home, state).unwrap());
    assert!(!dir.exists(), "plugin folder should be gone");
    assert!(!switch.exists(), "switch should be gone");
    let left = fs::read_to_string(&config).unwrap();
    assert!(left.contains("// plugins"), "comment kept:\n{left}");
    assert!(left.contains("/* theirs */"), "comment kept:\n{left}");
    let plain = left.replace("// plugins", "").replace("/* theirs */", "");
    let parsed: Value = serde_json::from_str(&plain).expect("config is JSON");
    assert_eq!(parsed["plugins"], serde_json::json!(["acme"]), "{left}");
    assert_eq!(parsed["model"], "a/b");
    assert!(
        !remove_legacy_opencode_plugin(home, state).unwrap(),
        "nothing left to remove"
    );
    assert_eq!(fs::read_to_string(&config).unwrap(), left);
}

#[test]
fn legacy_opencode_plugin_goes_as_an_object_entry_too() {
    let f = fixture();
    let (home, state) = (&f.home, &f.state);
    let config = home.join(".config").join("opencode").join("opencode.jsonc");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    let dir = opencode_plugin_dir(state);
    let package = dir.to_string_lossy();
    for ours in [
        format!("{{\"package\": \"{package}\", \"options\": {{\"binary\": \"/bin/dispatch\"}}}}"),
        format!("\"{package}\""),
    ] {
        let src = format!("{{\"plugins\": [\"a\", {ours}, \"b\"]}}");
        fs::write(&config, &src).unwrap();
        assert!(
            remove_legacy_opencode_plugin(home, state).unwrap(),
            "{ours}"
        );
        assert_eq!(
            fs::read_to_string(&config).unwrap(),
            "{\"plugins\": [\"a\", \"b\"]}",
            "{ours}"
        );
    }
}

#[test]
fn legacy_opencode_cleanup_keeps_the_config_mode() {
    let f = fixture();
    let (home, state) = (&f.home, &f.state);
    let config = home.join(".config").join("opencode").join("opencode.json");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    let dir = opencode_plugin_dir(state);
    let ours = serde_json::to_string(&dir.to_string_lossy().as_ref()).unwrap();
    fs::write(&config, format!("{{\"plugins\": [{ours}]}}")).unwrap();
    fs::set_permissions(&config, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(remove_legacy_opencode_plugin(home, state).unwrap());
    let mode = fs::metadata(&config).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    let left: Vec<_> = fs::read_dir(config.parent().unwrap())
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with(".opencode.json.") && n.ends_with(".tmp"))
        .collect();
    assert!(left.is_empty(), "temporary files left: {left:?}");
}

#[test]
fn legacy_opencode_cleanup_leaves_an_unreadable_config_alone() {
    let f = fixture();
    let (home, state) = (&f.home, &f.state);
    let config = home.join(".config").join("opencode").join("opencode.json");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    fs::write(&config, "{\"a\": ").unwrap();
    let dir = opencode_plugin_dir(state);
    fs::create_dir_all(&dir).unwrap();
    assert!(remove_legacy_opencode_plugin(home, state).is_err());
    assert_eq!(fs::read_to_string(&config).unwrap(), "{\"a\": ");
    assert!(dir.exists(), "the folder waits for the next try");
}

#[test]
fn opencode_config_prefers_jsonc() {
    let f = fixture();
    let home: &Path = &f.home;
    let dir = home.join(".config").join("opencode");
    assert_eq!(opencode_config_path(home), dir.join("opencode.json"));
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("opencode.jsonc"), "{}").unwrap();
    assert_eq!(opencode_config_path(home), dir.join("opencode.jsonc"));
    let xdg = tempfile::tempdir().unwrap();
    unsafe { std::env::set_var("XDG_CONFIG_HOME", xdg.path()) };
    let got = opencode_config_path(home);
    unsafe { std::env::remove_var("XDG_CONFIG_HOME") };
    assert_eq!(got, xdg.path().join("opencode").join("opencode.json"));
}
