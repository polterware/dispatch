//! `dispatch workspace` (and `dispatch repo`, the app's word for it): lists, adds, and
//! removes workspaces. Polterware's projects are listed while Polterware answers; a folder
//! is added as the git repository holding it (its main checkout), else as itself.

use super::{CliResult, Flags, dial, usage_err, write_json};
use crate::proto as pb;
use serde_json::{Map, Value};
use std::path::Path;

const WORKSPACE_USAGE: &str = "usage: dispatch workspace ls [--json]
       dispatch workspace add <folder> [--name <name>]
       dispatch workspace rm <id>";

const REPO_USAGE: &str = "usage: dispatch repo ls [--json]
       dispatch repo add <folder> [--name <name>]   (the git repository holding it)
       dispatch repo rm <id>   (deletes its runs and the worktrees Dispatch cut for them)";

/// The word the command was called by: `workspace`, which agents and Polterware parse, keeps
/// the Go binary's text; `repo` says the same in the app's words. The JSON is the same.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Noun {
    Workspace,
    Repository,
}

impl Noun {
    fn usage(self) -> &'static str {
        match self {
            Self::Workspace => WORKSPACE_USAGE,
            Self::Repository => REPO_USAGE,
        }
    }

    fn word(self) -> &'static str {
        match self {
            Self::Workspace => "workspace",
            Self::Repository => "repository",
        }
    }
}

/// A workspace as the Go binary printed its message: snake_case keys, empty
/// fields left out.
fn workspace_json(ws: &pb::Workspace) -> Value {
    let mut m = Map::new();
    for (k, v) in [
        ("id", &ws.id),
        ("name", &ws.name),
        ("path", &ws.path),
        ("polterware_project_id", &ws.polterware_project_id),
        ("created_at", &ws.created_at),
    ] {
        if !v.is_empty() {
            m.insert(k.into(), Value::String(v.clone()));
        }
    }
    Value::Object(m)
}

pub(super) async fn run_workspace(db_path: &Path, args: &[String], noun: Noun) -> CliResult {
    let ls = ["ls".to_owned()];
    let args = if args.is_empty() { &ls[..] } else { args };
    let (sub, rest) = (&args[0], &args[1..]);
    let mut f = Flags::new(format!("{} {sub}", noun.word()));
    f.bool("json", false, "print JSON").string(
        "name",
        "",
        if noun == Noun::Workspace {
            "workspace name (default: the folder's)"
        } else {
            "repository name (default: its folder's)"
        },
    );
    let positional = f.parse_interspersed(rest)?;
    let c = dial(db_path).await?;
    match sub.as_str() {
        "ls" | "list" => {
            let res = c
                .workspaces
                .list_workspaces(&pb::ListWorkspacesRequest {})
                .await?;
            if f.flag("json") {
                let mut m = Map::new();
                if !res.workspaces.is_empty() {
                    m.insert(
                        "workspaces".into(),
                        Value::Array(res.workspaces.iter().map(workspace_json).collect()),
                    );
                }
                if res.linked {
                    m.insert("linked".into(), Value::Bool(true));
                }
                write_json(&Value::Object(m))?;
                return Ok(());
            }
            if !res.linked {
                println!("Polterware is not running: only folders are listed");
            }
            if res.workspaces.is_empty() {
                match noun {
                    Noun::Workspace => {
                        println!("no workspaces; `dispatch run` in a folder adds it")
                    }
                    Noun::Repository => {
                        println!("no repositories; `dispatch run` in one adds it")
                    }
                }
            }
            for ws in &res.workspaces {
                let place = if ws.polterware_project_id.is_empty() {
                    ws.path.as_str()
                } else {
                    "Polterware project"
                };
                println!("{:<16}  {:<24}  {place}", ws.id, ws.name);
            }
            Ok(())
        }
        "add" => {
            let [folder] = positional.as_slice() else {
                return Err(usage_err(noun.usage()));
            };
            let path = std::path::absolute(folder)?;
            let res = c
                .workspaces
                .add_workspace(&pb::AddWorkspaceRequest {
                    path: path.to_string_lossy().into_owned(),
                    name: f.str("name"),
                })
                .await?;
            let ws = res.workspace.unwrap_or_default();
            println!("{} {} ({})", noun.word(), ws.id, ws.name);
            Ok(())
        }
        "rm" | "remove" => {
            let [id] = positional.as_slice() else {
                return Err(usage_err(noun.usage()));
            };
            let res = c
                .workspaces
                .remove_workspace(&pb::RemoveWorkspaceRequest { id: id.clone() })
                .await?;
            let runs = match res.deleted_runs {
                0 => String::new(),
                1 => " and its run".to_string(),
                n => format!(" and its {n} runs"),
            };
            let worktrees = match res.removed_worktrees {
                0 => String::new(),
                1 => " (1 worktree)".to_string(),
                n => format!(" ({n} worktrees)"),
            };
            println!("removed {id}{runs}{worktrees}");
            Ok(())
        }
        _ => Err(usage_err(noun.usage())),
    }
}
