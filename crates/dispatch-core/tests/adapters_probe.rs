use dispatch_core::adapters::probe::flag_present;
#[test]
fn flag_boundaries() {
    for (help, flag, want) in [
        ("  --mode ask\n  --model x\n", "--mode", true),
        ("  --mode ask\n  --model x\n", "--model", true),
        ("  --model x\n", "--mode", false),
        ("  --json-removed\n", "--json", false),
        ("  --json\n", "--json", true),
        ("  --output-format json\n", "-o", false),
        ("  --output-format json\n", "--output-format", true),
        ("execution", "exec", false),
        ("  exec resume\n", "exec", true),
        ("  exec resume\n", "resume", true),
        ("  -p\n", "-p", true),
        ("  --permission-mode plan\n", "-p", false),
        ("  --permission-mode plan\n", "--permission-mode", true),
    ] {
        assert_eq!(flag_present(help, flag), want, "{help} {flag}");
    }
}

#[test]
fn cursor_and_grok_have_no_official_usage() {
    use dispatch_core::adapters::probe::{capabilities, usage_source};
    for name in ["cursor", "grok"] {
        assert!(!capabilities(name).usage);
        assert!(usage_source(name).is_none());
    }
    for name in ["codex", "antigravity"] {
        assert!(capabilities(name).usage);
        assert!(!usage_source(name).unwrap().is_empty());
    }
}
