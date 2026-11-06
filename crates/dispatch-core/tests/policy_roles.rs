use dispatch_core::{
    config::Policy,
    roles::{self, Profile, RoleEntry},
};
use std::collections::{BTreeMap, HashSet};

#[test]
fn test_discover_defaults_on_and_explicit_false_wins() {
    for body in ["", "{}", r#"{"discover":true}"#, r#"{"discover":false}"#] {
        let want = body != r#"{"discover":false}"#;
        assert_eq!(Policy::parse(body).unwrap().discover, want);
    }
}
#[test]
fn wrong_known_types_are_rejected() {
    for body in [
        r#"{"discover":"yes"}"#,
        r#"{"priority":null}"#,
        r#"{"roles":null}"#,
    ] {
        assert!(Policy::parse(body).is_err(), "{body}");
    }
}
#[test]
fn test_validate_profile() {
    let known = HashSet::from(["codex".into()]);
    assert!(
        roles::validate_profile(
            "x",
            &Profile {
                tier: "hard".into(),
                prefer: vec!["codex".into()],
                ..Default::default()
            },
            &known
        )
        .is_empty()
    );
    let p = Profile {
        tier: "huge".into(),
        timeout: "soon".into(),
        prefer: vec!["codex".into()],
        avoid: vec!["codex".into()],
        ..Default::default()
    };
    assert_eq!(roles::validate_profile("x", &p, &known).len(), 3);
    assert_eq!(
        roles::validate_profile("e", &Profile::default(), &known).len(),
        1
    );
}
#[test]
fn test_valid_profile_name() {
    for (name, want) in [
        ("reviewer", true),
        ("code-review2", true),
        ("", false),
        ("2fast", false),
        ("Reviewer", false),
        ("a b", false),
        ("a/b", false),
        ("general", false),
        ("none", false),
    ] {
        assert_eq!(roles::valid_profile_name(name), want, "{name}");
    }
}
#[test]
fn test_pick_role_needs_a_clear_answer() {
    let roles = BTreeMap::from([
        ("reviewer".into(), "r".into()),
        ("tester".into(), "t".into()),
        ("general".into(), roles::GENERAL_WHEN.into()),
    ]);
    for (role, probs, want) in [
        (
            "",
            vec![("reviewer", 0.8), ("tester", 0.1), ("general", 0.1)],
            "reviewer",
        ),
        (
            "",
            vec![("reviewer", 0.45), ("tester", 0.4), ("general", 0.15)],
            "general",
        ),
        ("", vec![("ghost", 0.9)], "general"),
        ("tester", vec![], "tester"),
        ("ghost", vec![], "general"),
    ] {
        let probs = probs.into_iter().map(|(a, b)| (a.into(), b)).collect();
        assert_eq!(roles::pick_role(role, &probs, &roles), want);
    }
    assert_eq!(
        roles::pick_role("tester", &BTreeMap::new(), &BTreeMap::new()),
        ""
    );
}
#[test]
fn test_pick_roles_skips_manual_and_undescribed_profiles() {
    let p = BTreeMap::from([
        (
            "a".into(),
            Profile {
                when: "x".into(),
                ..Default::default()
            },
        ),
        (
            "b".into(),
            Profile {
                when: "y".into(),
                manual: true,
                ..Default::default()
            },
        ),
        (
            "c".into(),
            Profile {
                tier: "hard".into(),
                ..Default::default()
            },
        ),
    ]);
    let got = roles::pick_roles(&p);
    assert_eq!(got.len(), 2);
    assert_eq!(got["a"], "x");
    assert_eq!(got["general"], roles::GENERAL_WHEN);
    assert!(roles::pick_roles(&BTreeMap::from([("c".into(), p["c"].clone())])).is_empty());
}
#[test]
fn legacy_merge_and_board_validation() {
    let user = roles::parse_profiles(
        r#"{"agents":{"reviewer":{"tier":"normal","readonly":true},"mine":{"when":"w"}}}"#,
    )
    .unwrap();
    let profiles = roles::merge_profiles(user, BTreeMap::new());
    assert_eq!(profiles["reviewer"].tier, "normal");
    assert_eq!(
        profiles["reviewer"].when,
        roles::builtin_profiles()["reviewer"].when
    );
    assert_eq!(profiles["mine"].when, "w");
    assert!(profiles.contains_key("planner"));
    let p = Profile {
        tiers: vec![
            RoleEntry {
                tier: "normal".into(),
                agent: "ghost".into(),
                model: "m".into(),
                ..Default::default()
            },
            RoleEntry {
                tier: "normal".into(),
                agent: "codex".into(),
                ..Default::default()
            },
            RoleEntry {
                tier: "huge".into(),
                agent: "codex".into(),
                model: "m".into(),
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    assert_eq!(
        roles::validate_profile("x", &p, &HashSet::from(["codex".into()])).len(),
        3
    );
    assert!(
        roles::validate_profile(
            "x",
            &Profile {
                manual: true,
                ..Default::default()
            },
            &HashSet::new()
        )
        .is_empty()
    );
}

#[test]
fn test_apply_profile_explicit_wins() {
    let mut o = roles::ProfileOptions {
        prompt: "do it".into(),
        ..Default::default()
    };
    o.filters.model = "pinned".into();
    let p = Profile {
        tier: "easy".into(),
        model: "profile".into(),
        read_only: true,
        timeout: "2m".into(),
        instructions: "be brief".into(),
        prefer: vec!["codex".into()],
        ..Default::default()
    };
    roles::apply_profile(&mut o, "reviewer", &p).unwrap();
    assert_eq!(o.filters.model, "pinned");
    assert_eq!(o.tier_floor, "easy");
    assert!(o.read_only);
    assert_eq!(o.timeout_ns, 120_000_000_000);
    assert_eq!(o.profile, "reviewer");
    assert_eq!(o.prompt, "be brief\n\ndo it");
}

#[test]
fn test_load_all_profiles_project_wins() {
    let user =
        roles::parse_profiles(r#"{"agents":{"a":{"tier":"easy"},"b":{"tier":"easy"}}}"#).unwrap();
    let project = roles::parse_profiles(r#"{"agents":{"a":{"tier":"hard"}}}"#).unwrap();
    let p = roles::merge_profiles(user, project);
    assert_eq!(p["a"].tier, "hard");
    assert_eq!(p["b"].tier, "easy");
}

#[test]
fn retired_rule_shapes_are_ignored() {
    for body in [r#"{"allow":[]}"#, r#"{"deny":{"agents":"codex"}}"#] {
        assert_eq!(Policy::parse(body).unwrap(), Policy::default());
    }
}
