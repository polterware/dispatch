//! Run page: transcript in the center, properties in the right rail.
//!
//! The page is a `RailPage` (`wide splitScroll railCard`): the transcript in the column and
//! the run's properties in `RailGroup`s on the card rail, which the header's last button hides
//! and shows (remembered as `polterware.run.rail`). Header actions call the core on the
//! background executor; they use the same button colors with fast hover and press springs.

use std::rc::Rc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, Font, FontWeight, Hsla, IntoElement,
    MouseButton, SharedString, StyledText, TextRun, WeakEntity, Window, div, prelude::*, px,
};

use crate::avatar::agent_avatar;
use crate::client::{
    CancelRunRequest, CleanRunRequest, CoreClient, CoreError, DispatchRun, DispatchWorktree,
    GetRouteDecisionRequest, ListWorktreesRequest, MergeRunRequest, RecoverRunRequest,
    ResultRequest,
};
use crate::models::run_page::{
    AttemptCard, LinkedRow, MoreAction, ReviewAction, RouteDecision, TokenUsage,
    agent_attempt_cards, area_label, candidate_price, coalesce_text, context_usage,
    decision_summary, duration_label, file_counts, format_duration, format_tokens, has_worktree,
    is_run_active, linked_rows, map_route_decision, policy_label, proto_ms, rail_tokens,
    role_summary, route_label, run_cost, run_more_actions, run_review_action, stored_usage,
    timing_label, token_breakdown, total_tokens, worktree_display_status,
};
use crate::motion::{Animator, SpringTier, SpringValue};
use crate::state::{AppState, ConnectionState, Route, observe};
use crate::theme::{Theme, status_tone, tw};
use crate::ui::ellipsis;
use crate::ui::parts::run_page::{context_meter, file_change_line, prompt_text};
use crate::ui::{
    Button, ButtonKind, ButtonSize, Icon, MascotMood, Menu, MenuEntry, PageWidth, RailGroup,
    RailPresence, ToastTone, chip, mono, page_empty_with, rail_page_body, rail_page_header,
    rail_properties, rail_property, rail_toggle_label, sans, tabular, toolbar, tooltip,
};

/// Where the rail's open state is remembered, as `railStorageKey` names it.
const RAIL_PREFERENCE: &str = "polterware.run.rail";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Op {
    Cancel,
    Merge,
    Delete,
    Recover,
    Remove,
}

struct Pack {
    run: DispatchRun,
    links: Vec<LinkedRow>,
    decision: Option<RouteDecision>,
    token_line: Option<TokenUsage>,
    cost_line: Option<TokenUsage>,
    branch: String,
    path: String,
    chip_status: String,
    context_fill: f32,
    now_ms: i64,
}

pub struct RunPageView {
    state: Entity<AppState>,
    transcript: Entity<super::transcript::TranscriptView>,
    transcript_run: String,
    agent: Entity<RailGroup>,
    prompt: Entity<RailGroup>,
    run_section: Entity<RailGroup>,
    routing: Entity<RailGroup>,
    changes: Entity<RailGroup>,
    linked: Entity<RailGroup>,
    menu: Entity<Menu>,
    cancelling: bool,
    reviewing: Option<Op>,
    confirm_open: bool,
    confirm_progress: SpringValue,
    decision: Option<RouteDecision>,
    worktree: Option<DispatchWorktree>,
    output_log: String,
    meta_for: String,
    log_for: String,
    side_inflight: bool,
    side_gen: u64,
    now_ms: i64,
    clock_on: bool,
    motion_run: String,
    context_fill: SpringValue,
    animator: Animator,
    token_line: Option<TokenUsage>,
    cost_line: Option<TokenUsage>,
    snapshot_run: Option<DispatchRun>,
    snapshot_links: Vec<LinkedRow>,
    branch: String,
    path: String,
    chip_status: String,
    /// Beside the text: the remembered choice. On a narrow page the drawer, which starts closed.
    rail_open: bool,
    drawer_open: bool,
    rail: RailPresence,
    page_width: PageWidth,
}

impl RunPageView {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        observe(&state, cx);
        let run_id = match state.read(cx).route() {
            Route::Run(id) => id.clone(),
            _ => String::new(),
        };
        let transcript = cx.new(|cx| {
            super::transcript::TranscriptView::new(state.clone(), run_id.clone(), false, cx)
        });
        let agent = cx.new(|cx| RailGroup::new("Agent", Icon::Bot, cx));
        let prompt = cx.new(|cx| RailGroup::new("Prompt", Icon::FileText, cx));
        let run_section = cx.new(|cx| RailGroup::new("Run", Icon::InformationCircle, cx));
        let routing = cx.new(|cx| RailGroup::new("Routing", Icon::Route, cx));
        let changes = cx.new(|cx| RailGroup::new("Changes", Icon::FileDiff, cx));
        let linked = cx.new(|cx| RailGroup::new("Linked runs", Icon::Route, cx));
        let rail_open = state
            .read(cx)
            .read_preference::<String>(RAIL_PREFERENCE)
            .ok()
            .flatten()
            .is_none_or(|value| value != "hidden");
        let menu = cx.new(Menu::new);
        let mut view = Self {
            state,
            transcript,
            transcript_run: run_id,
            agent,
            prompt,
            run_section,
            routing,
            changes,
            linked,
            menu,
            cancelling: false,
            reviewing: None,
            confirm_open: false,
            confirm_progress: SpringValue::new(0.0, SpringTier::Moderate),
            decision: None,
            worktree: None,
            output_log: String::new(),
            meta_for: String::new(),
            log_for: String::new(),
            side_inflight: false,
            side_gen: 0,
            now_ms: unix_now_ms(),
            clock_on: false,
            motion_run: String::new(),
            context_fill: SpringValue::new(0.0, SpringTier::Moderate),
            animator: Animator::new(),
            token_line: None,
            cost_line: None,
            snapshot_run: None,
            snapshot_links: Vec::new(),
            branch: String::new(),
            path: String::new(),
            chip_status: String::new(),
            rail_open,
            drawer_open: false,
            rail: RailPresence::new(rail_open),
            page_width: PageWidth::default(),
        };
        view.install_bodies(cx);
        view
    }

    fn install_bodies(&mut self, cx: &mut Context<Self>) {
        let weak = cx.weak_entity();
        bind_body(&self.agent, weak.clone(), paint_agent, cx);
        bind_body(&self.prompt, weak.clone(), paint_prompt, cx);
        bind_body(&self.run_section, weak.clone(), paint_run, cx);
        bind_body(&self.routing, weak.clone(), paint_routing, cx);
        bind_body(&self.changes, weak.clone(), paint_changes, cx);
        bind_body(&self.linked, weak, paint_linked, cx);
    }

    fn leave(&mut self, cx: &mut Context<Self>) {
        self.state.update(cx, |state, cx| {
            if !state.go_back() {
                state.navigate(Route::Runs);
            }
            state.request_refresh(cx);
            cx.notify();
        });
        cx.notify();
    }

    /// The rail toggle: beside the text it flips the remembered choice; on a narrow page it
    /// opens and closes the drawer.
    fn toggle_rail(&mut self, cx: &mut Context<Self>) {
        if self.page_width.is_narrow() {
            self.drawer_open = !self.drawer_open;
        } else {
            self.rail_open = !self.rail_open;
            let value = if self.rail_open { "open" } else { "hidden" };
            let _ = self
                .state
                .read(cx)
                .write_preference(RAIL_PREFERENCE, &value);
        }
        cx.notify();
    }

    fn rail_visible(&self) -> bool {
        if self.page_width.is_narrow() {
            self.drawer_open
        } else {
            self.rail_open
        }
    }

    fn open_run(&mut self, id: String, cx: &mut Context<Self>) {
        self.state.update(cx, |state, cx| {
            state.navigate(Route::Run(id));
            state.request_refresh(cx);
            cx.notify();
        });
        cx.notify();
    }

    fn toast(&mut self, message: impl Into<SharedString>, tone: ToastTone, cx: &mut Context<Self>) {
        let message = message.into();
        self.state.update(cx, |state, cx| {
            state.push_toast(message, tone);
            cx.notify();
        });
    }

    fn start_op(&mut self, id: String, op: Op, cx: &mut Context<Self>) {
        if id.is_empty() {
            return;
        }
        let blocked = match op {
            Op::Cancel => self.cancelling,
            _ => self.reviewing.is_some(),
        };
        if blocked {
            return;
        }
        let Some(client) = self.state.read(cx).client() else {
            self.toast("Dispatch core is not running", ToastTone::Danger, cx);
            return;
        };
        match op {
            Op::Cancel => self.cancelling = true,
            other => self.reviewing = Some(other),
        }
        cx.notify();
        cx.spawn(async move |this, async_cx| {
            let result = async_cx
                .background_spawn(async move { perform(&client, &id, op) })
                .await;
            let _ = this.update(async_cx, |view, cx| view.finish_op(op, result, cx));
        })
        .detach();
    }

    fn finish_op(&mut self, op: Op, result: Result<bool, CoreError>, cx: &mut Context<Self>) {
        match op {
            Op::Cancel => self.cancelling = false,
            _ => self.reviewing = None,
        }
        let (message, tone, refresh) = match (op, result) {
            (Op::Cancel, Ok(_)) => ("Agent run cancelled.".to_string(), ToastTone::Success, true),
            (Op::Cancel, Err(error)) => (cancel_error(error), ToastTone::Danger, false),
            (Op::Merge, Ok(true)) => ("Run merged.".to_string(), ToastTone::Success, true),
            (Op::Merge, Ok(false)) => (
                "The merge reported a failure.".to_string(),
                ToastTone::Danger,
                true,
            ),
            (Op::Merge, Err(_)) => (
                "Could not merge the run.".to_string(),
                ToastTone::Danger,
                false,
            ),
            (Op::Delete, Ok(true)) => ("Worktree deleted.".to_string(), ToastTone::Success, true),
            (Op::Delete, Ok(false)) => (
                "The operation reported a failure.".to_string(),
                ToastTone::Danger,
                true,
            ),
            (Op::Delete, Err(_)) => (
                "Could not delete the worktree.".to_string(),
                ToastTone::Danger,
                false,
            ),
            (Op::Recover, Ok(true)) => ("Run recovered.".to_string(), ToastTone::Success, true),
            (Op::Remove, Ok(true)) => ("Worktree removed.".to_string(), ToastTone::Success, true),
            (Op::Recover | Op::Remove, Ok(false)) => (
                "The operation reported a failure.".to_string(),
                ToastTone::Danger,
                true,
            ),
            (Op::Recover | Op::Remove, Err(_)) => (
                "Could not update the run.".to_string(),
                ToastTone::Danger,
                false,
            ),
        };
        if refresh {
            self.meta_for.clear();
            self.log_for.clear();
            self.side_gen = self.side_gen.wrapping_add(1);
            self.side_inflight = false;
            self.state.update(cx, |state, cx| {
                state.request_refresh(cx);
            });
        }
        self.toast(message, tone, cx);
        cx.notify();
    }

    fn toggle_menu(&mut self, cx: &mut Context<Self>) {
        let open = !self.menu.read(cx).is_open();
        self.menu.update(cx, |menu, cx| {
            menu.set_open(open);
            cx.notify();
        });
    }

    fn sync_menu(&mut self, run_id: &str, actions: &[MoreAction], cx: &mut Context<Self>) {
        let weak = cx.weak_entity();
        let run_id = run_id.to_string();
        let mut items = Vec::new();
        for action in actions {
            let weak = weak.clone();
            let run_id = run_id.clone();
            let action = *action;
            let (label, danger) = match action {
                MoreAction::Recover => ("Recover run", false),
                MoreAction::Clean => ("Remove worktree", true),
            };
            items.push(MenuEntry {
                label: label.into(),
                icon: None,
                danger,
                on_select: Rc::new(move |_window, cx| {
                    let _ = weak.update(cx, |view, cx| {
                        if view.reviewing.is_some() {
                            return;
                        }
                        match action {
                            MoreAction::Recover => {
                                view.start_op(run_id.clone(), Op::Recover, cx);
                            }
                            MoreAction::Clean => {
                                view.confirm_open = true;
                                cx.notify();
                            }
                        }
                    });
                }),
            });
        }
        self.menu.update(cx, |menu, _| menu.set_items(items));
    }

    fn ensure_side(&mut self, run: &DispatchRun, cx: &mut Context<Self>) {
        let active = is_run_active(&run.status);
        let stamp = log_stamp(run);
        let need_meta = self.meta_for != run.id;
        let need_log = active && self.log_for != stamp;
        if self.side_inflight || (!need_meta && !need_log) {
            return;
        }
        let Some(client) = self.state.read(cx).client() else {
            return;
        };
        self.side_inflight = true;
        self.side_gen = self.side_gen.wrapping_add(1);
        let side_generation = self.side_gen;
        let run_id = run.id.clone();
        let project_id = run.project_id.clone();
        cx.spawn(async move |this, async_cx| {
            let loaded = async_cx
                .background_spawn(async move {
                    let decision = client
                        .get_route_decision(&GetRouteDecisionRequest {
                            run_id: run_id.clone(),
                        })
                        .ok();
                    let worktrees = client
                        .list_worktrees(&ListWorktreesRequest { project_id })
                        .ok();
                    let log = client
                        .result(&ResultRequest {
                            id: run_id.clone(),
                            section: "output".to_string(),
                            offset: 0,
                            limit: 0,
                        })
                        .ok()
                        .map(|response| response.content);
                    (decision, worktrees, log, run_id)
                })
                .await;
            let _ = this.update(async_cx, |view, cx| {
                if view.side_gen != side_generation {
                    return;
                }
                view.side_inflight = false;
                let current = match view.state.read(cx).route() {
                    Route::Run(id) => id.clone(),
                    _ => String::new(),
                };
                if current != loaded.3 {
                    cx.notify();
                    return;
                }
                view.decision = loaded
                    .0
                    .and_then(|response| map_route_decision(response.ok, &response.json));
                view.worktree = loaded.1.and_then(|response| {
                    response
                        .worktrees
                        .into_iter()
                        .find(|item| item.run_id == loaded.3)
                });
                if let Some(log) = loaded.2 {
                    view.output_log = log;
                }
                view.log_for = stamp;
                view.meta_for = loaded.3;
                cx.notify();
            });
        })
        .detach();
    }

    fn ensure_clock(&mut self, active: bool, cx: &mut Context<Self>) {
        if !active {
            self.clock_on = false;
            return;
        }
        if self.clock_on {
            return;
        }
        self.clock_on = true;
        cx.spawn(async move |this, async_cx| {
            loop {
                async_cx
                    .background_executor()
                    .timer(Duration::from_secs(15))
                    .await;
                let stop = this
                    .update(async_cx, |view, cx| {
                        let active = {
                            let state = view.state.read(cx);
                            match state.route().clone() {
                                Route::Run(id) => {
                                    state.run(&id).is_some_and(|run| is_run_active(&run.status))
                                }
                                _ => false,
                            }
                        };
                        if !view.clock_on || !active {
                            view.clock_on = false;
                            return true;
                        }
                        view.now_ms = unix_now_ms();
                        cx.notify();
                        false
                    })
                    .unwrap_or(true);
                if stop {
                    break;
                }
            }
        })
        .detach();
    }

    fn sync_transcript(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.transcript_run == id {
            return;
        }
        let state = self.state.clone();
        let owned = id.to_string();
        self.transcript =
            cx.new(|cx| super::transcript::TranscriptView::new(state, owned, false, cx));
        self.transcript_run = id.to_string();
    }

    fn apply_run(&mut self, run: &DispatchRun, reply: &str, cx: &mut Context<Self>) {
        if self.transcript_run != run.id {
            self.output_log.clear();
            self.log_for.clear();
            self.meta_for.clear();
            self.decision = None;
            self.worktree = None;
            self.side_gen = self.side_gen.wrapping_add(1);
            self.side_inflight = false;
        }
        let switched = self.motion_run != run.id;
        if switched {
            self.motion_run = run.id.clone();
            self.confirm_open = false;
            self.confirm_progress.snap(0.0);
        }
        let branch = coalesce_text(
            self.worktree
                .as_ref()
                .map(|item| item.branch.as_str())
                .unwrap_or(""),
            &run.branch,
        );
        let path = coalesce_text(
            self.worktree
                .as_ref()
                .map(|item| item.path.as_str())
                .unwrap_or(""),
            &run.worktree,
        );
        let worktree_status = coalesce_text(
            &run.worktree_status,
            self.worktree
                .as_ref()
                .map(|item| item.status.as_str())
                .unwrap_or(""),
        );
        self.branch = branch;
        self.path = path;
        self.chip_status = worktree_display_status(&run.status, &worktree_status);
        let stored = stored_usage(&run.usage);
        let live = is_run_active(&run.status);
        let log = self.output_log.clone();
        self.token_line = rail_tokens(stored.as_ref(), &[&log, reply], live);
        self.cost_line = stored;
        let fraction = self
            .token_line
            .as_ref()
            .and_then(|usage| context_usage(usage.context_tokens, usage.context_window))
            .and_then(|meter| meter.percent)
            .map(|percent| percent as f32 / 100.0)
            .unwrap_or(0.0);
        if switched {
            self.context_fill.snap(fraction);
            if live {
                self.now_ms = unix_now_ms();
            }
        } else {
            self.context_fill.set_target(fraction);
        }
        self.ensure_side(run, cx);
        self.ensure_clock(live, cx);
        self.sync_transcript(&run.id, cx);
    }

    fn step_motion(&mut self, window: &mut Window) {
        self.confirm_progress
            .set_target(if self.confirm_open { 1.0 } else { 0.0 });
        let moving = !self.confirm_progress.settled() || !self.context_fill.settled();
        let dt = self.animator.dt(window, moving);
        if dt > 0.0 {
            self.confirm_progress.step(dt);
            self.context_fill.step(dt);
        }
    }

    /// Back, a texture icon button.
    fn toolbar_start(&self, weak: WeakEntity<Self>) -> AnyElement {
        Button::new("run-back", ButtonKind::Secondary)
            .size(ButtonSize::Icon)
            .icon(Icon::Back)
            .tooltip("Back")
            .on_click(move |_, cx| {
                let _ = weak.update(cx, |view, cx| view.leave(cx));
            })
            .into_any_element()
    }

    /// The next step: Cancel while the run works, Merge for a finished run with a worktree
    /// that is not merged, Delete worktree once it is, each a small button with a 12px mark;
    /// then "…" with Recover run and Remove worktree, when the run offers either.
    fn toolbar_end(&self, run: &DispatchRun, weak: WeakEntity<Self>) -> AnyElement {
        let review = run_review_action(
            &run.status,
            &run.merged_at,
            &run.worktree_status,
            has_worktree(&self.branch, &self.path),
        );
        let review_busy = self.reviewing.is_some();
        let mut row = div().flex().items_center().gap(px(8.0));
        if is_run_active(&run.status) {
            let open = weak.clone();
            let id = run.id.clone();
            row = row.child(
                Button::new("run-cancel", ButtonKind::Ghost)
                    .size(ButtonSize::Sm)
                    .icon(Icon::Stop)
                    .icon_size(12.0)
                    .label("Cancel")
                    .disabled(self.cancelling)
                    .on_click(move |_, cx| {
                        let id = id.clone();
                        let _ = open.update(cx, |view, cx| view.start_op(id, Op::Cancel, cx));
                    }),
            );
        }
        if review == Some(ReviewAction::Merge) {
            let open = weak.clone();
            let id = run.id.clone();
            let label = if self.reviewing == Some(Op::Merge) {
                "Merging…"
            } else {
                "Merge"
            };
            row = row.child(
                Button::new("run-merge", ButtonKind::Primary)
                    .size(ButtonSize::Sm)
                    .icon(Icon::GitBranch)
                    .icon_size(12.0)
                    .label(label)
                    .disabled(review_busy)
                    .on_click(move |_, cx| {
                        let id = id.clone();
                        let _ = open.update(cx, |view, cx| view.start_op(id, Op::Merge, cx));
                    }),
            );
        }
        if review == Some(ReviewAction::Delete) {
            let open = weak.clone();
            let id = run.id.clone();
            let label = if review_busy {
                "Deleting…"
            } else {
                "Delete worktree"
            };
            row = row.child(
                Button::new("run-delete-worktree", ButtonKind::Ghost)
                    .size(ButtonSize::Sm)
                    .icon(Icon::Delete)
                    .icon_size(12.0)
                    .label(label)
                    .disabled(review_busy)
                    .on_click(move |_, cx| {
                        let id = id.clone();
                        let _ = open.update(cx, |view, cx| view.start_op(id, Op::Delete, cx));
                    }),
            );
        }
        let more = run_more_actions(
            &run.status,
            &run.merged_at,
            has_worktree(&self.branch, &self.path),
        );
        if !more.is_empty() {
            row = row.child(
                Button::new("run-more", ButtonKind::Ghost)
                    .size(ButtonSize::Icon)
                    .icon(Icon::More)
                    .tooltip("More run actions")
                    .on_click(move |_, cx| {
                        let _ = weak.update(cx, |view, cx| view.toggle_menu(cx));
                    }),
            );
        }
        row.into_any_element()
    }

    fn confirm_layer(&self, run_id: &str, weak: WeakEntity<Self>, theme: Theme) -> AnyElement {
        let t = self.confirm_progress.value().clamp(0.0, 1.0);
        if !self.confirm_open && self.confirm_progress.settled() {
            return div().into_any_element();
        }
        let dismiss = weak.clone();
        let open_cancel = weak.clone();
        let open_remove = weak.clone();
        let id = run_id.to_string();
        let enabled = self.reviewing.is_none();
        div()
            .absolute()
            .size_full()
            .occlude()
            .flex()
            .items_center()
            .justify_center()
            .px(px(theme.space_6))
            .bg(theme.color_overlay)
            .opacity(t)
            .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                let _ = dismiss.update(cx, |view, cx| {
                    view.confirm_open = false;
                    cx.notify();
                });
            })
            .child(
                div()
                    .w(px(420.0))
                    .max_w_full()
                    .p(px(theme.space_5))
                    .flex()
                    .flex_col()
                    .gap(px(theme.space_3))
                    .rounded(px(theme.radius_dialog))
                    .bg(theme.color_surface_raised)
                    .border_1()
                    .border_color(theme.color_border)
                    .mt(px((1.0 - t) * 8.0))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .text_size(px(theme.font_size_lg))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child("Remove the worktree?"),
                    )
                    .child(
                        div()
                            .whitespace_normal()
                            .text_size(px(theme.font_size_sm))
                            .text_color(theme.color_text_secondary)
                            .child("The run's worktree and branch are deleted, with any change not merged."),
                    )
                    .child(
                        div()
                            .text_size(px(theme.font_size_sm))
                            .text_color(theme.color_text_muted)
                            .child("This cannot be undone."),
                    )
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap(px(theme.space_2))
                            .child(
                                Button::new("run-confirm-cancel", ButtonKind::Secondary)
                                    .size(ButtonSize::Sm)
                                    .label("Cancel")
                                    .on_click(move |_, cx| {
                                        let _ = open_cancel.update(cx, |view, cx| {
                                            view.confirm_open = false;
                                            cx.notify();
                                        });
                                    }),
                            )
                            .child(
                                Button::new("run-confirm-remove", ButtonKind::Danger)
                                    .size(ButtonSize::Sm)
                                    .label("Remove worktree")
                                    .disabled(!enabled)
                                    .on_click(move |_, cx| {
                                        let id = id.clone();
                                        let _ = open_remove.update(cx, |view, cx| {
                                            view.confirm_open = false;
                                            view.start_op(id, Op::Remove, cx);
                                        });
                                    }),
                            ),
                    ),
            )
            .into_any_element()
    }
}

impl Render for RunPageView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *Theme::global(cx);
        let (route_id, connection, run, links, reply) = {
            let state = self.state.read(cx);
            let route_id = match state.route() {
                Route::Run(id) => id.clone(),
                _ => String::new(),
            };
            let connection = state.connection().clone();
            let run = state.run(&route_id).cloned();
            let links = run
                .as_ref()
                .map(|run| linked_rows(run, state.runs()))
                .unwrap_or_default();
            let reply = state.run_reply(&route_id).unwrap_or("").to_string();
            (route_id, connection, run, links, reply)
        };

        if let Some(run) = run.as_ref() {
            self.apply_run(run, &reply, cx);
            self.snapshot_run = Some(run.clone());
            self.snapshot_links = links;
        } else {
            self.snapshot_run = None;
            self.snapshot_links.clear();
            self.token_line = None;
            self.cost_line = None;
            self.clock_on = false;
        }
        self.step_motion(window);

        let prompt = self
            .snapshot_run
            .as_ref()
            .map(|run| run.prompt.trim().to_string())
            .unwrap_or_default();
        let show_prompt = !prompt.is_empty();
        let show_routing = self.decision.is_some();
        let show_linked = !self.snapshot_links.is_empty();
        if let Some(run) = self.snapshot_run.clone() {
            let actions = run_more_actions(
                &run.status,
                &run.merged_at,
                has_worktree(&self.branch, &self.path),
            );
            self.sync_menu(&run.id, &actions, cx);
        } else {
            self.sync_menu("", &[], cx);
        }

        let weak = cx.weak_entity();
        let visible = self.rail_visible();
        self.rail.set_open(visible);
        let bar = if let Some(run) = self.snapshot_run.clone() {
            let toggle_view = weak.clone();
            let end = div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(self.toolbar_end(&run, weak.clone()))
                .child(
                    Button::new("run-rail-toggle", ButtonKind::Secondary)
                        .size(ButtonSize::Icon)
                        .icon(Icon::SidebarRightIcon)
                        .tooltip(rail_toggle_label(visible))
                        .selected(visible)
                        .on_click(move |_, cx| {
                            let _ = toggle_view.update(cx, |view, cx| view.toggle_rail(cx));
                        }),
                );
            rail_page_header(self.toolbar_start(weak.clone()), end, cx).into_any_element()
        } else {
            toolbar(self.toolbar_start(weak.clone()), div(), cx).into_any_element()
        };

        let mut drawer = None;
        let body = if self.snapshot_run.is_some() {
            let agent = self.agent.clone();
            let prompt_section = self.prompt.clone();
            let run_section = self.run_section.clone();
            let routing = self.routing.clone();
            let changes = self.changes.clone();
            let linked = self.linked.clone();
            let rail = div()
                .w_full()
                .flex()
                .flex_col()
                .child(agent)
                .when(show_prompt, |rail| rail.child(prompt_section))
                .child(run_section)
                .when(show_routing, |rail| rail.child(routing))
                .child(changes)
                .when(show_linked, |rail| rail.child(linked))
                .into_any_element();
            // `flex min-w-0 flex-col gap-6 pt-2` around the transcript and `logEnd`: the
            // transcript's list holds the `pt-2` and the gap as its padding, so they scroll.
            let column = div()
                .flex_1()
                .min_h(px(0.0))
                .w_full()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .child(self.transcript.clone());
            let (row, over) =
                rail_page_body(column, rail, &self.rail, &self.page_width, window, cx);
            drawer = over;
            row.into_any_element()
        } else {
            missing_body(&connection, theme, cx)
        };

        let confirm_id = self
            .snapshot_run
            .as_ref()
            .map(|run| run.id.clone())
            .unwrap_or_else(|| route_id);
        let menu = self.menu.clone();
        div()
            .relative()
            .size_full()
            .min_w(px(0.0))
            .flex()
            .flex_col()
            .text_color(theme.color_text_primary)
            .child(self.page_width.probe())
            .child(bar)
            .child(body)
            .children(drawer)
            .child(
                div()
                    .absolute()
                    .top(px(0.0))
                    // Under "…", which sits before the rail toggle.
                    .right(px(theme.space_4 + 36.0))
                    .w(px(200.0))
                    .child(menu),
            )
            .child(self.confirm_layer(&confirm_id, weak, theme))
    }
}

fn bind_body(
    section: &Entity<RailGroup>,
    weak: WeakEntity<RunPageView>,
    paint: fn(&WeakEntity<RunPageView>, &mut Window, &mut App) -> AnyElement,
    cx: &mut Context<RunPageView>,
) {
    section.update(cx, |section, _| {
        section.set_body(Rc::new(move |window, app| paint(&weak, window, app)));
    });
}

fn perform(client: &CoreClient, id: &str, op: Op) -> Result<bool, CoreError> {
    match op {
        Op::Cancel => client
            .cancel_run(&CancelRunRequest { id: id.to_string() })
            .map(|_| true),
        Op::Merge => client
            .merge_run(&MergeRunRequest {
                id: id.to_string(),
                keep_worktree: true,
            })
            .map(|response| response.ok),
        Op::Delete => client
            .clean_run(&CleanRunRequest {
                id: id.to_string(),
                force: false,
            })
            .map(|response| response.ok),
        Op::Recover => client
            .recover_run(&RecoverRunRequest { id: id.to_string() })
            .map(|response| response.ok),
        Op::Remove => client
            .clean_run(&CleanRunRequest {
                id: id.to_string(),
                force: true,
            })
            .map(|response| response.ok),
    }
}

fn cancel_error(error: CoreError) -> String {
    match error {
        CoreError::Rpc(message) if !message.is_empty() => message,
        _ => "Could not cancel dispatch run.".to_string(),
    }
}

fn unix_now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

fn log_stamp(run: &DispatchRun) -> String {
    format!(
        "{}:{}:{}:{}:{}:{}:{}:{}",
        run.id,
        run.status,
        run.usage.input_tokens.0,
        run.usage.cache_read_tokens.0,
        run.usage.cache_write_tokens.0,
        run.usage.output_tokens.0,
        run.usage.context_tokens.0,
        run.finished_at,
    )
}

fn read_pack(view: &WeakEntity<RunPageView>, cx: &App) -> Option<Pack> {
    let entity = view.upgrade()?;
    let page = entity.read(cx);
    let run = page.snapshot_run.clone()?;
    Some(Pack {
        links: page.snapshot_links.clone(),
        decision: page.decision.clone(),
        token_line: page.token_line.clone(),
        cost_line: page.cost_line.clone(),
        branch: page.branch.clone(),
        path: page.path.clone(),
        chip_status: page.chip_status.clone(),
        context_fill: page.context_fill.value(),
        now_ms: page.now_ms,
        run,
    })
}

/// `RunAgentBar`: the agent that runs (or ran) the job, its 28px bot animated while it works,
/// its title, and its measured duration; failed attempts before it follow as 18px bots and a
/// line each with their error. `px-1` around it.
fn paint_agent(view: &WeakEntity<RunPageView>, window: &mut Window, cx: &mut App) -> AnyElement {
    let _ = window;
    let Some(pack) = read_pack(view, cx) else {
        return div().into_any_element();
    };
    let theme = *Theme::global(cx);
    let cards = agent_attempt_cards(&pack.run);
    let Some(current) = cards.last() else {
        return div().into_any_element();
    };
    let duration = current.duration_ms.map(format_duration);
    let lead = div()
        .flex()
        .min_w(px(0.0))
        .items_center()
        .gap(px(tw(2.5)))
        .child(agent_avatar(&current.agent, &pack.run.status, 28.0))
        .child(
            div()
                .min_w(px(0.0))
                .text_size(px(13.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.color_text_primary)
                .child(ellipsis(current_title(&pack.run, current))),
        )
        .children(duration.map(|duration| {
            div()
                .flex_none()
                .font(tabular(sans(&theme, FontWeight::NORMAL)))
                .text_size(px(12.0))
                .text_color(theme.color_text_muted)
                .child(duration)
        }));
    let mut bar = div()
        .flex()
        .min_w(px(0.0))
        .flex_col()
        .gap(px(tw(1.5)))
        .child(lead);
    for card in &cards[..cards.len() - 1] {
        bar = bar.child(
            div()
                .flex()
                .min_w(px(0.0))
                .items_center()
                .gap(px(tw(2.0)))
                .pl(px(tw(1.0)))
                .text_size(px(12.0))
                .text_color(theme.color_text_muted)
                .child(agent_avatar(&card.agent, "failed", 18.0))
                .child(div().flex_none().child(card.title.clone()))
                .when(!card.error.is_empty(), |row| {
                    row.child(
                        div()
                            .min_w(px(0.0))
                            .text_color(theme.color_danger)
                            .child(ellipsis(card.error.clone())),
                    )
                }),
        );
    }
    div().px(px(tw(1.0))).child(bar).into_any_element()
}

fn current_title(run: &DispatchRun, card: &AttemptCard) -> String {
    if !card.is_final {
        return card.title.clone();
    }
    let provider = run.provider.trim();
    if provider.is_empty() || card.title.split(" / ").any(|part| part == provider) {
        return card.title.clone();
    }
    if card.title == "\u{2014}" {
        provider.to_string()
    } else {
        format!("{} / {provider}", card.title)
    }
}

fn paint_prompt(view: &WeakEntity<RunPageView>, window: &mut Window, cx: &mut App) -> AnyElement {
    let _ = window;
    let Some(pack) = read_pack(view, cx) else {
        return div().into_any_element();
    };
    prompt_text(pack.run.prompt.trim(), *Theme::global(cx))
}

/// The Run group's properties: the run, its route, how long it took and how, its tokens,
/// context, and cost, and the card and note it carries. Empty ones are left out.
fn paint_run(view: &WeakEntity<RunPageView>, window: &mut Window, cx: &mut App) -> AnyElement {
    let _ = window;
    let Some(pack) = read_pack(view, cx) else {
        return div().into_any_element();
    };
    let theme = *Theme::global(cx);
    let figures = tabular(sans(&theme, FontWeight::MEDIUM));
    let run = &pack.run;
    let mut rows = rail_properties().child(rail_property("Run", wrapped(&run.id), true, cx));
    if let Some(route) = route_label(&run.tier, &run.effort) {
        rows = rows.child(rail_property("Route", wrapped(&route), false, cx));
    }
    if let Some(duration) = duration_label(
        proto_ms(run.run_ms),
        proto_ms(run.queued_ms),
        &run.status,
        &run.started_at,
        &run.created_at,
        pack.now_ms,
    ) {
        rows = rows.child(rail_property("Duration", wrapped(&duration), false, cx));
    }
    if let Some(timing) = timing_label(proto_ms(run.queued_ms), proto_ms(run.run_ms)) {
        rows = rows.child(rail_property("Timing", wrapped(&timing), false, cx));
    }
    if let Some(usage) = pack
        .token_line
        .as_ref()
        .filter(|usage| total_tokens(usage) > 0)
    {
        rows = rows.child(rail_property(
            "Tokens",
            div()
                .id("run-tokens-tip")
                .font(figures.clone())
                .tooltip(tooltip(token_breakdown(usage)))
                .child(format_tokens(total_tokens(usage))),
            false,
            cx,
        ));
    }
    if let Some(meter) = pack
        .token_line
        .as_ref()
        .and_then(|usage| context_usage(usage.context_tokens, usage.context_window))
    {
        let color = meter
            .percent
            .map(|percent| context_color(percent, theme))
            .unwrap_or(theme.color_accent);
        rows = rows.child(rail_property(
            "Context",
            div()
                .id("run-context-tip")
                .min_w(px(0.0))
                .tooltip(tooltip(meter.title.clone()))
                .child(context_meter(&meter, pack.context_fill, color, cx)),
            false,
            cx,
        ));
    }
    if let Some(usage) = pack.cost_line.as_ref() {
        let priced = run_cost(usage);
        rows = rows.child(rail_property(
            "Cost",
            div()
                .id("run-cost-tip")
                .font(figures)
                .tooltip(tooltip(priced.title))
                .child(priced.label),
            false,
            cx,
        ));
    }
    if !run.card_slug.trim().is_empty() {
        rows = rows.child(rail_property(
            "Card",
            wrapped(run.card_slug.trim()),
            true,
            cx,
        ));
    }
    if !run.note.trim().is_empty() {
        rows = rows.child(rail_property("Note", wrapped(run.note.trim()), false, cx));
    }
    rows.into_any_element()
}

/// `RunRoutingGroup`: the decision's policy, task, role, and area, then each candidate in
/// routing order (`mt-1 flex-col gap-0.5 px-1`), its 18px bot, `model @effort` in 12px mono
/// and its price in 11px muted figures, with Tried first on the chosen one and Below bar on an
/// Auto candidate that missed the bar.
fn paint_routing(view: &WeakEntity<RunPageView>, window: &mut Window, cx: &mut App) -> AnyElement {
    let _ = window;
    let Some(pack) = read_pack(view, cx) else {
        return div().into_any_element();
    };
    let Some(decision) = pack.decision.clone() else {
        return div().into_any_element();
    };
    let theme = *Theme::global(cx);
    let mut rows = rail_properties();
    let policy = policy_label(&decision.policy);
    if !policy.is_empty() {
        rows = rows.child(rail_property("Policy", wrapped(&policy), false, cx));
    }
    rows = rows.child(rail_property(
        "Task",
        wrapped(&decision_summary(&decision)),
        false,
        cx,
    ));
    if let Some(role) = role_summary(&decision) {
        rows = rows.child(rail_property("Role", wrapped(&role), false, cx));
    }
    let area = area_label(&decision);
    if !area.is_empty() {
        rows = rows.child(rail_property("Area", wrapped(&area), false, cx));
    }
    let mut candidates = div()
        .mt(px(tw(1.0)))
        .flex()
        .flex_col()
        .gap(px(tw(0.5)))
        .px(px(tw(1.0)));
    for (index, candidate) in decision.candidates.iter().enumerate() {
        let price = candidate_price(candidate);
        let model_line = if candidate.effort.is_empty() {
            candidate.model.clone()
        } else {
            format!("{} @{}", candidate.model, candidate.effort)
        };
        let marker = if decision.chosen == index as i64 {
            Some(("Tried first", crate::theme::StatusTone::Ok))
        } else if decision.policy == "auto-v1" && !candidate.qualified {
            Some(("Below bar", crate::theme::StatusTone::Muted))
        } else {
            None
        };
        candidates = candidates.child(
            div()
                .flex()
                .min_w(px(0.0))
                .items_center()
                .gap(px(tw(2.0)))
                .p(px(tw(1.0)))
                .child(agent_avatar(&candidate.agent, "", 18.0))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .font(mono(&theme, FontWeight::NORMAL))
                                .text_size(px(12.0))
                                .text_color(theme.color_text_primary)
                                .child(ellipsis(model_line)),
                        )
                        .when(!price.is_empty(), |column| {
                            column.child(
                                div()
                                    .font(tabular(mono(&theme, FontWeight::NORMAL)))
                                    .text_size(px(11.0))
                                    .text_color(theme.color_text_muted)
                                    .child(ellipsis(price)),
                            )
                        }),
                )
                .children(marker.map(|(label, tone)| chip(label, tone, cx))),
        );
    }
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(6.0))
        .child(rows)
        .child(candidates)
        .into_any_element()
}

fn paint_changes(view: &WeakEntity<RunPageView>, window: &mut Window, cx: &mut App) -> AnyElement {
    let _ = window;
    let Some(pack) = read_pack(view, cx) else {
        return div().into_any_element();
    };
    let theme = *Theme::global(cx);
    let run = &pack.run;
    let mut rows = rail_properties();
    if let Some(counts) = file_counts(run.files_changed, run.insertions, run.deletions) {
        rows = rows.child(rail_property(
            "Files",
            file_change_line(&counts, theme),
            false,
            cx,
        ));
    }
    if !pack.branch.is_empty() {
        rows = rows.child(rail_property("Branch", wrapped(&pack.branch), true, cx));
    }
    if !pack.chip_status.is_empty() {
        rows = rows.child(rail_property(
            "Worktree",
            chip(pack.chip_status.clone(), status_tone(&pack.chip_status), cx),
            false,
            cx,
        ));
    }
    if !pack.path.is_empty() {
        rows = rows.child(rail_property("Path", wrapped(&pack.path), true, cx));
    }
    if !run.merge_sha.trim().is_empty() {
        rows = rows.child(rail_property(
            "Merged",
            wrapped(run.merge_sha.trim()),
            true,
            cx,
        ));
    }
    rows.into_any_element()
}

/// `LinkedRunRow`s (`flex-col gap-0.5 px-1`): a plain row each, its 18px bot, the link in 12px
/// mono secondary, and its status; it fills on hover and opens the run.
fn paint_linked(view: &WeakEntity<RunPageView>, window: &mut Window, cx: &mut App) -> AnyElement {
    let _ = window;
    let Some(pack) = read_pack(view, cx) else {
        return div().into_any_element();
    };
    let theme = *Theme::global(cx);
    let mut column = div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(tw(0.5)))
        .px(px(tw(1.0)));
    for link in &pack.links {
        let open = view.clone();
        let run_id = link.run_id.clone();
        let status = link.show_status.then(|| link.status.clone());
        column = column.child(
            div()
                .id(SharedString::from(format!("run-link-{}", link.seed)))
                .w_full()
                .min_w(px(0.0))
                .flex()
                .items_center()
                .gap(px(tw(2.0)))
                .px(px(tw(2.0)))
                .py(px(tw(1.5)))
                .rounded(px(theme.radius_control))
                .cursor_pointer()
                .hover(|row| row.bg(theme.color_surface_hover))
                .on_click(move |_, _, cx| {
                    let run_id = run_id.clone();
                    let _ = open.update(cx, |view, cx| view.open_run(run_id, cx));
                })
                .child(agent_avatar(&link.agent, link.status.as_str(), 18.0))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .font(mono(&theme, FontWeight::NORMAL))
                        .text_size(px(12.0))
                        .text_color(theme.color_text_secondary)
                        .child(ellipsis(link.label.clone())),
                )
                .children(status.map(|status| chip(status.clone(), status_tone(&status), cx))),
        );
    }
    column.into_any_element()
}

/// A property's text, wrapping where it must (`overflow-wrap: anywhere`).
fn wrapped(text: &str) -> AnyElement {
    div()
        .min_w(px(0.0))
        .max_w_full()
        .whitespace_normal()
        .child(text.to_string())
        .into_any_element()
}

fn context_color(percent: u32, theme: Theme) -> gpui::Rgba {
    if percent >= 90 {
        theme.color_danger
    } else if percent >= 75 {
        theme.color_warning
    } else {
        theme.color_accent
    }
}

fn missing_body(connection: &ConnectionState, theme: Theme, cx: &App) -> AnyElement {
    match connection {
        ConnectionState::Connecting => centered("Connecting to Dispatch core...", theme),
        ConnectionState::Offline { core_json_path } => offline_body(core_json_path, theme),
        ConnectionState::Online => no_run_open(theme, cx),
    }
}

/// `PageEmpty` "No run open": how runs start, `dispatch run` in mono, 13px secondary.
fn no_run_open(theme: Theme, cx: &App) -> AnyElement {
    const LEAD: &str = "Agents start runs with ";
    const CODE: &str = "dispatch run";
    const TAIL: &str = ". Pick one in the sidebar to follow it.";
    let color: Hsla = theme.color_text_secondary.into();
    let run = |len: usize, font: Font| TextRun {
        len,
        font,
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let text = SharedString::from(format!("{LEAD}{CODE}{TAIL}"));
    let body = div()
        .text_size(px(13.0))
        .line_height(px(13.0 * 1.5))
        .text_color(theme.color_text_secondary)
        .child(StyledText::new(text).with_runs(vec![
            run(LEAD.len(), sans(&theme, FontWeight::NORMAL)),
            run(CODE.len(), mono(&theme, FontWeight::NORMAL)),
            run(TAIL.len(), sans(&theme, FontWeight::NORMAL)),
        ]));
    page_empty_with(
        Some("No run open"),
        MascotMood::Sleeping,
        Some(body.into_any_element()),
        cx,
    )
    .into_any_element()
}

fn centered(label: &str, theme: Theme) -> AnyElement {
    div()
        .flex_1()
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(theme.font_size_md))
        .text_color(theme.color_text_muted)
        .child(label.to_string())
        .into_any_element()
}

fn offline_body(path: &str, theme: Theme) -> AnyElement {
    div()
        .flex_1()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(theme.space_2))
        .p(px(theme.space_6))
        .child(
            div()
                .text_size(px(theme.font_size_lg))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(theme.color_text_primary)
                .child("Dispatch core is not running"),
        )
        .child(
            div()
                .font_family(theme.font_mono)
                .text_size(px(theme.font_size_xs))
                .text_color(theme.color_text_muted)
                .child(path.to_string()),
        )
        .into_any_element()
}
