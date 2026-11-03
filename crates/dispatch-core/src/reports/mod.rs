//! Spend, stats, pace, model benchmarks and reports, the state file, and the OpenCode route.

pub mod models_bench;
pub mod models_report;
pub mod opencode_route;
pub mod pace;
pub mod pace_history;
pub mod spend;
pub mod state_file;
pub mod stats;
pub mod stats_capacity;
pub mod stats_evidence;

pub use models_bench::{BenchLevel, BenchmarkReport, ModelBenchmark, benchmark_models};
pub use models_report::{
    BandPass, CatalogEntry, CatalogReport, refresh_catalog, refresh_catalog_from, report_catalog,
};
pub use opencode_route::{
    OPEN_CODE_AGENT, OPEN_CODE_PICK_NOTE, OpenCodeAdapter, TieredEntry, agent_pool, merge_agent,
    open_code_entries, route_with_opencode,
};
pub use pace::{AHEAD, FAR, ON_TRACK, Pace, SLIGHTLY, Stage, linear};
pub use pace_history::{History, Snapshot, historical, snapshots_from_quota};
pub use spend::{
    COST_AGENT, COST_CATALOG, COST_MIXED, CatalogSpendExt, RunSpendExt, UsageExt, add_usage,
    context_window, total_tokens, usage_cost,
};
pub use state_file::{with_file_lock, write_file_atomic, write_file_atomic_async};
pub use stats::{
    MIN_SAMPLES, RATE_LIMIT_PATTERN, Rankings, Report, Row, RunStatsExt, SCHEMA, STATS_SCHEMA,
    Spend, StatRank, StatsOptions, compute, from_store, is_rate_limit_message, observed_ms,
    parse_since, valid_by,
};
pub use stats_capacity::{
    CAPACITY_SCHEMA, CapacityKey, CapacityReport, WindowStat, capacity, capacity_from_store,
};
pub use stats_evidence::{ModelEvidence, evidence_by_tier_model, evidence_key};
