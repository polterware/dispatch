#![forbid(unsafe_code)]

use chrono::{TimeZone, Utc};
use dispatch_core::runstate::{
    CORE_FILE, Core, ERR_CORE_LOCKED, build_id, core_path, dir, ensure_dir, executable,
    is_core_locked, lock_core, lock_path, log_path, newer_build, pid_alive, read_core, remove_core,
    write_json,
};
use std::{fs, os::unix::fs::PermissionsExt, process::Command};
use tempfile::TempDir;

#[test]
fn test_lock_core_is_exclusive() {
    let tmp = TempDir::new().expect("tempdir");
    let db = tmp.path().join("local.db");

    let first = lock_core(&db).expect("first lock_core");
    let err = lock_core(&db).expect_err("second lock_core must fail");
    assert!(is_core_locked(&err), "err = {err:?}");
    assert_eq!(err.to_string(), ERR_CORE_LOCKED);

    first.close().expect("close first lock");

    let again = lock_core(&db).expect("lock after release");
    again.close().expect("close second lock");
}

#[test]
fn test_paths_permissions_and_core_json_lifecycle() {
    let tmp = TempDir::new().expect("tempdir");
    let db = tmp.path().join("sub").join("dispatch.db");

    assert_eq!(dir(&db), tmp.path().join("sub").join("run"));
    assert_eq!(
        core_path(&db),
        tmp.path().join("sub").join("run").join(CORE_FILE)
    );
    assert_eq!(
        log_path(&db),
        tmp.path().join("sub").join("run").join("core.log")
    );
    assert_eq!(
        lock_path(&db),
        tmp.path().join("sub").join("run").join("core.lock")
    );

    ensure_dir(&db).expect("ensure_dir");
    let dir_mode = fs::metadata(dir(&db))
        .expect("dir metadata")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(dir_mode, 0o700);

    let self_pid = i32::try_from(std::process::id()).expect("pid i32");
    assert!(pid_alive(self_pid));
    assert!(!pid_alive(0));
    assert!(!pid_alive(-5));

    let started = Utc
        .with_ymd_and_hms(2026, 4, 20, 12, 30, 0)
        .single()
        .expect("timestamp");
    let core = Core {
        pid: self_pid,
        endpoint: "http://127.0.0.1:41234".into(),
        token: "secret-token".into(),
        db_path: db.to_string_lossy().into_owned(),
        build: "1700000000-4096".into(),
        started_at: started,
    };

    let cpath = core_path(&db);
    write_json(&cpath, &core).expect("write_json");

    let file_mode = fs::metadata(&cpath)
        .expect("core.json metadata")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(file_mode, 0o600);

    let raw = fs::read_to_string(&cpath).expect("read core.json");
    assert!(raw.ends_with('\n'));
    let parsed_val: serde_json::Value = serde_json::from_str(&raw).expect("parse json");
    assert_eq!(parsed_val["pid"], self_pid);
    assert_eq!(parsed_val["endpoint"], "http://127.0.0.1:41234");
    assert_eq!(parsed_val["token"], "secret-token");
    assert_eq!(parsed_val["dbPath"], db.to_string_lossy().as_ref());
    assert_eq!(parsed_val["build"], "1700000000-4096");
    assert!(parsed_val["startedAt"].is_string());

    assert_eq!(read_core(&db), Some(core.clone()));

    // Mismatched PID leaves core.json untouched.
    remove_core(&db, self_pid + 99_999);
    assert!(cpath.exists());

    // Matching PID removes core.json.
    remove_core(&db, self_pid);
    assert!(!cpath.exists());
    assert_eq!(read_core(&db), None);

    // Dead PID is cleaned up on read_core.
    let mut child = Command::new("/usr/bin/true").spawn().expect("spawn true");
    let dead_pid = i32::try_from(child.id()).expect("child pid");
    child.wait().expect("wait true");
    assert!(!pid_alive(dead_pid));

    let stale = Core {
        pid: dead_pid,
        ..core
    };
    write_json(&cpath, &stale).expect("write stale core.json");
    assert!(cpath.exists());
    assert_eq!(read_core(&db), None);
    assert!(!cpath.exists(), "stale core.json must be removed");

    // Corrupt JSON is cleaned up on read_core.
    fs::write(&cpath, b"{not-json").expect("write corrupt core.json");
    assert_eq!(read_core(&db), None);
    assert!(!cpath.exists(), "corrupt core.json must be removed");
}

#[test]
fn test_build_handshake_helpers() {
    let exe = executable().expect("executable");
    assert!(exe.exists());
    let id = build_id(&exe);
    let (sec_str, size_str) = id.split_once('-').expect("mtime-size format");
    assert!(sec_str.parse::<i64>().expect("mtime") > 0);
    assert!(size_str.parse::<u64>().expect("size") > 0);

    let tmp = TempDir::new().expect("tempdir");
    assert_eq!(build_id(tmp.path().join("missing-binary")), "");

    assert!(newer_build("2000-100", "1999-999999"));
    assert!(!newer_build("2000-999", "2000-100"));
    assert!(!newer_build("1999-100", "2000-100"));
    assert!(newer_build("10-1", ""));
    assert!(!newer_build("", "10-1"));
}
