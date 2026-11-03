use std::fs;

use dispatch_core::skill::*;

#[test]
fn install_refuses_without_force() {
    let dir = tempfile::tempdir().unwrap();
    let p = install(dir.path(), false).unwrap();
    let b = fs::read_to_string(&p).unwrap();
    assert!(b.starts_with("---\nname: dispatch\n"), "bad frontmatter");
    assert!(
        install(dir.path(), false).is_err(),
        "second install must refuse without force"
    );
    install(dir.path(), true).unwrap();
    assert_eq!(p, dir.path().join("dispatch").join("SKILL.md"));
}

#[test]
fn is_installed_after_install() {
    let dir = tempfile::tempdir().unwrap();
    assert!(!is_installed(dir.path()));
    install(dir.path(), false).unwrap();
    assert!(is_installed(dir.path()));
}
