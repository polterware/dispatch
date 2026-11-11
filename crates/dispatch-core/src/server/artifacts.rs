//! DispatchService: the images runs made, as the gallery lists and deletes them.

use super::{Core, RpcError, RpcResult};
use crate::{
    artifacts, proto as pb,
    types::{self, Artifact, ArtifactFilter, Store as _},
};
use std::path::Path;

impl Core {
    /// The images the filters match, newest first, with how many match in all.
    pub(crate) async fn list_artifacts(
        &self,
        m: pb::ListArtifactsRequest,
    ) -> RpcResult<pb::ListArtifactsResponse> {
        let origin = m.origin.trim().to_ascii_lowercase();
        if !origin.is_empty() && origin != types::ORIGIN_APP && origin != types::ORIGIN_DELEGATED {
            return Err(RpcError::invalid(
                "origin must be app or delegated, or empty for both",
            ));
        }
        let filter = ArtifactFilter {
            origin,
            repo: m.repo.trim().into(),
            agent: m.agent.trim().into(),
            conversation_id: m.conversation_id.trim().into(),
            query: m.query.trim().into(),
            limit: i64::from(m.limit),
            offset: i64::from(m.offset),
            ..ArtifactFilter::default()
        };
        let (artifacts, total) = self
            .store
            .list_artifacts(&filter)
            .map_err(RpcError::internal)?;
        let base = self.artifacts_dir();
        Ok(pb::ListArtifactsResponse {
            artifacts: artifacts.iter().map(|a| convert(a, &base)).collect(),
            total: i32::try_from(total).unwrap_or(i32::MAX),
        })
    }

    /// Removes one artifact and the copy the run kept.
    pub(crate) async fn delete_artifact(
        &self,
        m: pb::DeleteArtifactRequest,
    ) -> RpcResult<pb::DeleteArtifactResponse> {
        let id = m.id.trim();
        if id.is_empty() {
            return Err(RpcError::invalid("artifact id is required"));
        }
        let artifact = self
            .store
            .delete_artifact(id)
            .map_err(RpcError::internal)?
            .ok_or_else(|| RpcError::not_found(format!("unknown artifact {id}")))?;
        let base = self.artifacts_dir();
        let removed = tokio::task::spawn_blocking(move || remove_copy(&base, &artifact))
            .await
            .map_err(|e| RpcError::internal(e.to_string()))?;
        if let Err(e) = removed {
            eprintln!("remove artifact {id}: {e}");
        }
        Ok(pb::DeleteArtifactResponse {})
    }
}

/// An artifact as the wire carries it, with the copy's absolute path.
fn convert(a: &Artifact, base: &Path) -> pb::DispatchArtifact {
    let path = artifacts::copy_path(base, &a.run_id, &a.rel_path)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    pb::DispatchArtifact {
        id: a.id.clone(),
        run_id: a.run_id.clone(),
        workspace_id: a.project_id.clone(),
        conversation_id: a.conversation_id.clone(),
        caller_agent: a.caller_agent.clone(),
        agent: a.agent.clone(),
        model: a.model.clone(),
        repo: a.repo.clone(),
        path,
        rel_path: a.rel_path.clone(),
        kind: a.kind.clone(),
        bytes: a.bytes,
        width: a.width,
        height: a.height,
        created_at: super::wire_time(&a.created_at),
        prompt: a.prompt.clone(),
        origin: a.origin().into(),
    }
}

/// Removes an artifact's copy and the folders it leaves empty, up to its run's.
fn remove_copy(base: &Path, a: &Artifact) -> std::io::Result<()> {
    let Some(path) = artifacts::copy_path(base, &a.run_id, &a.rel_path) else {
        return Ok(());
    };
    if path.exists() {
        std::fs::remove_file(&path)?;
    }
    let run_dir = base.join(&a.run_id);
    let mut parent = path.parent();
    while let Some(dir) = parent {
        if dir == run_dir || !dir.starts_with(&run_dir) {
            break;
        }
        if std::fs::remove_dir(dir).is_err() {
            break;
        }
        parent = dir.parent();
    }
    // An emptied run folder goes, as one that never held an image does.
    let _ = std::fs::remove_dir(&run_dir);
    Ok(())
}
