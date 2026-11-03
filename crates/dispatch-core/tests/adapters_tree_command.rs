use dispatch_core::process::Tree;
use std::{fs, os::unix::fs::PermissionsExt};

#[test]
fn capture_command_and_owner_identity() {
    const MARKER: &str = "DISPATCH_FAKE_PS_DIR";
    if std::env::var_os(MARKER).is_some() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let mut tree = rt.block_on(Tree::capture(10)).unwrap();
        assert_eq!(tree.members().len(), 2);
        assert!(tree.include(40, "different identity").is_err());
        tree.include(40, "Thu Oct 15 10:00:03 2026").unwrap();
        assert_eq!(tree.members().last().unwrap().pid, 40);
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ps");
    fs::write(&path, "#!/bin/sh\nprintf '%s\\n' '10 1 10 Thu Oct 15 10:00:00 2026' '20 10 20 Thu Oct 15 10:00:01 2026' '40 1 40 Thu Oct 15 10:00:03 2026'\n").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "capture_command_and_owner_identity",
            "--nocapture",
        ])
        .env(MARKER, dir.path())
        .env("PATH", dir.path())
        .status()
        .unwrap();
    assert!(status.success());
}
