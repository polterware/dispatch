//! Pages of a run's event text, so a client opens a long run at its end and reads further
//! back only as it scrolls there: `WatchRun` with `tail_bytes` and `ReadRunEvents`.
//!
//! A page is planned from the events' sizes alone, from its end backward, and only its
//! slices are read. Output can be cut anywhere, so a page holds exactly its budget of it; any
//! other event is a short `[kind] text` line and stays whole, the page's first one even past
//! its budget.

use crate::types::{EVENT_OUTPUT, EventSize};

/// A page's size when the request names none.
pub(crate) const DEFAULT_PAGE_BYTES: u64 = 256 * 1024;
/// The largest page a request may ask for.
pub(crate) const MAX_PAGE_BYTES: u64 = 8 * 1024 * 1024;

/// The bytes `start..end` of event `seq`'s text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Cut {
    pub seq: i64,
    pub start: u64,
    pub end: u64,
}

/// A request's page size: the default for none, held to the largest.
pub(crate) fn page_bytes(requested: i64) -> u64 {
    match u64::try_from(requested) {
        Ok(0) | Err(_) => DEFAULT_PAGE_BYTES,
        Ok(bytes) => bytes.min(MAX_PAGE_BYTES),
    }
}

/// Output, which a page edge may cut, as opposed to the core's one-line events.
fn cuttable(kind: &str) -> bool {
    kind == EVENT_OUTPUT || kind == "agent"
}

/// The slices, oldest first, of the last `max_bytes` of the run's text before `before`
/// (an event's seq and a byte offset into its text; None is the run's end), and whether
/// text remains before them.
pub(crate) fn page_before(
    sizes: &[EventSize],
    before: Option<(i64, u64)>,
    max_bytes: u64,
) -> (Vec<Cut>, bool) {
    let mut portions = sizes.iter().rev().filter_map(|event| {
        let end = match before {
            None => event.bytes,
            Some((seq, _)) if event.seq > seq => return None,
            Some((seq, offset)) if event.seq == seq => offset.min(event.bytes),
            Some(_) => event.bytes,
        };
        (end > 0).then_some((event, end))
    });
    let mut cuts = Vec::new();
    let mut budget = max_bytes.max(1);
    let mut more = false;
    while let Some((event, end)) = portions.next() {
        if end <= budget {
            cuts.push(Cut {
                seq: event.seq,
                start: 0,
                end,
            });
            budget -= end;
            if budget == 0 {
                more = portions.next().is_some();
                break;
            }
            continue;
        }
        if cuttable(&event.kind) {
            cuts.push(Cut {
                seq: event.seq,
                start: end - budget,
                end,
            });
            more = true;
        } else if cuts.is_empty() {
            cuts.push(Cut {
                seq: event.seq,
                start: 0,
                end,
            });
            more = portions.next().is_some();
        } else {
            more = true;
        }
        break;
    }
    cuts.reverse();
    (cuts, more)
}

#[cfg(test)]
mod tests {
    use super::{Cut, page_before};
    use crate::types::EventSize;

    fn size(seq: i64, kind: &str, bytes: u64) -> EventSize {
        EventSize {
            seq,
            kind: kind.into(),
            bytes,
        }
    }

    #[test]
    fn a_page_takes_its_budget_from_the_end_and_cuts_output_only() {
        let sizes = [
            size(1, "started", 20),
            size(2, "output", 1000),
            size(3, "output", 300),
            size(4, "merged", 12),
        ];
        // From the end: merged and the last output whole, then the end of event 2.
        let (cuts, more) = page_before(&sizes, None, 400);
        assert_eq!(
            cuts,
            vec![
                Cut {
                    seq: 2,
                    start: 912,
                    end: 1000
                },
                Cut {
                    seq: 3,
                    start: 0,
                    end: 300
                },
                Cut {
                    seq: 4,
                    start: 0,
                    end: 12
                },
            ]
        );
        assert!(more);
        // The page before that: the rest of event 2's start, then the started line.
        let (cuts, more) = page_before(&sizes, Some((2, 912)), 2000);
        assert_eq!(
            cuts,
            vec![
                Cut {
                    seq: 1,
                    start: 0,
                    end: 20
                },
                Cut {
                    seq: 2,
                    start: 0,
                    end: 912
                },
            ]
        );
        assert!(!more);
        // A one-line event is never cut: past the budget it waits for the next page.
        let (cuts, more) = page_before(&sizes, Some((2, 0)), 5);
        assert_eq!(
            cuts,
            vec![Cut {
                seq: 1,
                start: 0,
                end: 20
            }]
        );
        assert!(!more);
        let (cuts, more) = page_before(&sizes, None, 10);
        assert_eq!(
            cuts,
            vec![Cut {
                seq: 4,
                start: 0,
                end: 12
            }]
        );
        assert!(more);
    }
}
