//! The operational commands: wait, route, classify, models, config, doctor,
//! merge, clean, recover, roles, and the local ones (on, off, hook, skill,
//! setup, update) that need no core.

use super::{
    CliResult, Fail, Flags, caller, dial_workspace, or_unknown, run_error, runs, sort_keys,
    state_dir, usage_err, working_dir, write_json,
};
use crate::{
    Error, adapters, hook,
    pass_model::difficulty_tier,
    proto as pb,
    reports::{models_bench::BenchmarkReport, models_report::CatalogReport},
    roles::{Profile, RoleEntry},
    router::AutoInfo,
    sandbox, setup, skill,
    tiers::{known_adapters, table_from_rows},
    types::{
        RolePick, STATUS_CANCELLED, STATUS_COMPLETED, STATUS_FAILED, STATUS_ORPHANED,
        STATUS_TIMED_OUT, Tier,
    },
};
use serde_json::Value;
use std::collections::BTreeMap;
use std::io::BufRead;
use std::path::Path;
use std::time::Duration;

pub(crate) const OPS_USAGE: &str = "       dispatch wait <id> [--timeout <dur>] [--json=false]
       dispatch route [--tier <auto|easy|normal|hard>] [--provider <p>] [--model <m>] [--effort <e>] [--agent <a>] [--priority speed|balanced|quality] [--relax] [--readonly] [--role <name|none>] [--card <slug>] [--dir <repo>] [--json] [<prompt>]
       dispatch models [--json] | enable|disable <provider>[/<model>] | refresh | bench [--json]
       dispatch classify [--card <slug>] [--dir <repo>] [--json] <prompt>
       dispatch config get [priority|discover|roles|classifier|classifier_model|classifier_agent|classifier_keys|classifier_cli]
       dispatch config set priority <speed|balanced|quality> | discover <true|false> | roles <true|false> | classifier <off|jev|openai|cli> | classifier_model <model> | classifier_agent <agent[/model]|default>
       dispatch config key set|clear jev|openai   (set reads the key from stdin into a Keychain item only dispatch reads; TYPESAFE_API_KEY or OPENAI_API_KEY wins over it)
       dispatch doctor [--json]
       dispatch merge <id>
       dispatch clean <id> [--force]
       dispatch worktrees [--prune] [--dir <repo>] [--json]
       dispatch delete <id>... | --conversation <id> [--json]   (ended runs, their transcripts, and the worktrees Dispatch cut for them)
       dispatch rename <conversation-id> <name>   (an empty name gives the conversation back its first prompt)
       dispatch recover <id>
       dispatch roles [--check] [--dir <repo>]
       dispatch roles save <name> [--when <text>] [--tier easy|normal|hard] [--columns easy,normal,hard] [--easy|--normal|--hard agent/model,...] [--readonly] [--instructions <text>] [--manual] [--prefer a,b] [--avoid a,b] [--timeout <dur>] | remove <name>
       dispatch on|off|hook [--agent <name>]
       dispatch setup [--json] | link | skill [--remove] [<agent> | <folder>] | hook [<agent>] on|off | opencode [--remove | on | off]
       dispatch skill install|remove [--agent <name> | --dir <folder>] [--force] | show | refresh
       dispatch update [--check]";

const PROJECT_USAGE: &str = "workspace id (default: the workspace holding the cwd)";

/// Prints an operation body as Go printed the decoded value (keys in order)
/// and turns a reported failure into exit status 1 after the body is shown.
pub(super) fn print_op(ok: bool, body: &str) -> CliResult {
    let v: Value = serde_json::from_str(body)?;
    write_json(&sort_keys(v))?;
    if ok { Ok(()) } else { Err(Fail::Exit(1)) }
}

fn single_run_id(f: &mut Flags, args: &[String], verb: &str) -> Result<String, Fail> {
    match f.parse_interspersed(args)?.as_slice() {
        [id] if !id.trim().is_empty() => Ok(id.trim().to_owned()),
        _ => Err(usage_err(format!(
            "dispatch {verb} needs one run id\n{OPS_USAGE}"
        ))),
    }
}

fn is_terminal_run(status: &str) -> bool {
    [
        STATUS_COMPLETED,
        STATUS_FAILED,
        STATUS_CANCELLED,
        STATUS_TIMED_OUT,
        STATUS_ORPHANED,
    ]
    .contains(&status)
}

/// The exit status of wait for a terminal status.
fn exit_code(status: &str) -> i32 {
    match status {
        STATUS_COMPLETED => 0,
        STATUS_TIMED_OUT => 124,
        _ => 1,
    }
}

pub(super) async fn wait(db_path: &Path, args: &[String]) -> CliResult {
    let mut f = Flags::new("dispatch wait");
    f.duration(
        "timeout",
        Duration::ZERO,
        "stop waiting after this long (0 = no limit)",
    )
    .bool(
        "json",
        true,
        "print compact summary JSON; --json=false prints text",
    )
    .string("project", "", PROJECT_USAGE);
    let id = single_run_id(&mut f, args, "wait")?;
    let (c, _) = dial_workspace(db_path, &f.str("project")).await?;
    let as_json = f.flag("json");
    let timeout = f.dur("timeout");
    let deadline = (!timeout.is_zero()).then(|| tokio::time::Instant::now() + timeout);
    let stopped = async {
        let mut term =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).ok();
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            () = async { match term.as_mut() { Some(t) => { t.recv().await; } None => std::future::pending().await } } => {}
            () = async { match deadline { Some(d) => tokio::time::sleep_until(d).await, None => std::future::pending().await } } => {}
        }
    };
    tokio::pin!(stopped);
    let mut tick = tokio::time::interval(Duration::from_millis(500));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        let got = tokio::select! {
            r = runs::get_run(&c, &id) => Some(r),
            () = &mut stopped => None,
        };
        match got {
            Some(Err(e)) => return Err(e.into()),
            Some(Ok(run)) if is_terminal_run(&run.status) => {
                runs::write_proto_run(&run, as_json, true)?;
                return match exit_code(&run.status) {
                    0 => Ok(()),
                    code => Err(Fail::Exit(code)),
                };
            }
            Some(Ok(_)) => {}
            None => return still_running(&c, &id, as_json).await,
        }
        tokio::select! {
            _ = tick.tick() => {}
            () = &mut stopped => return still_running(&c, &id, as_json).await,
        }
    }
}

/// Prints the run as it stands when wait stopped before it ended: exit 3.
async fn still_running(c: &crate::coreclient::Clients, id: &str, as_json: bool) -> CliResult {
    if let Ok(latest) = runs::get_run(c, id).await {
        let _ = runs::write_proto_run(&latest, as_json, true);
    }
    Err(Fail::Exit(3))
}

pub(super) async fn route(db_path: &Path, args: &[String]) -> CliResult {
    let mut f = Flags::new("dispatch route");
    f.string(
        "tier",
        "",
        "auto (the default with a classifier, else normal) or a tier: easy|normal|hard",
    )
    .string("provider", "", "restrict to a provider")
    .string("model", "", "restrict to a model")
    .string(
        "effort",
        "",
        "restrict to an effort: low|medium|high|xhigh|max",
    )
    .string("agent", "", "restrict to an agent")
    .string("priority", "", "speed|balanced|quality")
    .bool(
        "relax",
        false,
        "fall back to tier routing when filters match nothing",
    )
    .bool("readonly", false, "require read-only capability")
    .string(
        "role",
        "",
        "role to route as; none routes with no role and no pick",
    )
    .string("dir", "", "repository (default: current directory)")
    .string(
        "card",
        "",
        "Work card whose component prices the candidates",
    )
    .string("project", "", PROJECT_USAGE)
    .bool("json", false, "print the full report as JSON");
    let positional = f.parse_interspersed(args)?;
    let dir = working_dir(&f.str("dir"))?;
    let (c, project) = dial_workspace(db_path, &f.str("project")).await?;
    let res = c
        .dispatch
        .route(&pb::RouteRequest {
            tier: f.str("tier"),
            provider: f.str("provider"),
            model: f.str("model"),
            effort: f.str("effort"),
            agent: f.str("agent"),
            priority: f.str("priority"),
            relax: f.flag("relax"),
            read_only: f.flag("readonly"),
            dir,
            profile: f.str("role"),
            prompt: positional.join(" ").trim().to_owned(),
            project_id: project,
            card_slug: f.str("card"),
        })
        .await
        .map_err(run_error)?;
    if f.flag("json") {
        return print_op(res.ok, &res.json);
    }
    let report: Value = serde_json::from_str(&res.json)?;
    let text = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_owned();
    let strings = |k: &str| -> Vec<String> {
        report
            .get(k)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect()
    };
    let Some(winner) = report.get("winner").filter(|w| !w.is_null()) else {
        println!("{}", text(&report, "reason"));
        for e in strings("eliminated") {
            println!("  - {e}");
        }
        return Err(Fail::Exit(1));
    };
    println!(
        "winner: {}/{} effort={} priority={}",
        text(winner, "Agent"),
        text(winner, "Model"),
        or_unknown(&text(winner, "Effort")),
        text(winner, "Priority")
    );
    println!("reason: {}", text(&report, "reason"));
    if let Some(d) = report.get("difficulty").and_then(Value::as_f64) {
        print!(
            "difficulty: d={d:.2} ({}) classifier={}",
            difficulty_tier(d),
            text(&report, "classifier")
        );
        let probs: Vec<f64> = report
            .get("probs")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_f64)
            .collect();
        if let [e, n, h] = probs.as_slice() {
            print!(" P(easy)={e:.2} P(normal)={n:.2} P(hard)={h:.2}");
        }
        println!();
    }
    let pick: Option<RolePick> = report
        .get("rolePick")
        .and_then(|v| serde_json::from_value(v.clone()).ok());
    let line = role_line(&text(&report, "role"), pick.as_ref());
    if !line.is_empty() {
        println!("{line}");
    }
    for (i, c) in report
        .get("candidates")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
    {
        if let Some(a) = c
            .get("auto")
            .and_then(|a| serde_json::from_value::<AutoInfo>(a.clone()).ok())
        {
            let mark = if a.qualified { "✓" } else { " " };
            let eff = a.effective_cost.unwrap_or(0.0);
            println!(
                "  {}. {mark} {}/{} effort={} P={:.2} bound={:.2} cost=${:.2} q={:.2} → ${eff:.2}  θ={:.2} {}",
                i + 1,
                text(c, "Agent"),
                text(c, "Model"),
                or_unknown(&text(c, "Effort")),
                a.p_pass,
                a.p_pass_low,
                eff / a.quota,
                a.quota,
                a.theta,
                a.theta_source
            );
            continue;
        }
        println!(
            "  {}. {}/{} score={:.2}  {}",
            i + 1,
            text(c, "Agent"),
            text(c, "Model"),
            c.get("score").and_then(Value::as_f64).unwrap_or(0.0),
            text(c, "reason")
        );
    }
    for e in strings("eliminated") {
        println!("  eliminated: {e}");
    }
    let note = text(&report, "note");
    if !note.is_empty() {
        println!("note: {note}");
    }
    Ok(())
}

pub(super) async fn classify(db_path: &Path, args: &[String]) -> CliResult {
    let mut f = Flags::new("dispatch classify");
    f.string(
        "dir",
        "",
        "repository whose policy picks the classifier (default: current directory)",
    )
    .string(
        "card",
        "",
        "Work card the task names: the classifier reads it before the prompt",
    )
    .string(
        "project",
        "",
        "workspace id for --card (default: the workspace holding the cwd)",
    )
    .bool("json", false, "print JSON");
    let positional = f.parse_interspersed(args)?;
    let prompt = positional.join(" ").trim().to_owned();
    if prompt.is_empty() {
        return Err(usage_err("dispatch classify needs a prompt"));
    }
    let dir = working_dir(&f.str("dir"))?;
    let card = f.str("card");
    let (c, project) = if card.is_empty() {
        (dial_workspace(db_path, "").await?.0, String::new())
    } else {
        dial_workspace(db_path, &f.str("project")).await?
    };
    let res = c
        .dispatch
        .classify(&pb::ClassifyRequest {
            prompt,
            dir,
            project_id: project,
            card_slug: card,
            ..Default::default()
        })
        .await
        .map_err(run_error)?;
    if f.flag("json") {
        return print_op(res.ok, &res.json);
    }
    let body: Value = serde_json::from_str(&res.json)?;
    if let Some(d) = body.get("difficulty").filter(|d| !d.is_null()) {
        let text = |k: &str| d.get(k).and_then(Value::as_str).unwrap_or("").to_owned();
        print!(
            "tier={} d={:.2} classifier={}",
            text("tier"),
            d.get("d").and_then(Value::as_f64).unwrap_or(0.0),
            text("provider")
        );
        let model = text("model");
        if !model.is_empty() {
            print!(" model={model}");
        }
        println!(
            " latency={}ms",
            d.get("latencyMs").and_then(Value::as_i64).unwrap_or(0)
        );
        let probs: Vec<f64> = d
            .get("probs")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_f64)
            .collect();
        if let [e, n, h] = probs.as_slice() {
            println!("  P(easy)={e:.2} P(normal)={n:.2} P(hard)={h:.2}");
        }
        if let Some(conf) = d.get("confidence").and_then(Value::as_f64) {
            println!("  confidence={conf:.2}");
        }
    }
    if let Some(s) = body.get("role").filter(|r| !r.is_null()) {
        if s.get("unsupported").and_then(Value::as_bool) == Some(true) {
            println!("role=general (this classifier does not pick one)");
        } else {
            let probs: BTreeMap<String, f64> = s
                .get("probs")
                .and_then(|p| serde_json::from_value(p.clone()).ok())
                .unwrap_or_default();
            let pick = RolePick {
                auto: true,
                role: s.get("role").and_then(Value::as_str).unwrap_or("").into(),
                probs,
                ..RolePick::default()
            };
            println!("{}", role_line("", Some(&pick)));
        }
    }
    Ok(())
}

/// Names a route's role: the classifier's pick with its probabilities,
/// highest first, or the one the request named.
fn role_line(profile: &str, pick: Option<&RolePick>) -> String {
    let Some(pick) = pick.filter(|p| p.auto) else {
        return if profile.is_empty() {
            String::new()
        } else {
            format!("role={profile}")
        };
    };
    let mut line = format!("role={} (picked)", pick.role);
    let mut names: Vec<(&String, f64)> = pick.probs.iter().map(|(k, v)| (k, *v)).collect();
    names.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    for (name, p) in names {
        line += &format!(" P({name})={p:.2}");
    }
    line
}

/// Prints the catalog the auto route prices models with, enables or disables a
/// provider or base model, refreshes prices from models.dev, or prints benchmarks.
pub(super) async fn models(db_path: &Path, args: &[String]) -> CliResult {
    let mut f = Flags::new("models");
    f.bool("json", false, "print JSON");
    let positional = f.parse_interspersed(args)?;
    let usage = || {
        usage_err(
            "usage: dispatch models [--json] | enable|disable <provider>[/<model>] | refresh | bench [--json]",
        )
    };
    let (sub, target) = match positional.as_slice() {
        [] => ("", ""),
        [s] if s == "refresh" || s == "bench" => (s.as_str(), ""),
        [s, t] if (s == "enable" || s == "disable") && !t.trim().is_empty() => {
            (s.as_str(), t.trim())
        }
        _ => return Err(usage()),
    };
    let (c, _) = dial_workspace(db_path, "").await?;
    let as_json = f.flag("json");
    match sub {
        "enable" | "disable" => {
            let enabled = sub == "enable";
            let res = c
                .dispatch
                .set_model_enabled(&pb::SetModelEnabledRequest {
                    provider: target.to_owned(),
                    model: String::new(),
                    enabled,
                })
                .await
                .map_err(run_error)?;
            if as_json {
                write_json(&serde_json::json!({
                    "provider": res.provider,
                    "model": res.model,
                    "enabled": res.enabled,
                }))?;
                return Ok(());
            }
            let state = if res.enabled { "enabled" } else { "disabled" };
            if res.model.is_empty() {
                println!("{}: {state}", res.provider);
            } else {
                println!("{}/{}: {state}", res.provider, res.model);
            }
            Ok(())
        }
        "bench" => {
            let res = c
                .dispatch
                .list_model_benchmarks(&pb::ListModelBenchmarksRequest {})
                .await
                .map_err(run_error)?;
            if as_json || !res.ok {
                return print_op(res.ok, &res.json);
            }
            print_benchmarks(&serde_json::from_str(&res.json)?);
            Ok(())
        }
        "refresh" => {
            let res = c
                .dispatch
                .refresh_model_catalog(&pb::RefreshModelCatalogRequest {})
                .await
                .map_err(run_error)?;
            if as_json || !res.ok {
                return print_op(res.ok, &res.json);
            }
            let body: Value = serde_json::from_str(&res.json)?;
            println!(
                "refreshed {} model prices from {}",
                body.get("models").and_then(Value::as_i64).unwrap_or(0),
                body.get("source").and_then(Value::as_str).unwrap_or("")
            );
            Ok(())
        }
        _ => {
            let res = c
                .dispatch
                .list_model_catalog(&pb::ListModelCatalogRequest {})
                .await
                .map_err(run_error)?;
            let settings = c
                .dispatch
                .list_model_settings(&pb::ListModelSettingsRequest {})
                .await
                .map_err(run_error)?;
            if as_json {
                let mut v: Value = serde_json::from_str(&res.json)?;
                if let Some(obj) = v.as_object_mut() {
                    // Written field by field: the wire form drops a false `enabled`.
                    let providers: Vec<Value> = settings
                        .providers
                        .iter()
                        .map(|p| {
                            serde_json::json!({
                                "provider": p.provider,
                                "enabled": p.enabled,
                                "models": p.models.iter().map(|m| serde_json::json!({
                                    "id": m.id,
                                    "enabled": m.enabled,
                                    "agents": m.agents,
                                })).collect::<Vec<_>>(),
                            })
                        })
                        .collect();
                    obj.insert("providers".into(), Value::Array(providers));
                }
                write_json(&sort_keys(v))?;
                return if res.ok { Ok(()) } else { Err(Fail::Exit(1)) };
            }
            let report: CatalogReport = serde_json::from_str(&res.json)?;
            print!(
                "catalog {}: {} models, {} CursorBench scores, {} ECI scores",
                report.generated, report.models, report.scores, report.eci
            );
            if !report.prices_fetched.is_empty() {
                print!("; prices refreshed {}", report.prices_fetched);
            }
            println!();
            for e in &report.entries {
                let est = &e.estimate;
                let cost = est.cost_per_task.map_or_else(
                    || "price unknown".to_owned(),
                    |c| format!("${c:.2}/task ({})", est.cost_source),
                );
                let mut theta = format!("θ={:.2} {}", est.theta, est.theta_source);
                if let Some(score) = est.score {
                    theta += &format!(" {}", catalog_score(&est.theta_source, score));
                }
                let pass: Vec<String> = e
                    .bands
                    .iter()
                    .map(|b| format!("{} {:.2}≥{:.2}", b.tier, b.p_pass, b.low))
                    .collect();
                let tier = if e.tier.is_empty() { "found" } else { &e.tier };
                println!(
                    "  {tier:<6} {}/{} effort={}  {theta}  {cost}  P(pass): {}",
                    e.agent,
                    e.model,
                    or_unknown(&e.effort),
                    pass.join(" ")
                );
            }
            for p in &settings.providers {
                let p_state = if p.enabled { "enabled" } else { "disabled" };
                println!("provider {}: {p_state}", p.provider);
                for m in &p.models {
                    let m_state = if p.enabled && m.enabled {
                        "enabled"
                    } else {
                        "disabled"
                    };
                    let via = if m.agents.is_empty() {
                        String::new()
                    } else {
                        format!(" ({})", m.agents.join(", "))
                    };
                    println!("  {}/{:<28} {m_state}{via}", p.provider, m.id);
                }
            }
            for s in &report.sources {
                println!("source: {} ({}): {}", s.name, s.license, s.fields);
            }
            Ok(())
        }
    }
}

/// The benchmark figure θ comes from, as the benchmark reports it.
fn catalog_score(source: &str, score: f64) -> String {
    if source.starts_with("eci") {
        format!("(ECI {score:.0})")
    } else {
        format!("(CursorBench {:.0}%)", score * 100.0)
    }
}

/// Prints each installed model's figures as the benchmarks publish them,
/// then where they come from.
fn print_benchmarks(report: &BenchmarkReport) {
    if report.models.is_empty() {
        println!("no installed agent CLI reported models");
        return;
    }
    for m in &report.models {
        let levels: Vec<String> = m
            .cursor_bench
            .iter()
            .map(|l| {
                let level = format!("{:.0}%", l.score * 100.0);
                if l.effort.is_empty() {
                    level
                } else {
                    format!("{} {level}", l.effort)
                }
            })
            .collect();
        let mut bench = "unmeasured".to_owned();
        if !levels.is_empty() {
            bench = levels.join(", ");
            if !m.cursor_bench_as.is_empty() {
                bench += &format!(" (as {})", m.cursor_bench_as);
            }
        }
        let mut eci = "unmeasured".to_owned();
        if let Some(v) = m.eci {
            eci = format!("{v:.0}");
            if !m.eci_as.is_empty() {
                eci += &format!(" (as {})", m.eci_as);
            }
        }
        println!(
            "{:<8}  {:<28}  cursorbench={bench}  eci={eci}",
            m.agent, m.model
        );
    }
    print!("catalog {}", report.generated);
    for src in &report.sources {
        print!("; {} {}", src.name, src.url);
    }
    println!();
}

pub(super) async fn config(db_path: &Path, args: &[String]) -> CliResult {
    let usage = || {
        usage_err(
            "usage: dispatch config get [key] | set priority|discover|roles|classifier|classifier_model|classifier_agent <value> | key set|clear jev|openai",
        )
    };
    // The routing policy is global, so the command takes no repository.
    let positional = Flags::new("dispatch config").parse_interspersed(args)?;
    if positional.is_empty() {
        return Err(usage());
    }
    let mut req = pb::ConfigRequest::default();
    match positional[0].as_str() {
        "get" => {
            req.action = "get".into();
            if let Some(key) = positional.get(1) {
                req.key = key.clone();
            }
        }
        "set" => {
            if positional.len() != 3 {
                return Err(usage());
            }
            req.action = "set".into();
            req.key = positional[1].clone();
            req.values = positional[2..].to_vec();
        }
        "key" => {
            // The key never goes in the arguments, where the shell history and ps would keep it.
            if positional.len() != 3 || (positional[1] != "set" && positional[1] != "clear") {
                return Err(usage());
            }
            req.action = format!("key-{}", positional[1]);
            req.key = positional[2].clone();
            if positional[1] == "set" {
                if super::stdin_is_terminal() {
                    eprint!("Paste the key, then press Return: ");
                }
                let mut line = String::new();
                let read = std::io::stdin().lock().read_line(&mut line);
                if read.is_err() && line.is_empty() || line.is_empty() {
                    return Err(usage_err("no key on stdin"));
                }
                req.values = vec![line.trim().to_owned()];
            }
        }
        _ => return Err(usage()),
    }
    let (c, _) = dial_workspace(db_path, "").await?;
    let res = c.dispatch.config(&req).await.map_err(run_error)?;
    print_op(res.ok, &res.json)
}

pub(super) async fn doctor(db_path: &Path, args: &[String]) -> CliResult {
    let mut f = Flags::new("dispatch doctor");
    f.bool("json", false, "print JSON");
    if !f.parse_interspersed(args)?.is_empty() {
        return Err(usage_err(OPS_USAGE));
    }
    let reports = tokio::time::timeout(Duration::from_secs(30), adapters::probe::doctor()).await;
    let mut reports = reports.unwrap_or_default();

    let mut table_problems = Vec::new();
    if let Ok((c, _)) = dial_workspace(db_path, "").await {
        match c.dispatch.list_tiers(&pb::ListTiersRequest {}).await {
            Err(e) => table_problems = vec![e.to_string()],
            Ok(res) => {
                let rows: Vec<Tier> = res
                    .tiers
                    .iter()
                    .map(|t| Tier {
                        name: t.name.clone(),
                        provider: t.provider.clone(),
                        agent: t.agent.clone(),
                        model: t.model.clone(),
                        position: i64::from(t.position),
                    })
                    .collect();
                table_problems = table_from_rows(&rows).capability_problems(&known_adapters());
            }
        }
    }
    if !table_problems.is_empty() {
        for r in &mut reports {
            r.status = "DEGRADED".into();
            r.notes.extend(table_problems.iter().cloned());
        }
    }
    if f.flag("json") {
        write_json(&reports)?;
        return Ok(());
    }
    if reports.is_empty() {
        println!("no installed agent CLIs");
        return Ok(());
    }
    for r in &reports {
        println!("{:<12} {:<14} {}", r.name, or_unknown(&r.version), r.status);
        let have: Vec<&str> = r
            .flags
            .iter()
            .filter(|f| f.present)
            .map(|f| f.name.as_str())
            .collect();
        let miss: Vec<&str> = r
            .flags
            .iter()
            .filter(|f| !f.present)
            .map(|f| f.name.as_str())
            .collect();
        if !have.is_empty() {
            println!("  flags: {}", have.join(" "));
        }
        if !miss.is_empty() {
            println!("  missing: {}", miss.join(" "));
        }
        for n in &r.notes {
            println!("  note: {}", sandbox::redact(n));
        }
    }
    Ok(())
}

pub(super) async fn merge(db_path: &Path, args: &[String]) -> CliResult {
    let mut f = Flags::new("dispatch merge");
    let id = single_run_id(&mut f, args, "merge")?;
    let (c, _) = dial_workspace(db_path, "").await?;
    let res = c
        .dispatch
        .merge_run(&pb::MergeRunRequest {
            id,
            ..Default::default()
        })
        .await
        .map_err(run_error)?;
    print_op(res.ok, &res.json)
}

pub(super) async fn clean(db_path: &Path, args: &[String]) -> CliResult {
    let mut f = Flags::new("dispatch clean");
    f.bool("force", false, "discard even if the branch is not merged");
    let id = single_run_id(&mut f, args, "clean")?;
    let (c, _) = dial_workspace(db_path, "").await?;
    let res = c
        .dispatch
        .clean_run(&pb::CleanRunRequest {
            id,
            force: f.flag("force"),
        })
        .await
        .map_err(run_error)?;
    print_op(res.ok, &res.json)
}

/// `dispatch delete <id>... | --conversation <id>`: deletes ended runs for good, as the app's
/// Delete run and Delete conversation do: their records, transcripts, and the worktrees and
/// branches Dispatch cut for them, merged or not. A run still working is refused.
pub(super) async fn delete(db_path: &Path, args: &[String]) -> CliResult {
    let mut f = Flags::new("dispatch delete");
    f.string("conversation", "", "delete every run of this conversation")
        .bool("json", false, "print one JSON object");
    let ids = f.parse_interspersed(args)?;
    let conversation = f.str("conversation");
    if ids.is_empty() && conversation.trim().is_empty() {
        return Err(usage_err(format!(
            "dispatch delete needs run ids or --conversation\n{OPS_USAGE}"
        )));
    }
    let c = super::dial(db_path).await?;
    let res = c
        .dispatch
        .delete_runs(&pb::DeleteRunsRequest {
            ids,
            conversation_id: conversation.trim().to_owned(),
        })
        .await
        .map_err(run_error)?;
    if f.flag("json") {
        return print_op(true, &res.json);
    }
    let body: Value = serde_json::from_str(&res.json).unwrap_or(Value::Null);
    let worktrees = body
        .get("worktrees")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    println!(
        "deleted {} run{}{}",
        res.deleted,
        if res.deleted == 1 { "" } else { "s" },
        match worktrees {
            0 => String::new(),
            1 => " and its worktree".to_string(),
            n => format!(" and {n} worktrees"),
        }
    );
    Ok(())
}

/// `dispatch rename <conversation-id> <name>`: the name the app shows for a conversation; an
/// empty one gives it back its first prompt.
pub(super) async fn rename(db_path: &Path, args: &[String]) -> CliResult {
    let mut f = Flags::new("dispatch rename");
    let positional = f.parse_interspersed(args)?;
    let [conversation, name @ ..] = positional.as_slice() else {
        return Err(usage_err(format!(
            "dispatch rename needs a conversation id and a name\n{OPS_USAGE}"
        )));
    };
    let name = name.join(" ");
    let c = super::dial(db_path).await?;
    c.dispatch
        .rename_conversation(&pb::RenameConversationRequest {
            conversation_id: conversation.trim().to_owned(),
            name: name.trim().to_owned(),
        })
        .await
        .map_err(run_error)?;
    if name.trim().is_empty() {
        println!("conversation {conversation} named by its first prompt again");
    } else {
        println!("conversation {conversation} renamed {:?}", name.trim());
    }
    Ok(())
}

/// `dispatch worktrees [--prune]`: every folder under a repository's `.dispatch/worktrees`,
/// with its run, the run's status, and whether it has anything to merge; `--prune` removes
/// the ones whose run ended with nothing to merge, or no longer exists.
pub(super) async fn worktrees(db_path: &Path, args: &[String]) -> CliResult {
    let mut f = Flags::new("dispatch worktrees");
    f.bool(
        "prune",
        false,
        "remove the worktrees whose run ended with nothing to merge, or no longer exists",
    )
    .string(
        "dir",
        "",
        "also look in the repository holding this folder (default: current directory)",
    )
    .bool("json", false, "print one JSON object");
    if !f.parse_interspersed(args)?.is_empty() {
        return Err(usage_err(format!(
            "dispatch worktrees takes no arguments\n{OPS_USAGE}"
        )));
    }
    let dir = working_dir(&f.str("dir"))?;
    let c = super::dial(db_path).await?;
    let (removed, kept, pruned) = if f.flag("prune") {
        let res = c
            .dispatch
            .prune_worktree_folders(&pb::PruneWorktreeFoldersRequest { dir })
            .await?;
        (res.removed, res.kept, true)
    } else {
        let res = c
            .dispatch
            .list_worktree_folders(&pb::ListWorktreeFoldersRequest { dir })
            .await?;
        (Vec::new(), res.folders, false)
    };
    if f.flag("json") {
        let rows = |items: &[pb::WorktreeFolder]| -> Vec<Value> {
            items.iter().map(folder_json).collect()
        };
        let body = if pruned {
            serde_json::json!({"removed": rows(&removed), "kept": rows(&kept)})
        } else {
            serde_json::json!({"worktrees": rows(&kept)})
        };
        write_json(&body)?;
        return Ok(());
    }
    let mut out = String::new();
    if pruned {
        out.push_str(&format!("removed {}\n", plural_worktrees(removed.len())));
        for item in &removed {
            out.push_str(&format!("  {}\n", item.path));
        }
    }
    if kept.is_empty() {
        if !pruned {
            out.push_str("no worktrees\n");
        }
    } else {
        if pruned {
            out.push_str("kept:\n");
        }
        for item in &kept {
            out.push_str(&format!("{}\n", folder_line(item)));
        }
        let prunable = kept.iter().filter(|item| item.prunable).count();
        if !pruned && prunable > 0 {
            out.push_str(&format!(
                "{} can go: dispatch worktrees --prune\n",
                plural_worktrees(prunable)
            ));
        }
    }
    super::print_out(&out)?;
    Ok(())
}

fn plural_worktrees(n: usize) -> String {
    if n == 1 {
        "1 worktree".into()
    } else {
        format!("{n} worktrees")
    }
}

/// One folder as `dispatch worktrees` prints it: run, status, changes, branch, path.
fn folder_line(item: &pb::WorktreeFolder) -> String {
    let run = item.run_id.get(..12).unwrap_or(&item.run_id);
    let status = if item.run_status.is_empty() {
        "no run".to_owned()
    } else {
        item.run_status.to_lowercase()
    };
    let changes = if item.changes {
        "changes"
    } else {
        "no changes"
    };
    let prune = if item.prunable { "  (can go)" } else { "" };
    format!(
        "{run:<12}  {status:<10}  {changes:<10}  {:<20}  {}{prune}",
        item.branch, item.path
    )
}

fn folder_json(item: &pb::WorktreeFolder) -> Value {
    serde_json::json!({
        "path": item.path,
        "repo": item.repo,
        "branch": item.branch,
        "run_id": item.run_id,
        "run_status": item.run_status,
        "changes": item.changes,
        "prunable": item.prunable,
    })
}

pub(super) async fn recover(db_path: &Path, args: &[String]) -> CliResult {
    let mut f = Flags::new("dispatch recover");
    let id = single_run_id(&mut f, args, "recover")?;
    let (c, _) = dial_workspace(db_path, "").await?;
    let res = c
        .dispatch
        .recover_run(&pb::RecoverRunRequest { id })
        .await
        .map_err(run_error)?;
    print_op(res.ok, &res.json)
}

pub(super) async fn roles(db_path: &Path, args: &[String]) -> CliResult {
    let mut f = Flags::new("dispatch roles");
    f.bool(
        "check",
        false,
        "validate roles against known and installed agents",
    )
    .string("dir", "", "repository (default: current directory)")
    .string(
        "tier",
        "",
        "save: the lowest tier it runs at, easy|normal|hard",
    )
    .bool("readonly", false, "save: the run may not change files")
    .string("instructions", "", "save: the prompt put before the task")
    .string("prefer", "", "save: agents to prefer, comma separated")
    .string("avoid", "", "save: agents to avoid, comma separated")
    .string("timeout", "", "save: how long a run may take, e.g. 15m")
    .string(
        "when",
        "",
        "save: the tasks it fits, read by the classifier that picks a role",
    )
    .bool(
        "manual",
        false,
        "save: only --role gives it; the classifier never picks it",
    )
    .string(
        "easy",
        "",
        "save: its easy models in order, agent/model, comma separated; empty uses the tier table",
    )
    .string(
        "normal",
        "",
        "save: its normal models in order, agent/model, comma separated; empty uses the tier table",
    )
    .string(
        "hard",
        "",
        "save: its hard models in order, agent/model, comma separated; empty uses the tier table",
    )
    .string(
        "columns",
        "",
        "save: the tier columns its board keeps, comma separated, such as easy,hard; empty keeps all three",
    );
    let positional = f.parse_interspersed(args)?;
    let check = f.flag("check");
    let mut action = "list".to_owned();
    let mut req = pb::SubagentsRequest::default();
    match positional.as_slice() {
        [verb, name] if (verb == "save" || verb == "remove") && !check => {
            action.clone_from(verb);
            req.name = name.clone();
            if verb == "save" {
                let mut entries = Vec::new();
                for t in ["easy", "normal", "hard"] {
                    entries.extend(parse_role_models(t, &f.str(t))?);
                }
                let profile = Profile {
                    tier: f.str("tier"),
                    read_only: f.flag("readonly"),
                    instructions: f.str("instructions"),
                    when: f.str("when"),
                    tiers: entries,
                    columns: role_columns(&f.str("columns"))?,
                    manual: f.flag("manual"),
                    prefer: split_list(&f.str("prefer")),
                    avoid: split_list(&f.str("avoid")),
                    timeout: f.str("timeout"),
                    ..Profile::default()
                };
                req.profile = serde_json::to_string(&profile)?;
            }
        }
        [] => {
            if check {
                action = "check".into();
            }
        }
        _ => return Err(usage_err(OPS_USAGE)),
    }
    let wd = working_dir(&f.str("dir"))?;
    let (c, _) = dial_workspace(db_path, "").await?;
    req.action = action.clone();
    req.dir = wd;
    let res = c.dispatch.subagents(&req).await.map_err(run_error)?;
    if action == "check" {
        let body: Value = serde_json::from_str(&res.json)?;
        let problems: Vec<String> = body
            .get("problems")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect();
        if problems.is_empty() {
            println!(
                "ok: {} role(s)",
                body.get("roles").and_then(Value::as_i64).unwrap_or(0)
            );
            return Ok(());
        }
        for p in &problems {
            println!("! {p}");
        }
        return Err(Fail::Exit(1));
    }
    print_op(res.ok, &res.json)
}

/// Reads the tier columns a role keeps; empty keeps all three.
fn role_columns(list: &str) -> Result<Option<Vec<String>>, Fail> {
    let columns = split_list(list);
    if columns.is_empty() {
        return Ok(None);
    }
    for column in &columns {
        if !matches!(column.as_str(), "easy" | "normal" | "hard") {
            return Err(usage_err(format!(
                "--columns: {column:?} is not easy, normal, or hard"
            )));
        }
    }
    Ok(Some(columns))
}

/// Reads one tier's agent/model entries, comma separated, in order.
fn parse_role_models(tier: &str, list: &str) -> Result<Vec<RoleEntry>, Fail> {
    let mut out = Vec::new();
    for item in split_list(list) {
        match item.split_once('/') {
            Some((agent, model)) if !agent.is_empty() && !model.is_empty() => out.push(RoleEntry {
                tier: tier.into(),
                agent: agent.into(),
                model: model.into(),
                ..RoleEntry::default()
            }),
            _ => {
                return Err(usage_err(format!("--{tier}: {item:?} is not agent/model")));
            }
        }
    }
    Ok(out)
}

/// Reads a comma-separated flag; empty is none.
fn split_list(s: &str) -> Vec<String> {
    s.split(',')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(str::to_owned)
        .collect()
}

// --- on / off / hook / skill / setup / update (local, no core) ---

fn executable() -> Result<String, Error> {
    Ok(crate::runstate::command_binary()?
        .to_string_lossy()
        .into_owned())
}

fn home() -> Result<std::path::PathBuf, Error> {
    dirs::home_dir().ok_or_else(|| Error::Message("$HOME is not defined".into()))
}

/// Turns on Claude Code's SessionStart hook (`dispatch setup hook <agent> on`
/// turns on another agent's).
pub(super) fn on(db_path: &Path) -> CliResult {
    let exe = executable()?;
    setup::set_hook(&home()?, &exe, &state_dir(db_path), "claude", true)?;
    println!(
        "dispatch on: SessionStart hook registered in {}",
        hook::settings_path(Path::new("")).display()
    );
    Ok(())
}

/// Turns off Claude Code's SessionStart hook; dispatch stays on for the
/// other agents whose hook is registered.
pub(super) fn off(db_path: &Path) -> CliResult {
    let exe = executable()?;
    setup::set_hook(&home()?, &exe, &state_dir(db_path), "claude", false)?;
    println!(
        "dispatch off: SessionStart hook removed from {}",
        hook::settings_path(Path::new("")).display()
    );
    Ok(())
}

/// The SessionStart hook target. `--agent` names the agent running it, Claude
/// Code when it is absent. It prints the context line, in the shape that
/// agent reads, while dispatch is on and nothing otherwise, so a stale hook
/// is harmless.
pub(super) fn hook(db_path: &Path, args: &[String]) -> CliResult {
    let mut f = Flags::new("dispatch hook");
    f.string(
        "agent",
        "claude",
        "the agent running the hook: claude|codex|cursor|gemini|qwen|copilot|grok",
    );
    f.parse_silent(args)?;
    let agent = f.str("agent");
    let (session, recorded) = caller::record_hook_stdin(&agent);
    if hook::is_enabled(&state_dir(db_path)) {
        // Grok runs Claude Code's and Cursor's hooks too: under it one hook speaks, in
        // Grok's shape.
        let grok = std::env::var_os("GROK_HOOK_EVENT").is_some_and(|v| !v.is_empty());
        if !(grok && hook::quiet_under_grok(Path::new(""), &agent)) {
            let shape = if grok { "grok" } else { agent.as_str() };
            let env = caller::hook_caller_env(&session, &agent);
            print!("{}", hook::context_output(shape, env));
        }
    }
    recorded.map_err(Fail::from)
}

pub(super) fn skill(args: &[String]) -> CliResult {
    let usage = || {
        usage_err(
            "usage: dispatch skill install [--agent <name> | --dir <folder>] [--force] | remove [--agent <name> | --dir <folder>] | show | refresh",
        )
    };
    let Some(sub) = args.first() else {
        return Err(usage());
    };
    let home = home()?;
    match sub.as_str() {
        "show" => {
            print!("{}", String::from_utf8_lossy(skill::CONTENT));
            Ok(())
        }
        "refresh" => {
            let r = skill::refresh(&home);
            let paths = match &r {
                Ok(p) => p.clone(),
                Err(_) => Vec::new(),
            };
            for p in &paths {
                println!("refreshed {}", p.display());
            }
            if r.is_ok() && paths.is_empty() {
                println!("no installed dispatch skill to refresh");
            }
            r.map(|_| ()).map_err(Fail::from)
        }
        "install" | "remove" => {
            let mut f = Flags::new(format!("dispatch skill {sub}"));
            f.string(
                "agent",
                "",
                "the folder this agent reads (claude, antigravity, or any other for ~/.agents/skills)",
            )
            .string("dir", "", "a skills folder by its path");
            if sub == "install" {
                f.bool("force", false, "overwrite an existing skill");
            }
            f.parse(&args[1..])?;
            let key = match f.str("dir") {
                dir if !dir.is_empty() => dir,
                _ => f.str("agent"),
            };
            if sub == "remove" {
                let removed = setup::remove_skill(&home, &key)?;
                for p in &removed {
                    println!("removed {}", p.display());
                }
                if removed.is_empty() {
                    println!("no dispatch skill to remove");
                }
                return Ok(());
            }
            let folders: Vec<skill::Folder> = if key.is_empty() {
                skill::folders(&home)
                    .into_iter()
                    .filter(|folder| !folder.extra)
                    .collect()
            } else {
                match skill::resolve(&home, &key) {
                    Some(folder) => vec![folder],
                    None => {
                        return Err(usage_err(format!("unknown agent or skills folder {key:?}")));
                    }
                }
            };
            for folder in folders {
                match skill::install(&folder.dir, f.flag("force")) {
                    Ok(path) => println!("installed {}", path.display()),
                    // The one folder asked for is an error; one of all of them is skipped.
                    Err(e) if !key.is_empty() => return Err(e.into()),
                    Err(e) => eprintln!("skip {}: {e}", folder.dir.display()),
                }
            }
            Ok(())
        }
        _ => Err(usage()),
    }
}

/// Reports how the agent CLIs reach dispatch, or changes one part of it: link
/// points ~/.local/bin/dispatch at this binary, skill installs the current
/// skill (into one folder, or the three Dispatch uses) or removes it, hook
/// switches an agent's SessionStart hook (Claude Code's when none is named),
/// and opencode installs or removes the Dispatch plugin in OpenCode or
/// switches its delegation. The app's Settings › Agents does the same.
pub(super) fn setup(db_path: &Path, args: &[String]) -> CliResult {
    let home = home()?;
    let binary = crate::runstate::command_binary()?
        .to_string_lossy()
        .into_owned();
    let state = state_dir(db_path);
    let action = args.first().map_or("", String::as_str);
    match action {
        "" | "--json" => {}
        "link" => {
            let path = setup::link_cli(&home, &binary)?;
            println!("linked {} -> {binary}", path.display());
        }
        "skill" => {
            let remove = args.get(1).is_some_and(|a| a == "--remove");
            let key = args
                .get(if remove { 2 } else { 1 })
                .map_or("", String::as_str);
            let (r, verb) = if remove {
                (setup::remove_skill(&home, key), "removed")
            } else {
                (setup::install_skill(&home, key), "installed")
            };
            if let Ok(paths) = &r {
                for p in paths {
                    println!("{verb} {}", p.display());
                }
            }
            r?;
        }
        "hook" => {
            let usage = || usage_err("usage: dispatch setup hook [<agent>] on|off");
            let (agent, switch) = match (args.get(1), args.get(2)) {
                (Some(agent), Some(switch)) => (agent.as_str(), switch.as_str()),
                (Some(switch), None) => ("claude", switch.as_str()),
                _ => return Err(usage()),
            };
            let on = match switch {
                "on" => true,
                "off" => false,
                _ => return Err(usage()),
            };
            setup::set_hook(&home, &binary, &state, agent, on)?;
            println!("{agent} hook {switch}");
        }
        "opencode" => match args.get(1).map_or("", String::as_str) {
            "" => {
                let path = setup::install_opencode_plugin(
                    &home,
                    &setup::opencode_binary(&home, &binary),
                    &state,
                )?;
                println!(
                    "installed {} and loaded it from {}",
                    path.display(),
                    setup::opencode_config_path(&home).display()
                );
            }
            "--remove" => {
                setup::remove_opencode_plugin(&home, &state)?;
                println!(
                    "removed the Dispatch plugin from {}",
                    setup::opencode_config_path(&home).display()
                );
            }
            sub @ ("on" | "off") => {
                setup::set_opencode_delegation(&state, sub == "on")?;
                println!("OpenCode delegation through dispatch {sub}");
            }
            _ => {
                return Err(usage_err(
                    "usage: dispatch setup opencode [--remove | on | off]",
                ));
            }
        },
        _ => {
            return Err(usage_err(
                "usage: dispatch setup [--json] | link | skill [--remove] [<agent> | <folder>] | hook [<agent>] on|off | opencode [--remove | on | off]",
            ));
        }
    }
    let report = setup::read(&home, &binary, &state);
    if action == "--json" {
        super::write_json_compact(&report, true)?;
    } else if action.is_empty() {
        print_setup(&report);
    }
    Ok(())
}

fn print_setup(r: &setup::Report) {
    let cli = match r.cli.state.as_str() {
        setup::LINKED => "linked to this binary".to_owned(),
        setup::OTHER => format!("another copy ({})", r.cli.target),
        setup::MISSING => "not installed".to_owned(),
        _ => String::new(),
    };
    println!("command  {}: {cli}", r.cli.path);
    for s in &r.skills {
        let extra = if s.extra { ", a second copy" } else { "" };
        println!(
            "skill    {:<9} {}  ({}{extra})",
            s.state,
            s.dir,
            s.agents.join(", ")
        );
    }
    for h in &r.hooks {
        let mut state = if h.on {
            "on".to_owned()
        } else {
            "off".to_owned()
        };
        if h.on && !h.current {
            state += &format!(" (runs {})", h.command);
        }
        println!("hook     {:<9} {state}  {}", h.agent, h.settings);
    }
    let mut plugin = match r.opencode.state.as_str() {
        setup::CURRENT => "current",
        setup::OUTDATED => "outdated",
        setup::MISSING => "not installed",
        _ => "",
    }
    .to_owned();
    if r.opencode.state != setup::MISSING {
        plugin += if r.opencode.on {
            ", delegation on"
        } else {
            ", delegation off"
        };
    }
    println!("plugin   OpenCode {plugin}  {}", r.opencode.path);
}

/// Explains where updates come from: dispatch ships inside the Polterware
/// Dispatch app, so it has no updater of its own.
pub(super) fn update(args: &[String]) -> CliResult {
    let mut f = Flags::new("dispatch update");
    f.bool("check", false, "report only");
    f.parse(args)?;
    println!("dispatch ships inside Dispatch and has no updater of its own.");
    println!(
        "Update Dispatch (Dispatch › Check for Updates…, or its daily check), then run `dispatch skill refresh`."
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_line_orders_by_probability() {
        let pick = RolePick {
            auto: true,
            role: "tester".into(),
            probs: BTreeMap::from([("general".into(), 0.2), ("tester".into(), 0.7)]),
            ..RolePick::default()
        };
        assert_eq!(
            role_line("", Some(&pick)),
            "role=tester (picked) P(tester)=0.70 P(general)=0.20"
        );
        assert_eq!(role_line("reviewer", None), "role=reviewer");
        assert_eq!(role_line("", None), "");
    }

    #[test]
    fn role_models_need_agent_and_model() {
        let got = parse_role_models("easy", "codex/gpt-5, claude/haiku").unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(got[1].agent, "claude");
        assert!(parse_role_models("hard", "codex").is_err());
    }
}
