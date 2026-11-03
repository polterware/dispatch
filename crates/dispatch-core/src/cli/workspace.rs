//! `dispatch workspace`: lists, adds, and removes workspaces. Polterware's
//! projects are listed while Polterware answers; a folder is added on its own.

use super::{CliResult, Flags, dial, usage_err, write_json};
use crate::proto as pb;
use serde_json::{Map, Value};
use std::path::Path;

const WORKSPACE_USAGE: &str = "usage: dispatch workspace ls [--json]
       dispatch workspace add <folder> [--name <name>]
       dispatch workspace rm <id>";

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

pub(super) async fn run_workspace(db_path: &Path, args: &[String]) -> CliResult {
    let ls = ["ls".to_owned()];
    let args = if args.is_empty() { &ls[..] } else { args };
    let (sub, rest) = (&args[0], &args[1..]);
    let mut f = Flags::new(format!("workspace {sub}"));
    f.bool("json", false, "print JSON").string(
        "name",
        "",
        "workspace name (default: the folder's)",
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
                println!("no workspaces; `dispatch run` in a folder adds it");
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
                return Err(usage_err(WORKSPACE_USAGE));
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
            println!("workspace {} ({})", ws.id, ws.name);
            Ok(())
        }
        "rm" | "remove" => {
            let [id] = positional.as_slice() else {
                return Err(usage_err(WORKSPACE_USAGE));
            };
            c.workspaces
                .remove_workspace(&pb::RemoveWorkspaceRequest { id: id.clone() })
                .await?;
            println!("removed {id}");
            Ok(())
        }
        _ => Err(usage_err(WORKSPACE_USAGE)),
    }
}
