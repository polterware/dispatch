use dispatch_core::{
    Error,
    adapters::Model,
    catalog::load_catalog,
    config::Policy,
    reports::{
        OPEN_CODE_AGENT, OPEN_CODE_PICK_NOTE, OpenCodeAdapter, agent_pool, merge_agent,
        route_with_opencode,
    },
    router::{Agent, Filters, RouteOptions, RouteReport},
    tiers::{Entry, Table},
    tiers_build::build_tiers,
};
use std::collections::BTreeMap;

struct FakeOpenCode {
    installed: bool,
    models: Vec<Model>,
}

impl OpenCodeAdapter for FakeOpenCode {
    fn installed(&self) -> bool {
        self.installed
    }

    async fn models(&self) -> Result<Vec<Model>, Error> {
        Ok(self.models.clone())
    }
}

struct OpenCodeHarness {
    dir: tempfile::TempDir,
    table: Table,
    policy: Policy,
    agents: Vec<Agent>,
    opencode: FakeOpenCode,
}

fn opencode_harness() -> OpenCodeHarness {
    let dir = tempfile::tempdir().expect("tempdir");
    let efforts = vec!["low".into(), "medium".into(), "high".into(), "xhigh".into()];
    let agents = vec![
        Agent {
            name: "a".into(),
            installed: true,
            read_only: true,
            file_editing: true,
            effort_mode: "config".into(),
            efforts: efforts.clone(),
            ..Default::default()
        },
        Agent {
            name: OPEN_CODE_AGENT.into(),
            installed: true,
            read_only: true,
            file_editing: true,
            effort_mode: "config".into(),
            efforts,
            discovered: vec![
                "zai/glm-5.3-flash".into(),
                "openai/gpt-5.6-sol".into(),
                "anthropic/claude-sonnet-5-5".into(),
                "opencode/glm-5.3".into(),
            ],
            ..Default::default()
        },
    ];
    let opencode = FakeOpenCode {
        installed: true,
        models: vec![
            Model {
                id: "zai/glm-5.3-flash".into(),
                ..Default::default()
            },
            Model {
                id: "openai/gpt-5.6-sol".into(),
                ..Default::default()
            },
            Model {
                id: "anthropic/claude-sonnet-5-5".into(),
                ..Default::default()
            },
            Model {
                id: "opencode/glm-5.3".into(),
                ..Default::default()
            },
        ],
    };
    let table = Table {
        tiers: BTreeMap::from([
            (
                "easy".into(),
                vec![Entry {
                    agent: "a".into(),
                    model: "gpt-5.6-luna".into(),
                    ..Default::default()
                }],
            ),
            (
                "normal".into(),
                vec![Entry {
                    agent: "a".into(),
                    model: "gpt-5.6-terra".into(),
                    ..Default::default()
                }],
            ),
            (
                "hard".into(),
                vec![Entry {
                    agent: "a".into(),
                    model: "gpt-5.6-sol".into(),
                    ..Default::default()
                }],
            ),
        ]),
    };
    OpenCodeHarness {
        dir,
        table,
        policy: Policy::default(),
        agents,
        opencode,
    }
}

fn candidate_models(r: &RouteReport) -> Vec<String> {
    r.candidates
        .iter()
        .map(|c| format!("{}/{}", c.filters.agent, c.filters.model))
        .collect()
}

#[tokio::test]
async fn test_opencode_auto_weighs_the_models_it_lists() {
    let h = opencode_harness();
    let cat = load_catalog(h.dir.path()).expect("load catalog");
    let r = route_with_opencode(
        &h.table,
        &h.policy,
        &h.agents,
        &RouteOptions {
            tier: "auto".into(),
            difficulty: Some(-3.0), // easyClassifier() equivalent
            filters: Filters {
                agent: OPEN_CODE_AGENT.into(),
                ..Default::default()
            },
            ..Default::default()
        },
        &cat,
        Some(&h.opencode),
    )
    .await
    .expect("route");

    let winner = r.winner.as_ref().expect("winner");
    assert_eq!(winner.agent, OPEN_CODE_AGENT);
    assert!(r.difficulty.is_some());

    let got = candidate_models(&r);
    for want in [
        "opencode/zai/glm-5.3-flash",
        "opencode/openai/gpt-5.6-sol",
        "opencode/anthropic/claude-sonnet-5-5",
        "opencode/opencode/glm-5.3",
    ] {
        assert!(got.iter().any(|m| m == want), "auto lacks {want}: {got:?}");
    }
    assert!(
        !got.iter().any(|m| m == "a/gpt-5.6-luna"),
        "auto weighs another agent: {got:?}"
    );
}

#[tokio::test]
async fn test_opencode_tier_without_entry_ranks_its_models_for_the_tier() {
    let h = opencode_harness();
    let cat = load_catalog(h.dir.path()).expect("load catalog");
    let pool = vec![
        Entry {
            agent: OPEN_CODE_AGENT.into(),
            model: "zai/glm-5.3-flash".into(),
            provider: "zai".into(),
            ..Default::default()
        },
        Entry {
            agent: OPEN_CODE_AGENT.into(),
            model: "openai/gpt-5.6-sol".into(),
            provider: "openai".into(),
            ..Default::default()
        },
        Entry {
            agent: OPEN_CODE_AGENT.into(),
            model: "anthropic/claude-sonnet-5-5".into(),
            provider: "anthropic".into(),
            ..Default::default()
        },
    ];
    for tier in ["easy", "hard"] {
        let r = route_with_opencode(
            &h.table,
            &h.policy,
            &h.agents,
            &RouteOptions {
                disabled: vec![dispatch_core::types::DisabledModel {
                    provider: "opencode".into(),
                    model: "glm-5.3".into(),
                }],
                tier: tier.into(),
                filters: Filters {
                    agent: OPEN_CODE_AGENT.into(),
                    ..Default::default()
                },
                ..Default::default()
            },
            &cat,
            Some(&h.opencode),
        )
        .await
        .expect("route");
        assert!(r.winner.is_some(), "{tier}: {r:?}");
        let want: Vec<String> = build_tiers(&pool, &cat, "balanced")
            .tiers
            .get(tier)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|b| format!("{}/{}", b.entry.agent, b.entry.model))
            .collect();
        let got = candidate_models(&r);
        assert!(
            !got.is_empty() && got[0] == want[0],
            "{tier}: got {got:?}, want {want:?}"
        );
        assert!(
            !got.iter().any(|m| m == "opencode/opencode/glm-5.3"),
            "{tier}: a denied model was ranked: {got:?}"
        );
        assert!(
            r.note.contains(OPEN_CODE_PICK_NOTE),
            "{tier}: note {:?}",
            r.note
        );
    }
}

#[tokio::test]
async fn test_opencode_tier_keeps_the_tables_opencode_entries() {
    let mut h = opencode_harness();
    h.table
        .tiers
        .get_mut("normal")
        .expect("normal")
        .push(Entry {
            agent: OPEN_CODE_AGENT.into(),
            model: "opencode/glm-5.3".into(),
            ..Default::default()
        });
    let cat = load_catalog(h.dir.path()).expect("load catalog");
    let r = route_with_opencode(
        &h.table,
        &h.policy,
        &h.agents,
        &RouteOptions {
            tier: "normal".into(),
            filters: Filters {
                agent: OPEN_CODE_AGENT.into(),
                ..Default::default()
            },
            ..Default::default()
        },
        &cat,
        Some(&h.opencode),
    )
    .await
    .expect("route");
    assert!(r.winner.is_some());
    let got = candidate_models(&r);
    assert_eq!(got, vec!["opencode/opencode/glm-5.3"]);
    assert!(!r.note.contains(OPEN_CODE_PICK_NOTE));
}

#[tokio::test]
async fn test_opencode_not_installed_adds_nothing() {
    let mut h = opencode_harness();
    h.opencode.installed = false;
    for a in &mut h.agents {
        if a.name == OPEN_CODE_AGENT {
            a.installed = false;
        }
    }
    let cat = load_catalog(h.dir.path()).expect("load catalog");
    let r = route_with_opencode(
        &h.table,
        &h.policy,
        &h.agents,
        &RouteOptions {
            tier: "hard".into(),
            filters: Filters {
                agent: OPEN_CODE_AGENT.into(),
                ..Default::default()
            },
            ..Default::default()
        },
        &cat,
        Some(&h.opencode),
    )
    .await
    .expect("route");
    assert!(r.winner.is_none(), "{r:?}");
}

#[test]
fn test_agent_pool_and_merge_agent() {
    let table = Table {
        tiers: BTreeMap::from([
            (
                "easy".into(),
                vec![
                    Entry {
                        agent: "a".into(),
                        model: "m1".into(),
                        ..Default::default()
                    },
                    Entry {
                        agent: OPEN_CODE_AGENT.into(),
                        model: "zai/glm-5.3-flash".into(),
                        ..Default::default()
                    },
                ],
            ),
            (
                "hard".into(),
                vec![
                    Entry {
                        agent: OPEN_CODE_AGENT.into(),
                        model: "openai/gpt-5.6-sol".into(),
                        ..Default::default()
                    },
                    Entry {
                        agent: "a".into(),
                        model: "m2".into(),
                        ..Default::default()
                    },
                ],
            ),
        ]),
    };
    let listed = vec![
        Entry {
            agent: OPEN_CODE_AGENT.into(),
            model: "zai/glm-5.3-flash".into(),
            ..Default::default()
        },
        Entry {
            agent: OPEN_CODE_AGENT.into(),
            model: "anthropic/claude-opus-5-5".into(),
            ..Default::default()
        },
        Entry {
            agent: OPEN_CODE_AGENT.into(),
            model: "opencode/glm-5.3".into(),
            ..Default::default()
        },
        Entry {
            agent: "a".into(),
            model: "m3".into(),
            ..Default::default()
        },
    ];
    let pool: Vec<String> = agent_pool(OPEN_CODE_AGENT, &table, &listed)
        .into_iter()
        .map(|e| e.model)
        .collect();
    assert_eq!(
        pool,
        vec![
            "zai/glm-5.3-flash",
            "openai/gpt-5.6-sol",
            "anthropic/claude-opus-5-5",
            "opencode/glm-5.3"
        ]
    );

    let built = Table {
        tiers: BTreeMap::from([
            (
                "normal".into(),
                vec![
                    Entry {
                        agent: OPEN_CODE_AGENT.into(),
                        model: "opencode/glm-5.3".into(),
                        ..Default::default()
                    },
                    Entry {
                        agent: "a".into(),
                        model: "ignored".into(),
                        ..Default::default()
                    },
                ],
            ),
            (
                "hard".into(),
                vec![Entry {
                    agent: OPEN_CODE_AGENT.into(),
                    model: "zai/glm-5.3-flash".into(),
                    ..Default::default()
                }],
            ),
        ]),
    };
    let merged = merge_agent(&table, &built, OPEN_CODE_AGENT);
    assert_eq!(
        merged.tiers.get("easy"),
        Some(&vec![Entry {
            agent: "a".into(),
            model: "m1".into(),
            ..Default::default()
        }])
    );
    assert_eq!(
        merged.tiers.get("normal"),
        Some(&vec![Entry {
            agent: OPEN_CODE_AGENT.into(),
            model: "opencode/glm-5.3".into(),
            ..Default::default()
        }])
    );
    assert_eq!(
        merged.tiers.get("hard"),
        Some(&vec![
            Entry {
                agent: "a".into(),
                model: "m2".into(),
                ..Default::default()
            },
            Entry {
                agent: OPEN_CODE_AGENT.into(),
                model: "zai/glm-5.3-flash".into(),
                ..Default::default()
            },
        ])
    );
}
