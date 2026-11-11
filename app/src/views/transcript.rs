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

use futures::StreamExt as _;
use futures::channel::mpsc;
use std::time::Duration;

use std::cell::Cell;

use gpui::{
    App, Context, Entity, IntoElement, ListAlignment, ListOffset, ListState, Render,
    ScrollWheelEvent, SharedString, Task, Window, canvas, div, list, prelude::*, px,
};

use crate::client::{
    CoreClient, DispatchRun, ListArtifactsRequest, ProtoI64, ReadRunEventsRequest, ResultRequest,
    WatchRunRequest,
};
use crate::format::is_active_status;
use crate::models::run_page::attempt_switches;
use crate::models::transcript::{
    LogEvent, PagedEvent, PromptBubble, StreamedLog, TranscriptBuilder, TranscriptEntry,
    TranscriptImage, build_transcript, enrich_transcript_switches, prompt_bubble, run_images,
};
use crate::motion::{Animator, SpringTier, SpringValue};
use crate::state::{AppState, observe};
use crate::theme::{Theme, tw};
use crate::ui::parts::transcript::{
    HighlightCache, PREVIEW_ENTRIES, PromptToggle, RowFrame, TranscriptRow, flatten_transcript,
    gap_row, offset_entries, prompt_transcript_row, render_row, terminal_font,
};
use crate::ui::patterns::{JUMP_TO_END_REST, SCROLL_FADE, jump_to_end};
use crate::ui::text_skeleton;
use crate::ui::{
    ListScroll, SCROLLBAR_GUTTER, ScrollAxis, ScrollGeometry, WIDE_MEASURE, scrollbar,
};

/// How fast the glide to the end closes the distance left: the share left after `t` seconds
/// is `e^(-rate·t)`, so about 0.3s to the end.
const GLIDE_RATE: f32 = 14.0;
/// The jump-to-end button over the bottom of the log.
const JUMP_INSET: f32 = 16.0;
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

/// Following the end: after each change to the log while the reader is at its end
/// ([`TranscriptView::stick`]), it waits for the list to lay the change out, then scrolls
/// down to the list's true end, its padding included, the end the jump-to-end button glides
/// to.
#[derive(Clone, Copy, PartialEq)]
enum Follow {
    Idle,
    /// The log changed; this frame's layout measures it.
    Pending,
    /// Laid out: an end below the viewport scrolls up to it.
    Check,
    /// The end goes on the bottom edge, in at most this many more passes.
    Settle(u8),
}

enum StreamMsg {
    Event(PagedEvent),
    End,
}

/// How much of a run's end the stream opens with: a long run's transcript starts there and
/// reads further back only as the reader scrolls up.
const TAIL_BYTES: i64 = 256 * 1024;
/// How much each scroll up reads back at first.
const PAGE_BYTES: i64 = 512 * 1024;
/// The most one read back asks for: a page inside one long line doubles toward it.
const MAX_PAGE_BYTES: i64 = 8 * 1024 * 1024;
/// How close to the top, in rows, a scroll up asks for the page before.
const EARLIER_ROWS: usize = 4;
/// Where the entries of each earlier run of a chat start: the open run's are below it.
const EARLIER_RUN_ENTRIES: usize = 1 << 24;
/// How much of each earlier run's end the chat reads.
const EARLIER_RUN_BYTES: i64 = 1024 * 1024;

/// One earlier run of the open run's chat: its message and what its agent did, read once.
struct EarlierRun {
    bubble: Option<PromptBubble>,
    entries: Vec<TranscriptEntry>,
    expanded: HashSet<usize>,
}

/// A wire event as the stream and the page reads hand it on.
fn paged(event: crate::client::WatchRunResponse) -> PagedEvent {
    PagedEvent {
        event: LogEvent {
            kind: event.kind,
            text: event.text,
        },
        seq: event.seq.0,
        offset: u64::try_from(event.text_offset.0).unwrap_or(0),
    }
}

pub struct TranscriptView {
    state: Entity<AppState>,
    run_id: String,
    compact: bool,
    list: ListState,
    rows: Rc<RefCell<Vec<TranscriptRow>>>,
    entries: Vec<TranscriptEntry>,
    expanded: HashSet<usize>,
    /// The prompt bubble is open past its folded lines.
    prompt_expanded: bool,
    /// The rows open with the prompt bubble.
    prompt_shown: bool,
    reveal_entry: Option<usize>,
    builder: TranscriptBuilder,
    processed: String,
    stored: String,
    /// What the stream has sent, written once per event as it arrives.
    streamed: StreamedLog,
    pinned: bool,
    held_start: usize,
    shown_live: bool,
    shown_switches: usize,
    parse_inflight: bool,
    /// The stored log has come back once: until then the text shows as bones, not as a run
    /// with no output.
    fetched: bool,
    started: bool,
    font: gpui::Font,
    animator: Animator,
    enter: SpringValue,
    reveal: SpringValue,
    cache: Rc<RefCell<HighlightCache>>,
    watch_task: Option<Task<()>>,
    follow: Follow,
    /// The reader is at the end, so the log follows what comes in. Scrolling away (the
    /// wheel, the scrollbar) lets go of it, and coming back to the end or the jump-to-end
    /// button takes it again.
    stick: bool,
    /// The wheel moved the list since the last frame.
    reader_moved: bool,
    /// The view moved the list itself since the last frame (following, gliding, keeping the
    /// reader's place), so a change of offset is not the reader's.
    moved_here: bool,
    /// How far the list had scrolled at the last frame.
    last_scrolled: f32,
    /// Whether the run page's list overflowed when it was last drawn, which gives it the gutter.
    overflowed: Rc<Cell<bool>>,
    /// The width the run page's list measured every row at, for the scrollbar's true length.
    measured_width: Rc<Cell<f32>>,
    /// How tall the header floating over the run page's list is: the list runs up under it
    /// and keeps that much clear above its first row.
    top_inset: f32,
    /// The room the run page keeps after the text: its rows stop that far from the list's
    /// end, where the scrollbar sits flush (`rail_page_end_pad`).
    end_pad: f32,
    /// The jump-to-end button's presence: in while the reader is away from the end.
    end_button: SpringValue,
    /// Text may come before what the stream opened with, and no read has said otherwise.
    earlier_more: bool,
    /// A read of the page before is out.
    earlier_inflight: bool,
    /// The next read back's size: doubled while pages land inside one long line.
    earlier_page: i64,
    /// The next rows put a page in front of the last ones: the list keeps the row the reader
    /// is on where it is.
    prepended: bool,
    /// How many rows there were before a page went in front, until its rows land: a page
    /// that adds none (stream deltas, usage lines) reads on further.
    rows_before_page: Option<usize>,
    /// The first events came in.
    opened: bool,
    /// A run longer than its first page opens at its end: the first rows with entries scroll
    /// there.
    open_at_end: bool,
    /// The runs this one followed up on, oldest first, once their logs are in: the run page
    /// shows the chat whole, each message over what its agent did.
    earlier_runs: Vec<EarlierRun>,
    earlier_runs_asked: bool,
    /// Bumped when the view moves on to a follow-up: reads still out for the run before
    /// drop their answers.
    generation: u64,
    /// The view moved on to a follow-up in place: the chat stays on screen while the new
    /// run's first text comes in.
    continued: bool,
    /// Gliding to the end after the button was pressed.
    glide: bool,
    /// The last motion frame's step, which the glide moves by.
    frame_dt: f32,
    /// The images the open run made, oldest first, shown under its reply.
    images: Vec<TranscriptImage>,
    /// The run whose images were asked of the core.
    images_for: String,
    /// The open run's images were asked for.
    images_asked: bool,
    /// The viewer a thumbnail opens.
    viewer: Entity<super::image_viewer::ImageViewerView>,
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
        // GPUI calls this while it holds the list's state, so the handler must not read the
        // list back (`logical_scroll_top` would panic on the borrow): the event says what shows.
        list.set_scroll_handler(move |event, _, cx| {
            if let Some(entity) = weak.upgrade() {
                let first = event.visible_range.start;
                entity.update(cx, |this, cx| this.on_scroll(event.is_scrolled, first, cx));
            }
        });
        observe(&state, cx);
        let viewer = cx.new(|cx| super::image_viewer::ImageViewerView::new(state.clone(), cx));
        let mut view = Self {
            state,
            run_id,
            compact,
            list,
            rows: Rc::new(RefCell::new(Vec::new())),
            entries: Vec::new(),
            expanded: HashSet::new(),
            prompt_expanded: false,
            prompt_shown: false,
            reveal_entry: None,
            builder: TranscriptBuilder::new(),
            processed: String::new(),
            stored: String::new(),
            streamed: StreamedLog::default(),
            pinned: true,
            held_start: 0,
            shown_live: false,
            shown_switches: 0,
            parse_inflight: false,
            fetched: false,
            started: false,
            font: terminal_font(cx),
            animator: Animator::new(),
            enter: SpringValue::new(1.0, SpringTier::Moderate),
            reveal: SpringValue::new(1.0, SpringTier::Moderate),
            cache: Rc::new(RefCell::new(HighlightCache::new())),
            watch_task: None,
            follow: Follow::Idle,
            stick: true,
            reader_moved: false,
            moved_here: false,
            last_scrolled: 0.0,
            overflowed: Rc::new(Cell::new(false)),
            measured_width: Rc::new(Cell::new(0.0)),
            top_inset: 0.0,
            end_pad: 0.0,
            end_button: SpringValue::new(0.0, SpringTier::Moderate),
            earlier_more: false,
            earlier_inflight: false,
            earlier_page: PAGE_BYTES,
            prepended: false,
            rows_before_page: None,
            opened: false,
            open_at_end: false,
            earlier_runs: Vec::new(),
            earlier_runs_asked: false,
            generation: 0,
            continued: false,
            glide: false,
            frame_dt: 0.0,
            images: Vec::new(),
            images_for: String::new(),
            images_asked: false,
            viewer,
        };
        view.ensure_stream(cx);
        view.ensure_earlier_runs(cx);
        view.rebuild_rows(cx);
        view
    }

    /// Moves the run page's transcript on to `run_id`, a follow-up of the open run, in
    /// place: what shows now becomes the chat's last earlier run, the scroll stays, and the
    /// follow-up's message and stream come in under it, as a chat app's next message does.
    pub fn continue_with(&mut self, run_id: String, cx: &mut Context<Self>) {
        if self.compact || run_id == self.run_id {
            return;
        }
        let bubble = self.prompt_bubble(cx);
        let switches = self
            .state
            .read(cx)
            .run(&self.run_id)
            .map(attempt_switches)
            .unwrap_or_default();
        let entries = enrich_transcript_switches(
            std::mem::take(&mut self.entries),
            &switches,
            !self.earlier_more,
        );
        self.earlier_runs.push(EarlierRun {
            bubble,
            entries,
            expanded: std::mem::take(&mut self.expanded),
        });
        self.generation = self.generation.wrapping_add(1);
        self.run_id = run_id;
        self.continued = true;
        self.earlier_runs_asked = true;
        self.prompt_expanded = false;
        self.reveal_entry = None;
        self.builder = TranscriptBuilder::new();
        self.processed.clear();
        self.stored.clear();
        self.streamed = StreamedLog::default();
        self.parse_inflight = false;
        self.fetched = false;
        self.started = false;
        self.watch_task = None;
        self.earlier_more = false;
        self.earlier_inflight = false;
        self.earlier_page = PAGE_BYTES;
        self.prepended = false;
        self.rows_before_page = None;
        self.opened = false;
        self.open_at_end = false;
        self.images.clear();
        self.images_for.clear();
        self.images_asked = false;
        self.viewer.update(cx, |viewer, cx| viewer.close(cx));
        self.stick = true;
        self.glide = false;
        self.follow = Follow::Pending;
        self.ensure_stream(cx);
        self.rebuild_rows(cx);
    }

    /// The runs the open one followed up on, oldest first, as the app lists them.
    fn chat_before(&self, cx: &App) -> Vec<DispatchRun> {
        let state = self.state.read(cx);
        let mut chain: Vec<DispatchRun> = Vec::new();
        let mut at = state.run(&self.run_id).map(|run| run.resumed_of.clone());
        while let Some(id) = at.filter(|id| !id.trim().is_empty()) {
            if id == self.run_id || chain.iter().any(|run| run.id == id) {
                break;
            }
            let Some(run) = state.run(&id) else {
                break;
            };
            at = Some(run.resumed_of.clone());
            chain.insert(0, run.clone());
        }
        chain
    }

    /// Reads the earlier runs' logs and replies once, and lays them over the open run's.
    fn ensure_earlier_runs(&mut self, cx: &mut Context<Self>) {
        if self.compact || self.earlier_runs_asked {
            return;
        }
        let chain = self.chat_before(cx);
        if chain.is_empty() {
            // The app may not have listed the run yet: a later draw asks again.
            if self.state.read(cx).run(&self.run_id).is_some() {
                self.earlier_runs_asked = true;
            }
            return;
        }
        let Some(client) = self.state.read(cx).client() else {
            return;
        };
        self.earlier_runs_asked = true;
        cx.spawn(async move |this, cx| {
            let read = cx
                .background_spawn(async move {
                    chain
                        .iter()
                        .map(|run| {
                            let section = |section: &str| {
                                client
                                    .result(&ResultRequest {
                                        id: run.id.clone(),
                                        section: section.to_string(),
                                        offset: 0,
                                        limit: 0,
                                    })
                                    .map(|response| response.content)
                                    .unwrap_or_default()
                            };
                            // Its end, as the open run's stream opens: a long run shows
                            // its last part under its message.
                            let reply = section("agent");
                            let events = client
                                .read_run_events(&ReadRunEventsRequest {
                                    id: run.id.clone(),
                                    before_seq: ProtoI64(0),
                                    before_offset: ProtoI64(0),
                                    max_bytes: ProtoI64(EARLIER_RUN_BYTES),
                                })
                                .map(|page| page.events.into_iter().map(paged).collect::<Vec<_>>())
                                .unwrap_or_default();
                            let mut streamed = StreamedLog::default();
                            streamed.extend_paged(&events);
                            let log = streamed.join(&reply);
                            let switches = attempt_switches(run);
                            EarlierRun {
                                bubble: prompt_bubble(
                                    &run.prompt,
                                    &run.caller_agent,
                                    &run.created_at,
                                ),
                                entries: enrich_transcript_switches(
                                    build_transcript(&log, &reply),
                                    &switches,
                                    true,
                                ),
                                expanded: HashSet::new(),
                            }
                        })
                        .collect::<Vec<_>>()
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                // A follow-up opened in place meanwhile appended the run it followed.
                let mut read = read;
                read.append(&mut view.earlier_runs);
                view.earlier_runs = read;
                // The chat opens at its latest message, as a chat app's does.
                if view.stick {
                    view.follow = Follow::Pending;
                }
                view.rebuild_rows(cx);
            });
        })
        .detach();
    }

    /// Asks the core for the images the open run made, once it has ended: the core indexes
    /// them as a run ends, so a run still working has none yet. A read that comes back empty
    /// is asked once more a moment later, since the status can land a beat before the index.
    fn ensure_images(&mut self, cx: &mut Context<Self>) {
        if self.compact || self.images_asked || self.images_for == self.run_id {
            return;
        }
        let Some(run) = self.state.read(cx).run(&self.run_id).cloned() else {
            return;
        };
        if crate::format::is_active_status(&run.status) {
            return;
        }
        let Some(client) = self.state.read(cx).client() else {
            return;
        };
        self.images_for = self.run_id.clone();
        self.images_asked = true;
        let run_id = run.id.clone();
        // The core filters by the run's repository and conversation, which its images carry,
        // and the app keeps the run's own from that list: the RPC has no run filter.
        let request = ListArtifactsRequest {
            repo: run.repo.clone(),
            conversation_id: run.conversation_id.clone(),
            ..ListArtifactsRequest::default()
        };
        cx.spawn(async move |this, cx| {
            let mut images = Vec::new();
            for attempt in 0..2 {
                let (client, request) = (client.clone(), request.clone());
                let artifacts = cx
                    .background_spawn(async move {
                        client
                            .list_artifacts(&request)
                            .map(|response| response.artifacts)
                            .unwrap_or_default()
                    })
                    .await;
                images = run_images(&artifacts, &run_id);
                if !images.is_empty() || attempt == 1 {
                    break;
                }
                cx.background_executor()
                    .timer(Duration::from_millis(900))
                    .await;
            }
            let _ = this.update(cx, |view, cx| {
                view.images = images;
                view.rebuild_rows(cx);
            });
        })
        .detach();
    }

    /// The earlier runs' rows, each message over what its agent did, with the space after
    /// each one.
    fn earlier_rows(&self) -> Vec<TranscriptRow> {
        let mut rows = Vec::new();
        for (index, run) in self.earlier_runs.iter().enumerate() {
            if let Some(bubble) = &run.bubble {
                rows.push(prompt_transcript_row(bubble, true));
            }
            let flat = flatten_transcript(&run.entries, &run.expanded, None);
            rows.extend(offset_entries(flat, EARLIER_RUN_ENTRIES * (index + 1)));
            rows.push(gap_row());
            rows.push(gap_row());
        }
        rows
    }

    fn ensure_stream(&mut self, cx: &mut Context<Self>) {
        if self.started {
            return;
        }
        let Some(client) = self.state.read(cx).client() else {
            return;
        };
        self.started = true;
        let generation = self.generation;
        // An async channel: the loop below awaits the next event, so it never holds a thread
        // the producer needs (gpui's test executor runs every task on one thread).
        let (tx, mut rx) = mpsc::unbounded();
        let run_id = self.run_id.clone();
        let watch_client = client.clone();
        self.watch_task = Some(cx.background_spawn(async move {
            produce_watch(watch_client, run_id, tx);
        }));
        let run_id = self.run_id.clone();
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
                    if view.generation != generation {
                        return Err(());
                    }
                    view.stored = stored;
                    view.fetched = true;
                    view.apply_log(cx);
                    cx.notify();
                    Ok(())
                })
                .map_or(true, |fresh| fresh.is_err())
            {
                return;
            }
            loop {
                let Some(batch) = next_batch(&mut rx).await else {
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
                        if view.generation != generation {
                            return false;
                        }
                        if !events.is_empty() {
                            view.streamed.extend_paged(&events);
                            if !view.opened {
                                view.opened = true;
                                view.earlier_more = view.streamed.may_have_earlier();
                                // A run longer than its first page opens where it is now, once
                                // its rows are in.
                                view.open_at_end = view.earlier_more && !view.compact;
                            }
                            view.apply_log(cx);
                        }
                        true
                    })
                    .unwrap_or(false);
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
        if !self.compact && self.stick && self.live_now(cx) {
            self.follow = Follow::Pending;
        }
        let log = self.streamed.join(&self.stored);
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
        let generation = self.generation;
        cx.spawn(async move |this, cx| {
            let parsed = cx
                .background_spawn(async move {
                    let mut builder = TranscriptBuilder::new();
                    builder.feed(&stable);
                    (builder, stable)
                })
                .await;
            this.update(cx, |view, cx| {
                if view.generation == generation {
                    view.install_parse(parsed.0, parsed.1, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    fn spawn_suffix(&mut self, suffix: String, stable: String, cx: &mut Context<Self>) {
        self.parse_inflight = true;
        let generation = self.generation;
        let builder = std::mem::take(&mut self.builder);
        cx.spawn(async move |this, cx| {
            let parsed = cx
                .background_spawn(async move {
                    let mut builder = builder;
                    builder.feed(&suffix);
                    (builder, stable)
                })
                .await;
            this.update(cx, |view, cx| {
                if view.generation == generation {
                    view.install_parse(parsed.0, parsed.1, cx);
                }
            })
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
        let switches = self
            .state
            .read(cx)
            .run(&self.run_id)
            .map(attempt_switches)
            .unwrap_or_default();
        self.shown_switches = switches.len();
        let slice = enrich_transcript_switches(
            self.visible_entries(),
            &switches,
            !self.compact && !self.earlier_more,
        );
        let mut rows = flatten_transcript(&slice, &self.expanded, self.reveal_entry);
        // A live run with nothing yet shows only its thinking bot.
        if rows.is_empty() && !self.shown_live {
            rows.push(TranscriptRow {
                signature: SIG_MUTED,
                body: crate::ui::parts::transcript::RowBody::Muted(
                    "No agent output recorded.".to_string(),
                ),
            });
        }
        // The images the run made close its transcript, under its reply. A run with none
        // adds no row, so the layout is the one it had.
        if !self.images.is_empty() {
            if !rows.is_empty() {
                rows.push(gap_row());
            }
            rows.push(crate::ui::parts::transcript::images_transcript_row(
                &self.images,
            ));
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
                body: crate::ui::parts::transcript::RowBody::Live {
                    agent: self
                        .state
                        .read(cx)
                        .run(&self.run_id)
                        .map(|run| run.agent.clone())
                        .unwrap_or_default(),
                },
            });
        }
        let bubble = self.prompt_bubble(cx);
        self.prompt_shown = bubble.is_some();
        if let Some(bubble) = bubble {
            rows.insert(0, prompt_transcript_row(&bubble, self.prompt_expanded));
            // The chat's earlier messages go over this one's, once its start is in.
            let earlier = self.earlier_rows();
            if !earlier.is_empty() {
                rows.splice(0..0, earlier);
            }
        }
        let rows_now = rows.len();
        self.install_rows(rows);
        if let Some(before) = self.rows_before_page.take() {
            if rows_now > before {
                self.earlier_page = PAGE_BYTES;
            } else {
                self.read_on(cx);
            }
        }
        if self.open_at_end && !self.entries.is_empty() {
            self.open_at_end = false;
            self.follow = Follow::Pending;
        }
        cx.notify();
    }

    /// After a page went in front: what came in is rows above the old ones, so the row the
    /// reader was on moves down by how many came. The old first rows may parse anew with
    /// their start, but they stay the same stretch of text.
    fn keep_place_after_prepend(&mut self, old_len: usize, new_len: usize) {
        let top = self.list.logical_scroll_top();
        let item_ix = top.item_ix + new_len.saturating_sub(old_len);
        self.moved_here = true;
        self.list.splice(0..old_len, new_len);
        self.list.scroll_to(ListOffset {
            item_ix: item_ix.min(new_len),
            offset_in_item: top.offset_in_item,
        });
        if !self.compact {
            let _ = self.list.clone().measure_all();
        }
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
        if std::mem::take(&mut self.prepended) {
            self.keep_place_after_prepend(old_len, new_len);
            return;
        }
        if new_len == old_len && shared == old_len {
            return;
        }
        self.moved_here = true;
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
            let top = self.list.logical_scroll_top();
            self.list
                .splice(start..old_len, new_len.saturating_sub(start));
            let tail = old_sigs[start..]
                .iter()
                .rev()
                .zip(new_sigs[start..].iter().rev())
                .take_while(|(left, right)| left == right)
                .count();
            if let Some(kept) = kept_top(top, start, old_len, new_len, tail) {
                self.list.scroll_to(kept);
            }
        }
        // Only a new last row fades in: the thinking bot under a growing log stays lit.
        if new_len > old_len && old_sigs.last() != new_sigs.last() {
            self.enter.snap(0.0);
            self.enter.set_target(1.0);
        }
        if !self.compact {
            // The new rows are measured with the rest, which keep the heights they had.
            let _ = self.list.clone().measure_all();
        }
    }

    /// `first` is the first row on show.
    fn on_scroll(&mut self, scrolled: bool, first: usize, cx: &mut Context<Self>) {
        self.reader_moved = true;
        if !self.compact && first < EARLIER_ROWS {
            self.load_earlier(cx);
        }
        let pinned = !scrolled;
        if self.pinned && !pinned && self.compact {
            self.held_start = self.entries.len().saturating_sub(PREVIEW_ENTRIES);
        }
        let was = self.pinned;
        self.pinned = pinned;
        if self.compact && pinned && !was {
            // Rebuilding splices the list, which the scroll handler's caller still holds.
            cx.spawn(async move |this, cx| {
                let _ = this.update(cx, |view, cx| view.rebuild_rows(cx));
            })
            .detach();
        }
    }

    /// Reads the page before the text the transcript holds and puts it in front, keeping the
    /// row the reader is on in place.
    fn load_earlier(&mut self, cx: &mut Context<Self>) {
        if !self.earlier_more || self.earlier_inflight || self.parse_inflight {
            return;
        }
        let Some((seq, offset)) = self.streamed.head() else {
            return;
        };
        let Some(client) = self.state.read(cx).client() else {
            return;
        };
        self.earlier_inflight = true;
        let generation = self.generation;
        let request = ReadRunEventsRequest {
            id: self.run_id.clone(),
            before_seq: ProtoI64(seq),
            before_offset: ProtoI64(i64::try_from(offset).unwrap_or(i64::MAX)),
            max_bytes: ProtoI64(self.earlier_page),
        };
        cx.spawn(async move |this, cx| {
            let page = cx
                .background_spawn(async move { client.read_run_events(&request) })
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.generation != generation {
                    return;
                }
                view.earlier_inflight = false;
                let Ok(page) = page else {
                    // A core from before pages answers nothing: the text stays as it is.
                    view.earlier_more = false;
                    return;
                };
                let events: Vec<PagedEvent> = page.events.into_iter().map(paged).collect();
                view.earlier_more = page.more && !events.is_empty();
                if events.is_empty() {
                    return;
                }
                if view.streamed.prepend(&events) {
                    view.prepended = true;
                    view.rows_before_page = Some(view.rows.borrow().len());
                    view.apply_log(cx);
                } else {
                    // Inside one long line: nothing new to show yet.
                    view.read_on(cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// A page brought nothing to show: read the one before it, twice as large, until rows
    /// come or the run's start does.
    fn read_on(&mut self, cx: &mut Context<Self>) {
        self.earlier_page = (self.earlier_page * 2).min(MAX_PAGE_BYTES);
        self.load_earlier(cx);
    }

    fn expand(&mut self, entry: usize, cx: &mut Context<Self>) {
        if entry >= EARLIER_RUN_ENTRIES {
            let run = entry / EARLIER_RUN_ENTRIES - 1;
            let idx = entry % EARLIER_RUN_ENTRIES;
            if let Some(earlier) = self.earlier_runs.get_mut(run) {
                let is_switch = matches!(
                    earlier.entries.get(idx),
                    Some(TranscriptEntry::Switch { .. })
                );
                if is_switch {
                    if !earlier.expanded.remove(&idx) {
                        earlier.expanded.insert(idx);
                    }
                    self.rebuild_rows(cx);
                } else if earlier.expanded.insert(idx) {
                    self.rebuild_rows(cx);
                }
            }
            return;
        }
        let switches = self
            .state
            .read(cx)
            .run(&self.run_id)
            .map(attempt_switches)
            .unwrap_or_default();
        let slice = enrich_transcript_switches(
            self.visible_entries(),
            &switches,
            !self.compact && !self.earlier_more,
        );
        if matches!(slice.get(entry), Some(TranscriptEntry::Switch { .. })) {
            if !self.expanded.remove(&entry) {
                self.expanded.insert(entry);
            }
            self.rebuild_rows(cx);
            return;
        }
        if !self.expanded.insert(entry) {
            return;
        }
        self.reveal_entry = Some(entry);
        self.reveal.snap(0.0);
        self.reveal.set_target(1.0);
        self.rebuild_rows(cx);
    }

    /// The prompt's bubble, which opens the run page's transcript once the text holds the
    /// run's start: a log read from its end gets it when the earlier pages are in.
    fn prompt_bubble(&self, cx: &App) -> Option<PromptBubble> {
        if self.compact || self.earlier_more {
            return None;
        }
        let state = self.state.read(cx);
        let run = state.run(&self.run_id)?;
        prompt_bubble(&run.prompt, &run.caller_agent, &run.created_at)
    }

    /// Whether [`Self::prompt_bubble`] has one, without building it.
    fn prompt_visible(&self, cx: &App) -> bool {
        !self.compact
            && !self.earlier_more
            && self
                .state
                .read(cx)
                .run(&self.run_id)
                .is_some_and(|run| !run.prompt.trim().is_empty())
    }

    fn set_prompt_expanded(&mut self, open: bool, cx: &mut Context<Self>) {
        if self.prompt_expanded == open {
            return;
        }
        self.prompt_expanded = open;
        self.rebuild_rows(cx);
    }

    fn live_now(&self, cx: &App) -> bool {
        self.state
            .read(cx)
            .run(&self.run_id)
            .is_some_and(|run| is_active_status(&run.status))
    }

    fn switches_len(&self, cx: &App) -> usize {
        self.state
            .read(cx)
            .run(&self.run_id)
            .map_or(0, |run| attempt_switches(run).len())
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
        // The reader scrolled away, or the glide to the end is taking the list there.
        if !self.stick || self.glide {
            self.follow = Follow::Idle;
            return;
        }
        let viewport = self.list.viewport_bounds();
        let count = self.list.item_count();
        if count == 0 || viewport.size.height <= px(0.0) || passes == Some(0) {
            self.follow = Follow::Idle;
            return;
        }
        let next = Follow::Settle(passes.unwrap_or(SETTLE_PASSES).saturating_sub(1));
        if self.list.bounds_for_item(count - 1).is_none() {
            // Not measured, so past the overdraw and far below: scroll past the end, which lays
            // the tail out from the bottom, then set the end on the edge.
            self.moved_here = true;
            self.list.scroll_to(ListOffset {
                item_ix: count,
                offset_in_item: px(0.0),
            });
            self.follow = next;
            window.request_animation_frame();
            return;
        }
        let left = page_scroll(&self.list).map_or(0.0, |page| {
            let scrolled = -f32::from(self.list.scroll_px_offset_for_scrollbar().y);
            page.content - page.viewport - scrolled
        });
        if left > 0.5 {
            self.moved_here = true;
            self.list.scroll_by(px(left));
            self.follow = next;
            window.request_animation_frame();
        } else {
            self.follow = Follow::Idle;
        }
    }

    /// Whether the reader is at the end: decided again each frame the reader moved the list
    /// (the wheel, or the scrollbar's drag), from how far the end is, `left`.
    fn track_stick(&mut self, scrolled: f32, left: f32, measured: bool) {
        let moved = Moved {
            wheel: self.reader_moved,
            here: self.moved_here || measured,
            by: scrolled - self.last_scrolled,
        };
        self.stick = sticks(self.stick, moved, self.glide, left);
        self.reader_moved = false;
        self.moved_here = false;
        self.last_scrolled = scrolled;
    }

    fn step_motion(&mut self, window: &mut Window) {
        let moving = !self.enter.settled()
            || !self.reveal.settled()
            || !self.end_button.settled()
            || self.glide;
        let dt = self.animator.dt(window, moving);
        self.frame_dt = dt;
        if dt > 0.0 {
            self.enter.step(dt);
            self.reveal.step(dt);
            self.end_button.step(dt);
        }
    }
}

impl TranscriptView {
    /// The text in bones while its log loads: a few lines in a preview, a page of them on
    /// the run page, at the column's measure under the header.
    fn loading(
        &self,
        top_inset: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let lines = if self.compact { 4 } else { 14 };
        div()
            .size_full()
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .when(!self.compact, |column| {
                column.items_center().pt(px(tw(PAGE_TOP) + top_inset + 8.0))
            })
            .child(
                div()
                    .w_full()
                    .max_w(px(WIDE_MEASURE))
                    .child(text_skeleton(lines, 12.0, window, cx)),
            )
            .into_any_element()
    }

    /// Keeps `inset` clear above the first row for a header that floats over the list.
    pub(crate) fn set_top_inset(&mut self, inset: f32, cx: &mut Context<Self>) {
        if (self.top_inset - inset).abs() > 0.5 {
            self.top_inset = inset;
            // The first row carries the inset: measure the rows again.
            self.measured_width.set(-1.0);
            cx.notify();
        }
    }

    /// Keeps `pad` clear after the rows, the scrollbar's gutter in it, for a list that runs
    /// to the page's edge.
    pub(crate) fn set_end_pad(&mut self, pad: f32, cx: &mut Context<Self>) {
        if (self.end_pad - pad).abs() > 0.5 {
            self.end_pad = pad;
            // The rows wrap in what is left: measure them again.
            self.measured_width.set(-1.0);
            cx.notify();
        }
    }

    /// The jump-to-end button: glide down to the end, where the log follows again.
    pub(crate) fn jump_to_end(&mut self, cx: &mut Context<Self>) {
        self.glide = true;
        self.stick = true;
        self.frame_dt = 0.0;
        self.end_button.set_target(0.0);
        cx.notify();
    }

    /// One frame of the glide to the end, `left` short of it in a `viewport` tall list: a
    /// long way is cut to one screen at once, and the last screen eases in, a share of what
    /// is left each frame.
    fn step_glide(&mut self, left: f32, viewport: f32, window: &mut Window) {
        if !self.glide {
            return;
        }
        if left <= 0.5 {
            self.glide = false;
            return;
        }
        self.moved_here = true;
        if left > viewport * 1.5 {
            self.list.scroll_by(px(left - viewport));
        } else {
            let dt = if self.frame_dt > 0.0 {
                self.frame_dt.min(0.05)
            } else {
                1.0 / 60.0
            };
            let share = 1.0 - (-dt * GLIDE_RATE).exp();
            self.list.scroll_by(px((left * share).max(1.0).min(left)));
        }
        window.request_animation_frame();
    }

    /// Scrolls the text `y` down (the screenshot tour).
    pub(crate) fn scroll_by(&mut self, y: f32, cx: &mut Context<Self>) {
        self.list.scroll_by(px(y));
        // As a reader's scroll: near the top it reads the page before.
        let first = self.list.logical_scroll_top().item_ix;
        self.on_scroll(true, first, cx);
        cx.notify();
    }

    /// Opens one of the run's images whole, by its artifact id.
    fn open_image(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(image) = self.images.iter().find(|image| image.id == id).cloned() else {
            return;
        };
        self.viewer.update(cx, |viewer, cx| viewer.open(image, cx));
    }
}

impl Render for TranscriptView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let top_inset = self.top_inset;
        // The scrollbar's gutter is part of the room after the rows, once the text overflows.
        let row_end = (self.end_pad
            - if self.overflowed.get() {
                SCROLLBAR_GUTTER
            } else {
                0.0
            })
        .max(0.0);
        self.ensure_stream(cx);
        self.ensure_earlier_runs(cx);
        self.ensure_images(cx);
        let live = self.live_now(cx);
        let switches_len = self.switches_len(cx);
        if live != self.shown_live
            || self.prompt_visible(cx) != self.prompt_shown
            || switches_len != self.shown_switches
        {
            self.shown_live = live;
            self.shown_switches = switches_len;
            self.rebuild_rows(cx);
        }
        self.step_motion(window);
        self.step_follow(window);
        if !self.continued && (!self.fetched || (self.parse_inflight && self.entries.is_empty())) {
            return self.loading(top_inset, window, cx);
        }
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
        let on_prompt_toggle: PromptToggle = Rc::new({
            let entity = entity.clone();
            move |open, _, cx| {
                entity.update(cx, |view, cx| view.set_prompt_expanded(open, cx));
            }
        });
        let on_image: Rc<dyn Fn(String, &mut Window, &mut App)> = Rc::new({
            let entity = entity.clone();
            move |id, _, cx| {
                entity.update(cx, |view, cx| view.open_image(&id, cx));
            }
        });
        let count = rows.borrow().len();
        let enter = self.enter.value();
        let reveal = self.reveal.value();
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
                cache: cache.clone(),
                on_expand: on_expand.clone(),
                on_prompt_toggle: on_prompt_toggle.clone(),
                on_read: Rc::new(|text: String, _, cx: &mut App| {
                    crate::voice::read_aloud(&text, cx);
                }),
                on_image: on_image.clone(),
            };
            let row = render_row(&frame, &row, window, cx);
            if compact {
                return row;
            }
            // `.pw-document__column > *`: at most the log's measure, centered. The space over
            // the transcript (and under the floating header) rides on the first row: a
            // list's own top padding stays put, and the rows would never pass beneath it.
            div()
                .w_full()
                .when(index == 0, |row| row.pt(px(tw(PAGE_TOP) + top_inset)))
                .pr(px(row_end))
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
        .when(!compact, |list| list.pb(px(tw(LOG_END_GAP) + COLUMN_END)));
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
        let remeasured = (width - self.measured_width.get()).abs() > 0.5;
        if remeasured {
            self.measured_width.set(width);
            let _ = self.list.clone().measure_all();
        }
        let page = page_scroll(&self.list);
        let overflows = page.is_some_and(|page| page.content > page.viewport + 0.5);
        self.overflowed.set(overflows);
        // Overflow and width are known once the list has laid out, so a frame whose layout
        // changes either draws again.
        // The fades under the toolbar and over the end read how far the text has scrolled.
        let scrolled = -f32::from(self.list.scroll_px_offset_for_scrollbar().y);
        let left = page.map_or(0.0, |page| {
            (page.content - page.viewport - scrolled).max(0.0)
        });
        self.track_stick(scrolled, left, remeasured);
        let watched = self.list.clone();
        let overflowed = self.overflowed.clone();
        let measured_width = self.measured_width.clone();
        let watcher = canvas(
            move |_, window, _| {
                let now =
                    page_scroll(&watched).is_some_and(|page| page.content > page.viewport + 0.5);
                let width = f32::from(watched.viewport_bounds().size.width);
                let at = -f32::from(watched.scroll_px_offset_for_scrollbar().y);
                if now != overflowed.get()
                    || (width - measured_width.get()).abs() > 0.5
                    || (at - scrolled).abs() > 0.5
                {
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
        let gutter = if overflows { SCROLLBAR_GUTTER } else { 0.0 };
        // The jump-to-end button: in while the reader is away from the end, out at it.
        let viewport = page.map_or(0.0, |page| page.viewport);
        self.step_glide(left, viewport, window);
        let away = overflows && !self.stick && left > JUMP_TO_END_REST && !self.glide;
        let target = if away { 1.0 } else { 0.0 };
        if (self.end_button.target() - target).abs() > 0.01 {
            self.end_button.set_target(target);
            window.request_animation_frame();
        }
        let jump = jump_to_end(
            SharedString::from(format!("transcript-end-{}", cx.entity_id())),
            "Jump to the end",
            self.end_button.value(),
            Rc::new({
                let entity = cx.entity();
                move |_, cx| entity.update(cx, |view, cx| view.jump_to_end(cx))
            }),
            cx,
        );
        let bottom = left.clamp(0.0, SCROLL_FADE);
        if bottom > 0.5 {
            area = area.child(
                div()
                    .absolute()
                    .bottom(px(0.0))
                    .left(px(0.0))
                    .right(px(gutter))
                    .h(px(bottom))
                    .bg(gpui::linear_gradient(
                        180.0,
                        gpui::linear_color_stop(
                            gpui::Rgba {
                                a: 0.0,
                                ..theme.color_surface
                            },
                            0.0,
                        ),
                        gpui::linear_color_stop(theme.color_surface, 1.0),
                    )),
            );
        }
        if let Some(jump) = jump {
            area = area.child(
                div()
                    .absolute()
                    .left(px(0.0))
                    // Centered on the text, which keeps the page's room after it.
                    .right(px(gutter.max(self.end_pad)))
                    .bottom(px(JUMP_INSET))
                    .flex()
                    .justify_center()
                    .child(jump),
            );
        }
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
                    track: (page.viewport - top_inset).max(0.0),
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
                    // The bar starts under the header, clear of its trail.
                    .top(px(top_inset))
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
        // The viewer a thumbnail opens: deferred over the whole window, and nothing at all
        // while it is closed.
        area = area.child(self.viewer.clone());
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
        content: items + tw(LOG_END_GAP) + COLUMN_END,
        viewport,
    })
}

/// How the list moved since the last frame: by the wheel, by the view itself (following,
/// gliding, keeping the reader's place, measuring at a new width), and by how many px.
#[derive(Clone, Copy)]
struct Moved {
    wheel: bool,
    here: bool,
    by: f32,
}

/// Whether the log follows its end after a frame that `moved` the list `left` short of it:
/// a move of the reader's decides it, anything else keeps it, and the glide to the end holds
/// it.
fn sticks(stick: bool, moved: Moved, glide: bool, left: f32) -> bool {
    let by_reader = moved.wheel || (!moved.here && moved.by.abs() > 0.5);
    if by_reader && !glide {
        left <= JUMP_TO_END_REST
    } else {
        stick
    }
}

/// Where the list's top goes back after rows from `start` on were spliced: gpui moves a top
/// row inside the spliced rows to their first one, at its start, which on a live log read at
/// its end jumps the text down to where the changed rows begin and then back up as the view
/// follows the end. The top row is the same stretch of text it was, so its place stays: a
/// row among the `tail` rows that end both lists alike keeps its distance from the end, and
/// one among the changed rows (the growing message) keeps its index.
fn kept_top(
    top: ListOffset,
    start: usize,
    old_len: usize,
    new_len: usize,
    tail: usize,
) -> Option<ListOffset> {
    if top.item_ix < start {
        return None;
    }
    let item_ix = if top.item_ix >= old_len.saturating_sub(tail) {
        new_len.saturating_sub(old_len.saturating_sub(top.item_ix))
    } else {
        top.item_ix
    };
    Some(ListOffset {
        item_ix: item_ix.min(new_len),
        offset_in_item: top.offset_in_item,
    })
}

fn split_stable(log: &str) -> (&str, &str) {
    match log.rfind('\n') {
        Some(index) => (&log[..=index], &log[index + 1..]),
        None => ("", log),
    }
}

/// Streams the run's log into `tx`, opening at its last [`TAIL_BYTES`]. A core from before
/// pages rejects the tail and ends the stream before any event: the watch then asks again
/// for the whole log, as that core expects.
fn produce_watch(client: CoreClient, run_id: String, tx: mpsc::UnboundedSender<StreamMsg>) {
    for tail in [TAIL_BYTES, 0] {
        let request = WatchRunRequest {
            id: run_id.clone(),
            after_seq: ProtoI64(0),
            tail_bytes: ProtoI64(tail),
        };
        match stream_watch(&client, &request, &tx) {
            Watched::Rejected if tail > 0 => continue,
            Watched::Gone => return,
            _ => break,
        }
    }
    let _ = tx.unbounded_send(StreamMsg::End);
}

enum Watched {
    /// The stream ran and ended.
    Ended,
    /// The core failed it before sending an event.
    Rejected,
    /// The view went away.
    Gone,
}

fn stream_watch(
    client: &CoreClient,
    request: &WatchRunRequest,
    tx: &mpsc::UnboundedSender<StreamMsg>,
) -> Watched {
    let Ok(iter) = client.watch_run(request) else {
        return Watched::Rejected;
    };
    let mut sent = false;
    for item in iter {
        match item {
            Ok(event) => {
                if tx.unbounded_send(StreamMsg::Event(paged(event))).is_err() {
                    return Watched::Gone;
                }
                sent = true;
            }
            Err(_) if !sent => return Watched::Rejected,
            Err(_) => break,
        }
    }
    Watched::Ended
}

/// The next message and every one already waiting behind it, up to [`WATCH_BATCH`]; `None`
/// once the producer is gone.
async fn next_batch(rx: &mut mpsc::UnboundedReceiver<StreamMsg>) -> Option<Vec<StreamMsg>> {
    let first = rx.next().await?;
    let mut batch = vec![first];
    while batch.len() < WATCH_BATCH {
        match rx.try_recv() {
            Ok(message) => batch.push(message),
            Err(_) => break,
        }
    }
    Some(batch)
}

#[cfg(test)]
mod tests {
    use super::kept_top;
    use gpui::{ListOffset, px};

    fn at(item_ix: usize, offset: f32) -> ListOffset {
        ListOffset {
            item_ix,
            offset_in_item: px(offset),
        }
    }

    #[test]
    fn a_live_splice_keeps_the_readers_row() {
        // Read at the end: the growing message (row 40) is the top row; it and the gap and
        // thinking bot after it are spliced. The top stays inside it, where it was.
        let kept = kept_top(at(40, 120.0), 40, 43, 43, 2).unwrap();
        assert_eq!((kept.item_ix, kept.offset_in_item), (40, px(120.0)));
        // The message split into two rows: a top among the rows after it follows them.
        let kept = kept_top(at(42, 6.0), 40, 43, 44, 2).unwrap();
        assert_eq!((kept.item_ix, kept.offset_in_item), (43, px(6.0)));
        // Rows changed below the top: gpui keeps it, nothing to do.
        assert!(kept_top(at(12, 0.0), 40, 43, 44, 2).is_none());
    }
    use super::{Moved, sticks};

    const STILL: Moved = Moved {
        wheel: false,
        here: false,
        by: 0.0,
    };

    #[test]
    fn the_log_lets_go_of_its_end_only_when_the_reader_scrolls_away() {
        // New lines push the end away while nobody scrolls: the log keeps following.
        assert!(sticks(true, STILL, false, 400.0));
        // The view's own scroll toward the end is no reader's.
        let here = Moved {
            here: true,
            by: 300.0,
            ..STILL
        };
        assert!(sticks(true, here, false, 0.0));
        // The wheel up, away from the end: it lets go, and new lines leave the reader there.
        let up = Moved {
            wheel: true,
            by: -200.0,
            ..STILL
        };
        assert!(!sticks(true, up, false, 200.0));
        assert!(!sticks(false, STILL, false, 900.0));
        // The scrollbar's drag moves it with no wheel: the same.
        let drag = Moved {
            by: -120.0,
            ..STILL
        };
        assert!(!sticks(true, drag, false, 120.0));
        // Back down to the end, by the wheel or the drag: it follows again.
        let down = Moved {
            wheel: true,
            by: 200.0,
            ..STILL
        };
        assert!(sticks(false, down, false, 10.0));
        // The jump-to-end button's glide holds it while the list moves.
        assert!(sticks(true, up, true, 200.0));
    }
}
