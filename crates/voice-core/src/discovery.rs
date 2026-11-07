//! The discovery file: how clients find a running engine and authenticate.
//!
//! The engine binds `127.0.0.1` on a free port, generates a random token, and
//! writes this file with mode 0600 (atomically: temp file + rename). Clients
//! read it, check `GET /v1/health`, and send the token as
//! `Authorization: Bearer <token>` or, for browser WebSockets that cannot set
//! headers, as the `token` query parameter of `/v1/ws`. The engine removes the
//! file on graceful shutdown; a stale file whose pid is dead or whose health
//! check fails means no engine is running.

use serde::{Deserialize, Serialize};
use std::io;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EngineInfo {
    pub protocol: u32,
    pub engine_version: String,
    pub pid: u32,
    pub port: u16,
    pub token: String,
}

impl EngineInfo {
    pub fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }
}

pub fn read(path: &Path) -> io::Result<EngineInfo> {
    let bytes = std::fs::read(path)?;
    serde_json::from_slice(&bytes).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// Writes the file atomically with owner-only permissions.
pub fn write(path: &Path, info: &EngineInfo) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
    let body = serde_json::to_vec_pretty(info)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    {
        use std::io::Write;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&tmp)?;
        file.write_all(&body)?;
        file.sync_all()?;
    }
    std::fs::rename(&tmp, path)
}

/// Removes the file only if it still belongs to `pid`.
pub fn remove_if_owned(path: &Path, pid: u32) -> io::Result<()> {
    match read(path) {
        Ok(info) if info.pid == pid => std::fs::remove_file(path),
        Ok(_) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(_) => std::fs::remove_file(path),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_ownership() {
        let dir = std::env::temp_dir().join(format!("voice-core-test-{}", std::process::id()));
        let path = dir.join("engine.json");
        let info = EngineInfo {
            protocol: 1,
            engine_version: "0.1.0".into(),
            pid: 42,
            port: 1234,
            token: "t".into(),
        };
        write(&path, &info).unwrap();
        assert_eq!(read(&path).unwrap(), info);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        remove_if_owned(&path, 7).unwrap();
        assert!(path.exists());
        remove_if_owned(&path, 42).unwrap();
        assert!(!path.exists());
        let _ = std::fs::remove_dir_all(dir);
    }
}
