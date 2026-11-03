//! The SQLite store: migrations, runs, events, sections, usage, route decisions.
//!
//! Ported from `dispatch/backend/providers/sqlite`.

use crate::Error;
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

mod dispatch;
mod sections;

pub use sections::{
    ERR_INVALID_SECTION, ERR_UNKNOWN_SECTION, is_invalid_section, is_unknown_section,
};

/// Applied in name order, each once, recorded in `PRAGMA user_version`.
const MIGRATIONS: [(&str, &str); 6] = [
    (
        "0001_baseline.sql",
        include_str!("migrations/0001_baseline.sql"),
    ),
    (
        "0002_drop_verify.sql",
        include_str!("migrations/0002_drop_verify.sql"),
    ),
    (
        "0003_run_usage.sql",
        include_str!("migrations/0003_run_usage.sql"),
    ),
    (
        "0004_run_subagent.sql",
        include_str!("migrations/0004_run_subagent.sql"),
    ),
    (
        "0005_run_role.sql",
        include_str!("migrations/0005_run_role.sql"),
    ),
    (
        "0006_conversations_sections.sql",
        include_str!("migrations/0006_conversations_sections.sql"),
    ),
];

pub const ERR_NO_WORKSPACE: &str = "no such workspace";

pub fn is_no_workspace(e: &Error) -> bool {
    matches!(e, Error::Message(m) if m == ERR_NO_WORKSPACE)
}

pub struct Store {
    conn: Mutex<Connection>,
}

/// Where runs belong; see the workspaces table.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Workspace {
    pub id: String,
    pub name: String,
    pub path: String,
    pub polterware_project_id: String,
    pub created_at: String,
}

pub(crate) fn db(e: rusqlite::Error) -> Error {
    Error::Message(e.to_string())
}

impl Store {
    /// Opens the database at `path` (or `:memory:`) and applies the migrations it has not run.
    pub fn new(path: &str) -> Result<Store, Error> {
        let mut conn = if path == ":memory:" {
            Connection::open_in_memory().map_err(db)?
        } else {
            let dir = match Path::new(path).parent() {
                Some(p) if !p.as_os_str().is_empty() => p,
                _ => Path::new("."),
            };
            create_private_dir(dir)?;
            Connection::open(path).map_err(db)?
        };
        conn.execute_batch(
            "PRAGMA foreign_keys = ON; PRAGMA journal_mode = WAL; PRAGMA busy_timeout = 5000;",
        )
        .map_err(db)?;
        migrate(&mut conn)?;
        Ok(Store {
            conn: Mutex::new(conn),
        })
    }

    pub(crate) fn conn(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Runs `f` on the connection, for reads the typed API does not cover.
    pub fn with_conn<R>(&self, f: impl FnOnce(&Connection) -> R) -> R {
        f(&self.conn())
    }

    pub fn save_workspace(&self, w: &Workspace) -> Result<(), Error> {
        if w.id.trim().is_empty() {
            return Err(Error::Message("workspace id is required".into()));
        }
        let created = if w.created_at.is_empty() {
            now()
        } else {
            w.created_at.clone()
        };
        self.conn()
            .execute(
                "INSERT INTO workspaces (id, name, path, polterware_project_id, created_at) VALUES (?, ?, ?, ?, ?)
		ON CONFLICT(id) DO UPDATE SET name = excluded.name, path = excluded.path, polterware_project_id = excluded.polterware_project_id",
                rusqlite::params![w.id, w.name, w.path, w.polterware_project_id, format_time(&created)],
            )
            .map_err(db)?;
        Ok(())
    }

    pub fn get_workspace(&self, id: &str) -> Result<Workspace, Error> {
        let conn = self.conn();
        let row = conn
            .query_row(
                "SELECT id, name, path, polterware_project_id, created_at FROM workspaces WHERE id = ?",
                [id],
                scan_workspace,
            )
            .optional()
            .map_err(db)?;
        row.ok_or_else(|| Error::Message(ERR_NO_WORKSPACE.into()))
    }

    /// The standalone workspace whose folder holds `dir`: the longest path
    /// that is `dir` or one of its parents.
    pub fn workspace_at(&self, dir: &str) -> Result<Workspace, Error> {
        let items = self.list_workspaces()?;
        let dir = clean_path(dir);
        let mut best = Workspace::default();
        for w in items {
            if w.path.is_empty() {
                continue;
            }
            let p = clean_path(&w.path);
            if (dir == p || dir.starts_with(&format!("{p}/"))) && p.len() > best.path.len() {
                best = w;
            }
        }
        if best.id.is_empty() {
            return Err(Error::Message(ERR_NO_WORKSPACE.into()));
        }
        Ok(best)
    }

    pub fn list_workspaces(&self) -> Result<Vec<Workspace>, Error> {
        let conn = self.conn();
        let mut stmt = conn
            .prepare(
                "SELECT id, name, path, polterware_project_id, created_at FROM workspaces ORDER BY name COLLATE NOCASE, id",
            )
            .map_err(db)?;
        let rows = stmt.query_map([], scan_workspace).map_err(db)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(db)
    }

    pub fn delete_workspace(&self, id: &str) -> Result<(), Error> {
        self.conn()
            .execute("DELETE FROM workspaces WHERE id = ?", [id])
            .map_err(db)?;
        Ok(())
    }

    /// Copies the tier table of another dispatch database when this store has
    /// none. The other database is opened read-only; a missing file or table
    /// imports nothing.
    pub fn import_tiers(&self, path: &str) -> Result<usize, Error> {
        let have: i64 = self
            .conn()
            .query_row("SELECT COUNT(*) FROM dispatch_tiers", [], |r| r.get(0))
            .map_err(db)?;
        if have > 0 || std::fs::metadata(path).is_err() {
            return Ok(0);
        }
        let Ok(src) = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY) else {
            return Ok(0);
        };
        let _ = src.busy_timeout(std::time::Duration::from_millis(5000));
        let has_table = src
            .query_row(
                "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'dispatch_tiers'",
                [],
                |r| r.get::<_, String>(0),
            )
            .is_ok();
        if !has_table {
            return Ok(0);
        }
        let tiers = dispatch::list_tiers_on(&src)?;
        if tiers.is_empty() {
            return Ok(0);
        }
        let n = tiers.len();
        crate::types::Store::replace_tiers(self, &tiers)?;
        Ok(n)
    }
}

fn migrate(conn: &mut Connection) -> Result<(), Error> {
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .map_err(db)?;
    for (name, body) in MIGRATIONS {
        let n: i64 = name
            .split('_')
            .next()
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| Error::Message(format!("migration {name}: no number")))?;
        if n <= version {
            continue;
        }
        let tx = conn.transaction().map_err(db)?;
        tx.execute_batch(body)
            .map_err(|e| Error::Message(format!("migration {name}: {e}")))?;
        tx.execute_batch(&format!("PRAGMA user_version = {n}"))
            .map_err(db)?;
        tx.commit().map_err(db)?;
    }
    Ok(())
}

fn create_private_dir(dir: &Path) -> Result<(), Error> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    Ok(())
}

fn scan_workspace(r: &rusqlite::Row<'_>) -> rusqlite::Result<Workspace> {
    Ok(Workspace {
        id: r.get(0)?,
        name: r.get(1)?,
        path: r.get(2)?,
        polterware_project_id: r.get(3)?,
        created_at: parse_time(&r.get::<_, String>(4)?),
    })
}

/// Lexical path cleaning like Go's `filepath.Clean` on Unix.
fn clean_path(p: &str) -> String {
    let rooted = p.starts_with('/');
    let mut parts: Vec<&str> = Vec::new();
    for seg in p.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                if parts.last().is_some_and(|l| *l != "..") {
                    parts.pop();
                } else if !rooted {
                    parts.push("..");
                }
            }
            s => parts.push(s),
        }
    }
    let joined = parts.join("/");
    match (rooted, joined.is_empty()) {
        (true, _) => format!("/{joined}"),
        (false, true) => ".".into(),
        (false, false) => joined,
    }
}

const TIME_FORMAT: &str = "%Y-%m-%dT%H:%M:%SZ";

pub(crate) fn now() -> String {
    chrono::Utc::now().format(TIME_FORMAT).to_string()
}

/// Normalizes a time to UTC RFC 3339 seconds; empty or unparsable is zero (empty).
pub(crate) fn format_time(t: &str) -> String {
    parse_time(t)
}

pub(crate) fn parse_time(value: &str) -> String {
    match chrono::DateTime::parse_from_rfc3339(value) {
        Ok(t) => t
            .with_timezone(&chrono::Utc)
            .format(TIME_FORMAT)
            .to_string(),
        Err(_) => String::new(),
    }
}
