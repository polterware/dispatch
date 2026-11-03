use dispatch_core::{
    catalog::Catalog,
    config::Policy,
    difficulty::new_difficulty,
    roles::{
        GENERAL_ROLE, GENERAL_WHEN, Profile, ProfileOptions, RoleEntry, apply_picked,
        apply_profile, builtin_profiles, global_profiles_path, is_builtin_profile,
        load_all_profiles, load_profiles, pick_role, pick_roles, profile_names,
        project_profiles_path, raise_to_floor, save_profile, valid_profile_name, validate_profile,
    },
    router::{Agent, Filters, RouteOptions, route},
    tiers::{Entry, Table},
};
use std::collections::{BTreeMap, HashSet};

#[test]
fn test_apply_profile_explicit_wins() {
    let mut o = ProfileOptions {
        tier: "hard".into(),
        prompt: "do it".into(),
        filters: Filters {
            model: "pinned".into(),
            ..Default::default()
        },
        ..Default::default()
    };
    let p = Profile {
        tier: "easy".into(),
        model: "profile".into(),
        read_only: true,
        timeout: "2m".into(),
        instructions: "be brief".into(),
        prefer: vec!["codex".into()],
        ..Default::default()
    };
    apply_profile(&mut o, "reviewer", &p).expect("apply_profile");
    assert_eq!(o.tier, "hard");
    assert_eq!(o.filters.model, "pinned");
    assert_eq!(o.tier_floor, "easy");
    assert!(o.read_only);
    assert_eq!(o.timeout_ns, 2 * 60 * 1_000_000_000);
    assert_eq!(o.profile, "reviewer");
    assert_eq!(o.prompt, "be brief\n\ndo it");
}

#[test]
fn test_load_all_profiles_project_wins() {
    let dir = tempfile::tempdir().expect("tempdir");
    let user = dir.path().join("u.json");
    let project = dir.path().join("p.json");
    std::fs::write(
        &user,
        r#"{"agents":{"a":{"tier":"easy"},"b":{"tier":"easy"}}}"#,
    )
    .expect("write user");
    std::fs::write(&project, r#"{"agents":{"a":{"tier":"hard"}}}"#).expect("write project");

    let m = load_all_profiles(&user, &project).expect("load_all_profiles");
    assert_eq!(m["a"].tier, "hard");
    assert_eq!(m["b"].tier, "easy");

    assert!(
        load_profiles(&dir.path().join("missing.json"))
            .expect("missing profiles")
            .is_empty()
    );
}

#[test]
fn test_validate_profile() {
    let known = HashSet::from(["codex".to_owned()]);
    let got = validate_profile(
        "x",
        &Profile {
            tier: "hard".into(),
            prefer: vec!["codex".into()],
            ..Default::default()
        },
        &known,
    );
    assert!(got.is_empty(), "unexpected problems: {got:?}");

    let got = validate_profile(
        "x",
        &Profile {
            tier: "huge".into(),
            timeout: "soon".into(),
            prefer: vec!["codex".into()],
            avoid: vec!["codex".into()],
            ..Default::default()
        },
        &known,
    );
    assert_eq!(got.len(), 3, "want 3 problems, got {got:?}");

    assert_eq!(
        validate_profile("e", &Profile::default(), &known).len(),
        1,
        "empty role not flagged"
    );
}

#[test]
fn test_save_profile_keeps_the_others_and_removes() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("agents.json");

    save_profile(
        &path,
        "reviewer",
        Profile {
            tier: "hard".into(),
            read_only: true,
            ..Default::default()
        },
        false,
    )
    .expect("save reviewer");
    save_profile(
        &path,
        "tester",
        Profile {
            tier: "normal".into(),
            instructions: "Write tests.".into(),
            ..Default::default()
        },
        false,
    )
    .expect("save tester");
    save_profile(
        &path,
        "reviewer",
        Profile {
            tier: "easy".into(),
            ..Default::default()
        },
        false,
    )
    .expect("overwrite reviewer");

    let got = load_profiles(&path).expect("load_profiles");
    assert_eq!(got.len(), 2);
    assert_eq!(got["reviewer"].tier, "easy");
    assert_eq!(got["tester"].instructions, "Write tests.");

    save_profile(&path, "tester", Profile::default(), true).expect("remove tester");
    assert!(
        save_profile(&path, "tester", Profile::default(), true).is_err(),
        "removing a missing role must fail"
    );
    let after = load_profiles(&path).expect("load after remove");
    assert_eq!(after.len(), 1);
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
    ] {
        assert_eq!(valid_profile_name(name), want, "{name:?}");
    }
}

#[test]
fn test_pick_role_needs_a_clear_answer() {
    let roles = BTreeMap::from([
        ("reviewer".into(), "r".into()),
        ("tester".into(), "t".into()),
        (GENERAL_ROLE.into(), GENERAL_WHEN.into()),
    ]);
    let cases: Vec<(&str, BTreeMap<String, f64>, &str)> = vec![
        (
            "",
            BTreeMap::from([
                ("reviewer".into(), 0.8),
                ("tester".into(), 0.1),
                (GENERAL_ROLE.into(), 0.1),
            ]),
            "reviewer",
        ),
        (
            "",
            BTreeMap::from([
                ("reviewer".into(), 0.45),
                ("tester".into(), 0.4),
                (GENERAL_ROLE.into(), 0.15),
            ]),
            GENERAL_ROLE,
        ),
        ("", BTreeMap::from([("ghost".into(), 0.9)]), GENERAL_ROLE),
        ("tester", BTreeMap::new(), "tester"),
        ("ghost", BTreeMap::new(), GENERAL_ROLE),
    ];
    for (role, probs, want) in cases {
        assert_eq!(pick_role(role, &probs, &roles), want, "{role:?} {probs:?}");
    }
    assert_eq!(
        pick_role("tester", &BTreeMap::new(), &BTreeMap::new()),
        "",
        "no roles picks nothing"
    );
}

#[test]
fn test_pick_roles_skips_manual_and_undescribed_profiles() {
    let roles = pick_roles(&BTreeMap::from([
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
    ]));
    assert_eq!(roles.len(), 2);
    assert_eq!(roles.get("a").map(String::as_str), Some("x"));
    assert!(!roles[GENERAL_ROLE].is_empty());

    let none = pick_roles(&BTreeMap::from([(
        "c".into(),
        Profile {
            tier: "hard".into(),
            ..Default::default()
        },
    )]));
    assert!(none.is_empty(), "no pickable profile asks no role");
}

#[test]
fn test_builtin_profiles_merge_under_the_files() {
    let dir = tempfile::tempdir().expect("tempdir");
    let user = global_profiles_path(dir.path());
    assert_eq!(
        project_profiles_path(dir.path()),
        dir.path().join(".dispatch").join("roles.json")
    );
    let builtins = builtin_profiles();
    assert!(is_builtin_profile("reviewer"));
    assert!(!is_builtin_profile("mine"));
    assert_eq!(
        profile_names(&builtins),
        vec!["explorer", "implementer", "planner", "reviewer", "tester"]
    );

    // An agents.json an earlier Dispatch wrote is read while no roles.json exists.
    std::fs::write(
        dir.path().join("agents.json"),
        r#"{"agents":{"reviewer":{"tier":"normal","readonly":true},"mine":{"when":"w"}}}"#,
    )
    .expect("write legacy agents.json");

    let m = load_all_profiles(&user, std::path::Path::new("")).expect("load_all_profiles");
    assert_eq!(m["reviewer"].tier, "normal");
    assert_eq!(m["reviewer"].when, builtins["reviewer"].when);
    assert!(m.contains_key("planner"));
    assert_eq!(m["mine"].when, "w");

    assert!(!valid_profile_name(GENERAL_ROLE));
    assert!(!valid_profile_name("none"));

    let known = HashSet::from(["codex".to_owned()]);
    let board_problems = validate_profile(
        "x",
        &Profile {
            tiers: vec![
                RoleEntry {
                    tier: "normal".into(),
                    agent: "ghost".into(),
                    model: "m".into(),
                    provider: String::new(),
                },
                RoleEntry {
                    tier: "normal".into(),
                    agent: "codex".into(),
                    model: String::new(),
                    provider: String::new(),
                },
                RoleEntry {
                    tier: "huge".into(),
                    agent: "codex".into(),
                    model: "m".into(),
                    provider: String::new(),
                },
            ],
            ..Default::default()
        },
        &known,
    );
    assert_eq!(board_problems.len(), 3, "{board_problems:?}");

    save_profile(
        &user,
        "mine",
        Profile {
            when: "w2".into(),
            ..Default::default()
        },
        false,
    )
    .expect("first save migrates agents.json to roles.json");
    let migrated = load_all_profiles(&user, std::path::Path::new("")).expect("load migrated");
    assert_eq!(migrated["mine"].when, "w2");
    assert_eq!(migrated["reviewer"].tier, "normal");

    let manual_ok = validate_profile(
        "x",
        &Profile {
            manual: true,
            ..Default::default()
        },
        &known,
    );
    assert!(
        manual_ok.is_empty(),
        "a manual-only override is a profile: {manual_ok:?}"
    );
}

#[test]
fn test_role_board_routing_and_floor_lifting() {
    // Auto-picked profiles do not force readOnly or pinned model/provider.
    let builtins = builtin_profiles();
    let mut picked_opts = ProfileOptions {
        prompt: "review branch x".into(),
        ..Default::default()
    };
    apply_picked(&mut picked_opts, "reviewer", &builtins["reviewer"]).expect("apply_picked");
    assert_eq!(picked_opts.profile, "reviewer");
    assert!(!picked_opts.read_only);
    assert_eq!(picked_opts.tier_floor, "hard");
    assert!(
        picked_opts
            .prompt
            .starts_with(&builtins["reviewer"].instructions)
    );

    // raise_to_floor lifts an easy task to reviewer's hard floor, and leaves hard untouched.
    let mut diff = new_difficulty(Some([0.9, 0.08, 0.02]), "").expect("easy diff");
    let note = raise_to_floor(&mut diff, "hard", "reviewer");
    assert_eq!(diff.tier, "hard");
    assert_eq!(note, "role reviewer: raised to its hard tier");
    assert_eq!(raise_to_floor(&mut diff, "easy", "explorer"), "");

    // Role board routes its band in board order and falls back to the general tier table when empty.
    let table = Table {
        tiers: BTreeMap::from([
            (
                "easy".into(),
                vec![Entry {
                    agent: "a".into(),
                    model: "general-easy".into(),
                    ..Default::default()
                }],
            ),
            (
                "normal".into(),
                vec![Entry {
                    agent: "a".into(),
                    model: "normal".into(),
                    ..Default::default()
                }],
            ),
        ]),
    };
    let agents = vec![
        Agent {
            name: "a".into(),
            installed: true,
            read_only: true,
            file_editing: true,
            ..Default::default()
        },
        Agent {
            name: "b".into(),
            installed: true,
            read_only: true,
            file_editing: true,
            ..Default::default()
        },
    ];
    let policy = Policy {
        discover: false,
        ..Default::default()
    };
    let tester = Profile {
        tiers: vec![
            RoleEntry {
                tier: "normal".into(),
                agent: "b".into(),
                model: "first".into(),
                provider: String::new(),
            },
            RoleEntry {
                tier: "normal".into(),
                agent: "a".into(),
                model: "second".into(),
                provider: String::new(),
            },
            RoleEntry {
                tier: "hard".into(),
                agent: "a".into(),
                model: "deep".into(),
                provider: String::new(),
            },
        ],
        ..Default::default()
    };
    let cat = Catalog::default();

    let normal_route = route(
        &table,
        &policy,
        &agents,
        &RouteOptions {
            tier: "auto".into(),
            difficulty: Some(-1.8), // normal band
            role_board: tester.board(),
            ..Default::default()
        },
        &cat,
    )
    .expect("route normal board");
    assert_eq!(normal_route.candidates.len(), 2);
    assert_eq!(
        normal_route
            .winner
            .as_ref()
            .map(|w| (w.agent.as_str(), w.model.as_str())),
        Some(("b", "first"))
    );
    assert_eq!(normal_route.candidates[1].filters.model, "second");

    let easy_route = route(
        &table,
        &policy,
        &agents,
        &RouteOptions {
            tier: "auto".into(),
            difficulty: Some(-3.0), // easy band -> empty in tester board -> falls back to tier table
            role_board: tester.board(),
            ..Default::default()
        },
        &cat,
    )
    .expect("route easy fallback");
    assert_eq!(
        easy_route.winner.as_ref().map(|w| w.model.as_str()),
        Some("general-easy")
    );
}
