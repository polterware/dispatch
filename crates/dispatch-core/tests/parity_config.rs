use dispatch_core::{
    adapters::acp::{custom_agents, set_custom_agents},
    config::{
        CLASSIFIER_JEV, Policy, Rules, apply_agent_config, default_policy, load,
        policy_config_path, validate_config, write_config, write_config_key,
    },
};
use std::collections::{BTreeMap, HashMap};

#[test]
fn test_load_missing_and_priority() {
    let dir = tempfile::tempdir().expect("tempdir");
    let missing = dir.path().join("config.json");

    let f = load(&missing).expect("load missing");
    assert!(f.priority.is_empty());
    assert!(f.discover);

    std::fs::write(&missing, b" \n").expect("write whitespace");
    let f = load(&missing).expect("load whitespace");
    assert!(f.priority.is_empty());
    assert!(f.discover);

    std::fs::write(&missing, br#"{"priority":"quality"}"#).expect("write quality");
    let f = load(&missing).expect("load quality");
    assert_eq!(f.priority, "quality");
    assert!(f.discover);

    std::fs::write(&missing, br#"{"priority":"turbo"}"#).expect("write turbo");
    assert!(load(&missing).is_err(), "accepted invalid priority 'turbo'");

    std::fs::write(&missing, b"{").expect("write broken");
    assert!(load(&missing).is_err(), "accepted broken json");
}

#[test]
fn test_rejects_wrong_known_types_and_write_preserves_unknown() {
    let dir = tempfile::tempdir().expect("tempdir");
    let p = dir.path().join("config.json");
    for body in [
        r#"{"discover":"yes"}"#,
        r#"{"allow":[]}"#,
        r#"{"deny":{"agents":"codex"}}"#,
    ] {
        std::fs::write(&p, body).expect("write invalid type");
        assert!(load(&p).is_err(), "accepted {body}");
    }

    std::fs::write(
        &p,
        br#"{"future":{"x":1},"subagents":true,"explore":false,"priority":"speed"}"#,
    )
    .expect("write with unknown and retired keys");
    write_config(
        &p,
        &Policy {
            priority: "quality".into(),
            discover: true,
            allow: Rules {
                providers: vec!["openai".into()],
                ..Default::default()
            },
            ..Default::default()
        },
    )
    .expect("write_config");

    let raw = std::fs::read_to_string(&p).expect("read back");
    assert!(raw.contains(r#""future""#), "{raw}");
    assert!(raw.contains(r#""quality""#), "{raw}");
    assert!(!raw.contains(r#""subagents""#), "{raw}");
    assert!(!raw.contains(r#""explore""#), "{raw}");
}

#[test]
fn test_binaries_round_trip() {
    let dir = tempfile::tempdir().expect("tempdir");
    let g = dir.path().join("g.json");

    write_config_key(
        &g,
        "binaries",
        BTreeMap::from([("opencode", "/x/opencode-cli")]),
    )
    .expect("write global binaries");

    let f = load(&g).expect("load");
    assert_eq!(f.binaries.len(), 1);
    assert_eq!(
        f.binaries.get("opencode").map(String::as_str),
        Some("/x/opencode-cli")
    );

    write_config_key(&g, "binaries", serde_json::Value::Null).expect("clear binaries");
    let cleared = load(&g).expect("load cleared");
    assert!(
        cleared.binaries.is_empty(),
        "not cleared: {:?}",
        cleared.binaries
    );
}

#[test]
fn test_acp_agents_round_trip_and_are_validated() {
    let dir = tempfile::tempdir().expect("tempdir");
    let g = dir.path().join("g.json");

    write_config_key(
        &g,
        "acp_agents",
        BTreeMap::from([("mine", vec!["/opt/mine", "acp"])]),
    )
    .expect("write global acp_agents");

    let f = load(&g).expect("load");
    assert_eq!(f.acp_agents.len(), 1);
    assert_eq!(f.acp_agents["mine"][1], "acp");

    write_config_key(&g, "acp_agents", BTreeMap::from([("codex", vec!["/x"])]))
        .expect("write built-in collision");
    let err = load(&g)
        .expect_err("a built-in agent's name must be refused")
        .to_string();
    assert!(err.contains("built-in"), "unexpected error: {err}");
}

#[test]
fn test_discover_defaults_on_and_explicit_false_wins() {
    let dir = tempfile::tempdir().expect("tempdir");
    let global = dir.path().join("global.json");

    for body in ["", "{}", r#"{"discover":true}"#, r#"{"discover":false}"#] {
        std::fs::write(&global, body).expect("write global");
        let want = body != r#"{"discover":false}"#;
        let f = load(&global).expect("load");
        assert_eq!(f.discover, want, "{body:?}: {f:?}");
    }
}

#[test]
fn test_validate_config_and_apply_agent_config() {
    let mut p = default_policy();
    assert!(p.discover && p.roles);
    validate_config(&p).expect("default policy valid");

    p.classifier = CLASSIFIER_JEV.into();
    p.priority = "speed".into();
    validate_config(&p).expect("jev + speed valid");

    // Unnormalized "off" in struct validation is rejected just like Go's ValidateConfig.
    p.classifier = "off".into();
    assert!(validate_config(&p).is_err());

    let dir = tempfile::tempdir().expect("tempdir");
    let cfg_path = policy_config_path(dir.path());
    write_config(
        &cfg_path,
        &Policy {
            acp_agents: BTreeMap::from([(
                "custom-acp".into(),
                vec!["/usr/local/bin/custom-acp".into(), "serve".into()],
            )]),
            ..Default::default()
        },
    )
    .expect("write config");
    apply_agent_config(dir.path()).expect("apply_agent_config");
    assert!(custom_agents().contains_key("custom-acp"));
    set_custom_agents(HashMap::new());
}
