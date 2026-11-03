use dispatch_core::process::{Tree, parse_table};
#[test]
fn invalid_process_table() {
    for out in ["", "10 1", "bad 1 10 Thu Oct 15 10:00:00 2026"] {
        assert!(parse_table(out).is_err(), "{out}");
    }
}
#[test]
fn capture_owner_identity_from_snapshot() {
    let table = parse_table("10 1 10 Thu Oct 15 10:00:00 2026\n20 10 20 Thu Oct 15 10:00:01 2026\n40 1 40 Thu Oct 15 10:00:03 2026").unwrap();
    let mut tree = Tree::from_table(table, 10);
    assert_eq!(tree.members().len(), 2);
    assert!(tree.include(40, "different identity").is_err());
    tree.include(40, "Thu Oct 15 10:00:03 2026").unwrap();
    assert_eq!(tree.members().last().unwrap().pid, 40);
}
