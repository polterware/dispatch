use dispatch_core::adapters::binaries;
use std::{collections::HashMap, fs, os::unix::fs::PermissionsExt};

#[test]
fn binary_override_and_detect() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fake");
    fs::write(&path, "#!/bin/sh\nexit 0\n").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    let path = path.to_str().unwrap().to_string();
    assert!(binaries::valid_binary(&path).is_ok());
    assert!(binaries::valid_binary("relative").is_err());
    assert!(binaries::valid_binary("/no/such/binary").is_err());
    binaries::set_binaries(HashMap::from([("opencode".into(), path.clone())]));
    assert_eq!(binaries::bin("opencode"), path);
    let detection = binaries::detect()
        .into_iter()
        .find(|d| d.agent == "opencode")
        .unwrap();
    assert_eq!(detection.source, "configured");
    assert_eq!(detection.path, path);
    binaries::set_binaries(HashMap::from([(
        "opencode".into(),
        "/no/such/binary".into(),
    )]));
    let detection = binaries::detect()
        .into_iter()
        .find(|d| d.agent == "opencode")
        .unwrap();
    assert_eq!(detection.source, "invalid");
    binaries::set_binaries(HashMap::new());
}
