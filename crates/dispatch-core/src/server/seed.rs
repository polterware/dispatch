//! The first start's copy of what Polterware kept before Dispatch became its
//! own app.

use super::Core;
use crate::{Error, config};
use std::path::Path;

/// Marks a state folder whose first start already looked for Polterware's
/// dispatch state, so a later reset is not undone.
const SEEDED_FILE: &str = "seeded";

impl Core {
    /// Runs once per state folder: it copies the tier table and the routing
    /// policy (config.json) Polterware kept. Runs and history are not imported.
    pub fn seed_from_polterware(&self, polterware_db: &Path) -> Result<usize, Error> {
        let state = self.state_dir();
        let marker = state.join(SEEDED_FILE);
        if marker.exists() || polterware_db.as_os_str().is_empty() {
            return Ok(0);
        }
        let old = polterware_db
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("dispatch");
        let name = config::policy_config_path(Path::new(""));
        copy_if_missing(&old.join(&name), &state.join(&name))?;
        let tiers = self.store.import_tiers(&polterware_db.to_string_lossy())?;
        create_private_dir(&state)?;
        write_private(&marker, b"")?;
        Ok(tiers)
    }
}

fn create_private_dir(dir: &Path) -> Result<(), Error> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    Ok(())
}

fn write_private(path: &Path, body: &[u8]) -> Result<(), Error> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(body)?;
    Ok(())
}

fn copy_if_missing(from: &Path, to: &Path) -> Result<(), Error> {
    if to.exists() {
        return Ok(());
    }
    let body = match std::fs::read(from) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.into()),
    };
    if let Some(parent) = to.parent() {
        create_private_dir(parent)?;
    }
    write_private(to, &body)
}
