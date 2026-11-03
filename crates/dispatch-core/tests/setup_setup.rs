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

// fixture clears CLAUDE_CONFIG_DIR and XDG_CONFIG_HOME while it lives, so the
// tests never touch the real agent folders.
fn fixture() -> Fixture {
    let env = ENV.lock().unwrap_or_else(|e| e.into_inner());
    unsafe {
        std::env::remove_var("CLAUDE_CONFIG_DIR");
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

fn skill_state(r: &Report, agent: &str) -> String {
    r.skills
        .iter()
        .find(|s| s.agent == agent)
        .map(|s| s.state.clone())
        .unwrap_or_default()
}

#[test]
fn read_starts_missing() {
    let f = fixture();
    let r = read(&f.home, &f.binary, &f.state);
    assert!(
        r.cli.state == MISSING && !r.hook.on && !r.skills.is_empty(),
        "report = {r:?}"
    );
    for s in &r.skills {
        assert_eq!(s.state, MISSING, "{}", s.agent);
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
    let dir = skill::targets(&f.home)["codex"].dir.clone();
    fs::create_dir_all(dir.join(skill::NAME)).unwrap();
    fs::write(dir.join(skill::NAME).join("SKILL.md"), "old").unwrap();
    let r = read(&f.home, &f.binary, &f.state);
    assert!(
        skill_state(&r, "codex") == OUTDATED && skill_state(&r, "grok") == OUTDATED,
        "codex and grok share a folder: {:?}",
        r.skills
    );
    install_skill(&f.home, "codex").unwrap();
    let r = read(&f.home, &f.binary, &f.state);
    assert!(
        skill_state(&r, "codex") == CURRENT && skill_state(&r, "claude") == MISSING,
        "skills = {:?}",
        r.skills
    );
    assert!(
        install_skill(&f.home, "nope").is_err(),
        "an unknown agent must be refused"
    );
}

#[test]
fn set_hook_on_and_off() {
    let f = fixture();
    set_hook(&f.home, &f.binary, &f.state, true).unwrap();
    let h = read(&f.home, &f.binary, &f.state).hook;
    assert!(h.on && h.current, "hook on = {h:?}");
    set_hook(&f.home, &f.binary, &f.state, false).unwrap();
    let h = read(&f.home, &f.binary, &f.state).hook;
    assert!(!h.on && h.command.is_empty(), "hook off = {h:?}");
}

#[test]
fn opencode_plugin_install_update_and_remove() {
    let f = fixture();
    let (home, binary, state) = (&f.home, f.binary.as_str(), &f.state);
    let config = home.join(".config").join("opencode").join("opencode.json");
    let got = read(home, binary, state).opencode;
    assert!(
        got.state == MISSING && got.on && got.config == config.to_string_lossy(),
        "before install = {got:?}"
    );
    let dir = install_opencode_plugin(home, binary, state).unwrap();
    assert_eq!(dir, state.join("opencode-plugin"));
    for name in [
        "package.json",
        "server.js",
        "rpc.js",
        "gui-experimental.cjs",
    ] {
        assert!(dir.join(name).exists(), "{name}");
    }
    let body = fs::read_to_string(&config).unwrap();
    let parsed: Value = serde_json::from_str(&body).expect("config is JSON");
    let plugins = parsed["plugins"].as_array().unwrap();
    let state_file = state.join(OPENCODE_SWITCH);
    assert!(
        plugins.len() == 1
            && plugins[0]["package"] == dir.to_string_lossy().as_ref()
            && plugins[0]["options"]["binary"] == binary
            && plugins[0]["options"]["state"] == state_file.to_string_lossy().as_ref()
            && parsed["$schema"].as_str().is_some_and(|s| !s.is_empty()),
        "config = {body}"
    );
    assert_eq!(read(home, binary, state).opencode.state, CURRENT);

    fs::write(dir.join("server.js"), "export default {}\n").unwrap();
    assert_eq!(read(home, binary, state).opencode.state, OUTDATED);
    let other_dir = tempfile::tempdir().unwrap();
    let other = other_dir.path().join("dispatch");
    assert_eq!(
        read(home, &other.to_string_lossy(), state).opencode.state,
        OUTDATED
    );
    install_opencode_plugin(home, binary, state).unwrap();
    assert_eq!(read(home, binary, state).opencode.state, CURRENT);
    let after = fs::read_to_string(&config).unwrap();
    assert_eq!(
        after.matches("\"package\":").count(),
        1,
        "an update must replace the entry, not add one:\n{after}"
    );

    set_opencode_delegation(state, false).unwrap();
    let got = read(home, binary, state).opencode;
    assert!(!got.on && got.state == CURRENT, "delegation off = {got:?}");

    remove_opencode_plugin(home, state).unwrap();
    let got = read(home, binary, state).opencode;
    assert!(got.state == MISSING && got.on, "after remove = {got:?}");
    assert!(!dir.exists(), "plugin folder should be gone");
    let left = fs::read_to_string(&config).unwrap();
    assert!(
        !left.contains(dir.to_string_lossy().as_ref()),
        "entry left behind:\n{left}"
    );
    serde_json::from_str::<Value>(&left).expect("config after remove is JSON");
    remove_opencode_plugin(home, state).expect("removing twice");
}

#[test]
fn edit_opencode_config_keeps_the_rest_of_the_file() {
    let dir = "/state/opencode-plugin";
    let entry = r#"{"package":"/state/opencode-plugin","options":{"binary":"/bin/dispatch"}}"#;
    let cases = [
        (
            "no plugins",
            "{\n  // models\n  \"model\": \"a/b\"\n}\n".to_string(),
            format!(
                "{{\n  \"plugins\": [\n    {entry}\n  ],\n  // models\n  \"model\": \"a/b\"\n}}\n"
            ),
        ),
        (
            "empty object",
            "{}".to_string(),
            format!("{{\n  \"plugins\": [\n    {entry}\n  ]\n}}"),
        ),
        (
            "empty list",
            "{\"plugins\": []}".to_string(),
            format!("{{\"plugins\": [\n    {entry}\n  ]}}"),
        ),
        (
            "other plugins",
            "{\n  \"plugins\": [\n    \"acme\", /* keep */\n    \"./local\"\n  ]\n}".to_string(),
            format!(
                "{{\n  \"plugins\": [\n    \"acme\", /* keep */\n    \"./local\",\n    {entry}\n  ]\n}}"
            ),
        ),
        (
            "older entry as a path",
            "{\"plugins\": [\"acme\", \"/state/opencode-plugin\", \"b\"]}".to_string(),
            format!("{{\"plugins\": [\"acme\", {entry}, \"b\"]}}"),
        ),
    ];
    for (name, src, want) in cases {
        let tmp = tempfile::tempdir().unwrap();
        let config = tmp.path().join("opencode.jsonc");
        fs::write(&config, &src).unwrap();
        edit_opencode_config(&config, dir, Some(entry.as_bytes())).unwrap();
        let got = fs::read_to_string(&config).unwrap();
        assert_eq!(got, want, "{name}");
    }
}

#[test]
fn edit_opencode_config_keeps_the_file_mode() {
    let tmp = tempfile::tempdir().unwrap();
    let config = tmp.path().join("opencode.json");
    fs::write(&config, "{}").unwrap();
    fs::set_permissions(&config, fs::Permissions::from_mode(0o600)).unwrap();
    edit_opencode_config(&config, "/d", Some(br#""/d""#)).unwrap();
    let mode = fs::metadata(&config).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    let left: Vec<_> = fs::read_dir(tmp.path())
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with(".opencode.json.") && n.ends_with(".tmp"))
        .collect();
    assert!(left.is_empty(), "temporary files left: {left:?}");
}

#[test]
fn edit_opencode_config_removes_only_its_entry() {
    let dir = "/state/opencode-plugin";
    let ours = r#"{"package": "/state/opencode-plugin", "options": {}}"#;
    let cases = [
        (
            format!("{{\"plugins\": [\"a\", {ours}, \"b\"]}}"),
            "{\"plugins\": [\"a\", \"b\"]}",
        ),
        (
            format!("{{\"plugins\": [\"a\", {ours}]}}"),
            "{\"plugins\": [\"a\"]}",
        ),
        (
            format!("{{\"plugins\": [{ours}], \"model\": \"x\"}}"),
            "{\"plugins\": [], \"model\": \"x\"}",
        ),
        (
            "{\"plugins\": [\"a\"]}".to_string(),
            "{\"plugins\": [\"a\"]}",
        ),
    ];
    for (src, want) in cases {
        let tmp = tempfile::tempdir().unwrap();
        let config = tmp.path().join("opencode.json");
        fs::write(&config, &src).unwrap();
        edit_opencode_config(&config, dir, None).unwrap();
        assert_eq!(fs::read_to_string(&config).unwrap(), want);
    }
}

#[test]
fn edit_opencode_config_refuses_what_it_cannot_read() {
    for src in ["[]", "{\"plugins\": \"acme\"}", "{\"a\": "] {
        let tmp = tempfile::tempdir().unwrap();
        let config = tmp.path().join("opencode.json");
        fs::write(&config, src).unwrap();
        assert!(
            edit_opencode_config(&config, "/d", Some(br#""x""#)).is_err(),
            "{src}: want an error"
        );
        assert_eq!(fs::read_to_string(&config).unwrap(), src);
    }
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
