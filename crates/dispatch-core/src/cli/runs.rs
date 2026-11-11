//! The run commands: run, runs, status, result, cancel, tiers, usage, agents.

use super::{
    CliResult, Fail, Flags, caller, dial_workspace, one_line, ops, or_unknown, run_error, short_id,
    sort_top, stdin_is_terminal, usage_err, working_dir, write_json_compact,
};
use crate::{
    Error,
    coreclient::Clients,
    proto as pb,
    server::convert::run_meta,
    types::{Attempt, Run, STATUS_FAILED, STATUS_PENDING, Usage},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;
use std::time::Duration;

const RUN_USAGE: &str = "usage: dispatch run [--tier <auto|easy|normal|hard>] [--role <name|none>] [--card <slug>] [--agent <name>] [--provider <p>] [--model <m>] [--effort <e>] [--priority <p>] [--relax] [--readonly] [--images] [--timeout <dur>] [--dir <repo>] [--worktree-mode worktree|in-place] [--worktree <dir>] [--json] [--compact] <prompt|->
       dispatch send <id> [--json] [--compact] <message|->
       dispatch runs [--conversation <id>] [--project <id>] [--json] [--compact]
       dispatch status <id> [--json] [--compact]
       dispatch result <id> [--section summary|agent|files|attempts|meta|output] [--offset <n>] [--limit <n>] [--json] [--compact]
       dispatch events <id> [--tail-bytes <n>] [--before-seq <n> --before-offset <n>] [--json] [--compact]
       dispatch cancel <id> [--json] [--compact]
       dispatch tiers [--check] [--json] [--compact]
       dispatch tiers build [--agent <a>] [--apply] [--json]
       dispatch stats [--json] [--compact]
       dispatch usage [--by <model|agent|provider|tier|effort>] [--since <7d>] [--conversation <id>] [--json] [--compact]
       dispatch agents [--json] [--compact]
       dispatch agents add <name> -- <command that starts its ACP server...>
       dispatch agents remove <name>
       dispatch agents enable|disable <name>
";

pub(crate) fn dispatch_usage() -> String {
    format!("{RUN_USAGE}{}", ops::OPS_USAGE)
}

fn usage() -> Fail {
    usage_err(dispatch_usage())
}

const PROJECT_USAGE: &str = "workspace id (default: the workspace holding the cwd)";

/// How much of a run's log `dispatch events` reads by default, from its end.
const DEFAULT_EVENT_BYTES: i64 = 256 * 1024;

/// The flags most run commands share.
fn common(f: &mut Flags) {
    f.string("project", "", PROJECT_USAGE)
        .bool("json", false, "print one JSON object")
        .bool("compact", false, "compact output");
}

/// The fields of a run the CLI prints, read from the core's message or from
/// the `--section meta` record.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct RunView {
    #[serde(rename = "ConversationID")]
    conversation_id: String,
    #[serde(rename = "CallerAgent")]
    caller_agent: String,
    #[serde(rename = "Origin")]
    origin: String,
    #[serde(rename = "ID")]
    id: String,
    #[serde(rename = "ProjectID")]
    project_id: String,
    #[serde(rename = "Status")]
    status: String,
    #[serde(rename = "Tier")]
    tier: String,
    #[serde(rename = "Agent")]
    agent: String,
    #[serde(rename = "Model")]
    model: String,
    #[serde(rename = "Provider")]
    provider: String,
    #[serde(rename = "Effort")]
    effort: String,
    #[serde(rename = "CardSlug")]
    card_slug: String,
    #[serde(rename = "AgentRunID")]
    agent_run_id: String,
    #[serde(rename = "Branch")]
    branch: String,
    #[serde(rename = "Worktree")]
    worktree: String,
    #[serde(rename = "Linked", skip_serializing_if = "Vec::is_empty")]
    linked: Vec<crate::types::LinkedRepo>,
    #[serde(rename = "ExitClass")]
    exit_class: String,
    #[serde(rename = "QueuedMs")]
    queued_ms: Option<i64>,
    #[serde(rename = "RunMs")]
    run_ms: Option<i64>,
    #[serde(rename = "FilesChanged")]
    files_changed: Option<i64>,
    #[serde(rename = "Insertions")]
    insertions: Option<i64>,
    #[serde(rename = "Deletions")]
    deletions: Option<i64>,
    #[serde(rename = "Usage")]
    usage: Option<Usage>,
    #[serde(rename = "Error")]
    error: String,
    #[serde(rename = "Note")]
    note: String,
    #[serde(rename = "Attempts")]
    attempts: Option<Vec<Attempt>>,
}

impl From<&Run> for RunView {
    fn from(r: &Run) -> Self {
        Self {
            conversation_id: r.conversation_id.clone(),
            caller_agent: r.caller_agent.clone(),
            origin: crate::types::origin(&r.caller_agent).into(),
            id: r.id.clone(),
            project_id: r.project_id.clone(),
            status: r.status.clone(),
            tier: r.tier.clone(),
            agent: r.agent.clone(),
            model: r.model.clone(),
            provider: r.provider.clone(),
            effort: r.effort.clone(),
            card_slug: r.card_slug.clone(),
            agent_run_id: r.agent_run_id.clone(),
            branch: r.branch.clone(),
            worktree: r.worktree.clone(),
            linked: r.linked.clone(),
            exit_class: r.exit_class.clone(),
            queued_ms: r.queued_ms,
            run_ms: r.run_ms,
            files_changed: r.files_changed,
            insertions: r.insertions,
            deletions: r.deletions,
            usage: r.usage.clone(),
            error: r.error.clone(),
            note: r.note.clone(),
            attempts: (!r.attempts.is_empty()).then(|| r.attempts.clone()),
        }
    }
}

/// The run as the core's message carries it, its attempts included, so a summary counts a
/// failed run's tries.
pub(super) fn run_from_proto(p: &pb::DispatchRun) -> Run {
    Run {
        conversation_id: p.conversation_id.clone(),
        caller_agent: p.caller_agent.clone(),
        id: p.id.clone(),
        project_id: p.project_id.clone(),
        status: p.status.clone(),
        tier: p.tier.clone(),
        agent: p.agent.clone(),
        model: p.model.clone(),
        provider: p.provider.clone(),
        effort: p.effort.clone(),
        prompt: p.prompt.clone(),
        card_slug: p.card_slug.clone(),
        agent_run_id: p.agent_run_id.clone(),
        dir: p.dir.clone(),
        repo: p.repo.clone(),
        worktree: p.worktree.clone(),
        linked: p
            .linked
            .iter()
            .map(|l| crate::types::LinkedRepo {
                dir: l.dir.clone(),
                repo: l.repo.clone(),
                worktree: l.worktree.clone(),
                branch: l.branch.clone(),
                base_sha: l.base_sha.clone(),
                merge_sha: l.merge_sha.clone(),
            })
            .collect(),
        branch: p.branch.clone(),
        base_sha: p.base_sha.clone(),
        read_only: p.read_only,
        exit_code: i64::from(p.exit_code),
        exit_class: p.exit_class.clone(),
        error: p.error.clone(),
        note: p.note.clone(),
        created_at: p.created_at.clone(),
        started_at: p.started_at.clone(),
        finished_at: p.finished_at.clone(),
        merge_sha: p.merge_sha.clone(),
        files_changed: p.files_changed.map(i64::from),
        insertions: p.insertions.map(i64::from),
        deletions: p.deletions.map(i64::from),
        queued_ms: p.queued_ms,
        run_ms: p.run_ms,
        usage: p.usage.as_ref().map(|u| Usage {
            input_tokens: u.input_tokens,
            cache_read_tokens: u.cache_read_tokens,
            cache_write_tokens: u.cache_write_tokens,
            output_tokens: u.output_tokens,
            reasoning_tokens: u.reasoning_tokens,
            cost_usd: u.cost_usd,
            cost_source: u.cost_source.clone(),
            context_tokens: u.context_tokens,
            context_window: u.context_window,
            cost_partial: u.cost_partial,
            image_generations: u.image_generations,
            image_cost_usd: u.image_cost_usd,
        }),
        merged_at: (!p.merged_at.trim().is_empty()
            && chrono::DateTime::parse_from_rfc3339(p.merged_at.trim()).is_ok())
        .then(|| p.merged_at.clone()),
        attempts: p
            .attempts
            .iter()
            .map(|a| Attempt {
                agent: a.agent.clone(),
                model: a.model.clone(),
                error: a.error.clone(),
                exit_class: a.exit_class.clone(),
                duration_ms: a.duration_ms,
                handoff: a.handoff.clone(),
            })
            .collect(),
        ..Run::default()
    }
}

#[derive(Serialize)]
struct Summary {
    #[serde(skip_serializing_if = "String::is_empty")]
    conversation_id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    caller_agent: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    origin: String,
    id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    project_id: String,
    status: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    tier: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    agent: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    model: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    provider: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    effort: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    card_slug: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    agent_run_id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    branch: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    worktree: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    linked: Vec<crate::types::LinkedRepo>,
    attempts: usize,
    #[serde(skip_serializing_if = "String::is_empty")]
    exit_class: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    queued_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    run_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    files_changed: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    insertions: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    deletions: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    usage: Option<SummaryUsage>,
    #[serde(skip_serializing_if = "String::is_empty")]
    error: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    note: String,
    details: BTreeMap<&'static str, String>,
}

/// A run's reported tokens and API-equivalent cost.
#[derive(Serialize)]
struct SummaryUsage {
    input_tokens: i64,
    cache_read_tokens: i64,
    cache_write_tokens: i64,
    output_tokens: i64,
    reasoning_tokens: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    cost_usd: Option<f64>,
    #[serde(skip_serializing_if = "String::is_empty")]
    cost_source: String,
    #[serde(skip_serializing_if = "is_zero")]
    context_tokens: i64,
    #[serde(skip_serializing_if = "is_zero")]
    context_window: i64,
}

fn is_zero(v: &i64) -> bool {
    *v == 0
}

fn summarize(run: &RunView) -> Summary {
    let mut attempts = run.attempts.as_ref().map_or(0, Vec::len);
    if !run.agent.is_empty() && run.status != STATUS_FAILED && run.status != STATUS_PENDING {
        attempts += 1;
    }
    let id = &run.id;
    let details = BTreeMap::from([
        ("summary", format!("dispatch result {id} --section summary")),
        ("files", format!("dispatch result {id} --section files")),
        ("agent", format!("dispatch result {id} --section agent")),
        (
            "attempts",
            format!("dispatch result {id} --section attempts"),
        ),
        ("meta", format!("dispatch result {id} --section meta")),
    ]);
    Summary {
        conversation_id: run.conversation_id.clone(),
        caller_agent: run.caller_agent.clone(),
        origin: run.origin.clone(),
        id: run.id.clone(),
        project_id: run.project_id.clone(),
        status: run.status.clone(),
        tier: run.tier.clone(),
        agent: run.agent.clone(),
        model: run.model.clone(),
        provider: run.provider.clone(),
        effort: run.effort.clone(),
        card_slug: run.card_slug.clone(),
        agent_run_id: run.agent_run_id.clone(),
        branch: run.branch.clone(),
        worktree: run.worktree.clone(),
        linked: run.linked.clone(),
        attempts,
        exit_class: run.exit_class.clone(),
        queued_ms: run.queued_ms,
        run_ms: run.run_ms,
        files_changed: run.files_changed,
        insertions: run.insertions,
        deletions: run.deletions,
        usage: run.usage.as_ref().map(|u| SummaryUsage {
            input_tokens: u.input_tokens,
            cache_read_tokens: u.cache_read_tokens,
            cache_write_tokens: u.cache_write_tokens,
            output_tokens: u.output_tokens,
            reasoning_tokens: u.reasoning_tokens,
            cost_usd: u.cost_usd,
            cost_source: u.cost_source.clone(),
            context_tokens: u.context_tokens,
            context_window: u.context_window,
        }),
        error: run.error.clone(),
        note: run.note.clone(),
        details,
    }
}

impl Summary {
    fn text(&self) -> String {
        let line1 = format!(
            "{} {}/{} tier={} {} {} {} attempts={}",
            self.status,
            or_unknown(&self.agent),
            or_unknown(&self.model),
            or_unknown(&self.tier),
            format_ms(self.run_ms),
            format_files(self.files_changed, self.insertions, self.deletions),
            format_usage(self.usage.as_ref()),
            self.attempts
        );
        let mut line2 = format!(
            "branch={} worktree={}{}",
            or_unknown(&self.branch),
            or_unknown(&self.worktree),
            conversation_suffix(&self.conversation_id)
        );
        if !self.card_slug.is_empty() {
            line2 += &format!(" card={}", self.card_slug);
        }
        if !self.error.is_empty() {
            line2 += &format!(" error={}", self.error);
        }
        if !self.note.is_empty() {
            line2 += &format!(" note={}", self.note);
        }
        let line3 = format!(
            "details: dispatch result {} --section summary|files|agent|attempts|meta",
            self.id
        );
        format!("{line1}\n{line2}\n{line3}\n")
    }
}

fn write_run(run: &RunView, as_json: bool, compact: bool) -> CliResult {
    let sum = summarize(run);
    if as_json {
        write_json_compact(&sum, compact)?;
    } else {
        print!("{}", sum.text());
    }
    Ok(())
}

pub(super) fn write_proto_run(run: &pb::DispatchRun, as_json: bool, compact: bool) -> CliResult {
    write_run(&RunView::from(&run_from_proto(run)), as_json, compact)
}

pub(super) async fn get_run(c: &Clients, id: &str) -> Result<pb::DispatchRun, Error> {
    let res = c
        .dispatch
        .get_run(&pb::GetRunRequest { id: id.into() })
        .await
        .map_err(run_error)?;
    Ok(res.run.unwrap_or_default())
}

pub(super) fn format_ms(ms: Option<i64>) -> String {
    let Some(ms) = ms else {
        return "unknown".into();
    };
    if ms < 1000 {
        return format!("{ms}ms");
    }
    let rounded = (ms + 50) / 100 * 100;
    super::flags::format_duration(Duration::from_millis(u64::try_from(rounded).unwrap_or(0)))
}

pub fn format_attempts(attempts: &[Attempt]) -> String {
    if attempts.is_empty() {
        return "no fallback attempts\n".into();
    }
    let mut out = String::new();
    for a in attempts {
        out.push_str(&format!(
            "{}/{} class={} duration={} {}\n",
            a.agent,
            a.model,
            or_unknown(&a.exit_class),
            format_ms(a.duration_ms),
            a.error
        ));
        if !a.handoff.trim().is_empty() {
            for line in a.handoff.trim().lines() {
                out.push_str("  ");
                out.push_str(line);
                out.push('\n');
            }
        }
    }
    out
}

/// A token count as 950, 12k, or 6.5M.
fn format_tokens(n: i64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1e6)
    } else if n >= 10_000 {
        format!("{}k", n / 1000)
    } else if n >= 1000 {
        format!("{:.1}k", n as f64 / 1e3)
    } else {
        n.to_string()
    }
}

fn format_usage(u: Option<&SummaryUsage>) -> String {
    let Some(u) = u else {
        return "tokens=unknown".into();
    };
    let total = u.input_tokens + u.cache_read_tokens + u.cache_write_tokens + u.output_tokens;
    let cost = u
        .cost_usd
        .map_or_else(|| "unknown".to_owned(), |c| format!("${c:.2}"));
    let mut out = format!("tokens={} cost={cost}", format_tokens(total));
    let c = format_context(u.context_tokens, u.context_window);
    if !c.is_empty() {
        out += &format!(" context={c}");
    }
    out
}

/// What the last model call held, out of its window: 45k/272k(17%).
fn format_context(tokens: i64, window: i64) -> String {
    if tokens <= 0 {
        return String::new();
    }
    if window <= 0 {
        return format_tokens(tokens);
    }
    format!(
        "{}/{}({}%)",
        format_tokens(tokens),
        format_tokens(window),
        (tokens * 100 / window).min(100)
    )
}

fn format_files(files: Option<i64>, ins: Option<i64>, del: Option<i64>) -> String {
    match (files, ins, del) {
        (None, _, _) => "files=unknown".into(),
        (Some(f), Some(i), Some(d)) => format!("files={f} +{i} -{d}"),
        (Some(f), _, _) => format!("files={f}"),
    }
}

pub(super) fn conversation_suffix(id: &str) -> String {
    if id.is_empty() {
        return String::new();
    }
    let id = match id.split_once(':') {
        Some((agent, session)) => format!("{agent}:{}", short_id(session)),
        None => short_id(id).to_owned(),
    };
    format!(" conversation={id}")
}

fn valid_tier(tier: &str) -> bool {
    matches!(tier, "easy" | "normal" | "hard")
}

fn read_prompt(positional: &[String]) -> Result<String, Fail> {
    let mut prompt = positional.join(" ").trim().to_owned();
    if prompt.is_empty() || prompt == "-" {
        if positional.is_empty() && stdin_is_terminal() {
            return Err(usage());
        }
        let mut body = String::new();
        std::io::stdin().read_to_string(&mut body)?;
        prompt = body.trim().to_owned();
    }
    if prompt.is_empty() {
        return Err(usage());
    }
    Ok(prompt)
}

fn one_id(positional: &[String]) -> Result<String, Fail> {
    match positional {
        [id] if !id.trim().is_empty() => Ok(id.trim().to_owned()),
        _ => Err(usage()),
    }
}

pub(super) async fn run(db_path: &Path, args: &[String]) -> CliResult {
    let mut f = Flags::new("dispatch run");
    f.string(
        "tier",
        "",
        "auto (the default with a classifier, else normal) or a tier: easy|normal|hard",
    )
    .string("card", "", "optional Work card slug")
    .string("agent", "", "optional agent pin")
    .string("worktree", "", "optional worktree path")
    .string("provider", "", "restrict to a provider")
    .string("model", "", "restrict to a model")
    .string(
        "effort",
        "",
        "restrict to an effort: low|medium|high|xhigh|max",
    )
    .string("priority", "", "speed|balanced|quality")
    .bool(
        "relax",
        false,
        "fall back to tier routing when filters match nothing",
    )
    .bool("readonly", false, "run read-only")
    .bool(
        "images",
        false,
        "ask for images: routes only to agents that make them, and names the run's artifacts folder",
    )
    .duration(
        "timeout",
        Duration::ZERO,
        "per-attempt timeout (0 = default)",
    )
    .string(
        "role",
        "",
        "role to run as; none runs with no role and no pick",
    )
    .string("dir", "", "repository (default: current directory)")
    .string(
        "with",
        "",
        "other repositories the task spans, comma-separated: each gets a worktree on the run's branch, merged and cleaned with it",
    )
    .string(
        "worktree-mode",
        "",
        "worktree: a git worktree of its own, so runs go in parallel (the default in a git checkout with commits); in-place: edit the folder itself",
    );
    common(&mut f);
    let positional = f.parse_interspersed(args)?;
    let tier = f.str("tier");
    if !tier.is_empty() && tier != "auto" && !valid_tier(&tier) {
        return Err(usage());
    }
    let worktree_mode = match f.str("worktree-mode").trim() {
        "" | "auto" => String::new(),
        "worktree" => "worktree".to_owned(),
        "in-place" | "in_place" => "in_place".to_owned(),
        _ => return Err(usage()),
    };
    let prompt = read_prompt(&positional)?;
    let wd = working_dir(&f.str("dir"))?;
    let with = f
        .str("with")
        .split(',')
        .map(str::trim)
        .filter(|d| !d.is_empty())
        .map(working_dir)
        .collect::<Result<Vec<_>, _>>()?;
    let (c, project) = dial_workspace(db_path, &f.str("project")).await?;
    let who = caller::resolve(db_path);
    let res = c
        .dispatch
        .start_run(&pb::StartRunRequest {
            project_id: project,
            tier,
            prompt,
            card_slug: f.str("card").trim().into(),
            agent: f.str("agent").trim().into(),
            worktree: f.str("worktree").trim().into(),
            dir: wd,
            read_only: f.flag("readonly"),
            images: f.flag("images"),
            provider: f.str("provider").trim().into(),
            model: f.str("model").trim().into(),
            effort: f.str("effort").trim().into(),
            priority: f.str("priority").trim().into(),
            relax: f.flag("relax"),
            timeout_sec: i32::try_from(f.dur("timeout").as_secs()).unwrap_or(i32::MAX),
            profile: f.str("role").trim().into(),
            worktree_mode,
            with,
            caller: Some(pb::DispatchCaller {
                agent: who.agent,
                session_id: who.session_id,
                conversation_id: who.conversation_id,
                label: who.label,
            }),
            ..Default::default()
        })
        .await
        .map_err(run_error)?;
    write_proto_run(
        &res.run.unwrap_or_default(),
        f.flag("json"),
        f.flag("compact"),
    )
}

/// `dispatch send <id> <message|->`: a follow-up to an ended run, in its agent's session
/// and worktree, as a new run.
pub(super) async fn send(db_path: &Path, args: &[String]) -> CliResult {
    let mut f = Flags::new("dispatch send");
    common(&mut f);
    let positional = f.parse_interspersed(args)?;
    let Some((id, message)) = positional.split_first() else {
        return Err(usage());
    };
    let id = id.trim().to_owned();
    if id.is_empty() || id == "-" {
        return Err(usage());
    }
    let prompt = read_prompt(message)?;
    // The earlier run names its workspace: the cwd's is not looked up.
    let c = super::dial(db_path).await?;
    let who = caller::resolve(db_path);
    let res = c
        .dispatch
        .start_run(&pb::StartRunRequest {
            project_id: f.str("project").trim().into(),
            prompt,
            resume: id,
            caller: Some(pb::DispatchCaller {
                agent: who.agent,
                session_id: who.session_id,
                conversation_id: who.conversation_id,
                label: who.label,
            }),
            ..Default::default()
        })
        .await
        .map_err(run_error)?;
    write_proto_run(
        &res.run.unwrap_or_default(),
        f.flag("json"),
        f.flag("compact"),
    )
}

pub(super) async fn list(db_path: &Path, args: &[String]) -> CliResult {
    let mut f = Flags::new("dispatch runs");
    f.int("limit", 20, "max runs to list").string(
        "conversation",
        "",
        "filter by calling conversation id",
    );
    common(&mut f);
    if !f.parse_interspersed(args)?.is_empty() {
        return Err(usage());
    }
    let (c, mut project) = dial_workspace(db_path, &f.str("project")).await?;
    let conversation = f.str("conversation").trim().to_owned();
    // A conversation lists its runs in every workspace unless --project narrows it.
    if !conversation.is_empty() && f.str("project").trim().is_empty() {
        project.clear();
    }
    let res = c
        .dispatch
        .list_runs(&pb::ListRunsRequest {
            project_id: project,
            limit: i32::try_from(f.int_value("limit")).unwrap_or(i32::MAX),
            offset: 0,
            conversation_id: conversation,
        })
        .await
        .map_err(run_error)?;
    let runs: Vec<Run> = res.runs.iter().map(run_from_proto).collect();
    if f.flag("json") {
        let runs: Vec<Value> = runs.iter().map(run_meta).collect();
        write_json_compact(&json!({ "runs": runs }), f.flag("compact"))?;
        return Ok(());
    }
    if runs.is_empty() {
        println!("no dispatch runs");
        return Ok(());
    }
    for r in &runs {
        println!(
            "{}  {:<10}  {:<6}  {}/{}  {}{}",
            short_id(&r.id),
            r.status,
            or_unknown(&r.tier),
            or_unknown(&r.agent),
            or_unknown(&r.model),
            one_line(&r.prompt),
            conversation_suffix(&r.conversation_id)
        );
    }
    Ok(())
}

pub(super) async fn status(db_path: &Path, args: &[String]) -> CliResult {
    let mut f = Flags::new("dispatch status");
    common(&mut f);
    let id = one_id(&f.parse_interspersed(args)?)?;
    let (c, _) = dial_workspace(db_path, &f.str("project")).await?;
    let run = get_run(&c, &id).await?;
    write_proto_run(&run, f.flag("json"), f.flag("compact"))
}

pub(super) async fn cancel(db_path: &Path, args: &[String]) -> CliResult {
    let mut f = Flags::new("dispatch cancel");
    common(&mut f);
    let id = one_id(&f.parse_interspersed(args)?)?;
    let (c, _) = dial_workspace(db_path, &f.str("project")).await?;
    let res = c
        .dispatch
        .cancel_run(&pb::CancelRunRequest { id })
        .await
        .map_err(run_error)?;
    write_proto_run(
        &res.run.unwrap_or_default(),
        f.flag("json"),
        f.flag("compact"),
    )
}

pub(super) async fn result(db_path: &Path, args: &[String]) -> CliResult {
    let mut f = Flags::new("dispatch result");
    f.string(
        "section",
        "summary",
        "summary|agent|files|attempts|meta|output",
    )
    .int("offset", 0, "first line offset")
    .int("limit", 200, "max lines (0 = all)");
    common(&mut f);
    let positional = f.parse_interspersed(args)?;
    let mut section = f.str("section");
    if !matches!(
        section.as_str(),
        "summary" | "agent" | "files" | "attempts" | "meta" | "output"
    ) {
        return Err(usage());
    }
    let id = one_id(&positional)?;
    let (c, _) = dial_workspace(db_path, &f.str("project")).await?;
    let (offset, limit) = (f.int_value("offset"), f.int_value("limit"));
    if section.trim().is_empty() {
        section = "summary".into();
    }
    let res = c
        .dispatch
        .result(&pb::ResultRequest {
            id: id.clone(),
            section: section.clone(),
            offset: i32::try_from(offset).unwrap_or(0),
            limit: i32::try_from(limit).unwrap_or(0),
        })
        .await
        .map_err(run_error)?;
    let mut run = match get_run(&c, &id).await {
        Ok(p) => RunView::from(&run_from_proto(&p)),
        Err(_) => RunView {
            id: id.clone(),
            ..RunView::default()
        },
    };
    let content = res.content;
    let trimmed = content.trim();
    let mut meta: Option<Value> = None;
    match section.as_str() {
        "attempts" if !trimmed.is_empty() => {
            if let Ok(attempts) = serde_json::from_str::<Vec<Attempt>>(trimmed) {
                run.attempts = Some(attempts);
            }
        }
        "meta" if !trimmed.is_empty() => {
            if let Ok(v) = serde_json::from_str::<Value>(trimmed)
                && let Ok(view) = serde_json::from_value::<RunView>(v.clone())
            {
                run = view;
                meta = Some(v);
            }
        }
        _ => {}
    }
    let total = i64::from(res.total);
    let has_more = limit > 0 && offset.max(0) + limit < total;
    let (as_json, compact) = (f.flag("json"), f.flag("compact"));
    match section.as_str() {
        "summary" => write_run(&run, as_json, compact),
        "files" => {
            if as_json {
                let payload = json!({
                    "id": run.id, "status": run.status, "section": "files",
                    "files_changed": run.files_changed, "insertions": run.insertions,
                    "deletions": run.deletions,
                });
                write_json_compact(&sort_top(payload), compact)?;
            } else {
                println!(
                    "{} {}",
                    run.id,
                    format_files(run.files_changed, run.insertions, run.deletions)
                );
            }
            Ok(())
        }
        "attempts" => {
            if as_json {
                let payload = json!({
                    "id": run.id, "status": run.status, "section": "attempts",
                    "attempts": run.attempts,
                });
                write_json_compact(&sort_top(payload), compact)?;
                return Ok(());
            }
            let attempts = run.attempts.unwrap_or_default();
            print!("{}", format_attempts(&attempts));
            Ok(())
        }
        "meta" => {
            if as_json {
                let mut v = meta.unwrap_or_else(|| {
                    run_meta(&Run {
                        id: run.id.clone(),
                        ..Run::default()
                    })
                });
                if let Some(o) = v.as_object_mut() {
                    o.insert("Output".into(), Value::String(String::new()));
                }
                write_json_compact(&v, compact)?;
                return Ok(());
            }
            print!("{}", summarize(&run).text());
            Ok(())
        }
        _ => {
            if as_json {
                let payload = json!({
                    "id": run.id, "status": run.status, "section": section,
                    "offset": offset, "limit": limit, "total_lines": total,
                    "has_more": has_more, "content": content,
                });
                write_json_compact(&sort_top(payload), compact)?;
                return Ok(());
            }
            if !content.is_empty() {
                print!("{content}");
                if !content.ends_with('\n') {
                    println!();
                }
            }
            Ok(())
        }
    }
}

pub(super) async fn events(db_path: &Path, args: &[String]) -> CliResult {
    let mut f = Flags::new("dispatch events");
    f.int(
        "tail-bytes",
        DEFAULT_EVENT_BYTES,
        "bytes of the run's log to read, from its end",
    )
    .int(
        "before-seq",
        0,
        "read the page before this event's seq (0 reads the run's end)",
    )
    .int("before-offset", 0, "bytes into that event's text");
    common(&mut f);
    let positional = f.parse_interspersed(args)?;
    let id = one_id(&positional)?;
    let (c, _) = dial_workspace(db_path, &f.str("project")).await?;
    let run = get_run(&c, &id).await?;
    let res = c
        .dispatch
        .read_run_events(&pb::ReadRunEventsRequest {
            id: run.id.clone(),
            before_seq: f.int_value("before-seq"),
            before_offset: f.int_value("before-offset"),
            max_bytes: f.int_value("tail-bytes"),
        })
        .await
        .map_err(run_error)?;
    if f.flag("json") {
        let events: Vec<Value> = res
            .events
            .iter()
            .map(|e| {
                json!({
                    "seq": e.seq,
                    "offset": e.text_offset,
                    "kind": e.kind,
                    "text": e.text,
                })
            })
            .collect();
        let payload = json!({
            "status": run.status,
            "events": events,
            "more": res.more,
        });
        write_json_compact(&payload, f.flag("compact"))?;
        return Ok(());
    }
    let text = events_text(&res.events);
    if !text.is_empty() {
        print!("{text}");
        if !text.ends_with('\n') {
            println!();
        }
    }
    Ok(())
}

/// A run's events as the app draws them: output as it came, any other kind as one
/// `[kind] text` line, and the `started` event, which repeats the prompt, not at all. A page
/// cut inside the first event starts at its first whole line, as the transcript's does.
fn events_text(events: &[pb::WatchRunResponse]) -> String {
    let mut text = String::new();
    for (index, event) in events.iter().enumerate() {
        if event.kind == "started" {
            continue;
        }
        if event.kind == "output" || event.kind == "agent" {
            let body = if index == 0 && event.text_offset > 0 {
                match event.text.find('\n') {
                    Some(newline) => &event.text[newline + 1..],
                    None => "",
                }
            } else {
                &event.text
            };
            text.push_str(body);
            continue;
        }
        let line = event.text.trim();
        if !line.is_empty() {
            text.push('[');
            text.push_str(&event.kind);
            text.push_str("] ");
            text.push_str(line);
            text.push('\n');
        }
    }
    strip_ansi(&text)
}

/// Drops terminal escapes and keeps what a carriage return left visible, as the app does.
fn strip_ansi(text: &str) -> String {
    let mut cleaned = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\u{1b}' {
            cleaned.push(ch);
            continue;
        }
        match chars.peek().copied() {
            Some('[') => {
                chars.next();
                let mut params = true;
                for next in chars.by_ref() {
                    if params && matches!(next, '0'..='9' | ';' | '?') {
                        continue;
                    }
                    params = false;
                    if ('\u{20}'..='\u{2f}').contains(&next) {
                        continue;
                    }
                    if ('\u{40}'..='\u{7e}').contains(&next) {
                        break;
                    }
                    cleaned.push(next);
                    break;
                }
            }
            Some(']') => {
                chars.next();
                while let Some(next) = chars.next() {
                    if next == '\u{7}' {
                        break;
                    }
                    if next == '\u{1b}' && chars.peek() == Some(&'\\') {
                        chars.next();
                        break;
                    }
                }
            }
            Some(next) if ('@'..='Z').contains(&next) || matches!(next, '\\' | '_' | '-') => {
                chars.next();
            }
            _ => {}
        }
    }
    cleaned
        .split('\n')
        .map(|line| {
            let trimmed = line.strip_suffix('\r').unwrap_or(line);
            match trimmed.rfind('\r') {
                Some(index) => &trimmed[index + 1..],
                None => trimmed,
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub(super) async fn tiers(db_path: &Path, args: &[String]) -> CliResult {
    let mut f = Flags::new("dispatch tiers");
    f.bool(
        "check",
        false,
        "validate tiers against installed agents and models",
    )
    .bool(
        "apply",
        false,
        "with build: replace the stored tiers with the proposal",
    )
    .string(
        "agent",
        "",
        "with build: only this agent's entries and models; --apply replaces only its entries",
    )
    .string(
        "role",
        "",
        "with build: the board for this role (its lowest tier leaves the tiers under it empty); not with --apply",
    )
    .string(
        "judge",
        "",
        "with build: jev asks TypeSafe's Jev once which candidates fit the role's work; classifier asks the classifier Settings › Classifier picked",
    );
    common(&mut f);
    let positional = f.parse_interspersed(args)?;
    if positional.len() == 1 && positional[0] == "build" {
        let req = pb::BuildTiersRequest {
            apply: f.flag("apply"),
            agent: f.str("agent"),
            role: f.str("role"),
            judge: f.str("judge"),
        };
        return tiers_build(db_path, &f.str("project"), req, f.flag("json")).await;
    }
    if !positional.is_empty() {
        return Err(usage());
    }
    let (c, _) = dial_workspace(db_path, &f.str("project")).await?;
    let (as_json, compact) = (f.flag("json"), f.flag("compact"));
    if f.flag("check") {
        let res = c
            .dispatch
            .check_tiers(&pb::CheckTiersRequest {})
            .await
            .map_err(run_error)?;
        if as_json {
            let problems = (!res.problems.is_empty()).then_some(&res.problems);
            write_json_compact(
                &json!({ "ok": res.problems.is_empty(), "problems": problems }),
                compact,
            )?;
            return Ok(());
        }
        if res.problems.is_empty() {
            println!("ok: every tier entry resolves to an installed agent and model");
            return Ok(());
        }
        for p in &res.problems {
            println!("! {p}");
        }
        return Err(Fail::Exit(1));
    }
    let res = c
        .dispatch
        .list_tiers(&pb::ListTiersRequest {})
        .await
        .map_err(run_error)?;
    if as_json {
        let tiers: Vec<Value> = res
            .tiers
            .iter()
            .map(|t| {
                json!({
                    "Name": t.name, "Provider": t.provider, "Agent": t.agent, "Model": t.model,
                    "Position": t.position,
                })
            })
            .collect();
        write_json_compact(&json!({ "tiers": tiers }), compact)?;
        return Ok(());
    }
    if res.tiers.is_empty() {
        println!("no dispatch tiers");
        return Ok(());
    }
    for t in &res.tiers {
        println!(
            "{:<6}  {:<12}  {:<24}  provider={}",
            t.name,
            t.agent,
            t.model,
            or_unknown(&t.provider)
        );
    }
    Ok(())
}

/// Prints the tier table the model catalog proposes and, with apply, stores it.
async fn tiers_build(
    db_path: &Path,
    project_id: &str,
    req: pb::BuildTiersRequest,
    as_json: bool,
) -> CliResult {
    let (c, _) = dial_workspace(db_path, project_id).await?;
    let res = c.dispatch.build_tiers(&req).await.map_err(run_error)?;
    if as_json {
        return ops::print_op(res.ok, &res.json);
    }
    let body: Value = serde_json::from_str(&res.json)?;
    let num = |v: &Value, k: &str| v.get(k).and_then(Value::as_f64).unwrap_or(0.0);
    let text = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_owned();
    for tier in ["easy", "normal", "hard"] {
        let entries = body
            .pointer(&format!("/tiers/{tier}"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let d = entries
            .first()
            .and_then(|e| e.get("auto"))
            .map_or(0.0, |a| num(a, "difficulty"));
        println!("{tier} (d={d:.2}):");
        for (i, e) in entries.iter().enumerate() {
            let auto = e.get("auto").cloned().unwrap_or(Value::Null);
            let mark = if auto.get("qualified").and_then(Value::as_bool) == Some(true) {
                "✓"
            } else {
                " "
            };
            let mut model = format!("{}/{}", text(e, "agent"), text(e, "model"));
            let effort = text(e, "effort");
            if !effort.is_empty() {
                model += &format!("@{effort}");
            }
            println!(
                "  {}. {mark} {model} P={:.2} bound={:.2} cost=${:.2}  θ={:.2} {}",
                i + 1,
                num(&auto, "pPass"),
                num(&auto, "pPassLow"),
                num(&auto, "effectiveCost"),
                num(&auto, "theta"),
                text(&auto, "thetaSource")
            );
        }
    }
    for p in body
        .get("problems")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        eprintln!("models: {}", p.as_str().unwrap_or_default());
    }
    let applied = body.get("applied").and_then(Value::as_bool) == Some(true);
    let priority = text(&body, "priority");
    let agent = &req.agent;
    match (applied, agent.is_empty()) {
        (true, false) => {
            println!("stored as {agent}'s entries, after the other agents' in each tier");
        }
        (true, true) => println!("stored as the tier table"),
        (false, false) => println!(
            "priority {priority}; run `dispatch tiers build --agent {agent} --apply` to store it as {agent}'s entries"
        ),
        (false, true) => {
            println!("priority {priority}; run `dispatch tiers build --apply` to store it");
        }
    }
    Ok(())
}

#[derive(Serialize)]
struct StatsRow<'a> {
    key: &'a str,
    runs: i32,
    success_pct: f64,
    first_try_pct: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    p50_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    p90_ms: Option<i64>,
    fallbacks: i32,
    rate_limits: i32,
    orphans: i32,
    usage_runs: i32,
    input_tokens: i64,
    cache_read_tokens: i64,
    cache_write_tokens: i64,
    output_tokens: i64,
    reasoning_tokens: i64,
    cost_usd: f64,
    cost_runs: i32,
}

#[derive(Serialize)]
struct StatsBody<'a> {
    by: &'a str,
    #[serde(skip_serializing_if = "str::is_empty")]
    since: &'a str,
    #[serde(skip_serializing_if = "is_zero_i32")]
    in_flight: i32,
    rows: Vec<StatsRow<'a>>,
}

fn is_zero_i32(v: &i32) -> bool {
    *v == 0
}

pub(super) async fn stats(db_path: &Path, args: &[String]) -> CliResult {
    let mut f = Flags::new("dispatch stats");
    f.string("by", "model", "group by: model|agent|provider|tier|effort")
        .string("since", "7d", "window such as 7d, 24h, or all")
        .string(
            "conversation",
            "",
            "a conversation id, such as claude-code:<session> or opencode:ses_<id>",
        );
    common(&mut f);
    if !f.parse_interspersed(args)?.is_empty() {
        return Err(usage());
    }
    let conversation = f.str("conversation");
    if !conversation.trim().is_empty() {
        return conversation_usage(
            db_path,
            conversation.trim(),
            f.flag("json"),
            f.flag("compact"),
        )
        .await;
    }
    let (c, project) = dial_workspace(db_path, &f.str("project")).await?;
    let res = c
        .dispatch
        .stats(&pb::StatsRequest {
            dir: String::new(),
            project_id: project,
            by: f.str("by"),
            since: f.str("since"),
        })
        .await
        .map_err(run_error)?;
    let rows: Vec<StatsRow> = res
        .rows
        .iter()
        .map(|r| StatsRow {
            key: &r.key,
            runs: r.runs,
            success_pct: r.success_pct,
            first_try_pct: r.first_try_pct,
            p50_ms: r.p50_ms,
            p90_ms: r.p90_ms,
            fallbacks: r.fallbacks,
            rate_limits: r.rate_limits,
            orphans: r.orphans,
            usage_runs: r.usage_runs,
            input_tokens: r.input_tokens,
            cache_read_tokens: r.cache_read_tokens,
            cache_write_tokens: r.cache_write_tokens,
            output_tokens: r.output_tokens,
            reasoning_tokens: r.reasoning_tokens,
            cost_usd: r.cost_usd,
            cost_runs: r.cost_runs,
        })
        .collect();
    if f.flag("json") {
        let body = StatsBody {
            by: &res.by,
            since: &res.since,
            in_flight: res.in_flight,
            rows,
        };
        write_json_compact(&body, f.flag("compact"))?;
        return Ok(());
    }
    if rows.is_empty() {
        println!("no dispatch runs in window");
        return Ok(());
    }
    for row in &rows {
        println!(
            "{:<24}  runs={}  ok={:.0}%  p50={}  fallbacks={}  {}",
            row.key,
            row.runs,
            row.success_pct,
            format_ms(row.p50_ms),
            row.fallbacks,
            format_spend(row)
        );
    }
    Ok(())
}

/// What one conversation used: its Dispatch runs and, when this build can read that
/// agent's local transcript, the calling session's own usage.
async fn conversation_usage(
    db_path: &Path,
    conversation: &str,
    json: bool,
    compact: bool,
) -> CliResult {
    let c = super::dial(db_path).await?;
    let res = c
        .dispatch
        .conversation_usage(&pb::GetConversationUsageRequest {
            conversation_id: conversation.to_string(),
        })
        .await
        .map_err(run_error)?;
    if json {
        let slice = |s: &pb::DispatchUsageSlice| {
            json!({
                "agent": s.agent,
                "model": s.model,
                "runs": s.runs,
                "inputTokens": s.input_tokens,
                "cacheReadTokens": s.cache_read_tokens,
                "cacheWriteTokens": s.cache_write_tokens,
                "outputTokens": s.output_tokens,
                "reasoningTokens": s.reasoning_tokens,
                "costUsd": s.cost_usd,
                "costPartial": s.cost_partial,
            })
        };
        let body = json!({
            "conversationId": res.conversation_id,
            "callerAgent": res.caller_agent,
            "callerSession": res.caller_session,
            "callerMeasured": res.caller_measured,
            "callerNote": res.caller_note,
            "runs": res.runs.iter().map(slice).collect::<Vec<_>>(),
            "caller": res.caller.iter().map(slice).collect::<Vec<_>>(),
            "runTotals": res.run_totals.as_ref().map(slice),
            "callerTotals": res.caller_totals.as_ref().map(slice),
            "total": res.total.as_ref().map(slice),
        });
        write_json_compact(&body, compact)?;
        return Ok(());
    }
    let section = |title: &str, slices: &[pb::DispatchUsageSlice], empty: &str| {
        println!("{title}");
        if slices.is_empty() {
            println!("  {empty}");
        }
        for s in slices {
            println!("  {}", format_usage_slice(s));
        }
    };
    println!("conversation {}", res.conversation_id);
    section("Dispatch runs", &res.runs, "no runs");
    println!(
        "  total {}",
        format_usage_slice_value(res.run_totals.as_ref())
    );
    let agent = if res.caller_agent.trim().is_empty() {
        "unknown agent"
    } else {
        res.caller_agent.trim()
    };
    println!("Calling agent {agent} · {}", res.caller_session.trim());
    if res.caller_measured {
        for s in &res.caller {
            println!("  {}", format_usage_slice(s));
        }
        println!(
            "  total {}",
            format_usage_slice_value(res.caller_totals.as_ref())
        );
    } else if res.caller_note.trim().is_empty() {
        println!("  not measured");
    } else {
        println!("  {}", res.caller_note.trim());
    }
    println!("Total {}", format_usage_slice_value(res.total.as_ref()));
    println!("Costs are API-equivalent prices, not what a subscription bills.");
    Ok(())
}

fn format_usage_slice_value(slice: Option<&pb::DispatchUsageSlice>) -> String {
    slice.map_or_else(|| "none".into(), format_usage_slice)
}

/// One agent and model's tokens and cost, as `dispatch usage` prints it.
fn format_usage_slice(s: &pb::DispatchUsageSlice) -> String {
    let total = s.input_tokens + s.cache_read_tokens + s.cache_write_tokens + s.output_tokens;
    let mut out = if s.agent.is_empty() && s.model.is_empty() {
        format!("tokens={}", format_tokens(total))
    } else {
        format!(
            "{:<10} {:<24} runs={} tokens={}",
            s.agent,
            if s.model.trim().is_empty() {
                "—"
            } else {
                s.model.as_str()
            },
            s.runs,
            format_tokens(total)
        )
    };
    out += &format!(
        " (in {} cache {} write {} out {})",
        format_tokens(s.input_tokens),
        format_tokens(s.cache_read_tokens),
        format_tokens(s.cache_write_tokens),
        format_tokens(s.output_tokens)
    );
    match s.cost_usd {
        Some(cost) => {
            out += &format!(" cost=${cost:.2}");
            if s.cost_partial {
                out += " (partial)";
            }
        }
        None if total > 0 => out += " cost=unknown",
        None => {}
    }
    out
}

/// A stats row's tokens and the cost of its priced runs.
fn format_spend(row: &StatsRow) -> String {
    if row.usage_runs == 0 {
        return "tokens=unknown".into();
    }
    let total =
        row.input_tokens + row.cache_read_tokens + row.cache_write_tokens + row.output_tokens;
    let mut out = format!("tokens={}", format_tokens(total));
    if row.cost_runs > 0 {
        out += &format!(" cost=${:.2}", row.cost_usd);
        if row.cost_runs < row.runs {
            out += &format!(" ({}/{} runs priced)", row.cost_runs, row.runs);
        }
    }
    out
}

pub(super) async fn agents(db_path: &Path, args: &[String]) -> CliResult {
    if matches!(
        args.first().map(String::as_str),
        Some("add" | "remove" | "enable" | "disable")
    ) {
        return agents_edit(db_path, args).await;
    }
    let mut f = Flags::new("dispatch agents");
    common(&mut f);
    if !f.parse_interspersed(args)?.is_empty() {
        return Err(usage());
    }
    let (c, _) = dial_workspace(db_path, &f.str("project")).await?;
    let res = c
        .dispatch
        .list_agents(&pb::ListAgentsRequest {})
        .await
        .map_err(run_error)?;
    if f.flag("json") {
        let agents: Vec<Value> = res
            .agents
            .iter()
            .map(|a| {
                let cooldown = chrono::DateTime::parse_from_rfc3339(a.cooldown_until.trim())
                    .ok()
                    .map(|t| {
                        t.with_timezone(&chrono::Utc)
                            .to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true)
                    });
                json!({
                    "Name": a.name, "Bin": a.bin, "Provider": a.provider,
                    "UsedPercent": a.used_percent, "CooldownUntil": cooldown,
                    "Enabled": a.enabled, "Images": a.images,
                })
            })
            .collect();
        write_json_compact(&json!({ "agents": agents }), f.flag("compact"))?;
        return Ok(());
    }
    if res.agents.is_empty() {
        println!("no dispatch agents");
        return Ok(());
    }
    for a in &res.agents {
        let used = a
            .used_percent
            .map_or_else(|| "unknown".to_owned(), |p| format!("{p:.0}%"));
        let state = if a.enabled { "" } else { "  disabled" };
        let images = if a.images { "  images" } else { "" };
        println!(
            "{:<12}  {:<10}  used={used}{state}{images}",
            a.name,
            or_unknown(&a.provider)
        );
    }
    Ok(())
}

/// Adds an ACP agent by name and the command that starts its server, or
/// removes one. Everything after "--" is the command, flags included, so it
/// is read before any flag parsing.
async fn agents_edit(db_path: &Path, args: &[String]) -> CliResult {
    if let [action, name] = args
        && (action == "enable" || action == "disable")
        && !name.trim().is_empty()
    {
        let (c, _) = dial_workspace(db_path, "").await?;
        let res = c
            .dispatch
            .set_agent_enabled(&pb::SetAgentEnabledRequest {
                agent: name.trim().to_owned(),
                enabled: action == "enable",
            })
            .await
            .map_err(run_error)?;
        let state = if res.enabled { "enabled" } else { "disabled" };
        println!("{}: {state}", res.agent);
        return Ok(());
    }
    let mut req = pb::ConfigRequest::default();
    match args {
        [add, name, sep, command @ ..] if add == "add" && sep == "--" && !command.is_empty() => {
            req.action = "agent-add".into();
            req.key = name.clone();
            req.values = command.to_vec();
        }
        [remove, name] if remove == "remove" => {
            req.action = "agent-remove".into();
            req.key = name.clone();
        }
        _ => {
            return Err(usage_err(
                "usage: dispatch agents add <name> -- <command...> | dispatch agents remove <name> | dispatch agents enable|disable <name>",
            ));
        }
    }
    let (c, _) = dial_workspace(db_path, "").await?;
    let res = c.dispatch.config(&req).await.map_err(run_error)?;
    ops::print_op(res.ok, &res.json)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_like_the_go_cli() {
        assert_eq!(format_ms(Some(950)), "950ms");
        assert_eq!(format_ms(Some(61_249)), "1m1.2s");
        assert_eq!(format_ms(None), "unknown");
        assert_eq!(format_tokens(950), "950");
        assert_eq!(format_tokens(1_500), "1.5k");
        assert_eq!(format_tokens(12_345), "12k");
        assert_eq!(format_tokens(6_500_000), "6.5M");
        assert_eq!(format_context(45_000, 272_000), "45k/272k(16%)");
        assert_eq!(
            conversation_suffix("codex:0123456789abcdef"),
            " conversation=codex:0123456789ab"
        );
        assert_eq!(format_files(Some(3), Some(10), Some(2)), "files=3 +10 -2");
    }

    #[test]
    fn events_read_as_the_app_draws_them() {
        let event = |kind: &str, text: &str| pb::WatchRunResponse {
            seq: 1,
            kind: kind.into(),
            text: text.into(),
            at: String::new(),
            text_offset: 0,
        };
        let text = events_text(&[
            event("started", "the prompt"),
            event("output", "\u{1b}[32mok\u{1b}[0m\n"),
            event("merged", "dispatch/r1\n"),
        ]);
        assert_eq!(text, "ok\n[merged] dispatch/r1\n");
        // A page cut inside the first event starts at its first whole line.
        let cut = pb::WatchRunResponse {
            text_offset: 7,
            ..event("output", "e end\nnext line\n")
        };
        assert_eq!(events_text(&[cut]), "next line\n");
        assert_eq!(strip_ansi("10%\r50%\r100%"), "100%");
        assert_eq!(strip_ansi("\u{1b}[1mbold\u{1b}[0m"), "bold");
    }

    #[test]
    fn summary_counts_the_attempt_that_ran() {
        let run = RunView {
            id: "r1".into(),
            status: crate::types::STATUS_COMPLETED.into(),
            agent: "codex".into(),
            ..RunView::default()
        };
        let s = summarize(&run);
        assert_eq!(s.attempts, 1);
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v["details"]["meta"], "dispatch result r1 --section meta");
        assert!(v.get("tier").is_none());
    }
}
