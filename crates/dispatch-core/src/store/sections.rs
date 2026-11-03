use super::{Store, db, format_time, now, parse_time};
use crate::{
    Error,
    types::{Section, SectionItem, new_id},
};
use rusqlite::OptionalExtension;

pub const ERR_UNKNOWN_SECTION: &str = "no such section";
pub const ERR_INVALID_SECTION: &str = "invalid section or item";

fn unknown() -> Error {
    Error::Message(ERR_UNKNOWN_SECTION.into())
}

fn invalid() -> Error {
    Error::Message(ERR_INVALID_SECTION.into())
}

pub fn is_unknown_section(e: &Error) -> bool {
    matches!(e, Error::Message(m) if m == ERR_UNKNOWN_SECTION)
}

pub fn is_invalid_section(e: &Error) -> bool {
    matches!(e, Error::Message(m) if m == ERR_INVALID_SECTION)
}

impl Store {
    /// Reads the global sections and their ordered items in one snapshot.
    pub fn list_sections(&self) -> Result<Vec<Section>, Error> {
        let conn = self.conn();
        let mut stmt = conn
            .prepare(
                "SELECT s.id, s.name, s.position, s.created_at, i.kind, i.ref, i.position
        FROM dispatch_sections s LEFT JOIN dispatch_section_items i ON i.section_id = s.id
        ORDER BY s.position, s.created_at, s.id, i.position, i.kind, i.ref",
            )
            .map_err(db)?;
        let mut rows = stmt.query([]).map_err(db)?;
        let mut out: Vec<Section> = Vec::new();
        while let Some(r) = rows.next().map_err(db)? {
            let id: String = r.get(0).map_err(db)?;
            if out.last().is_none_or(|s| s.id != id) {
                out.push(Section {
                    id,
                    name: r.get(1).map_err(db)?,
                    position: r.get(2).map_err(db)?,
                    created_at: parse_time(&r.get::<_, String>(3).map_err(db)?),
                    items: Vec::new(),
                });
            }
            let kind: Option<String> = r.get(4).map_err(db)?;
            if let (Some(kind), Some(last)) = (kind, out.last_mut()) {
                let item_ref: Option<String> = r.get(5).map_err(db)?;
                let position: Option<i64> = r.get(6).map_err(db)?;
                last.items.push(SectionItem {
                    kind,
                    r#ref: item_ref.unwrap_or_default(),
                    position: position.unwrap_or_default(),
                });
            }
        }
        Ok(out)
    }

    /// Creates when the id is empty, otherwise renames and reorders an existing section.
    pub fn save_section(&self, section: &Section) -> Result<Section, Error> {
        let name = section.name.trim().to_string();
        if name.is_empty() || section.position < 0 {
            return Err(invalid());
        }
        let id = if section.id.is_empty() {
            let id = new_id();
            self.conn()
                .execute(
                    "INSERT INTO dispatch_sections(id, name, position, created_at) VALUES (?, ?, ?, ?)",
                    rusqlite::params![id, name, section.position, format_time(&now())],
                )
                .map_err(db)?;
            id
        } else {
            let created: Option<String> = self
                .conn()
                .query_row(
                    "UPDATE dispatch_sections SET name = ?, position = ? WHERE id = ? RETURNING created_at",
                    rusqlite::params![name, section.position, section.id],
                    |r| r.get(0),
                )
                .optional()
                .map_err(db)?;
            created.ok_or_else(unknown)?;
            section.id.clone()
        };
        self.list_sections()?
            .into_iter()
            .find(|s| s.id == id)
            .ok_or_else(unknown)
    }

    pub fn delete_section(&self, id: &str) -> Result<(), Error> {
        if id.trim().is_empty() {
            return Err(invalid());
        }
        let n = self
            .conn()
            .execute("DELETE FROM dispatch_sections WHERE id = ?", [id])
            .map_err(db)?;
        if n == 0 {
            return Err(unknown());
        }
        Ok(())
    }

    /// Appends an item to a section; an empty `section_id` removes its assignment.
    /// The primary key makes an item belong to at most one section across all workspaces.
    pub fn move_to_section(
        &self,
        kind: &str,
        item_ref: &str,
        section_id: &str,
    ) -> Result<(), Error> {
        if (kind != "conversation" && kind != "run") || item_ref.trim().is_empty() {
            return Err(invalid());
        }
        let conn = self.conn();
        if section_id.is_empty() {
            conn.execute(
                "DELETE FROM dispatch_section_items WHERE kind = ? AND ref = ?",
                [kind, item_ref],
            )
            .map_err(db)?;
            return Ok(());
        }
        // One statement validates the destination and atomically appends or moves it.
        let n = conn
            .execute(
                "INSERT INTO dispatch_section_items(section_id, kind, ref, position)
        SELECT id, ?, ?, COALESCE((SELECT MAX(position) + 1 FROM dispatch_section_items WHERE section_id = ?), 0)
        FROM dispatch_sections WHERE id = ?
        ON CONFLICT(kind, ref) DO UPDATE SET section_id = excluded.section_id, position = excluded.position
        WHERE dispatch_section_items.section_id != excluded.section_id",
                [kind, item_ref, section_id, section_id],
            )
            .map_err(db)?;
        if n != 0 {
            return Ok(());
        }
        let exists = conn
            .query_row(
                "SELECT 1 FROM dispatch_sections WHERE id = ?",
                [section_id],
                |r| r.get::<_, i64>(0),
            )
            .optional()
            .map_err(db)?;
        exists.map(|_| ()).ok_or_else(unknown)
    }
}
