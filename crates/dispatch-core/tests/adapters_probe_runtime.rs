use dispatch_core::adapters::{binaries, probe};
use std::{collections::HashMap, fs, os::unix::fs::PermissionsExt, path::Path};

fn install(dir: &Path, binary: &str, version: &str, help: &str) -> String {
    let path = dir.join(binary);
    let version_path = dir.join(format!("{binary}.version"));
    let help_path = dir.join(format!("{binary}.help"));
    let log_path = dir.join(format!("{binary}.log"));
    fs::write(&version_path, version).unwrap();
    fs::write(&help_path, help).unwrap();
    fs::write(&path, format!("#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\ncase \"$*\" in\n--version) /bin/cat '{}' ;;\n--help|*' --help') /bin/cat '{}' ;;\n*) exit 97 ;;\nesac\n", log_path.display(), version_path.display(), help_path.display())).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path.to_string_lossy().into_owned()
}

fn fixture(name: &str) -> (String, String, String) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/adapters/probes")
        .join(name);
    let version = fs::read_dir(root).unwrap().next().unwrap().unwrap().path();
    (
        version.file_name().unwrap().to_string_lossy().into_owned(),
        fs::read_to_string(version.join("version.txt")).unwrap(),
        fs::read_to_string(version.join("help.txt")).unwrap(),
    )
}

#[tokio::test]
async fn golden_probes_and_doctor_degraded() {
    let dir = tempfile::tempdir().unwrap();
    let names = [
        "codex",
        "claude",
        "cursor",
        "grok",
        "antigravity",
        "opencode",
        "pi",
        "gemini",
        "qwen",
        "copilot",
        "kimi",
    ];
    let defaults = binaries::default_binaries();
    let mut overrides = HashMap::new();
    for name in names {
        let (version_name, version, help) = fixture(name);
        let binary = &defaults[name];
        let path = install(dir.path(), binary, &version, &help);
        let report =
            probe::probe_binary(name, &path, probe::capabilities(name), &probe::flags(name)).await;
        assert_eq!(report.status, "OK", "{name}: {:?}", report.notes);
        assert_eq!(report.version, version_name);
        let log = fs::read_to_string(dir.path().join(format!("{binary}.log"))).unwrap();
        assert!(
            log.lines()
                .all(|line| line == "--version" || line.ends_with("--help"))
        );
        overrides.insert(name.to_string(), path);
    }
    binaries::set_binaries(overrides);
    let reports = probe::doctor().await;
    assert_eq!(reports.len(), names.len());
    assert!(reports.iter().all(|r| r.status == "OK"));
    let path = dir.path().join("codex.help");
    fs::write(
        &path,
        fs::read_to_string(&path)
            .unwrap()
            .replace("--json", "--json-removed"),
    )
    .unwrap();
    let reports = probe::doctor().await;
    assert_eq!(
        reports.iter().find(|r| r.name == "codex").unwrap().status,
        "DEGRADED"
    );
    assert!(
        reports
            .iter()
            .filter(|r| r.name != "codex")
            .all(|r| r.status == "OK")
    );
    binaries::set_binaries(HashMap::new());
}

#[tokio::test]
async fn probe_degraded_when_flag_removed() {
    let dir = tempfile::tempdir().unwrap();
    let (_, version, help) = fixture("codex");
    let binary = install(
        dir.path(),
        "codex",
        &version,
        &help.replace("--json", "--json-removed"),
    );
    let r = probe::probe_binary(
        "codex",
        &binary,
        probe::capabilities("codex"),
        &probe::flags("codex"),
    )
    .await;
    assert_eq!(r.status, "DEGRADED");
    assert!(!r.flags.iter().find(|f| f.name == "--json").unwrap().present);
}

/// A run waits for its CLI's probe, so the probe asks for the version and every help at once:
/// with each answer taking a second, it ends near one second, not near three.
#[tokio::test]
async fn probe_asks_the_cli_everything_at_once() {
    let dir = tempfile::tempdir().unwrap();
    let (_, version, help) = fixture("codex");
    let binary = install(dir.path(), "codex", &version, &help);
    let body = fs::read_to_string(&binary).unwrap();
    fs::write(
        &binary,
        body.replacen("#!/bin/sh\n", "#!/bin/sh\n/bin/sleep 1\n", 1),
    )
    .unwrap();
    let started = std::time::Instant::now();
    let r = probe::probe_binary(
        "codex",
        &binary,
        probe::capabilities("codex"),
        &probe::flags("codex"),
    )
    .await;
    let took = started.elapsed();
    assert_eq!(r.status, "OK", "{:?}", r.notes);
    let log = fs::read_to_string(dir.path().join("codex.log")).unwrap();
    assert_eq!(log.lines().count(), 3, "{log}");
    assert!(
        took < std::time::Duration::from_millis(2500),
        "the probe took {took:?}"
    );
}

#[tokio::test]
async fn probe_unverified() {
    let dir = tempfile::tempdir().unwrap();
    let (_, _, help) = fixture("grok");
    let binary = install(dir.path(), "grok", "grok dev\n", &help);
    let r = probe::probe_binary(
        "grok",
        &binary,
        probe::capabilities("grok"),
        &probe::flags("grok"),
    )
    .await;
    assert_eq!(r.status, "UNVERIFIED");
    assert!(r.version.is_empty());
    fs::write(&binary, "#!/bin/sh\nexit 1\n").unwrap();
    let r = probe::probe_binary(
        "grok",
        &binary,
        probe::capabilities("grok"),
        &probe::flags("grok"),
    )
    .await;
    assert_eq!(r.status, "UNVERIFIED");
}

#[test]
fn cursor_and_grok_have_no_official_usage() {
    for name in ["cursor", "grok"] {
        assert!(!probe::capabilities(name).usage);
    }
    for name in ["codex", "antigravity"] {
        assert!(probe::capabilities(name).usage);
    }
}

#[test]
fn probe_does_not_leak_secret() {
    const MARKER: &str = "DISPATCH_PROBE_SECRET_CHILD";
    if let Ok(dir) = std::env::var(MARKER) {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let binary = Path::new(&dir).join("grok");
        let report = runtime.block_on(probe::probe_binary(
            "grok",
            binary.to_str().unwrap(),
            probe::capabilities("grok"),
            &probe::flags("grok"),
        ));
        assert_eq!(report.status, "OK", "{:?}", report.notes);
        assert_eq!(
            fs::read_to_string(Path::new(&dir).join("env.txt"))
                .unwrap()
                .trim(),
            "clean"
        );
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let (_, _, help) = fixture("grok");
    fs::write(dir.path().join("help"), help).unwrap();
    let binary = dir.path().join("grok");
    fs::write(&binary, format!("#!/bin/sh\nif [ -n \"$AWS_SECRET_ACCESS_KEY\" ]; then echo LEAKED > '{0}/env.txt'; else echo clean > '{0}/env.txt'; fi\ncase \"$*\" in --version) echo 'grok 1.0.52';; --help) /bin/cat '{0}/help';; *) exit 97;; esac\n", dir.path().display())).unwrap();
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).unwrap();
    let result = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "probe_does_not_leak_secret", "--nocapture"])
        .env(MARKER, dir.path())
        .env("AWS_SECRET_ACCESS_KEY", "super-secret-value")
        .status()
        .unwrap();
    assert!(result.success());
}
