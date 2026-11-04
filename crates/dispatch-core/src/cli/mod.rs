//! The `dispatch` command: the local core (`serve`) and the commands that
//! delegate tasks to other coding agents. Only `serve` opens the store; every
//! other command is a client of the core and starts it when none is running.
//!
//! Its arguments, text, JSON, and exit statuses are the ones the Go binary had,
//! because agents, the skill, the OpenCode plugin, and Polterware call it.

mod caller;
mod flags;
mod ops;
mod runs;
mod serve;
mod workspace;

use crate::{Error, coreclient, proto as pb};
use flags::{FlagError, Flags};
use serde::Serialize;
use serde_json::Value;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Maps each top-level command to the name `run_dispatch` knows it by.
const COMMANDS: &[(&str, &str)] = &[
    ("run", "run"),
    ("runs", "ls"),
    ("status", "status"),
    ("wait", "wait"),
    ("result", "result"),
    ("cancel", "cancel"),
    ("route", "route"),
    ("classify", "classify"),
    ("models", "models"),
    ("merge", "merge"),
    ("clean", "clean"),
    ("recover", "recover"),
    ("roles", "roles"),
    ("tiers", "tiers"),
    ("usage", "stats"),
    ("agents", "agents"),
    ("doctor", "doctor"),
    ("config", "config"),
    ("skill", "skill"),
    ("setup", "setup"),
    ("update", "update"),
    ("on", "on"),
    ("off", "off"),
    ("hook", "hook"),
];

/// How a command ends when it does not succeed.
#[derive(Debug)]
pub(crate) enum Fail {
    /// Ends the process with this status after the command printed its output.
    Exit(i32),
    Err(Error),
}

impl From<Error> for Fail {
    fn from(e: Error) -> Self {
        Fail::Err(e)
    }
}

impl From<std::io::Error> for Fail {
    fn from(e: std::io::Error) -> Self {
        Fail::Err(e.into())
    }
}

impl From<serde_json::Error> for Fail {
    fn from(e: serde_json::Error) -> Self {
        Fail::Err(e.into())
    }
}

impl From<FlagError> for Fail {
    fn from(e: FlagError) -> Self {
        Fail::Err(Error::Message(e.to_string()))
    }
}

pub(crate) type CliResult = Result<(), Fail>;

pub(crate) fn usage_err(text: impl Into<String>) -> Fail {
    Fail::Err(Error::Message(text.into()))
}

/// Whether this process was started as the `dispatch` command rather than
/// the app: under that name, or with a command. A Finder launch passes no
/// arguments, or only the ones macOS adds (`-psn_…`, `-NS…`, `-Apple…`).
pub fn is_command_line(argv0: &str, args: &[String]) -> bool {
    let name = Path::new(argv0)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");
    if name == "dispatch" {
        return true;
    }
    args.first().is_some_and(|a| {
        !(a.starts_with("-psn_") || a.starts_with("-NS") || a.starts_with("-Apple"))
    })
}

/// Runs the command line `args` (without the program name) and returns the
/// process exit status.
pub fn main(args: Vec<String>) -> i32 {
    let mut global = Flags::new("dispatch");
    global.string("db", &default_db().to_string_lossy(), "sqlite path");
    let rest = match global.parse_silent(&args) {
        Ok(rest) => rest,
        Err(FlagError::Help) => {
            usage();
            return 0;
        }
        Err(FlagError::Bad(msg)) => {
            eprintln!("{msg}");
            usage();
            return 2;
        }
    };
    let db = global.str("db");
    let db_path = std::path::absolute(&db).unwrap_or_else(|_| PathBuf::from(&db));
    let Some((command, rest)) = rest.split_first() else {
        usage();
        return 2;
    };

    let result = match command.as_str() {
        // The core sets up its environment before its runtime starts threads.
        "serve" => serve::run_serve(&db_path, rest),
        "help" | "-h" | "--help" => {
            usage();
            Ok(())
        }
        _ => {
            let sub = COMMANDS
                .iter()
                .find(|(name, _)| name == command)
                .map(|(_, s)| *s);
            if sub.is_none()
                && !matches!(
                    command.as_str(),
                    "stop" | "workspace" | "workspaces" | "dispatch"
                )
            {
                eprintln!("unknown command {command:?}");
                usage();
                return 2;
            }
            let runtime = match tokio::runtime::Runtime::new() {
                Ok(r) => r,
                Err(e) => return fail(Fail::Err(e.into())),
            };
            runtime.block_on(async {
                match command.as_str() {
                    "stop" => serve::run_stop(&db_path).await,
                    "workspace" | "workspaces" => workspace::run_workspace(&db_path, rest).await,
                    // `dispatch dispatch hook` is the hook Polterware registered before.
                    "dispatch" => run_dispatch(&db_path, rest).await,
                    _ => {
                        let mut args = vec![sub.unwrap_or_default().to_owned()];
                        args.extend_from_slice(rest);
                        run_dispatch(&db_path, &args).await
                    }
                }
            })
        }
    };
    match result {
        Ok(()) => 0,
        Err(f) => fail(f),
    }
}

fn fail(f: Fail) -> i32 {
    match f {
        Fail::Exit(code) => code,
        Fail::Err(e) => {
            eprintln!("dispatch: {}", message(&e));
            1
        }
    }
}

/// The Connect codes the client puts before a core error's message; the CLI
/// prints the message alone, as it always has.
const CONNECT_CODES: &[&str] = &[
    "canceled",
    "unknown",
    "invalid_argument",
    "deadline_exceeded",
    "not_found",
    "already_exists",
    "permission_denied",
    "resource_exhausted",
    "failed_precondition",
    "aborted",
    "out_of_range",
    "unimplemented",
    "internal",
    "unavailable",
    "data_loss",
    "unauthenticated",
];

fn message(e: &Error) -> String {
    let s = e.to_string();
    for code in CONNECT_CODES {
        if let Some(rest) = s.strip_prefix(code).and_then(|r| r.strip_prefix(": ")) {
            return rest.to_owned();
        }
    }
    s
}

/// A run the core does not know reads as it always has.
pub(crate) fn run_error(e: Error) -> Error {
    let s = e.to_string();
    if s.starts_with("not_found: ") {
        return Error::Message(format!("run not found: unknown dispatch run ({s})"));
    }
    e
}

fn usage() {
    eprint!(
        "Usage:
  dispatch [-db path] <command> [flags]

Runs:
{}
       dispatch runs | status <id> | cancel <id> | tiers [build] | usage | agents

Workspaces (Polterware projects when Polterware is installed, plus folders):
  dispatch workspace ls [--json]
  dispatch workspace add <folder> [--name <name>]
  dispatch workspace rm <id>

Core:
  dispatch serve [--listen 127.0.0.1:0] [--idle <dur>]
  dispatch stop

The database defaults to $PWDISPATCH_DATA_DIR/dispatch.db, else
~/Library/Application Support/Polterware Dispatch/dispatch.db.
",
        runs::dispatch_usage()
    );
}

/// `$PWDISPATCH_DATA_DIR/dispatch.db`, else the app's folder.
pub fn default_db() -> PathBuf {
    if let Ok(data) = std::env::var("PWDISPATCH_DATA_DIR")
        && !data.is_empty()
    {
        return Path::new(&data).join("dispatch.db");
    }
    match dirs::home_dir() {
        Some(home) => home
            .join("Library")
            .join("Application Support")
            .join("Polterware Dispatch")
            .join("dispatch.db"),
        None => PathBuf::from("dispatch.db"),
    }
}

async fn run_dispatch(db_path: &Path, args: &[String]) -> CliResult {
    let Some((sub, rest)) = args.split_first() else {
        return Err(usage_err(runs::dispatch_usage()));
    };
    match sub.as_str() {
        "run" => runs::run(db_path, rest).await,
        "ls" | "runs" => runs::list(db_path, rest).await,
        "status" => runs::status(db_path, rest).await,
        "result" => runs::result(db_path, rest).await,
        "cancel" => runs::cancel(db_path, rest).await,
        "tiers" => runs::tiers(db_path, rest).await,
        "stats" => runs::stats(db_path, rest).await,
        "agents" => runs::agents(db_path, rest).await,
        "wait" => ops::wait(db_path, rest).await,
        "route" => ops::route(db_path, rest).await,
        "classify" => ops::classify(db_path, rest).await,
        "models" => ops::models(db_path, rest).await,
        "config" => ops::config(db_path, rest).await,
        "doctor" => ops::doctor(db_path, rest).await,
        "merge" => ops::merge(db_path, rest).await,
        "clean" => ops::clean(db_path, rest).await,
        "recover" => ops::recover(db_path, rest).await,
        "roles" => ops::roles(db_path, rest).await,
        "on" => ops::on(db_path),
        "off" => ops::off(db_path),
        "hook" => ops::hook(db_path, rest),
        "skill" => ops::skill(rest),
        "setup" => ops::setup(db_path, rest),
        "update" => ops::update(rest),
        _ => Err(usage_err(runs::dispatch_usage())),
    }
}

/// Clients for the core that owns `db_path`, starting it when needed.
pub(crate) async fn dial(db_path: &Path) -> Result<coreclient::Clients, Error> {
    coreclient::connect(db_path, "cli").await
}

/// Dials the core and resolves the workspace: the one with `workspace_id`,
/// else the one holding the cwd (created on first use). A cwd no workspace
/// resolves leaves it empty.
pub(crate) async fn dial_workspace(
    db_path: &Path,
    workspace_id: &str,
) -> Result<(coreclient::Clients, String), Error> {
    let c = dial(db_path).await?;
    match resolve_workspace(&c, workspace_id, "").await {
        Ok(ws) => Ok((c, ws.map(|w| w.id).unwrap_or_default())),
        Err(e) if !workspace_id.is_empty() => Err(e),
        Err(_) => Ok((c, String::new())),
    }
}

/// The workspace with `id`, else the one holding `dir` (the cwd when empty).
pub(crate) async fn resolve_workspace(
    c: &coreclient::Clients,
    id: &str,
    dir: &str,
) -> Result<Option<pb::Workspace>, Error> {
    let mut dir = dir.to_owned();
    if id.trim().is_empty() && dir.trim().is_empty() {
        dir = std::env::current_dir()?.to_string_lossy().into_owned();
    }
    let res = c
        .workspaces
        .resolve_workspace(&pb::ResolveWorkspaceRequest { id: id.into(), dir })
        .await?;
    Ok(res.workspace)
}

/// The state folder beside the database: `<db dir>/dispatch`.
pub(crate) fn state_dir(db_path: &Path) -> PathBuf {
    let db = if db_path.as_os_str().is_empty() {
        default_db()
    } else {
        db_path.to_path_buf()
    };
    db.parent()
        .unwrap_or_else(|| Path::new("."))
        .join("dispatch")
}

/// `dir` made absolute, else the cwd.
pub(crate) fn working_dir(dir: &str) -> Result<String, Error> {
    let p = if dir.trim().is_empty() {
        std::env::current_dir()?
    } else {
        std::path::absolute(dir)?
    };
    Ok(p.to_string_lossy().into_owned())
}

/// Writes numbers as Go's encoding/json does: `1`, not `1.0`, and an
/// exponent only below 1e-6 or from 1e21.
struct GoFormatter<F>(F);

macro_rules! delegate {
    ($($name:ident),*) => {$(
        fn $name<W: ?Sized + std::io::Write>(&mut self, w: &mut W) -> std::io::Result<()> {
            self.0.$name(w)
        }
    )*};
}

impl<F: serde_json::ser::Formatter> serde_json::ser::Formatter for GoFormatter<F> {
    delegate!(
        begin_array,
        end_array,
        begin_object,
        end_object,
        end_array_value,
        end_object_key,
        begin_object_value,
        end_object_value
    );

    fn begin_array_value<W: ?Sized + std::io::Write>(
        &mut self,
        w: &mut W,
        first: bool,
    ) -> std::io::Result<()> {
        self.0.begin_array_value(w, first)
    }

    fn begin_object_key<W: ?Sized + std::io::Write>(
        &mut self,
        w: &mut W,
        first: bool,
    ) -> std::io::Result<()> {
        self.0.begin_object_key(w, first)
    }

    fn write_f64<W: ?Sized + std::io::Write>(&mut self, w: &mut W, v: f64) -> std::io::Result<()> {
        w.write_all(go_float(v).as_bytes())
    }

    fn write_f32<W: ?Sized + std::io::Write>(&mut self, w: &mut W, v: f32) -> std::io::Result<()> {
        w.write_all(go_float(f64::from(v)).as_bytes())
    }
}

fn go_float(v: f64) -> String {
    let abs = v.abs();
    if abs != 0.0 && !(1e-6..1e21).contains(&abs) {
        let s = format!("{v:e}");
        return match s.split_once('e') {
            Some((m, e)) if !e.starts_with('-') => format!("{m}e+{e}"),
            _ => s,
        };
    }
    v.to_string()
}

/// Encodes JSON as Go's encoding/json does: its numbers, `<`, `>`, `&`,
/// U+2028, and U+2029 escaped, and a newline at the end.
fn go_json<T: Serialize + ?Sized>(v: &T, indent: bool) -> Result<String, Error> {
    let mut buf = Vec::new();
    if indent {
        let fmt = GoFormatter(serde_json::ser::PrettyFormatter::new());
        v.serialize(&mut serde_json::Serializer::with_formatter(&mut buf, fmt))?;
    } else {
        let fmt = GoFormatter(serde_json::ser::CompactFormatter);
        v.serialize(&mut serde_json::Serializer::with_formatter(&mut buf, fmt))?;
    }
    let s = String::from_utf8_lossy(&buf);
    let mut out = String::with_capacity(s.len() + 1);
    for c in s.chars() {
        match c {
            '<' => out.push_str("\\u003c"),
            '>' => out.push_str("\\u003e"),
            '&' => out.push_str("\\u0026"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            c => out.push(c),
        }
    }
    out.push('\n');
    Ok(out)
}

fn print_out(s: &str) -> Result<(), Error> {
    let mut out = std::io::stdout().lock();
    out.write_all(s.as_bytes())?;
    out.flush()?;
    Ok(())
}

/// Prints `v` as indented JSON.
pub(crate) fn write_json<T: Serialize + ?Sized>(v: &T) -> Result<(), Error> {
    print_out(&go_json(v, true)?)
}

/// Prints `v` as indented JSON, or on one line when `compact`.
pub(crate) fn write_json_compact<T: Serialize + ?Sized>(v: &T, compact: bool) -> Result<(), Error> {
    print_out(&go_json(v, !compact)?)
}

/// Orders every object's keys, as Go does for a decoded map.
pub(crate) fn sort_keys(v: Value) -> Value {
    match v {
        Value::Object(map) => {
            let mut pairs: Vec<(String, Value)> = map.into_iter().collect();
            pairs.sort_by(|a, b| a.0.cmp(&b.0));
            Value::Object(pairs.into_iter().map(|(k, v)| (k, sort_keys(v))).collect())
        }
        Value::Array(items) => Value::Array(items.into_iter().map(sort_keys).collect()),
        other => other,
    }
}

/// Orders the top-level keys alone, as Go does for a map of typed values.
pub(crate) fn sort_top(v: Value) -> Value {
    match v {
        Value::Object(map) => {
            let mut pairs: Vec<(String, Value)> = map.into_iter().collect();
            pairs.sort_by(|a, b| a.0.cmp(&b.0));
            Value::Object(pairs.into_iter().collect())
        }
        other => other,
    }
}

pub(crate) fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub(crate) fn short_id(id: &str) -> &str {
    match id.char_indices().nth(12) {
        Some((i, _)) => &id[..i],
        None => id,
    }
}

pub(crate) fn or_unknown(s: &str) -> &str {
    if s.trim().is_empty() { "unknown" } else { s }
}

/// Whether stdin is a terminal rather than a pipe or a file.
pub(crate) fn stdin_is_terminal() -> bool {
    use std::io::IsTerminal;
    std::io::stdin().is_terminal()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tells_the_command_from_the_app() {
        let args = |v: &[&str]| v.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        assert!(is_command_line("/Users/x/.local/bin/dispatch", &[]));
        assert!(is_command_line(
            "/Applications/Dispatch.app/Contents/MacOS/Dispatch",
            &args(&["-db", "x", "serve"])
        ));
        assert!(!is_command_line(
            "/Applications/Dispatch.app/Contents/MacOS/Dispatch",
            &[]
        ));
        assert!(!is_command_line("Dispatch", &args(&["-psn_0_12345"])));
        assert!(!is_command_line(
            "Dispatch",
            &args(&["-NSDocumentRevisionsDebugMode", "YES"])
        ));
    }

    #[test]
    fn json_reads_like_go() {
        let v = serde_json::json!({"a": 1.0, "b": 0.5, "c": 1e-7, "d": 1e21, "e": "<&>", "f": [], "g": 3});
        assert_eq!(
            go_json(&v, false).unwrap(),
            "{\"a\":1,\"b\":0.5,\"c\":1e-7,\"d\":1e+21,\"e\":\"\\u003c\\u0026\\u003e\",\"f\":[],\"g\":3}\n"
        );
        assert_eq!(
            go_json(&serde_json::json!({"a": [1]}), true).unwrap(),
            "{\n  \"a\": [\n    1\n  ]\n}\n"
        );
        assert_eq!(message(&Error::Message("not_found: run x".into())), "run x");
    }
}
