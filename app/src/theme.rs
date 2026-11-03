//! The design tokens come from `polterware-theme`; Dispatch adds its root size and the tone of
//! its status words.

pub use polterware_ui::theme::*;

/// Dispatch's root font size: its Tailwind spacing (`p-4`, `gap-1.5`) is in rem.
pub const REM: f32 = 13.0;

/// A Tailwind spacing step (`0.25rem`) at Dispatch's root size: `tw(4.0)` is `p-4`, 13px. The
/// controls read the same size from [`set_root_rem`], which `run` sets to [`REM`].
pub fn tw(steps: f32) -> f32 {
    steps * REM / 4.0
}

/// Dispatch's status words: a run's, a worktree's, and a quota pace's, in the same
/// tone buckets as `dispatchStatusTone` in the Electron app.
pub fn dispatch_status_tone(status: &str) -> StatusTone {
    let normalized = status.trim().to_ascii_lowercase();
    if matches!(
        normalized.as_str(),
        "merged"
            | "completed"
            | "passed"
            | "clean"
            | "ok"
            | "low"
            | "high"
            | "on pace"
            | "behind"
            | "ready"
    ) {
        return StatusTone::Ok;
    }
    if matches!(
        normalized.as_str(),
        "running" | "started" | "starting" | "queued" | "pending" | "active"
    ) {
        return StatusTone::Syncing;
    }
    if matches!(
        normalized.as_str(),
        "failed" | "error" | "blocked" | "orphaned"
    ) {
        return StatusTone::Danger;
    }
    if matches!(
        normalized.as_str(),
        "cancelled" | "canceled" | "timed_out" | "abandoned" | "dirty" | "medium" | "ahead"
    ) {
        return StatusTone::Warn;
    }
    StatusTone::Muted
}

#[cfg(test)]
mod tests {
    use super::{StatusTone, dispatch_status_tone};

    #[test]
    fn status_tones_match_the_electron_mapping() {
        assert_eq!(dispatch_status_tone("running"), StatusTone::Syncing);
        assert_eq!(dispatch_status_tone("Completed"), StatusTone::Ok);
        assert_eq!(dispatch_status_tone("failed"), StatusTone::Danger);
        assert_eq!(dispatch_status_tone("timed_out"), StatusTone::Warn);
        assert_eq!(dispatch_status_tone("cancelled"), StatusTone::Warn);
        assert_eq!(dispatch_status_tone("mystery"), StatusTone::Muted);
    }
}
