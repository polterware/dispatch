//! The status a run's bot draws: its run status, with a merge as a state of its own.
//!
//! A run that completed and merged says so with a different face from one that merely
//! completed, the way the sidebar and the run page read it.

/// The status a bot draws for a run: `"merged"` for a completed run that has a merge time,
/// else the run's own status. Every bot that shows a run passes this in.
pub fn avatar_status<'a>(status: &'a str, merged_at: &str) -> &'a str {
    let completed = matches!(
        status.trim().to_ascii_lowercase().as_str(),
        "completed" | "succeeded"
    );
    if completed && !merged_at.trim().is_empty() {
        "merged"
    } else {
        status
    }
}

#[cfg(test)]
mod tests {
    use super::avatar_status;

    #[test]
    fn every_run_status_keeps_its_own_face_and_a_merge_stands_apart() {
        for status in [
            "pending",
            "running",
            "failed",
            "cancelled",
            "timed_out",
            "orphaned",
        ] {
            assert_eq!(avatar_status(status, "2026-11-04T10:05:00Z"), status);
        }
        assert_eq!(avatar_status("completed", ""), "completed");
        assert_eq!(avatar_status("completed", "2026-11-04T10:05:00Z"), "merged");
        assert_eq!(avatar_status("COMPLETED", "2026-11-04T10:05:00Z"), "merged");
        assert_eq!(avatar_status("succeeded", "2026-11-04T10:05:00Z"), "merged");
        assert_eq!(avatar_status("merged", ""), "merged");
        assert_eq!(avatar_status("", ""), "");
    }
}
