//! A run's transcript, laid out like the agent's terminal.
//!
//! Stored output and `WatchRun` frames are read on the background executor.
//! Rows are measured by `gpui::list`, so a long log only builds the lines on
//! screen: bottom-aligned in the sidebar's preview, top-aligned on the run
//! page, which opens a run at its start, keeps the log's end in view while it
//! is live, and gives the list the column's classic scrollbar. Parsing stays
//! off the UI thread when the new text is not a short suffix of the previous
//! log.

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use std::cell::Cell;

use gpui::{
    App, Context, Entity, IntoElement, ListAlignment, ListOffset, ListState, Render,
    ScrollWheelEvent, SharedString, Task, Window, canvas, div, list, prelude::*, px,
};

use crate::client::{CoreClient, ProtoI64, ResultRequest, WatchRunRequest};
use crate::format::is_active_status;
use crate::models::transcript::{LogEvent, TranscriptBuilder, TranscriptEntry, run_log};
use crate::motion::{Animator, SpringTier, SpringValue};
use crate::state::{AppState, observe};
use crate::theme::{Theme, tw};
use crate::ui::parts::transcript::{
    HighlightCache, PREVIEW_ENTRIES, RowFrame, TranscriptRow, flatten_transcript, render_row,
    terminal_font,
};
use crate::ui::{
    ListScroll, SCROLLBAR_GUTTER, ScrollAxis, ScrollGeometry, WIDE_MEASURE, scrollbar,
};

const SUFFIX_INLINE: usize = 64 * 1024;
const WATCH_BATCH: usize = 400;
/// Reserved so an empty or live row cannot share a hash with a real line.
const SIG_MUTED: u64 = u64::MAX;
const SIG_LIVE_GAP: u64 = u64::MAX - 1;
const SIG_LIVE: u64 = u64::MAX - 2;
/// The run page's `pt-2` over the transcript, which scrolls with it.
const PAGE_TOP: f32 = 2.0;
/// `gap-6` between the transcript and the page's `logEnd` marker under it.
const LOG_END_GAP: f32 = 6.0;
/// `splitScroll`'s 64px under the column's content.
const COLUMN_END: f32 = 64.0;
/// How many exact passes the end gets before it stays where it is.
const SETTLE_PASSES: u8 = 8;

/// The run page's effect that, after each change to a live run's log, scrolls `logEnd` into
/// view (`block: "nearest"`): it waits for the list to lay the change out, then lifts an end
/// below the viewport to its bottom edge.
#[derive(Clone, Copy, PartialEq)]
enum Follow {
    Idle,
    /// The log changed; this frame's layout measures it.
    Pending,
    /// Laid out: an end below the viewport scrolls up to its bottom edge.
    Check,
    /// The end goes on the bottom edge exactly, in at most this many more passes.
    Settle(u8),
}

enum StreamMsg {
    Event(LogEvent),
    End,
}

pub struct TranscriptView {
    state: Entity<AppState>,
    run_id: String,
    compact: bool,
    list: ListState,
    rows: Rc<RefCell<Vec<TranscriptRow>>>,
    entries: Vec<TranscriptEntry>,
    expanded: HashSet<usize>,
    reveal_entry: Option<usize>,
    builder: TranscriptBuilder,
    processed: String,
    stored: String,
    events: Vec<LogEvent>,
    pinned: bool,
    held_start: usize,
    shown_live: bool,
    parse_inflight: bool,
    started: bool,
    font: gpui::Font,
    animator: Animator,
    enter: SpringValue,
    reveal: SpringValue,
    cursor: SpringValue,
    cache: Rc<RefCell<HighlightCache>>,
    watch_task: Option<Task<()>>,
    follow: Follow,
    /// Whether the run page's list overflowed when it was last drawn, which gives it the gutter.
    overflowed: Rc<Cell<bool>>,
    /// The width the run page's list measured every row at, for the scrollbar's true length.
    measured_width: Rc<Cell<f32>>,
}

impl TranscriptView {
    pub fn new(
        state: Entity<AppState>,
        run_id: String,
        compact: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        let alignment = if compact {
            ListAlignment::Bottom
        } else {
            ListAlignment::Top
        };
        let list = ListState::new(0, alignment, px(200.0));
        // The run page's scrollbar and its end of the log need every row's height, as the
        // browser lays the whole log out.
        let list = if compact { list } else { list.measure_all() };
        let weak = cx.entity().downgrade();
        list.set_scroll_handler(move |event, _, cx| {
            if let Some(entity) = weak.upgrade() {
                entity.update(cx, |this, cx| this.on_scroll(event.is_scrolled, cx));
            }
        });
        observe(&state, cx);
        let mut view = Self {
            state,
            run_id,
            compact,
            list,
            rows: Rc::new(RefCell::new(Vec::new())),
            entries: Vec::new(),
            expanded: HashSet::new(),
            reveal_entry: None,
            builder: TranscriptBuilder::new(),
            processed: String::new(),
            stored: String::new(),
            events: Vec::new(),
            pinned: true,
            held_start: 0,
            shown_live: false,
            parse_inflight: false,
            started: false,
            font: terminal_font(cx),
            animator: Animator::new(),
            enter: SpringValue::new(1.0, SpringTier::Moderate),
            reveal: SpringValue::new(1.0, SpringTier::Moderate),
            cursor: SpringValue::new(1.0, SpringTier::Slow),
            cache: Rc::new(RefCell::new(HighlightCache::new())),
            watch_task: None,
            follow: Follow::Idle,
            overflowed: Rc::new(Cell::new(false)),
            measured_width: Rc::new(Cell::new(0.0)),
        };
        view.ensure_stream(cx);
        view.rebuild_rows(cx);
        view
    }

    fn ensure_stream(&mut self, cx: &mut Context<Self>) {
        if self.started {
            return;
        }
        let Some(client) = self.state.read(cx).client() else {
            return;
        };
        self.started = true;
        let (tx, rx) = mpsc::channel();
        let rx = Arc::new(Mutex::new(rx));
        let run_id = self.run_id.clone();
        let watch_client = client.clone();
        self.watch_task = Some(cx.background_spawn(async move {
            produce_watch(watch_client, run_id, tx);
        }));
        let run_id = self.run_id.clone();
        let rx_for_loop = rx.clone();
        cx.spawn(async move |this, cx| {
            let stored = cx
                .background_spawn(async move {
                    client
                        .result(&ResultRequest {
                            id: run_id,
                            section: "output".to_string(),
                            offset: 0,
                            limit: 0,
                        })
                        .map(|response| response.content)
                        .unwrap_or_default()
                })
                .await;
            if this
                .update(cx, |view, cx| {
                    view.stored = stored;
                    view.apply_log(cx);
                })
                .is_err()
            {
                return;
            }
            loop {
                let rx = rx_for_loop.clone();
                let batch = cx.background_spawn(async move { recv_batch(&rx) }).await;
                let Some(batch) = batch else {
                    break;
                };
                let mut events = Vec::new();
                let mut ended = false;
                for message in batch {
                    match message {
                        StreamMsg::Event(event) => events.push(event),
                        StreamMsg::End => ended = true,
                    }
                }
                let alive = this
                    .update(cx, |view, cx| {
                        if !events.is_empty() {
                            view.events.extend(events);
                            view.apply_log(cx);
                        }
                    })
                    .is_ok();
                if !alive || ended {
                    break;
                }
            }
        })
        .detach();
    }

    fn apply_log(&mut self, cx: &mut Context<Self>) {
        if self.parse_inflight {
            return;
        }
        if !self.compact && self.live_now(cx) {
            self.follow = Follow::Pending;
        }
        let log = run_log(&self.stored, &self.events);
        let (stable, tail) = split_stable(&log);
        let answer = self.stored.clone();
        if stable == self.processed {
            self.entries = self.builder.finish_with_tail(&answer, tail);
            self.rebuild_rows(cx);
            return;
        }
        let extends = self.processed.is_empty()
            || (self.processed.ends_with('\n') && stable.starts_with(&self.processed));
        if !extends {
            self.spawn_full(stable.to_string(), cx);
            return;
        }
        let suffix = if self.processed.is_empty() {
            stable.to_string()
        } else {
            stable[self.processed.len()..].to_string()
        };
        if suffix.len() > SUFFIX_INLINE {
            self.spawn_suffix(suffix, stable.to_string(), cx);
            return;
        }
        if !suffix.is_empty() {
            self.builder.feed(&suffix);
        }
        self.processed = stable.to_string();
        self.entries = self.builder.finish_with_tail(&answer, tail);
        self.rebuild_rows(cx);
    }

    fn spawn_full(&mut self, stable: String, cx: &mut Context<Self>) {
        self.parse_inflight = true;
        cx.spawn(async move |this, cx| {
            let parsed = cx
                .background_spawn(async move {
                    let mut builder = TranscriptBuilder::new();
                    builder.feed(&stable);
                    (builder, stable)
                })
                .await;
            this.update(cx, |view, cx| view.install_parse(parsed.0, parsed.1, cx))
                .ok();
        })
        .detach();
    }

    fn spawn_suffix(&mut self, suffix: String, stable: String, cx: &mut Context<Self>) {
        self.parse_inflight = true;
        let builder = std::mem::take(&mut self.builder);
        cx.spawn(async move |this, cx| {
            let parsed = cx
                .background_spawn(async move {
                    let mut builder = builder;
                    builder.feed(&suffix);
                    (builder, stable)
                })
                .await;
            this.update(cx, |view, cx| view.install_parse(parsed.0, parsed.1, cx))
                .ok();
        })
        .detach();
    }

    fn install_parse(
        &mut self,
        builder: TranscriptBuilder,
        stable: String,
        cx: &mut Context<Self>,
    ) {
        self.parse_inflight = false;
        self.builder = builder;
        self.processed = stable;
        self.apply_log(cx);
    }

    fn rebuild_rows(&mut self, cx: &mut Context<Self>) {
        self.shown_live = self.live_now(cx);
        let slice = self.visible_entries();
        let mut rows = flatten_transcript(&slice, &self.expanded, self.reveal_entry);
        if rows.is_empty() {
            let text = if self.shown_live {
                "Waiting for agent output…"
            } else {
                "No agent output recorded."
            };
            rows.push(TranscriptRow {
                signature: SIG_MUTED,
                body: crate::ui::parts::transcript::RowBody::Muted(text.to_string()),
            });
        }
        if self.shown_live {
            if !rows.is_empty() {
                rows.push(TranscriptRow {
                    signature: SIG_LIVE_GAP,
                    body: crate::ui::parts::transcript::RowBody::Gap,
                });
            }
            rows.push(TranscriptRow {
                signature: SIG_LIVE,
                body: crate::ui::parts::transcript::RowBody::Live,
            });
        }
        self.install_rows(rows);
        cx.notify();
    }

    fn visible_entries(&self) -> Vec<TranscriptEntry> {
        if !self.compact {
            return self.entries.clone();
        }
        let start = if self.pinned {
            self.entries.len().saturating_sub(PREVIEW_ENTRIES)
        } else {
            self.held_start.min(self.entries.len())
        };
        self.entries[start..].to_vec()
    }

    fn install_rows(&mut self, rows: Vec<TranscriptRow>) {
        let old_sigs: Vec<u64> = self.rows.borrow().iter().map(|row| row.signature).collect();
        let new_sigs: Vec<u64> = rows.iter().map(|row| row.signature).collect();
        let old_len = old_sigs.len();
        let new_len = new_sigs.len();
        let shared = old_sigs
            .iter()
            .zip(new_sigs.iter())
            .take_while(|(left, right)| left == right)
            .count();
        *self.rows.borrow_mut() = rows;
        if new_len == old_len && shared == old_len {
            return;
        }
        if self.pinned && shared == 0 {
            self.list.reset(new_len);
        } else if !self.pinned && shared == 0 {
            let offset = self.list.logical_scroll_top();
            self.list.splice(0..old_len, new_len);
            self.list.scroll_to(ListOffset {
                item_ix: offset.item_ix.min(new_len),
                offset_in_item: offset.offset_in_item,
            });
        } else {
            let start = shared.min(old_len);
            self.list
                .splice(start..old_len, new_len.saturating_sub(start));
        }
        if new_len > old_len {
            self.enter.snap(0.0);
            self.enter.set_target(1.0);
        }
        if !self.compact {
            // The new rows are measured with the rest, which keep the heights they had.
            let _ = self.list.clone().measure_all();
        }
    }

    fn on_scroll(&mut self, scrolled: bool, cx: &mut Context<Self>) {
        let pinned = !scrolled;
        if self.pinned && !pinned && self.compact {
            self.held_start = self.entries.len().saturating_sub(PREVIEW_ENTRIES);
        }
        let was = self.pinned;
        self.pinned = pinned;
        if self.compact && pinned && !was {
            self.rebuild_rows(cx);
        }
    }

    fn expand(&mut self, entry: usize, cx: &mut Context<Self>) {
        if !self.expanded.insert(entry) {
            return;
        }
        self.reveal_entry = Some(entry);
        self.reveal.snap(0.0);
        self.reveal.set_target(1.0);
        self.rebuild_rows(cx);
    }

    fn live_now(&self, cx: &App) -> bool {
        self.state
            .read(cx)
            .run(&self.run_id)
            .is_some_and(|run| is_active_status(&run.status))
    }

    /// One step of [`Follow`], before this frame lays the list out from the last one's.
    fn step_follow(&mut self, window: &mut Window) {
        let passes = match self.follow {
            Follow::Idle => return,
            Follow::Pending => {
                self.follow = Follow::Check;
                window.request_animation_frame();
                return;
            }
            Follow::Check => None,
            Follow::Settle(passes) => Some(passes),
        };
        let viewport = self.list.viewport_bounds();
        let count = self.list.item_count();
        if count == 0 || viewport.size.height <= px(0.0) || passes == Some(0) {
            self.follow = Follow::Idle;
            return;
        }
        let next = Follow::Settle(passes.unwrap_or(SETTLE_PASSES).saturating_sub(1));
        let Some(last) = self.list.bounds_for_item(count - 1) else {
            // Not measured, so past the overdraw and far below: scroll past the end, which lays
            // the tail out from the bottom, then set the end on the edge.
            self.list.scroll_to(ListOffset {
                item_ix: count,
                offset_in_item: px(0.0),
            });
            self.follow = next;
            window.request_animation_frame();
            return;
        };
        // `bounds_for_item` leaves out the list's top padding.
        let end = last.bottom() + px(tw(PAGE_TOP) + tw(LOG_END_GAP));
        let overflow = end - viewport.bottom();
        if overflow > px(0.5) || (passes.is_some() && overflow < px(-0.5)) {
            self.list.scroll_by(overflow);
            self.follow = next;
            window.request_animation_frame();
        } else {
            self.follow = Follow::Idle;
        }
    }

    fn step_motion(&mut self, window: &mut Window) {
        let moving = self.shown_live
            || !self.enter.settled()
            || !self.reveal.settled()
            || !self.cursor.settled();
        let dt = self.animator.dt(window, moving);
        if dt > 0.0 {
            self.enter.step(dt);
            self.reveal.step(dt);
            self.cursor.step(dt);
        }
        if self.shown_live && self.cursor.settled() {
            let next = if self.cursor.value() < 0.5 { 1.0 } else { 0.0 };
            self.cursor.set_target(next);
        }
    }
}

impl Render for TranscriptView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_stream(cx);
        let live = self.live_now(cx);
        if live != self.shown_live {
            self.shown_live = live;
            self.rebuild_rows(cx);
        }
        self.step_motion(window);
        self.step_follow(window);
        let theme = *Theme::global(cx);
        let rows = self.rows.clone();
        let cache = self.cache.clone();
        let entity = cx.entity();
        let on_expand: Rc<dyn Fn(usize, &mut Window, &mut App)> = Rc::new({
            let entity = entity.clone();
            move |entry, _, cx| {
                entity.update(cx, |view, cx| view.expand(entry, cx));
            }
        });
        let count = rows.borrow().len();
        let enter = self.enter.value();
        let reveal = self.reveal.value();
        let cursor = self.cursor.value();
        let font = self.font.clone();
        let compact = self.compact;
        let lines = list(self.list.clone(), move |index, window, cx| {
            let row = rows.borrow().get(index).cloned();
            let Some(row) = row else {
                return div().into_any_element();
            };
            let frame = RowFrame {
                theme,
                index,
                count,
                enter,
                reveal,
                cursor,
                cache: cache.clone(),
                on_expand: on_expand.clone(),
            };
            let row = render_row(&frame, &row, window, cx);
            if compact {
                return row;
            }
            // `.pw-document__column > *`: at most the log's measure, centered.
            div()
                .w_full()
                .flex()
                .justify_center()
                .child(
                    div()
                        .w_full()
                        .min_w(px(0.0))
                        .max_w(px(WIDE_MEASURE))
                        .child(row),
                )
                .into_any_element()
        })
        .size_full()
        // On the run page the space over the transcript, the gap to `logEnd`, and the
        // column's 64px under it scroll with the text, as they do in `splitScroll`.
        .when(!compact, |list| {
            list.pt(px(tw(PAGE_TOP)))
                .pb(px(tw(LOG_END_GAP) + COLUMN_END))
        });
        let root = div()
            .size_full()
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .font(font)
            .text_size(px(12.5))
            .line_height(px(20.0))
            .text_color(theme.color_text_secondary);
        if compact {
            return root.child(lines).into_any_element();
        }
        // The column's classic scrollbar: once the text overflows, a 12px gutter comes out of
        // the width the lines wrap in, and the list measures them again at the new width.
        // A new width leaves the rows unmeasured, so they are all measured again at it.
        let width = f32::from(self.list.viewport_bounds().size.width);
        if (width - self.measured_width.get()).abs() > 0.5 {
            self.measured_width.set(width);
            let _ = self.list.clone().measure_all();
        }
        let page = page_scroll(&self.list);
        let overflows = page.is_some_and(|page| page.content > page.viewport + 0.5);
        self.overflowed.set(overflows);
        // Overflow and width are known once the list has laid out, so a frame whose layout
        // changes either draws again.
        let watched = self.list.clone();
        let overflowed = self.overflowed.clone();
        let measured_width = self.measured_width.clone();
        let watcher = canvas(
            move |_, window, _| {
                let now =
                    page_scroll(&watched).is_some_and(|page| page.content > page.viewport + 0.5);
                let width = f32::from(watched.viewport_bounds().size.width);
                if now != overflowed.get() || (width - measured_width.get()).abs() > 0.5 {
                    window.refresh();
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_0();
        let mut area = root.relative().child(
            div()
                .size_full()
                .min_h(px(0.0))
                .when(overflows, |lines| lines.pr(px(SCROLLBAR_GUTTER)))
                .child(lines)
                .child(watcher),
        );
        if let Some(page) = page.filter(|_| overflows) {
            let id = SharedString::from(format!("transcript-bar-{}", cx.entity_id()));
            let bar = scrollbar(
                id,
                &ListScroll {
                    state: self.list.clone(),
                    max: page.content - page.viewport,
                },
                ScrollAxis::Vertical,
                ScrollGeometry {
                    track: page.viewport,
                    viewport: page.viewport,
                    content: page.content,
                },
                window,
                cx,
            );
            // A wheel over the gutter scrolls the column too.
            let wheeled = self.list.clone();
            let view = cx.entity_id();
            area = area.child(
                div()
                    .id("transcript-gutter")
                    .absolute()
                    .top(px(0.0))
                    .right(px(0.0))
                    .bottom(px(0.0))
                    .w(px(SCROLLBAR_GUTTER))
                    .on_scroll_wheel(move |event: &ScrollWheelEvent, window, cx| {
                        let delta = event.delta.pixel_delta(window.line_height());
                        wheeled.scroll_by(-delta.y);
                        cx.notify(view);
                    })
                    .child(bar),
            );
        }
        area.into_any_element()
    }
}

/// The run page list's whole height with its padding, and the height it shows, as the last
/// frame laid it out. A row not measured yet counts for nothing.
#[derive(Clone, Copy)]
struct PageScroll {
    content: f32,
    viewport: f32,
}

fn page_scroll(list: &ListState) -> Option<PageScroll> {
    let bounds = list.viewport_bounds();
    let viewport = f32::from(bounds.size.height);
    let count = list.item_count();
    if viewport <= 0.0 || count == 0 {
        return None;
    }
    let scrolled = -f32::from(list.scroll_px_offset_for_scrollbar().y);
    let items = match list.bounds_for_item(count - 1) {
        Some(last) => f32::from(last.bottom() - bounds.top()) + scrolled,
        None => f32::from(list.max_offset_for_scrollbar().height) + viewport,
    };
    Some(PageScroll {
        content: items + tw(PAGE_TOP) + tw(LOG_END_GAP) + COLUMN_END,
        viewport,
    })
}

fn split_stable(log: &str) -> (&str, &str) {
    match log.rfind('\n') {
        Some(index) => (&log[..=index], &log[index + 1..]),
        None => ("", log),
    }
}

fn produce_watch(client: CoreClient, run_id: String, tx: mpsc::Sender<StreamMsg>) {
    let request = WatchRunRequest {
        id: run_id,
        after_seq: ProtoI64(0),
    };
    let Ok(iter) = client.watch_run(&request) else {
        let _ = tx.send(StreamMsg::End);
        return;
    };
    for item in iter {
        match item {
            Ok(event) => {
                let message = StreamMsg::Event(LogEvent {
                    kind: event.kind,
                    text: event.text,
                });
                if tx.send(message).is_err() {
                    return;
                }
            }
            Err(_) => break,
        }
    }
    let _ = tx.send(StreamMsg::End);
}

fn recv_batch(rx: &Mutex<mpsc::Receiver<StreamMsg>>) -> Option<Vec<StreamMsg>> {
    let rx = rx.lock().ok()?;
    let first = rx.recv().ok()?;
    let mut batch = vec![first];
    let deadline = Instant::now() + Duration::from_millis(16);
    while batch.len() < WATCH_BATCH {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        match rx.recv_timeout(left) {
            Ok(message) => batch.push(message),
            Err(_) => break,
        }
    }
    Some(batch)
}
