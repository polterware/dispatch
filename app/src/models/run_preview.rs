//! The live preview on a run's card: the end of what its agent printed, as a few lines.
//!
//! The tail comes from the same pipeline as the run page ([`StreamedLog`] over the core's
//! `ReadRunEvents` page at the run's end, falling back to `Result` output when a finished run
//! has none in its stream, then [`build_transcript`]), reduced here to the last lines of its
//! entries. [`PreviewCache`] decides, per run id, when a card needs the core again: once for a
//! finished run, on every poll for an active one, and never for a card that is not on screen,
//! since the page asks only for the ones its virtual list draws.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::{Duration, Instant};

use crate::client::{CoreClient, ProtoI64, ReadRunEventsRequest, ResultRequest, WatchRunResponse};
use crate::models::transcript::{
    LogEvent, PagedEvent, StreamedLog, TranscriptEntry, build_transcript, fold_repeats, strip_bold,
};

/// How many lines a card keeps; the card shows as many of them as fit.
pub const PREVIEW_LINES: usize = 16;
/// How much of the run's end one read takes: enough for a dozen lines of agent output.
const PREVIEW_BYTES: i64 = 64 * 1024;
/// Wider read when a single long line filled [`PREVIEW_BYTES`].
const RETRY_BYTES: i64 = 256 * 1024;
/// A line is cut at this many characters; the card ends it with `…` at its own width.
const LINE_CHARS: usize = 240;
/// The marker before a step's line.
const STEP_MARK: &str = "› ";

/// One line of the preview.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreviewLine {
    pub text: String,
    /// A step, a thought, or a note around the agent's words, drawn muted.
    pub muted: bool,
}

/// The tail of a run's output, and the mark that says whether it moved.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RunPreview {
    pub lines: Vec<PreviewLine>,
    /// The last event's seq and the text bytes read, so a poll that read the same thing changes
    /// nothing.
    pub fingerprint: (i64, usize),
}

/// Up to `limit` of the last non-empty lines of `text`, oldest first.
fn text_tail(text: &str, limit: usize, muted: bool, prefix: &str) -> Vec<PreviewLine> {
    let mut lines: Vec<PreviewLine> = text
        .lines()
        .rev()
        .map(|line| line.trim_end().replace('\t', "    "))
        .filter(|line| !line.trim().is_empty())
        .take(limit)
        .map(|line| PreviewLine {
            text: format!("{prefix}{}", clip(&line)),
            muted,
        })
        .collect();
    lines.reverse();
    lines
}

fn clip(line: &str) -> String {
    match line.char_indices().nth(LINE_CHARS) {
        Some((end, _)) => line[..end].to_string(),
        None => line.to_string(),
    }
}

/// The lines one entry reads as, at most `limit` of its last.
fn entry_lines(entry: &TranscriptEntry, repeats: Option<usize>, limit: usize) -> Vec<PreviewLine> {
    match entry {
        TranscriptEntry::Step { text, output, .. } => {
            let head = match repeats {
                Some(count) => format!("{text} ×{count}"),
                None => text.clone(),
            };
            let mut lines = text_tail(&head, 1, true, STEP_MARK);
            if let Some(output) = output {
                lines.extend(text_tail(output, limit, true, "  "));
            }
            let skip = lines.len().saturating_sub(limit);
            lines.split_off(skip)
        }
        TranscriptEntry::Thinking { text } => text_tail(&strip_bold(text), limit, true, ""),
        TranscriptEntry::Message { text } | TranscriptEntry::Output { text } => {
            text_tail(text, limit, false, "")
        }
        TranscriptEntry::Event { event, text } => {
            text_tail(text, limit, true, &format!("[{event}] "))
        }
        TranscriptEntry::Switch { label, .. } => text_tail(label, 1, true, ""),
    }
}

/// The last `limit` lines of a transcript, oldest first: the agent's words and the steps it
/// took, with repeated polling steps folded and a step's own output after it.
pub fn tail_lines(entries: &[TranscriptEntry], limit: usize) -> Vec<PreviewLine> {
    let mut tail: Vec<PreviewLine> = Vec::new();
    for (index, repeats) in fold_repeats(entries).into_iter().rev() {
        if tail.len() >= limit {
            break;
        }
        let mut lines = entry_lines(&entries[index], repeats, limit - tail.len());
        lines.append(&mut tail);
        tail = lines;
    }
    tail
}

/// The preview of a page of a run's events and optional stored output, plus whether the
/// transcript held a `Message` entry.
pub fn preview_from_log_parts(
    events: &[WatchRunResponse],
    stored: &str,
    limit: usize,
) -> (RunPreview, bool) {
    let event_bytes: usize = events.iter().map(|event| event.text.len()).sum();
    let fingerprint = (
        events.last().map_or(0, |event| event.seq.0),
        event_bytes.saturating_add(stored.len()),
    );
    let paged: Vec<PagedEvent> = events
        .iter()
        .map(|event| PagedEvent {
            event: LogEvent {
                kind: event.kind.clone(),
                text: event.text.clone(),
            },
            seq: event.seq.0,
            offset: u64::try_from(event.text_offset.0).unwrap_or(0),
        })
        .collect();
    let mut streamed = StreamedLog::default();
    streamed.extend_paged(&paged);
    let log = streamed.join(stored);
    let entries = build_transcript(&log, stored);
    let has_message = entries
        .iter()
        .any(|entry| matches!(entry, TranscriptEntry::Message { .. }));
    (
        RunPreview {
            lines: tail_lines(&entries, limit),
            fingerprint,
        },
        has_message,
    )
}

/// The preview of a page of a run's events read from its end.
pub fn preview_from_events(events: Vec<WatchRunResponse>, limit: usize) -> RunPreview {
    preview_from_log_parts(&events, "", limit).0
}

/// Reads the end of run `id`'s output. `None` when the core did not answer, so the card keeps
/// what it had.
pub fn fetch_preview(client: &CoreClient, id: &str, active: bool) -> Option<RunPreview> {
    let mut page = client
        .read_run_events(&ReadRunEventsRequest {
            id: id.to_string(),
            before_seq: ProtoI64(0),
            before_offset: ProtoI64(0),
            max_bytes: ProtoI64(PREVIEW_BYTES),
        })
        .ok()?;
    let (mut preview, mut has_message) = preview_from_log_parts(&page.events, "", PREVIEW_LINES);
    if page.more
        && preview.lines.is_empty()
        && let Ok(wider) = client.read_run_events(&ReadRunEventsRequest {
            id: id.to_string(),
            before_seq: ProtoI64(0),
            before_offset: ProtoI64(0),
            max_bytes: ProtoI64(RETRY_BYTES),
        })
    {
        page = wider;
        (preview, has_message) = preview_from_log_parts(&page.events, "", PREVIEW_LINES);
    }
    if !active
        && (preview.lines.is_empty() || !has_message)
        && let Ok(response) = client.result(&ResultRequest {
            id: id.to_string(),
            section: "output".to_string(),
            offset: 0,
            limit: 0,
        })
        && !response.content.trim().is_empty()
    {
        (preview, _) = preview_from_log_parts(&page.events, &response.content, PREVIEW_LINES);
    }
    Some(preview)
}

#[derive(Default)]
struct Entry {
    preview: Option<Rc<RunPreview>>,
    fetched: Option<Instant>,
    inflight: bool,
    /// Whether the last fetch attempt saw the run active: transitioning to finished fetches
    /// immediately instead of waiting for `every`.
    fetched_active: bool,
    /// The last read came after the run ended: its output cannot change.
    settled: bool,
}

/// The previews the Runs page holds, by run id.
#[derive(Default)]
pub struct PreviewCache {
    entries: HashMap<String, Entry>,
}

impl PreviewCache {
    /// What the card shows for run `id`; `None` until the first read lands.
    pub fn get(&self, id: &str) -> Option<Rc<RunPreview>> {
        self.entries.get(id)?.preview.clone()
    }

    /// Whether the card for `id` needs a read at `now`: never asked, an active run whose last
    /// read is `every` old, a run that just transitioned from active to finished, or a finished
    /// run whose last read failed `every` ago. A read in flight is never repeated.
    pub fn due(&self, id: &str, active: bool, now: Instant, every: Duration) -> bool {
        let Some(entry) = self.entries.get(id) else {
            return true;
        };
        if entry.inflight {
            return false;
        }
        let aged = entry
            .fetched
            .is_none_or(|at| now.saturating_duration_since(at) >= every);
        if active {
            aged
        } else {
            !entry.settled && (entry.fetched_active || aged)
        }
    }

    /// A read of `id` is on its way.
    pub fn begin(&mut self, id: &str) {
        self.entries.entry(id.to_string()).or_default().inflight = true;
    }

    /// A read of `id` ended. `active` is whether the run still worked when it was asked.
    /// Returns whether the card has anything new to draw.
    pub fn finish(
        &mut self,
        id: &str,
        read: Option<RunPreview>,
        active: bool,
        now: Instant,
    ) -> bool {
        let entry = self.entries.entry(id.to_string()).or_default();
        entry.inflight = false;
        entry.fetched = Some(now);
        entry.fetched_active = active;
        let Some(read) = read else {
            return false;
        };
        entry.settled = !active;
        let same = entry
            .preview
            .as_ref()
            .is_some_and(|held| held.fingerprint == read.fingerprint);
        if same {
            return false;
        }
        entry.preview = Some(Rc::new(read));
        true
    }

    /// Drops cached entries for runs no longer listed by the core.
    pub fn retain(&mut self, keep: &HashSet<&str>) {
        self.entries.retain(|id, _| keep.contains(id.as_str()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(text: &str) -> TranscriptEntry {
        TranscriptEntry::Message {
            text: text.to_string(),
        }
    }

    fn texts(lines: &[PreviewLine]) -> Vec<&str> {
        lines.iter().map(|line| line.text.as_str()).collect()
    }

    #[test]
    fn the_tail_is_the_last_lines_oldest_first() {
        let entries = vec![message("one\ntwo"), message("three\n\nfour")];
        assert_eq!(
            texts(&tail_lines(&entries, 3)),
            ["two", "three", "four"],
            "blank lines are not lines"
        );
        assert_eq!(tail_lines(&entries, 10).len(), 4);
        assert!(tail_lines(&entries, 0).is_empty());
        assert!(tail_lines(&[], 5).is_empty());
    }

    #[test]
    fn a_step_reads_as_a_muted_line_before_its_output() {
        let entries = vec![
            message("Looking"),
            TranscriptEntry::Step {
                text: "Ran cargo test".to_string(),
                detail: None,
                failed: None,
                output: Some("ok\nall passed".to_string()),
            },
        ];
        let lines = tail_lines(&entries, 10);
        assert_eq!(
            texts(&lines),
            ["Looking", "› Ran cargo test", "  ok", "  all passed"]
        );
        assert!(!lines[0].muted);
        assert!(lines[1..].iter().all(|line| line.muted));
        // Cut short, the end of a step's output is what stays.
        assert_eq!(texts(&tail_lines(&entries, 2)), ["  ok", "  all passed"]);
    }

    #[test]
    fn repeated_polling_steps_collapse_with_a_count() {
        let poll = || TranscriptEntry::Step {
            text: "Checking status".to_string(),
            detail: None,
            failed: None,
            output: None,
        };
        let entries = vec![poll(), poll(), poll(), message("Done")];
        let lines = tail_lines(&entries, 10);
        assert_eq!(texts(&lines), ["› Checking status ×3", "Done"]);
    }

    #[test]
    fn one_long_entry_gives_only_its_last_lines() {
        let long: String = (0..500).map(|n| format!("line {n}\n")).collect();
        let lines = tail_lines(&[message(&long)], 4);
        assert_eq!(
            texts(&lines),
            ["line 496", "line 497", "line 498", "line 499"]
        );
    }

    #[test]
    fn a_long_line_is_cut_and_tabs_become_spaces() {
        let long = "x".repeat(LINE_CHARS + 50);
        let lines = tail_lines(&[message(&format!("{long}\n\tindented"))], 5);
        assert_eq!(lines[0].text.chars().count(), LINE_CHARS);
        assert_eq!(lines[1].text, "    indented");
        // A cut never lands inside a character.
        let wide = "é".repeat(LINE_CHARS + 5);
        assert_eq!(
            tail_lines(&[message(&wide)], 1)[0].text.chars().count(),
            LINE_CHARS
        );
    }

    #[test]
    fn notes_and_thoughts_are_muted_with_their_kind() {
        let entries = vec![
            TranscriptEntry::Thinking {
                text: "**Checking** files".to_string(),
            },
            TranscriptEntry::Event {
                event: "retry".to_string(),
                text: "again".to_string(),
            },
        ];
        let lines = tail_lines(&entries, 5);
        assert_eq!(texts(&lines), ["Checking files", "[retry] again"]);
        assert!(lines.iter().all(|line| line.muted));
    }

    fn event(seq: i64, kind: &str, text: &str) -> WatchRunResponse {
        WatchRunResponse {
            seq: ProtoI64(seq),
            kind: kind.to_string(),
            text: text.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn events_become_a_preview_with_a_fingerprint() {
        let preview = preview_from_events(
            vec![
                event(1, "started", "the prompt"),
                event(2, "output", "hello\nworld\n"),
            ],
            5,
        );
        assert_eq!(texts(&preview.lines), ["hello", "world"]);
        assert_eq!(preview.fingerprint, (2, "the prompt".len() + 12));
        let empty = preview_from_events(Vec::new(), 5);
        assert!(empty.lines.is_empty());
        assert_eq!(empty.fingerprint, (0, 0));
    }

    #[test]
    fn stored_output_closes_a_stream_without_a_message() {
        let (preview, has_message) = preview_from_log_parts(&[], "Final stored reply", 5);
        assert!(has_message);
        assert_eq!(texts(&preview.lines), ["Final stored reply"]);
    }

    fn preview(seq: i64) -> RunPreview {
        RunPreview {
            lines: vec![PreviewLine {
                text: "x".to_string(),
                muted: false,
            }],
            fingerprint: (seq, 1),
        }
    }

    const EVERY: Duration = Duration::from_secs(3);

    #[test]
    fn a_card_asks_once_and_an_active_run_asks_again_each_interval() {
        let start = Instant::now();
        let mut cache = PreviewCache::default();
        assert!(cache.due("a", true, start, EVERY), "never asked");
        cache.begin("a");
        assert!(!cache.due("a", true, start + EVERY * 9, EVERY), "in flight");
        assert!(cache.finish("a", Some(preview(1)), true, start));
        assert!(!cache.due("a", true, start + Duration::from_secs(1), EVERY));
        assert!(cache.due("a", true, start + EVERY, EVERY));
        assert!(cache.get("a").is_some());
    }

    #[test]
    fn a_read_that_found_the_same_text_changes_nothing() {
        let start = Instant::now();
        let mut cache = PreviewCache::default();
        assert!(cache.finish("a", Some(preview(1)), true, start));
        let held = cache.get("a").unwrap();
        assert!(!cache.finish("a", Some(preview(1)), true, start + EVERY));
        assert!(Rc::ptr_eq(&held, &cache.get("a").unwrap()));
        assert!(cache.finish("a", Some(preview(2)), true, start + EVERY * 2));
    }

    #[test]
    fn a_finished_run_is_read_once_after_it_ended() {
        let start = Instant::now();
        let soon = start + Duration::from_millis(200);
        let later = start + EVERY * 10;
        let mut cache = PreviewCache::default();
        // Read while it still worked: transitioning to finished is due immediately.
        cache.finish("a", Some(preview(1)), true, start);
        assert!(cache.due("a", false, soon, EVERY));
        cache.finish("a", Some(preview(2)), false, later);
        assert!(!cache.due("a", false, later + EVERY * 10, EVERY));
        // A read that failed on an already-finished run backs off by the poll interval.
        cache.finish("b", None, false, start);
        assert!(cache.get("b").is_none());
        assert!(!cache.due("b", false, start + Duration::from_secs(1), EVERY));
        assert!(cache.due("b", false, start + EVERY, EVERY));
    }
}
