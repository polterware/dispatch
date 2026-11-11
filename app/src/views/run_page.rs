//! Run page: transcript in the center, properties in the right rail.
//!
//! The page is a `RailPage` (`wide splitScroll railCard`): the transcript in the column and
//! the run's properties in `RailGroup`s on the card rail, which the header's last button hides
//! and shows (remembered as `polterware.run.rail`). Header actions call the core on the
//! background executor; they use the same button colors with fast hover and press springs.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gpui::{
    AnyElement, AnyView, App, AppContext as _, Context, Entity, Font, FontWeight, Hsla,
    IntoElement, MouseButton, ScrollHandle, SharedString, StyleRefinement, StyledText, TextRun,
    WeakEntity, Window, div, prelude::*, px,
};

use super::repository_actions;
use crate::avatar::{agent_avatar, route_avatar};
use crate::client::{
    AddWorkspaceRequest, CancelRunRequest, CleanRunRequest, ConfigRequest, CoreClient, CoreError,
    DispatchAttempt, DispatchRun, DispatchWorktree, GetRouteDecisionRequest, ListModelsRequest,
    ListTiersRequest, ListWorkspaceCardsRequest, ListWorktreesRequest, MergeRunRequest,
    RecoverRunRequest, ResultRequest, StartRunRequest, WorkspaceRepository,
};
use crate::models::avatar::avatar_status;
use crate::models::new_run::{
    FolderChoice, IMAGE_PILL, PLACE_PREFERENCE, PlaceMode, RouteChoice, RouteLabel, RunPlace,
    default_pick, default_place, folder_choices, image_hint, offers_worktree, pick_label,
    place_label, route_choices, run_label, start_request,
};
use crate::models::run_page::{
    AttemptCard, AttemptSwitch, CandidateExplain, CandidateOutcome, ClassifierBands, LinkedRow,
    MoreAction, ReviewAction, RouteDecision, TokenUsage, area_label, attempt_switches,
    candidate_explain, candidate_line, candidate_outcomes, classifier_bands, coalesce_text,
    context_usage, difficulty_label, duration_label, file_counts, follow_up_blocker, format_tokens,
    has_worktree, is_run_active, linked_rows, map_route_decision, percent, policy_label,
    priority_label, proto_ms, rail_tokens, role_summary, route_label, route_reason, run_cost,
    run_more_actions, run_review_action, stored_usage, timing_label, token_breakdown, total_tokens,
    unmatched_attempts, worktree_display_status,
};
use crate::models::settings::parse_policy;
use crate::motion::{Animator, SpringTier, SpringValue};
use crate::state::{AppState, ConnectionState, Route, observe};
use crate::theme::{Theme, status_tone, tw};
use crate::ui::ellipsis;
use crate::ui::parts::run_page::{context_meter, file_change_line};
use crate::ui::parts::voice::voice_blob;
use crate::ui::patterns::{
    AnchorSlot, ComposerAction, message_composer, message_input, trailing_toolbar, window_anchor,
};
use crate::ui::{
    Button, ButtonKind, ButtonSize, Icon, MascotMood, Menu, MenuEntry, PageWidth, RailGroup,
    RailPresence, ToastTone, button, chip, mono, page_empty_with, rail_page_body_flush,
    rail_page_body_under, rail_page_end_pad, rail_page_header, rail_properties, rail_property,
    rail_toggle_label, rich_tooltip, sans, tabular, toolbar, tooltip, tooltip_heading, tooltip_row,
    wrap_text,
};
use crate::ui::{PropertyOption, PropertyPicker, PropertyPickerEvent, text_swap};
use crate::ui::{SkeletonLayout, TextInput, WIDE_MEASURE, page_skeleton};
use crate::voice::{self, EngineMessage, VoiceEngine, VoicedTurns};

/// The route picker's Loading row, which cannot be picked.
const ROUTES_LOADING: &str = "routes-loading";

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
    run_section: Entity<RailGroup>,
    routing: Entity<RailGroup>,
    changes: Entity<RailGroup>,
    linked: Entity<RailGroup>,
    menu: Entity<Menu>,
    cancelling: bool,
    reviewing: Option<Op>,
    confirm_open: bool,
    confirm_progress: SpringValue,
    /// The rail toggle's entrance: 0 while the run works, springing to 1 once it ends.
    rail_reveal: SpringValue,
    decision: Option<RouteDecision>,
    /// The Routing group lists every candidate, not just the first few.
    /// How many pages of five the Routing group lists.
    candidate_pages: usize,
    expanded_handoffs: HashSet<usize>,
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
    /// The message composer under the transcript: a follow-up to the run's agent session.
    message: Entity<TextInput>,
    message_scroll: ScrollHandle,
    /// A follow-up is on its way to the core.
    sending: bool,
    /// The voice engine: one connection, shared with every view.
    voice: VoiceEngine,
    /// The utterance being dictated, from the tap until its final transcript.
    listening: Option<String>,
    /// The words a dictation put in the composer, kept until the message starts a run: that run's
    /// turn was voiced.
    voiced_text: Option<String>,
    /// The runs started from a dictation, and which of them were read aloud.
    voiced: VoicedTurns,
    /// Voiced runs whose reply was asked of the core, so it is asked once.
    fetching_reply: HashSet<String>,
    /// What the last tick saw, so the page redraws only when the blob comes or goes.
    voice_active: bool,
    /// The new conversation (⌘N) before its first message.
    draft: Draft,
}

/// The new conversation page: the run page before its run, with its route in the header.
struct Draft {
    /// The page shows the new conversation: entering it again starts a fresh one.
    open: bool,
    /// Counts new conversations, so each one's header starts on its own route.
    generation: u64,
    /// The picked route (`models::new_run` values).
    pick: String,
    /// The user picked a route, so the routes coming in leave it.
    picked: bool,
    /// The routes offered, once the core has answered.
    choices: Vec<RouteChoice>,
    /// The agents' models are still being asked for: the picker says so in a row.
    models_loading: bool,
    /// The routes were asked for in this new conversation.
    asked: bool,
    picker: Entity<PropertyPicker>,
    /// Where the header pill is, which the picker hangs from.
    anchor: AnchorSlot,
    /// The run the last new conversation started: its id, the conversation's generation, and
    /// the route it was started on, which its header shows until the run has an agent and
    /// then turns into it.
    started: Option<(String, u64, String)>,
    /// Where the run works, once picked; until then the default (`default_place`).
    place: Option<RunPlace>,
    /// The place last picked in each workspace (`PLACE_PREFERENCE`).
    remembered: HashMap<String, RunPlace>,
    /// Each linked project's cloned repositories, by workspace id, once the core answered.
    repos: HashMap<String, Vec<WorkspaceRepository>>,
    /// The repositories were asked for in this new conversation.
    repos_asked: bool,
    /// The folder picker, hung from the folder pill.
    place_picker: Entity<PropertyPicker>,
    folder_anchor: AnchorSlot,
    /// The last start was refused because another run edits the folder itself: the core's
    /// message, over Worktree, which starts it again in a worktree of its own.
    busy: Option<String>,
    /// The Image mode: on, the run asks for images and routes to the agents that make them.
    images: bool,
}

/// The header pill's key for a new conversation's route, which the run it starts keeps.
fn draft_key(generation: u64) -> String {
    format!("route-draft-{generation}")
}

/// The composer's placeholder.
const MESSAGE_PLACEHOLDER: &str = "Send a message to the agent";

impl RunPageView {
    /// Presses the transcript's jump-to-end button (the screenshot tour).
    pub(crate) fn jump_transcript_to_end(&mut self, cx: &mut Context<Self>) {
        self.transcript
            .update(cx, |transcript, cx| transcript.jump_to_end(cx));
    }

    /// Scrolls the transcript `y` down (the screenshot tour).
    pub(crate) fn scroll_transcript(&mut self, y: f32, cx: &mut Context<Self>) {
        self.transcript
            .update(cx, |transcript, cx| transcript.scroll_by(y, cx));
    }

    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        observe(&state, cx);
        let run_id = match state.read(cx).route() {
            Route::Run(id) => id.clone(),
            _ => String::new(),
        };
        let transcript = cx.new(|cx| {
            super::transcript::TranscriptView::new(state.clone(), run_id.clone(), false, cx)
        });
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
        let weak = cx.weak_entity();
        let message = message_input(
            MESSAGE_PLACEHOLDER,
            move |text, _, cx| {
                let text = text.to_string();
                let _ = weak.update(cx, |view, cx| view.submit(text, cx));
            },
            cx,
        );
        let picker = cx.new(PropertyPicker::new);
        cx.subscribe(&picker, |view, _, event: &PropertyPickerEvent, cx| {
            if let PropertyPickerEvent::Pick(value) = event {
                view.draft.pick = value.to_string();
                view.draft.picked = true;
                cx.notify();
            }
        })
        .detach();
        let place_picker = cx.new(PropertyPicker::new);
        cx.subscribe(
            &place_picker,
            |view, _, event: &PropertyPickerEvent, cx| match event {
                PropertyPickerEvent::Pick(value)
                    if value.as_ref() == crate::models::new_run::ADD_REPOSITORY =>
                {
                    view.add_repository(cx)
                }
                PropertyPickerEvent::Pick(value) => view.pick_place(value, cx),
                PropertyPickerEvent::Action(value, id) => view.repository_action(value, id, cx),
                _ => {}
            },
        )
        .detach();
        let mut view = Self {
            state,
            transcript,
            transcript_run: run_id,
            run_section,
            routing,
            changes,
            linked,
            menu,
            cancelling: false,
            candidate_pages: 1,
            expanded_handoffs: HashSet::new(),
            reviewing: None,
            confirm_open: false,
            confirm_progress: SpringValue::new(0.0, SpringTier::Moderate),
            rail_reveal: SpringValue::new(0.0, SpringTier::Moderate),
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
            message,
            message_scroll: ScrollHandle::new(),
            sending: false,
            voice: crate::voice::engine(cx),
            listening: None,
            voiced_text: None,
            voiced: VoicedTurns::default(),
            fetching_reply: HashSet::new(),
            voice_active: false,
            draft: Draft {
                open: false,
                generation: 0,
                pick: default_pick(false).to_string(),
                picked: false,
                choices: Vec::new(),
                models_loading: false,
                asked: false,
                picker,
                anchor: AnchorSlot::default(),
                started: None,
                place: None,
                remembered: HashMap::new(),
                repos: HashMap::new(),
                repos_asked: false,
                place_picker,
                folder_anchor: AnchorSlot::default(),
                busy: None,
                images: false,
            },
        };
        view.install_bodies(cx);
        Self::spawn_voice_tick(cx);
        view
    }

    /// Drains the voice engine's events and reads completed voiced turns aloud, ten times a
    /// second: a transcript lands in the composer as soon as the engine transcribes it.
    fn spawn_voice_tick(cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                if this.update(cx, |view, cx| view.voice_tick(cx)).is_err() {
                    break;
                }
                cx.background_executor()
                    .timer(Duration::from_millis(100))
                    .await;
            }
        })
        .detach();
    }

    /// One voice tick: what the engine said, and the voiced runs that have completed.
    fn voice_tick(&mut self, cx: &mut Context<Self>) {
        let mut changed = false;
        let messages = self.voice.drain();
        for message in messages {
            match message {
                EngineMessage::TranscriptFinal { id, text, .. } => {
                    if self.listening.as_deref() != Some(id.as_str()) {
                        continue;
                    }
                    self.listening = None;
                    changed = true;
                    self.finish_dictation(text, cx);
                }
                EngineMessage::Error { code, message, id } => {
                    if id
                        .as_deref()
                        .is_some_and(|id| self.listening.as_deref() == Some(id))
                    {
                        self.listening = None;
                    }
                    self.toast(voice::error_message(code, &message), ToastTone::Danger, cx);
                    changed = true;
                }
                EngineMessage::SpeakEnded { .. } => changed = true,
                _ => {}
            }
        }
        let due: Vec<(String, Option<String>)> = {
            let state = self.state.read(cx);
            state
                .runs()
                .iter()
                .filter(|run| self.voiced.due(run))
                .map(|run| (run.id.clone(), state.run_reply(&run.id).map(str::to_string)))
                .collect()
        };
        if !due.is_empty() {
            changed = true;
            for (id, reply) in due {
                self.read_run_reply(id, reply, cx);
            }
        }
        let active = self.voice.is_listening() || self.voice.is_speaking();
        if active != self.voice_active {
            self.voice_active = active;
            changed = true;
        }
        if changed {
            self.transcript.update(cx, |_, cx| cx.notify());
            cx.notify();
        }
    }

    /// The words a dictation was heard as: they go into the composer, and the message is sent
    /// when the person asked for that.
    fn finish_dictation(&mut self, said: String, cx: &mut Context<Self>) {
        let said = said.trim().to_string();
        if said.is_empty() {
            self.toast("Nothing was heard.", ToastTone::Info, cx);
            return;
        }
        let current = self.message.read(cx).text().to_string();
        let next = voice::with_transcript(&current, &said);
        self.message
            .update(cx, |input, cx| input.set_text(next.clone(), cx));
        self.voiced_text = Some(said);
        if self.voice.prefs().send_when_i_stop {
            self.send_dictation(next, cx);
        }
    }

    /// Sends a dictated message on the page that holds the composer: a new conversation starts
    /// on the New page, and a message goes to the open run.
    fn send_dictation(&mut self, text: String, cx: &mut Context<Self>) {
        match self.state.read(cx).route().clone() {
            Route::NewRun => self.start_conversation(text, cx),
            Route::Run(id) if id == self.transcript_run => self.send_message(text, cx),
            _ => {}
        }
    }

    /// The dictation a message carries, if any: the voiced text it was written with.
    fn take_voiced(&mut self, prompt: &str) -> bool {
        let Some(said) = self.voiced_text.take() else {
            return false;
        };
        let said = said.trim();
        !said.is_empty() && (prompt.trim() == said || prompt.contains(said))
    }

    /// Reads a voiced run's completed reply aloud, asking the core for it when the page does
    /// not hold it.
    fn read_run_reply(&mut self, id: String, reply: Option<String>, cx: &mut Context<Self>) {
        if let Some(reply) = reply {
            self.speak_run_reply(&id, &reply);
            return;
        }
        if !self.fetching_reply.insert(id.clone()) {
            return;
        }
        let Some(client) = self.state.read(cx).client() else {
            self.fetching_reply.remove(&id);
            return;
        };
        cx.spawn(async move |this, cx| {
            let asked = id.clone();
            let answer = cx
                .background_spawn(async move { client.run_reply(&asked) })
                .await;
            let _ = this.update(cx, |view, cx| {
                view.fetching_reply.remove(&id);
                let reply = answer.unwrap_or_default();
                view.speak_run_reply(&id, &reply);
                cx.notify();
            });
        })
        .detach();
    }

    /// Speaks a voiced run's reply once, when reading replies is on.
    fn speak_run_reply(&mut self, id: &str, reply: &str) {
        self.voiced.mark_spoken(id);
        if reply.trim().is_empty() || !self.voice.prefs().read_replies {
            return;
        }
        self.voice.speak_reply(id, reply);
    }

    /// The composer's microphone: the first tap opens it, the second ends the utterance.
    fn toggle_dictation(&mut self, cx: &mut Context<Self>) {
        if let Some(id) = self.listening.take() {
            self.voice.listen_stop(&id);
        } else {
            let id = self.voice.listen_start();
            self.listening = Some(id);
        }
        cx.notify();
    }

    /// The composer's text, which the tests read after a dictation.
    pub fn composer_text(&self, cx: &App) -> String {
        self.message.read(cx).text().to_string()
    }

    /// The microphone action before Send, pressed while the microphone is open.
    fn dictation_action(&self, cx: &mut Context<Self>) -> ComposerAction {
        let listening = self.listening.is_some() || self.voice.is_listening();
        let weak = cx.weak_entity();
        ComposerAction::new(
            "voice-mic",
            Icon::Microphone,
            if listening {
                "Stop dictation"
            } else {
                "Dictate"
            },
        )
        .active(listening)
        .on_click(move |_, cx| {
            let _ = weak.update(cx, |view, cx| view.toggle_dictation(cx));
        })
    }

    /// The blob beside the composer, while a dictation or a reply is playing.
    fn voice_mark(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let listening = self.listening.is_some() || self.voice.is_listening();
        let speaking = self.voice.is_speaking();
        if !listening && !speaking {
            return None;
        }
        Some(voice_blob(&self.voice, speaking, Theme::global(cx)))
    }

    fn install_bodies(&mut self, cx: &mut Context<Self>) {
        let weak = cx.weak_entity();
        bind_body(&self.run_section, weak.clone(), paint_run, cx);
        bind_body(&self.routing, weak.clone(), paint_routing, cx);
        bind_body(&self.changes, weak.clone(), paint_changes, cx);
        bind_body(&self.linked, weak, paint_linked, cx);
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

    /// What Enter or the send button does: the new conversation's first message starts its
    /// run, and on a run page a message follows it up.
    fn submit(&mut self, text: String, cx: &mut Context<Self>) {
        if matches!(self.state.read(cx).route(), Route::NewRun) {
            self.start_conversation(text, cx);
        } else {
            self.send_message(text, cx);
        }
    }

    /// Starts a fresh new conversation the first time the page shows it: Auto again (or
    /// normal with no classifier), an empty composer, and the routes asked for again.
    fn enter_draft(&mut self, cx: &mut Context<Self>) {
        if self.draft.open {
            return;
        }
        self.draft.open = true;
        self.draft.generation = self.draft.generation.wrapping_add(1);
        let auto = self.draft.choices.iter().any(|choice| {
            choice.value == crate::models::new_run::AUTO && choice.disabled.is_none()
        });
        self.draft.pick = default_pick(auto).to_string();
        self.draft.picked = false;
        self.draft.asked = false;
        self.draft.place = None;
        self.draft.busy = None;
        self.draft.images = false;
        self.draft.repos_asked = false;
        self.draft.remembered = self
            .state
            .read(cx)
            .read_preference::<HashMap<String, RunPlace>>(PLACE_PREFERENCE)
            .ok()
            .flatten()
            .unwrap_or_default();
        self.message.update(cx, |input, cx| input.set_text("", cx));
        self.load_routes(cx);
        self.load_folders(cx);
    }

    /// The folders a new conversation's run can work in, from the workspaces and the
    /// repositories read so far.
    fn folders(&self, cx: &App) -> Vec<FolderChoice> {
        folder_choices(self.state.read(cx).workspaces(), &self.draft.repos)
    }

    /// Where the new conversation's run works: the place picked, else the current
    /// workspace's last pick, else its folder itself.
    fn place(&self, cx: &App) -> Option<RunPlace> {
        if let Some(place) = &self.draft.place {
            return Some(place.clone());
        }
        default_place(
            self.state.read(cx).current_workspace(),
            &self.draft.remembered,
            &self.folders(cx),
        )
    }

    /// Reads each linked project's cloned repositories, which the folder picker offers.
    fn load_folders(&mut self, cx: &mut Context<Self>) {
        if self.draft.repos_asked {
            return;
        }
        let (client, linked) = {
            let state = self.state.read(cx);
            let linked: Vec<String> = state
                .workspaces()
                .iter()
                .filter(|workspace| !workspace.polterware_project_id.is_empty())
                .map(|workspace| workspace.id.clone())
                .collect();
            (state.client(), linked)
        };
        let Some(client) = client else {
            return;
        };
        if linked.is_empty() {
            return;
        }
        self.draft.repos_asked = true;
        cx.spawn(async move |this, cx| {
            let repos = cx
                .background_spawn(async move {
                    linked
                        .into_iter()
                        .filter_map(|workspace_id| {
                            client
                                .list_workspace_cards(&ListWorkspaceCardsRequest {
                                    workspace_id: workspace_id.clone(),
                                })
                                .ok()
                                .map(|response| (workspace_id, response.repositories))
                        })
                        .collect::<HashMap<_, _>>()
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                view.draft.repos = repos;
                cx.notify();
            });
        })
        .detach();
    }

    /// A pick from the folder or mode picker: the place changes and is remembered for its
    /// workspace.
    fn pick_place(&mut self, value: &str, cx: &mut Context<Self>) {
        let Some(mut place) = self.place(cx) else {
            return;
        };
        if let Some(mode) = PlaceMode::from_value(value) {
            place.mode = mode;
        } else if let Some(folder) = self
            .folders(cx)
            .into_iter()
            .find(|folder| folder.value() == value)
        {
            place.workspace_id = folder.workspace_id;
            place.dir = folder.dir;
        } else {
            return;
        }
        self.draft.busy = None;
        self.draft
            .remembered
            .insert(place.workspace_id.clone(), place.clone());
        let _ = self
            .state
            .read(cx)
            .write_preference(PLACE_PREFERENCE, &self.draft.remembered);
        self.draft.place = Some(place);
        cx.notify();
    }

    /// Opens the folder picker under its pill, or closes it.
    fn toggle_folder_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.draft.picker.read(cx).is_open() {
            self.draft.picker.update(cx, |picker, cx| picker.close(cx));
        }
        if self.draft.place_picker.read(cx).is_open() {
            self.draft
                .place_picker
                .update(cx, |picker, cx| picker.close(cx));
            return;
        }
        let Some(place) = self.place(cx) else {
            return;
        };
        let options = self.folder_options(cx);
        let height = self
            .draft
            .folder_anchor
            .get()
            .map_or(28.0, |bounds| f32::from(bounds.size.height));
        let value = SharedString::from(format!("dir:{}", place.dir));
        self.draft.place_picker.update(cx, |picker, cx| {
            picker.set_options("Repository", options, Some(value), 320.0, cx);
            picker.set_trigger_height(height);
            picker.open(window, cx);
        });
    }

    /// The repository picker's rows: each repository by name, its folder in a tooltip and
    /// the repository actions on its ⋯ (as its sidebar group's menu has them), then Add
    /// repository.
    fn folder_options(&self, cx: &App) -> Vec<PropertyOption> {
        let state = self.state.read(cx);
        let mut options: Vec<PropertyOption> = self
            .folders(cx)
            .iter()
            .map(|folder| {
                let repository =
                    repository_actions::Repository::at(&folder.dir, &folder.label, state);
                let option = PropertyOption::new(folder.value(), folder.label.clone())
                    .tooltip(folder.dir.clone())
                    .icon(Icon::Folder);
                repository_actions::with_actions(option, &repository)
            })
            .collect();
        options.push(
            PropertyOption::new(crate::models::new_run::ADD_REPOSITORY, "Add repository")
                .icon(Icon::Plus)
                .action(),
        );
        options
    }

    /// A repository action picked from a row's menu in the folder picker.
    fn repository_action(&mut self, value: &str, id: &str, cx: &mut Context<Self>) {
        let Some(folder) = self.folders(cx).into_iter().find(|f| f.value() == value) else {
            return;
        };
        let repository =
            repository_actions::Repository::at(&folder.dir, &folder.label, self.state.read(cx));
        if id == repository_actions::REMOVE {
            self.draft
                .place_picker
                .update(cx, |picker, cx| picker.close(cx));
        }
        let state = self.state.clone();
        repository_actions::perform(id, &repository, &state, cx);
    }

    /// Add repository in the folder picker: the folder chosen becomes a workspace, and the
    /// new conversation's place, in the mode picked so far.
    fn add_repository(&mut self, cx: &mut Context<Self>) {
        let mode = self
            .place(cx)
            .map(|place| place.mode)
            .unwrap_or(PlaceMode::InPlace);
        let page = cx.entity().downgrade();
        self.state.update(cx, |state, cx| {
            state.add_repository(cx, move |_, workspace, dir, cx| {
                let place = RunPlace {
                    workspace_id: workspace.id.clone(),
                    dir: dir.to_string(),
                    mode,
                };
                // After the state's update ends: the page reads the state to list folders.
                cx.defer(move |cx| {
                    let _ = page.update(cx, |view, cx| {
                        view.draft.place = Some(place);
                        view.draft.busy = None;
                        // A project's repositories are read again, the new one among them.
                        view.draft.repos_asked = false;
                        view.load_folders(cx);
                        cx.notify();
                    });
                });
            });
        });
    }

    /// Worktree, after a refusal for a folder another run edits: the same message
    /// again, in a worktree of its own.
    fn retry_in_worktree(&mut self, cx: &mut Context<Self>) {
        self.pick_place(PlaceMode::Worktree.value(), cx);
        let text = self.message.read(cx).text().to_string();
        self.start_conversation(text, cx);
    }

    /// Reads whether a classifier is on, the tier table, and the agents' models, and offers
    /// them in the header's picker.
    fn load_routes(&mut self, cx: &mut Context<Self>) {
        if self.draft.asked {
            return;
        }
        let Some(client) = self.state.read(cx).client() else {
            return;
        };
        self.draft.asked = true;
        self.draft.models_loading = true;
        cx.spawn(async move |this, cx| {
            // Auto and the tiers first, which the core answers at once; the agents' models
            // after, which can take seconds while a CLI is asked for the first time.
            let models_client = client.clone();
            let (auto, tiers) = cx
                .background_spawn(async move {
                    let auto = client
                        .config(&ConfigRequest {
                            action: "get".to_string(),
                            ..Default::default()
                        })
                        .map(|response| parse_policy(&response.json).classifier_on())
                        .unwrap_or(false);
                    let tiers = client
                        .list_tiers(&ListTiersRequest {})
                        .map(|response| response.tiers)
                        .unwrap_or_default();
                    (auto, tiers)
                })
                .await;
            let shown_tiers = tiers.clone();
            let _ = this.update(cx, |view, cx| {
                if !view.draft.picked {
                    view.draft.pick = default_pick(auto).to_string();
                }
                // Keep the models an earlier load listed until the new list comes.
                let models: Vec<RouteChoice> = view
                    .draft
                    .choices
                    .iter()
                    .filter(|choice| choice.agent.is_some())
                    .cloned()
                    .collect();
                view.draft.choices = route_choices(auto, &shown_tiers, &[]);
                view.draft.choices.extend(models);
                view.refresh_route_picker(cx);
                cx.notify();
            });
            let models = cx
                .background_spawn(async move {
                    models_client
                        .list_models(&ListModelsRequest {})
                        .map(|response| response.models)
                        .unwrap_or_default()
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                view.draft.choices = route_choices(auto, &tiers, &models);
                view.draft.models_loading = false;
                view.refresh_route_picker(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// The route picker's rows, from the routes read so far, with a Loading row while the
    /// agents' models are still being asked for.
    fn route_options(&self) -> Vec<PropertyOption> {
        let mut options: Vec<PropertyOption> = self
            .draft
            .choices
            .iter()
            .map(|choice| {
                let mut option = PropertyOption::new(choice.value.clone(), choice.label.clone())
                    .group(choice.group.clone());
                if !choice.hint.is_empty() {
                    option = option.hint(choice.hint.clone());
                }
                if let Some(reason) = &choice.disabled {
                    option = option.disabled(reason.clone());
                }
                match &choice.agent {
                    Some(agent) => {
                        let agent = agent.clone();
                        option.mark(move |size, _, _| {
                            agent_avatar(&agent, "", size).into_any_element()
                        })
                    }
                    None => option.icon(Icon::Activity),
                }
            })
            .collect();
        if self.draft.models_loading {
            options.push(
                PropertyOption::new(ROUTES_LOADING, "Loading models\u{2026}")
                    .group("Models")
                    .hint("Asking each agent CLI")
                    .icon(Icon::Activity)
                    .disabled("The agents' models are still loading"),
            );
        }
        options
    }

    /// Puts the routes read so far in the route picker while it is open.
    fn refresh_route_picker(&mut self, cx: &mut Context<Self>) {
        if !self.draft.picker.read(cx).is_open() {
            return;
        }
        let options = self.route_options();
        let value = SharedString::from(self.draft.pick.clone());
        self.draft.picker.update(cx, |picker, cx| {
            picker.set_options("Route", options, Some(value), 320.0, cx);
        });
    }

    /// Opens the header's picker of routes under its pill, or closes it.
    pub(crate) fn toggle_route_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.draft.place_picker.read(cx).is_open() {
            self.draft
                .place_picker
                .update(cx, |picker, cx| picker.close(cx));
        }
        if self.draft.picker.read(cx).is_open() {
            self.draft.picker.update(cx, |picker, cx| picker.close(cx));
            return;
        }
        // The core did not answer the routes (it may have been starting): ask again.
        if self.draft.choices.is_empty() {
            self.draft.asked = false;
            self.load_routes(cx);
        }
        let options = self.route_options();
        let height = self
            .draft
            .anchor
            .get()
            .map_or(28.0, |bounds| f32::from(bounds.size.height));
        let value = SharedString::from(self.draft.pick.clone());
        self.draft.picker.update(cx, |picker, cx| {
            picker.set_options("Route", options, Some(value), 320.0, cx);
            picker.set_trigger_height(height);
            picker.open(window, cx);
        });
    }

    /// Starts the new conversation's run with its first message, on the picked route in the
    /// open workspace, and opens it in the conversation's place.
    fn start_conversation(&mut self, text: String, cx: &mut Context<Self>) {
        let prompt = text.trim().to_string();
        if prompt.is_empty() || self.sending {
            return;
        }
        let voiced = self.take_voiced(&prompt);
        let client = self.state.read(cx).client();
        let Some(client) = client else {
            self.toast("Dispatch core is not running", ToastTone::Danger, cx);
            return;
        };
        let Some(place) = self.place(cx) else {
            self.toast(
                "Add a repository to start a conversation",
                ToastTone::Danger,
                cx,
            );
            return;
        };
        let pick = self.draft.pick.clone();
        let generation = self.draft.generation;
        let request = start_request(&pick, &prompt, &place, self.draft.images);
        self.draft.busy = None;
        self.sending = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let mut request = request;
                    // A repository only runs named: the core adds it, as its main checkout.
                    if request.project_id.trim().is_empty() {
                        request.project_id = client
                            .add_workspace(&AddWorkspaceRequest {
                                path: request.dir.clone(),
                                name: String::new(),
                            })?
                            .workspace
                            .id;
                    }
                    client.start_run(&request)
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                view.sending = false;
                match result {
                    Ok(response) if !response.run.id.is_empty() => {
                        let id = response.run.id.clone();
                        view.message.update(cx, |input, cx| input.set_text("", cx));
                        if voiced {
                            view.voiced.mark_voiced(&id);
                        }
                        view.draft.started = Some((id.clone(), generation, pick));
                        view.draft.open = false;
                        view.state.update(cx, |state, cx| {
                            state.note_run(response.run);
                            state.open_started_run(id);
                            state.request_refresh(cx);
                            cx.notify();
                        });
                    }
                    Ok(_) => cx.notify(),
                    // Another run edits the folder itself: Worktree, under the message.
                    Err(CoreError::Rpc(message))
                        if offers_worktree(&message) && place.mode == PlaceMode::InPlace =>
                    {
                        view.draft.busy = Some(message);
                        cx.notify();
                    }
                    Err(error) => {
                        let message = match error {
                            CoreError::Rpc(message) if !message.is_empty() => {
                                format!("The conversation did not start: {message}")
                            }
                            _ => "The conversation did not start.".to_string(),
                        };
                        view.toast(message, ToastTone::Danger, cx);
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }

    /// The header pill's key and route for `run`: a run a new conversation started keeps
    /// that conversation's key, so its pill turns from the route into the agent it got.
    fn run_route(&self, run: &DispatchRun) -> (String, RouteLabel) {
        match &self.draft.started {
            Some((id, generation, pick)) if *id == run.id => (
                draft_key(*generation),
                run_label(&run.agent, &run.model, Some(pick)),
            ),
            _ => (run.id.clone(), run_label(&run.agent, &run.model, None)),
        }
    }

    /// The new conversation's page: the run page's header with its route, which opens the
    /// picker, an empty rail, and the composer, whose first message starts the run.
    fn render_draft(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        self.enter_draft(cx);
        self.sync_menu("", &[], cx);
        let theme = *Theme::global(cx);
        let weak = cx.weak_entity();
        // Nothing has run yet: no rail, and no toggle to open one.
        self.rail.set_open(false);
        let label = pick_label(&self.draft.pick);
        let open_picker: PillClick = Rc::new({
            let weak = weak.clone();
            move |window, cx| {
                let _ = weak.update(cx, |view, cx| view.toggle_route_picker(window, cx));
            }
        });
        let route = anchored(
            route_pill(
                &draft_key(self.draft.generation),
                &label,
                "",
                Some(open_picker),
                cx,
            ),
            self.draft.anchor.clone(),
        );
        self.load_folders(cx);
        // Image mode: beside the route, on when the conversation asks for images.
        let image = {
            let on = self.draft.images;
            let toggle = weak.clone();
            let click: PillClick = Rc::new(move |_, cx| {
                let _ = toggle.update(cx, |view, cx| {
                    view.draft.images = !view.draft.images;
                    cx.notify();
                });
            });
            toggle_pill(
                "draft-images",
                Icon::Eye,
                IMAGE_PILL,
                image_hint(on),
                on,
                click,
            )
        };
        let mut pills = div()
            .flex()
            .items_center()
            .gap(px(6.0))
            .child(route)
            .child(image);
        if let Some(place) = self.place(cx) {
            let folder_label = {
                let state = self.state.read(cx);
                place_label(&place, &self.folders(cx), state.workspaces())
            };
            let click: PillClick = Rc::new({
                let weak = weak.clone();
                move |window, cx| {
                    let _ = weak.update(cx, |view, cx| view.toggle_folder_picker(window, cx));
                }
            });
            pills = pills.child(anchored(
                place_pill(
                    "draft-folder",
                    Icon::Folder,
                    &folder_label,
                    &place.dir,
                    Some(click),
                ),
                self.draft.folder_anchor.clone(),
            ));
        }
        let bar = rail_page_header(pills, div(), cx).into_any_element();
        let header = theme.page_header_height;
        let can_send = self.state.read(cx).current_workspace().is_some();
        let input = self.message.clone();
        let send = weak.clone();
        let mut card = message_composer("run-message", &self.message, &self.message_scroll)
            .busy(self.sending)
            .on_send(Rc::new(move |_, cx| {
                let text = input.read(cx).text().to_string();
                let _ = send.update(cx, |view, cx| view.start_conversation(text, cx));
            }));
        if !can_send {
            card = card.can_send(false);
        }
        card = card.trailing_action(self.dictation_action(cx));
        // Worktree: a ghost button that takes the primary colors while on, when the run gets
        // a git worktree and branch of its own; off, it edits the folder itself.
        if let Some(place) = self.place(cx) {
            let on = place.mode == PlaceMode::Worktree;
            let toggle = weak.clone();
            // A small neutral toggle at its own width, at the right beside Send: the raised
            // secondary face when on.
            let button = Button::new(
                "draft-worktree",
                if on {
                    ButtonKind::Secondary
                } else {
                    ButtonKind::Ghost
                },
            )
            .size(ButtonSize::Sm)
            .height(24.0)
            .pad_x(8.0)
            .icon(Icon::GitBranch)
            .label(PlaceMode::Worktree.label())
            .selected(on)
            .tooltip(PlaceMode::Worktree.hint())
            .on_click(move |_, cx| {
                let mode = if on {
                    PlaceMode::InPlace
                } else {
                    PlaceMode::Worktree
                };
                let _ = toggle.update(cx, |view, cx| view.pick_place(mode.value(), cx));
            });
            card = card.footer_start(
                div()
                    .w_full()
                    .flex()
                    .justify_end()
                    .child(div().flex_none().child(button)),
            );
        }
        // Another run edits the folder itself: say so over the composer, with Worktree.
        let busy = self.draft.busy.clone().map(|message| {
            let retry = weak.clone();
            div()
                .w_full()
                .mb(px(8.0))
                .flex()
                .items_center()
                .gap(px(10.0))
                .text_size(px(theme.font_size_xs))
                .text_color(theme.color_text_muted)
                .child(div().flex_1().min_w(px(0.0)).child(wrap_text(message)))
                .child(
                    button(
                        "draft-new-worktree",
                        "Worktree",
                        ButtonKind::Primary,
                        Rc::new(move |_, cx| {
                            let _ = retry.update(cx, |view, cx| view.retry_in_worktree(cx));
                        }),
                        cx,
                    )
                    .size(ButtonSize::Sm),
                )
        });
        let card = div().w_full().children(busy).child(
            div()
                .w_full()
                .flex()
                .items_end()
                .gap(px(8.0))
                .children(self.voice_mark(cx))
                .child(div().flex_1().min_w(px(0.0)).child(card)),
        );
        let column = div()
            .flex_1()
            .min_h(px(0.0))
            .w_full()
            .min_w(px(0.0))
            .flex()
            .flex_col()
            .child(div().flex_1().min_h(px(0.0)))
            .child(
                div()
                    .w_full()
                    .flex_none()
                    .flex()
                    .justify_center()
                    .px(px(16.0))
                    .pb(px(16.0))
                    .child(div().w_full().max_w(px(WIDE_MEASURE)).child(card)),
            );
        // Nothing has run yet, so the rail has nothing to hold.
        let (row, _) = rail_page_body_under(
            column,
            div().into_any_element(),
            &self.rail,
            &self.page_width,
            header,
            window,
            cx,
        );
        let picker = self.draft.picker.clone();
        let overlay = (picker.read(cx).is_presented())
            .then(|| self.draft.anchor.get())
            .flatten()
            .map(|anchor| window_anchor(anchor, picker.into_any_element()));
        let place_picker = self.draft.place_picker.clone();
        let place_overlay = (place_picker.read(cx).is_presented())
            .then(|| self.draft.folder_anchor.get())
            .flatten()
            .map(|anchor| window_anchor(anchor, place_picker.into_any_element()));
        div()
            .relative()
            .size_full()
            .min_w(px(0.0))
            .flex()
            .flex_col()
            .text_color(theme.color_text_primary)
            .child(self.page_width.probe())
            .child(row)
            .child(
                div()
                    .absolute()
                    .top(px(0.0))
                    .left(px(0.0))
                    .right(px(0.0))
                    .child(trailing_toolbar(
                        bar,
                        theme.color_surface,
                        header,
                        theme.radius_surface,
                    )),
            )
            .children(overlay)
            .children(place_overlay)
            .into_any_element()
    }

    /// Sends `text` to the open run's agent session: a new run that resumes it, which the
    /// page opens once the core takes it.
    fn send_message(&mut self, text: String, cx: &mut Context<Self>) {
        let prompt = text.trim().to_string();
        if prompt.is_empty() || self.sending {
            return;
        }
        let voiced = self.take_voiced(&prompt);
        let Some(run) = self.snapshot_run.clone() else {
            return;
        };
        if !run.resumable || follow_up_blocker(&run, self.state.read(cx).runs()).is_some() {
            return;
        }
        let Some(client) = self.state.read(cx).client() else {
            self.toast("Dispatch core is not running", ToastTone::Danger, cx);
            return;
        };
        self.sending = true;
        cx.notify();
        let request = StartRunRequest {
            project_id: run.project_id.clone(),
            prompt,
            resume: run.id.clone(),
            // A follow-up keeps the mode its run was started with.
            images: run.images,
            ..StartRunRequest::default()
        };
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { client.start_run(&request) })
                .await;
            let _ = this.update(cx, |view, cx| {
                view.sending = false;
                match result {
                    Ok(response) => {
                        view.message.update(cx, |input, cx| input.set_text("", cx));
                        if response.run.id.is_empty() {
                            cx.notify();
                        } else {
                            let id = response.run.id.clone();
                            if voiced {
                                view.voiced.mark_voiced(&id);
                            }
                            view.state
                                .update(cx, |state, _| state.note_run(response.run));
                            view.open_run(id, cx);
                        }
                    }
                    Err(error) => {
                        let message = match error {
                            CoreError::Rpc(message) if !message.is_empty() => {
                                format!("The message was not sent: {message}")
                            }
                            _ => "The message was not sent.".to_string(),
                        };
                        view.toast(message, ToastTone::Danger, cx);
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }

    /// The composer under the transcript, for a run whose agent kept its session.
    /// The composer under the transcript, on every run's page: its send waits while the run
    /// (or a message to it, or another run in its folder) works, and stays off for a run that
    /// kept no session to send to.
    fn composer(
        &self,
        run: &DispatchRun,
        end_pad: f32,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let blocked =
            follow_up_blocker(run, self.state.read(cx).runs()).is_some() || !run.resumable;
        let weak = cx.weak_entity();
        let input = self.message.clone();
        let mut card = message_composer("run-message", &self.message, &self.message_scroll)
            .busy(self.sending)
            .on_send(Rc::new(move |_, cx| {
                let text = input.read(cx).text().to_string();
                let _ = weak.update(cx, |view, cx| view.send_message(text, cx));
            }));
        if blocked {
            card = card.can_send(false);
        }
        card = card.trailing_action(self.dictation_action(cx));
        Some(
            div()
                .w_full()
                .flex_none()
                .flex()
                .justify_center()
                .pl(px(16.0))
                // The column runs to the page's edge: the composer keeps the text's room.
                .pr(px(16.0 + end_pad))
                .pb(px(16.0))
                .child(
                    div()
                        .w_full()
                        .max_w(px(WIDE_MEASURE))
                        .flex()
                        .items_end()
                        .gap(px(8.0))
                        .children(self.voice_mark(cx))
                        .child(div().flex_1().min_w(px(0.0)).child(card)),
                )
                .into_any_element(),
        )
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
                    let log = log_tail(&client, &run_id);
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
        let follows = self
            .state
            .read(cx)
            .run(id)
            .is_some_and(|run| run.resumed_of == self.transcript_run);
        if follows && !self.transcript_run.is_empty() {
            // A message to the open run: the chat goes on in the same transcript.
            let owned = id.to_string();
            self.transcript
                .update(cx, |transcript, cx| transcript.continue_with(owned, cx));
        } else {
            let state = self.state.clone();
            let owned = id.to_string();
            self.transcript =
                cx.new(|cx| super::transcript::TranscriptView::new(state, owned, false, cx));
        }
        self.transcript_run = id.to_string();
    }

    fn apply_run(&mut self, run: &DispatchRun, reply: &str, cx: &mut Context<Self>) {
        if self.transcript_run != run.id {
            self.output_log.clear();
            self.log_for.clear();
            self.meta_for.clear();
            self.decision = None;
            self.candidate_pages = 1;
            self.expanded_handoffs.clear();
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
        // A run shows its rail toggle while it works too, so its routing and changes can
        // be read live; a new conversation, with no run yet, has none.
        self.rail_reveal.snap(1.0);
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
        let moving = !self.confirm_progress.settled()
            || !self.context_fill.settled()
            || !self.rail_reveal.settled();
        let dt = self.animator.dt(window, moving);
        if dt > 0.0 {
            self.confirm_progress.step(dt);
            self.context_fill.step(dt);
            self.rail_reveal.step(dt);
        }
    }

    /// The next step: Cancel while the run works, Merge for a finished run with a worktree
    /// that is not merged, Delete worktree once it is, each a small button with a 12px mark;
    /// then "…" with Recover run and Remove worktree, when the run offers either.
    fn toolbar_end(
        &self,
        run: &DispatchRun,
        menu_open: bool,
        weak: WeakEntity<Self>,
    ) -> AnyElement {
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
                Button::new("run-cancel", ButtonKind::Danger)
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
                Button::new("run-delete-worktree", ButtonKind::Danger)
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
            // A face like the rail toggle's beside it; no tooltip over its own open menu.
            let more = Button::new("run-more", ButtonKind::Secondary)
                .size(ButtonSize::Icon)
                .icon(Icon::More)
                .selected(menu_open)
                .on_click(move |_, cx| {
                    let _ = weak.update(cx, |view, cx| view.toggle_menu(cx));
                });
            row = row.child(if menu_open {
                more
            } else {
                more.tooltip("More run actions")
            });
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
        if matches!(self.state.read(cx).route(), Route::NewRun) {
            return self.render_draft(window, cx);
        }
        self.draft.open = false;
        if self.draft.picker.read(cx).is_open() {
            self.draft.picker.update(cx, |picker, cx| picker.close(cx));
        }
        if self.draft.place_picker.read(cx).is_open() {
            self.draft
                .place_picker
                .update(cx, |picker, cx| picker.close(cx));
        }
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
        let menu_open = self.menu.read(cx).is_open();
        // Any run has its rail and toggle, working or ended.
        let ended = self.snapshot_run.is_some();
        let visible = ended && self.rail_visible();
        self.rail.set_open(visible);
        let reveal = self.rail_reveal.value().clamp(0.0, 1.2);
        let bar = if let Some(run) = self.snapshot_run.clone() {
            let toggle_view = weak.clone();
            let end = div()
                .flex()
                .items_center()
                .child(self.toolbar_end(&run, menu_open, weak.clone()))
                .when(ended || reveal > 0.001, |end| {
                    // The toggle grows into its slot (the 8px gap and the 28px button) and
                    // fades in, pushing the actions before it aside, on the spring's overshoot.
                    end.child(
                        div()
                            .flex_none()
                            .w(px(RAIL_TOGGLE_SLOT * reveal.max(0.0)))
                            // gpui 0.2.2 clips both axes even for x alone, so the clip box
                            // reaches 4px past the button above and below: its edge and drop
                            // stay whole while the width grows.
                            .overflow_hidden()
                            .py(px(4.0))
                            .my(px(-4.0))
                            .opacity(reveal.clamp(0.0, 1.0))
                            .flex()
                            .justify_end()
                            .child(
                                Button::new("run-rail-toggle", ButtonKind::Secondary)
                                    .size(ButtonSize::Icon)
                                    .icon(Icon::SidebarRightIcon)
                                    .tooltip(rail_toggle_label(visible))
                                    .selected(visible)
                                    .on_click(move |_, cx| {
                                        let _ =
                                            toggle_view.update(cx, |view, cx| view.toggle_rail(cx));
                                    }),
                            ),
                    )
                });
            let (key, label) = self.run_route(&run);
            // Whether the run has a worktree of its own, once that is known (a pending run has
            // not cut one yet).
            let worktree = match worktree_badge(&run) {
                Some(true) => Some(place_pill(
                    "run-worktree",
                    Icon::GitBranch,
                    "Worktree",
                    &run.branch,
                    None,
                )),
                Some(false) => Some(place_pill(
                    "run-worktree",
                    Icon::Folder,
                    "No worktree",
                    &run.dir,
                    None,
                )),
                None => None,
            };
            let start = div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .child(route_pill(
                    &key,
                    &label,
                    avatar_status(&run.status, &run.merged_at),
                    None,
                    cx,
                ))
                .children(worktree)
                .children(run.images.then(|| {
                    place_pill(
                        "run-images",
                        Icon::Eye,
                        IMAGE_PILL,
                        "This run asked for images",
                        None,
                    )
                }));
            rail_page_header(start, end, cx).into_any_element()
        } else {
            toolbar(div(), div(), cx).into_any_element()
        };

        let header = theme.page_header_height;
        let mut drawer = None;
        let body = if self.snapshot_run.is_some() {
            let run_section = self.run_section.clone();
            let routing = self.routing.clone();
            let changes = self.changes.clone();
            let linked = self.linked.clone();
            let rail = div()
                .w_full()
                .flex()
                .flex_col()
                .child(run_section)
                .when(show_routing, |rail| rail.child(routing))
                .child(changes)
                .when(show_linked, |rail| rail.child(linked))
                .into_any_element();
            // The column runs to the page's edge (or the rail), so the transcript's scrollbar
            // sits as far right as it can; the text and the composer keep the room after.
            let end_pad = rail_page_end_pad(&self.rail, &self.page_width);
            self.transcript
                .update(cx, |transcript, cx| transcript.set_end_pad(end_pad, cx));
            // `flex min-w-0 flex-col gap-6 pt-2` around the transcript and `logEnd`: the
            // transcript's list holds the `pt-2` and the gap as its padding, so they scroll.
            let column = div()
                .flex_1()
                .min_h(px(0.0))
                .w_full()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                // Cached: a live bot's frames redraw the window, and the transcript keeps its
                // last layout and paint until it notifies.
                .child(
                    AnyView::from(self.transcript.clone())
                        .cached(StyleRefinement::default().flex_1().min_h_0().w_full()),
                )
                .children(
                    self.snapshot_run
                        .clone()
                        .and_then(|run| self.composer(&run, end_pad, cx)),
                );
            // The transcript runs up under the header, which fades it out on its trail.
            self.transcript
                .update(cx, |transcript, cx| transcript.set_top_inset(header, cx));
            let (row, over) = rail_page_body_flush(
                column,
                rail,
                &self.rail,
                &self.page_width,
                header,
                window,
                cx,
            );
            drawer = over;
            row.into_any_element()
        } else {
            div()
                .flex_1()
                .min_h(px(0.0))
                .flex()
                .flex_col()
                .pt(px(header))
                .child(missing_body(&connection, theme, cx))
                .into_any_element()
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
            .child(body)
            // The header floats over the page; its background is the toolbar's trail.
            .child(
                div()
                    .absolute()
                    .top(px(0.0))
                    .left(px(0.0))
                    .right(px(0.0))
                    .child(trailing_toolbar(
                        bar,
                        theme.color_surface,
                        header,
                        theme.radius_surface,
                    )),
            )
            .children(drawer)
            .child(
                div()
                    .absolute()
                    .top(px(0.0))
                    // Under "…", which sits before the rail toggle once it is in.
                    .right(px(
                        theme.space_4 + RAIL_TOGGLE_SLOT * self.rail_reveal.value().clamp(0.0, 1.0)
                    ))
                    .w(px(200.0))
                    .child(menu),
            )
            .child(self.confirm_layer(&confirm_id, weak, theme))
            .into_any_element()
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

/// The lines of a run's log the rail reads its live token count from, the latest usage line
/// being among the last ones.
const LOG_TAIL_LINES: i32 = 200;

/// The last [`LOG_TAIL_LINES`] lines of the run's log: the line count first, then that page,
/// so a live run's rail never fetches its whole log again on each usage change.
fn log_tail(client: &CoreClient, run_id: &str) -> Option<String> {
    let page = |offset: i32, limit: i32| {
        client
            .result(&ResultRequest {
                id: run_id.to_string(),
                section: "output".to_string(),
                offset,
                limit,
            })
            .ok()
    };
    let total = page(0, 1)?.total;
    let offset = (total - LOG_TAIL_LINES).max(0);
    page(offset, LOG_TAIL_LINES).map(|response| response.content)
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

/// The header's lead, in Roles' role title (a small secondary pill button): the run's live
/// bot, the agent's name, and the model in muted ink. Inert: the material, not a control.
/// What a press on the header pill does.
type PillClick = Rc<dyn Fn(&mut Window, &mut App)>;

/// `element` with its bounds written to `anchor` each frame, for a picker to hang from.
/// Whether `run` has a worktree of its own: `None` while that is not known yet.
fn worktree_badge(run: &DispatchRun) -> Option<bool> {
    if !run.worktree.is_empty() && !run.in_place {
        Some(true)
    } else if run.in_place
        || run.read_only
        || !is_run_active(&run.status)
        || run.status == "RUNNING"
    {
        Some(false)
    } else {
        None
    }
}

fn anchored(element: AnyElement, anchor: AnchorSlot) -> gpui::Div {
    div().relative().child(element).child(anchor.probe())
}

/// A header pill for where a run works: its folder, or its mode. A run's is inert; the
/// new conversation's folder opens its picker. `detail` is its tooltip.
fn place_pill(
    id: &'static str,
    icon: Icon,
    label: &str,
    detail: &str,
    on_click: Option<PillClick>,
) -> AnyElement {
    let mut pill = Button::new(id, ButtonKind::Secondary)
        .size(ButtonSize::Sm)
        .pill()
        .pad_x(10.0)
        .icon(icon)
        .label(label.to_string());
    if !detail.is_empty() {
        pill = pill.tooltip(detail.to_string());
    }
    match on_click {
        Some(click) => pill
            .on_click(move |window, cx| click(window, cx))
            .into_any_element(),
        None => pill.inert().into_any_element(),
    }
}

/// A header pill that is a switch: Image mode, and anything else the header toggles. It takes
/// the raised secondary face while on, and its tooltip says what it does either way.
fn toggle_pill(
    id: &'static str,
    icon: Icon,
    label: &str,
    detail: &str,
    on: bool,
    on_click: PillClick,
) -> AnyElement {
    Button::new(id, ButtonKind::Secondary)
        .size(ButtonSize::Sm)
        .pill()
        .pad_x(10.0)
        .icon(icon)
        .label(label.to_string())
        .selected(on)
        .tooltip(detail.to_string())
        .on_click(move |window, cx| on_click(window, cx))
        .into_any_element()
}

/// The header pill: the route's bot (the Polterware ghost for a tier, the agent's live bot
/// once it has one) in Roles' role title material, then its name and, muted, its model. A new
/// conversation's pill opens the route picker; a run's is inert. `key` holds what it showed,
/// so a route that gets its agent turns into it: the bot spins into the agent's, and the
/// name and model roll into theirs.
fn route_pill(
    key: &str,
    label: &RouteLabel,
    status: &str,
    on_click: Option<PillClick>,
    cx: &App,
) -> AnyElement {
    let theme = Theme::global(cx);
    let model = div()
        .when(!label.model.is_empty(), |model| model.ml(px(6.0)))
        .text_size(px(theme.font_size_xs))
        .text_color(theme.color_text_muted)
        .child(text_swap(
            SharedString::from(format!("{key}-model")),
            label.model.clone(),
        ));
    let title = Button::new("run-agent", ButtonKind::Secondary)
        .size(ButtonSize::Sm)
        .pill()
        .pad_x(5.0)
        .leading(route_avatar(
            SharedString::from(format!("{key}-bot")),
            label.agent.as_deref(),
            status,
            20.0,
        ))
        .trailing(
            div()
                .flex()
                .items_center()
                .child(text_swap(
                    SharedString::from(format!("{key}-name")),
                    label.name.clone(),
                ))
                .child(model),
        );
    match on_click {
        Some(click) => title
            .on_click(move |window, cx| click(window, cx))
            .into_any_element(),
        None => title.inert().into_any_element(),
    }
}

/// The attempts that failed before the one that ran, as 18px bots beside their route with
/// the error wrapped under it, for the Routing group. `None` when the first attempt is the last.
/// The failed attempts no routing candidate stands for, each with its bot, its model, and
/// its error wrapped under it; the others show their error on their own candidate row.
fn failed_attempts(attempts: &[&DispatchAttempt], theme: &Theme) -> Option<gpui::Div> {
    if attempts.is_empty() {
        return None;
    }
    let mut list = div().flex().min_w(px(0.0)).flex_col().gap(px(tw(1.5)));
    for attempt in attempts {
        let title = [attempt.agent.trim(), attempt.model.trim()]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(" / ");
        list = list.child(
            div()
                .flex()
                .min_w(px(0.0))
                .items_start()
                .gap(px(tw(2.0)))
                .text_size(px(12.0))
                .text_color(theme.color_text_muted)
                .child(agent_avatar(&attempt.agent, "failed", 18.0))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .child(wrapped(&title))
                        .when(!attempt.error.trim().is_empty(), |column| {
                            column.child(
                                div()
                                    .text_color(theme.color_danger)
                                    .child(wrapped(attempt.error.trim())),
                            )
                        }),
                ),
        );
    }
    Some(list)
}

fn routing_attempt_switches(
    switches: &[AttemptSwitch],
    expanded: &HashSet<usize>,
    view: &WeakEntity<RunPageView>,
    theme: &Theme,
) -> Option<gpui::Div> {
    if switches.is_empty() {
        return None;
    }
    let mut list = div().flex().min_w(px(0.0)).flex_col().gap(px(tw(1.0)));
    for sw in switches {
        let index = sw.index;
        let is_open = expanded.contains(&index);
        let handoff = (!sw.handoff.trim().is_empty()).then_some(sw.handoff.as_str());
        let toggle = handoff.map(|_| {
            let view = view.clone();
            Rc::new(move |_: &mut Window, cx: &mut App| {
                let _ = view.update(cx, |page, cx| {
                    if !page.expanded_handoffs.remove(&index) {
                        page.expanded_handoffs.insert(index);
                    }
                    page.routing.update(cx, |_, cx| cx.notify());
                    cx.notify();
                });
            }) as Rc<dyn Fn(&mut Window, &mut App)>
        });
        list = list.child(crate::ui::parts::transcript::attempt_switch_divider(
            10_000 + index,
            &sw.label,
            handoff,
            is_open,
            theme,
            toggle,
        ));
    }
    Some(list)
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
                .child(wrap_text(format_tokens(total_tokens(usage)))),
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
                .child(wrap_text(priced.label)),
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

/// `RunRoutingGroup`: the decision's policy, task, role, and area, the failed attempts, then the
/// first few candidates in routing order (Show more lists the rest) (`mt-1 flex-col gap-0.5 px-1`), its 18px bot, `model @effort` in 12px mono
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
    if let Some(difficulty) = difficulty_label(&decision) {
        rows = rows.child(rail_property("Difficulty", wrapped(&difficulty), false, cx));
    }
    if let Some(classifier) = classifier_bands(&decision) {
        rows = rows.child(rail_property(
            "Classifier",
            classifier_chart(&classifier, &theme),
            false,
            cx,
        ));
    }
    rows = rows.child(rail_property(
        "Priority",
        wrapped(&priority_label(&decision)),
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
    if let Some(reason) = route_reason(&decision) {
        rows = rows.child(rail_property("Why", wrapped(&reason), false, cx));
    }
    let mut candidates = div()
        .mt(px(tw(1.0)))
        .flex()
        .flex_col()
        .gap(px(tw(0.5)))
        .px(px(tw(1.0)));
    let pages = view.upgrade().map_or(1, |v| v.read(cx).candidate_pages);
    let total = decision.candidates.len();
    let outcomes = candidate_outcomes(&decision, &pack.run);
    // The ones the run tried stay in view, however far down the list.
    let pinned: Vec<bool> = outcomes
        .iter()
        .map(|outcome| {
            matches!(
                outcome,
                Some(CandidateOutcome::Used { .. } | CandidateOutcome::Failed { .. })
            )
        })
        .collect();
    for (index, candidate) in decision.candidates.iter().enumerate() {
        if !shown_candidate(index, &pinned, pages) {
            continue;
        }
        let outcome = outcomes[index].clone();
        let price = candidate_line(candidate);
        let explain = candidate_explain(&decision, candidate);
        let title = if candidate.effort.is_empty() {
            candidate.model.clone()
        } else {
            format!("{} @{}", candidate.model, candidate.effort)
        };
        let model_line = if candidate.effort.is_empty() {
            candidate.model.clone()
        } else {
            format!("{} @{}", candidate.model, candidate.effort)
        };
        let error = match &outcome {
            Some(CandidateOutcome::Failed { error, .. }) if !error.is_empty() => {
                Some(error.clone())
            }
            _ => None,
        };
        // What became of it in this run; otherwise whether Auto counted it out.
        let marker = match &outcome {
            Some(found @ CandidateOutcome::Used { status }) => Some((
                found.label(),
                if is_run_active(status) {
                    crate::theme::StatusTone::Syncing
                } else {
                    crate::theme::dispatch_status_tone(status)
                },
            )),
            Some(found @ CandidateOutcome::Failed { .. }) => {
                Some((found.label(), crate::theme::StatusTone::Danger))
            }
            Some(found @ CandidateOutcome::Skipped) => {
                Some((found.label(), crate::theme::StatusTone::Muted))
            }
            None if decision.policy == "auto-v1" && !candidate.qualified => {
                Some(("Below bar".to_string(), crate::theme::StatusTone::Muted))
            }
            None => None,
        };
        candidates = candidates.child(
            div()
                .id(("routing-candidate", index))
                .flex()
                .min_w(px(0.0))
                .items_start()
                .gap(px(tw(2.0)))
                .p(px(tw(1.0)))
                .tooltip(rich_tooltip(CANDIDATE_TIP_WIDTH, move |_, cx| {
                    candidate_tip(&title, &explain, cx)
                }))
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
                        })
                        .children(error.map(|error| {
                            div()
                                .pt(px(2.0))
                                .text_size(px(12.0))
                                .text_color(theme.color_danger)
                                .child(wrapped(&error))
                        })),
                )
                .children(marker.map(|(label, tone)| {
                    // Level with the model line when the error below it wraps.
                    div().flex_none().child(chip(label, tone, cx))
                })),
        );
    }
    if let Some((label, next)) = more_candidates(total, &pinned, pages) {
        let toggle = view.clone();
        candidates = candidates.child(
            div()
                .id("routing-more")
                .p(px(tw(1.0)))
                .cursor_pointer()
                .text_size(px(12.0))
                .text_color(theme.color_text_muted)
                .hover(|style| style.text_color(theme.color_text_primary))
                .child(label)
                .on_click(move |_, _, cx| {
                    let _ = toggle.update(cx, |page, cx| {
                        page.candidate_pages = next;
                        page.routing.update(cx, |_, cx| cx.notify());
                        cx.notify();
                    });
                }),
        );
    }
    let expanded_handoffs = view
        .upgrade()
        .map_or_else(HashSet::new, |v| v.read(cx).expanded_handoffs.clone());
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(6.0))
        .child(rows)
        .children(
            failed_attempts(&unmatched_attempts(&decision, &pack.run), &theme)
                .map(|list| list.px(px(tw(1.0)))),
        )
        .children(
            routing_attempt_switches(
                &attempt_switches(&pack.run),
                &expanded_handoffs,
                view,
                &theme,
            )
            .map(|list| list.px(px(tw(1.0)))),
        )
        .child(candidates)
        .into_any_element()
}

/// The width of a routing candidate's tooltip, so its sentences wrap inside it.
const CANDIDATE_TIP_WIDTH: f32 = 300.0;

/// A routing candidate's tooltip: what it was judged on, its numbers, and how they ranked it.
fn candidate_tip(title: &str, explain: &CandidateExplain, cx: &App) -> AnyElement {
    let theme = Theme::global(cx);
    let mut tip = div().flex().flex_col().gap(px(8.0)).child(tooltip_heading(
        title.to_string(),
        Some(explain.about.clone().into()),
        cx,
    ));
    for (label, value) in &explain.rows {
        tip = tip.child(tooltip_row(
            label.clone(),
            SharedString::from(value.clone()),
            cx,
        ));
    }
    tip.child(
        div()
            .text_color(theme.color_text_secondary)
            .child(SharedString::from(explain.how.clone())),
    )
    .into_any_element()
}

/// The rail toggle's room in the header: the 8px gap before it and the 28px icon button.
const RAIL_TOGGLE_SLOT: f32 = 36.0;

/// How many candidates a page of the Routing group lists.
const ROUTING_PAGE: usize = 5;

/// The most ranked candidates the Routing group ever lists; the ones the run tried show
/// beyond it.
const ROUTING_MAX: usize = 10;

/// Whether the Routing group lists this candidate: those within the pages shown (never past
/// [`ROUTING_MAX`]), or one the run tried.
fn shown_candidate(index: usize, pinned: &[bool], pages: usize) -> bool {
    index < (pages.max(1) * ROUTING_PAGE).min(ROUTING_MAX)
        || pinned.get(index).copied().unwrap_or(false)
}

/// The Show more / Show fewer line and the page count a click sets, when there is one.
fn more_candidates(total: usize, pinned: &[bool], pages: usize) -> Option<(String, usize)> {
    let pages = pages.max(1);
    let limit = total.min(ROUTING_MAX);
    let shown = (pages * ROUTING_PAGE).min(limit);
    if shown < limit {
        let more = (limit - shown).min(ROUTING_PAGE);
        return Some((format!("Show {more} more"), pages + 1));
    }
    let first = (0..total)
        .filter(|&i| shown_candidate(i, pinned, 1))
        .count();
    let now = (0..total)
        .filter(|&i| shown_candidate(i, pinned, pages))
        .count();
    (pages > 1 && now > first).then(|| ("Show fewer".to_string(), 1))
}

/// The classifier's name over one bar per band: the band, a thin track filled to its
/// chance, and the chance in figures.
fn classifier_chart(classifier: &ClassifierBands, theme: &Theme) -> AnyElement {
    let mut chart = div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(4.0))
        .child(wrapped(&classifier.name));
    for (band, p) in &classifier.bands {
        chart = chart.child(
            div()
                .w_full()
                .flex()
                .items_center()
                .gap(px(8.0))
                .text_size(px(11.0))
                .child(
                    div()
                        .w(px(44.0))
                        .flex_none()
                        .text_color(theme.color_text_muted)
                        .child(SharedString::from(*band)),
                )
                .child(
                    div()
                        .flex_1()
                        .h(px(4.0))
                        .rounded_full()
                        .bg(theme.color_surface_active)
                        .child(
                            div()
                                .h_full()
                                .rounded_full()
                                .bg(theme.color_accent)
                                .w(gpui::relative(*p as f32)),
                        ),
                )
                .child(
                    div()
                        .w(px(32.0))
                        .flex_none()
                        .flex()
                        .justify_end()
                        .font(tabular(mono(theme, FontWeight::NORMAL)))
                        .text_color(theme.color_text_secondary)
                        .child(SharedString::from(percent(*p))),
                ),
        );
    }
    chart.into_any_element()
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
    // The other repositories the run spans, each by name, with its worktree.
    for linked in &run.linked {
        let name = linked
            .repo
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or(&linked.repo)
            .to_string();
        let place = if linked.merge_sha.is_empty() {
            linked.worktree.clone()
        } else {
            format!(
                "merged {}",
                &linked.merge_sha[..linked.merge_sha.len().min(8)]
            )
        };
        rows = rows.child(rail_property(&name, wrapped(&place), true, cx));
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
                .child(agent_avatar(
                    &link.agent,
                    avatar_status(&link.status, &link.merged_at),
                    18.0,
                ))
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
    wrap_text(text.to_string()).into_any_element()
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
        // The core is starting: the run's page, in bones, until it answers.
        ConnectionState::Connecting => div()
            .size_full()
            .px(px(theme.space_4))
            .child(page_skeleton(SkeletonLayout::Detail).without_toolbar())
            .into_any_element(),
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

#[cfg(test)]
mod tests {
    use super::{more_candidates, shown_candidate};

    #[test]
    fn routing_pages_by_five_up_to_ten() {
        let pinned = vec![false; 71];
        assert_eq!(
            more_candidates(71, &pinned, 1),
            Some(("Show 5 more".to_string(), 2))
        );
        assert!(shown_candidate(9, &pinned, 2));
        assert!(!shown_candidate(10, &pinned, 2));
        assert!(!shown_candidate(12, &pinned, 3));
        assert_eq!(
            more_candidates(71, &pinned, 2),
            Some(("Show fewer".to_string(), 1))
        );
        assert_eq!(
            more_candidates(8, &pinned[..8], 1),
            Some(("Show 3 more".to_string(), 2))
        );
        assert_eq!(more_candidates(4, &pinned[..4], 1), None);
    }

    #[test]
    fn a_tried_candidate_shows_past_the_limit() {
        let mut pinned = vec![false; 30];
        pinned[25] = true;
        assert!(shown_candidate(25, &pinned, 1));
        assert!(!shown_candidate(24, &pinned, 2));
    }
}
