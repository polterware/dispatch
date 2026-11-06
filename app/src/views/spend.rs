//! Spend: what Dispatch's runs used over the Spend window. The window's totals sit on top,
//! then one card per agent that ran, dearest first, with its tokens and its API-equivalent
//! cost. Account limits are the tiers' business, not this page's. The page reloads on entry,
//! filter changes, and observed app-state updates, with a two-second minimum between routine
//! reloads (a changed window can reload sooner); stats arrive before agent CLI quota probes.
//! Running runs are excluded by the stats report. The toolbar holds the search
//! and the shared `FilterPopover` (Provider and the Spend window), whose pills sit in the
//! `ActiveFilterBar`.

use std::time::{Duration, Instant};

use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, IntoElement, Render, Window, div,
    prelude::*, px,
};

use crate::client::{CoreClient, DispatchAgent, DispatchStatsRow, ListAgentsRequest, StatsRequest};
use crate::models::spend::{
    SpendRow, agent_rows, filter_specs, row_matches, since_value, spend_totals, spent_rows,
};
use crate::motion::{Animator, SpringTier, SpringValue};
use crate::state::{AppState, ConnectionState, Route};
use crate::theme::Theme;
use crate::ui::parts::spend as parts;
use crate::ui::{
    FilterField, FilterPopover, FiltersChanged, Icon, MascotMood, TextInput, card_grid, page_empty,
    page_under, page_under_inset, scroll_area_under, toolbar,
};
use crate::ui::{SkeletonLayout, page_skeleton};

const RELOAD_GAP: Duration = Duration::from_secs(2);
/// The page's chrome state.
const PAGE: &str = "spend-page";
/// The narrowest an agent card gets before the grid drops a column.
const CARD_MIN: f32 = 260.0;
const CARD_GAP: f32 = 10.0;

enum Slot<T> {
    Ready(T),
    Keep,
    Clear,
}

struct Bundle {
    agents: Slot<Vec<DispatchAgent>>,
    stats: Slot<Vec<DispatchStatsRow>>,
}

pub struct SpendView {
    state: Entity<AppState>,
    search: Entity<TextInput>,
    filters: Entity<FilterPopover>,
    /// The option ids the filter fields were last built from.
    fields_key: String,
    query: String,
    agents: Vec<DispatchAgent>,
    stats: Vec<DispatchStatsRow>,
    loaded: bool,
    on_page: bool,
    in_flight: bool,
    again: bool,
    last_kick: Instant,
    fetched_since: Option<String>,
    ticket: u64,
    animator: Animator,
    enter: SpringValue,
}

impl SpendView {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| {
            TextInput::new(cx)
                .placeholder("Search agents and models\u{2026}")
                .pill()
        });
        cx.observe(&search, |this, input, cx| {
            let text = input.read(cx).text().to_string();
            if text != this.query {
                this.query = text;
                cx.notify();
            }
        })
        .detach();
        cx.observe(&state, |this, _, cx| this.kick(cx)).detach();
        let filters = cx.new(|cx| FilterPopover::new("spend-filters", "Agent filters", true, cx));
        cx.subscribe(&filters, |this, _, _: &FiltersChanged, cx| {
            this.kick(cx);
            cx.notify();
        })
        .detach();
        cx.observe(&filters, |_, _, cx| cx.notify()).detach();
        let view = Self {
            state,
            search,
            filters,
            fields_key: String::new(),
            query: String::new(),
            agents: Vec::new(),
            stats: Vec::new(),
            loaded: false,
            on_page: false,
            in_flight: false,
            again: false,
            last_kick: Instant::now(),
            fetched_since: None,
            ticket: 0,
            animator: Animator::new(),
            enter: SpringValue::new(0.0, SpringTier::Moderate),
        };
        cx.spawn(async move |this, cx| {
            let _ = this.update(cx, |view, cx| view.kick(cx));
        })
        .detach();
        view
    }

    fn kick(&mut self, cx: &mut Context<Self>) {
        let on_spend = matches!(self.state.read(cx).route(), Route::Spend);
        if !on_spend {
            self.on_page = false;
            return;
        }
        let entered = !self.on_page;
        self.on_page = true;
        let since = since_value(self.filters.read(cx).filters()).to_string();
        let since_changed = self.fetched_since.as_deref() != Some(since.as_str());
        if self.in_flight {
            if entered || since_changed {
                self.again = true;
            }
            return;
        }
        if !entered && !since_changed && self.last_kick.elapsed() < RELOAD_GAP {
            return;
        }
        let Some(client) = self.state.read(cx).client() else {
            let offline = matches!(
                self.state.read(cx).connection(),
                ConnectionState::Offline { .. }
            );
            if offline && self.clear_for_offline() {
                cx.notify();
            }
            return;
        };
        self.in_flight = true;
        self.again = false;
        self.last_kick = Instant::now();
        self.fetched_since = Some(since.clone());
        self.ticket = self.ticket.wrapping_add(1);
        let ticket = self.ticket;
        cx.spawn(async move |this, cx| {
            // The window's figures answer at once; the agents' list can wait on a quota probe
            // of every CLI, so the cards draw first and take their providers when it comes.
            let first = client.clone();
            let stats = cx
                .background_spawn(async move { load_stats(&first, since) })
                .await;
            let current = this
                .update(cx, |view, cx| {
                    if view.ticket != ticket {
                        return false;
                    }
                    view.apply(
                        Bundle {
                            agents: Slot::Keep,
                            stats,
                        },
                        cx,
                    );
                    true
                })
                .unwrap_or(false);
            if !current {
                return;
            }
            let agents = cx
                .background_spawn(async move { load_agents(&client) })
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.ticket != ticket {
                    return;
                }
                view.in_flight = false;
                view.apply(
                    Bundle {
                        agents,
                        stats: Slot::Keep,
                    },
                    cx,
                );
                if view.again {
                    view.again = false;
                    view.kick(cx);
                }
            });
        })
        .detach();
    }

    fn clear_for_offline(&mut self) -> bool {
        let dirty = !self.loaded || !self.agents.is_empty() || !self.stats.is_empty();
        if !dirty {
            return false;
        }
        self.agents.clear();
        self.stats.clear();
        self.loaded = true;
        self.enter.set_target(1.0);
        true
    }

    fn apply(&mut self, bundle: Bundle, cx: &mut Context<Self>) {
        match bundle.agents {
            Slot::Ready(agents) => self.agents = agents,
            Slot::Keep => {}
            Slot::Clear => self.agents.clear(),
        }
        match bundle.stats {
            Slot::Ready(stats) => self.stats = stats,
            Slot::Keep => {}
            Slot::Clear => self.stats.clear(),
        }
        self.loaded = true;
        self.enter.set_target(1.0);
        cx.notify();
    }

    /// The F key's control, while this page shows.
    pub fn filters(&self) -> &Entity<FilterPopover> {
        &self.filters
    }

    fn tick_motion(&mut self, window: &mut Window) {
        self.enter.set_target(if self.loaded { 1.0 } else { 0.0 });
        let dt = self.animator.dt(window, !self.enter.settled());
        if dt > 0.0 {
            self.enter.step(dt);
        }
    }

    /// The fields follow the rows' providers, rebuilt only when those change.
    fn sync_fields(&mut self, rows: &[SpendRow], cx: &mut Context<Self>) {
        let specs = filter_specs(rows);
        let key = specs[0]
            .options
            .iter()
            .map(|option| option.id.as_str())
            .collect::<Vec<_>>()
            .join("\u{1f}");
        if key == self.fields_key && !self.filters.read(cx).fields().is_empty() {
            return;
        }
        self.fields_key = key;
        let icons = [Icon::Cpu, Icon::CalendarClock];
        self.filters.update(cx, |popover, _| {
            popover.set_fields(
                specs
                    .into_iter()
                    .zip(icons)
                    .map(|(spec, icon)| FilterField::new(spec, Some(icon)))
                    .collect(),
            );
        });
    }

    /// The agents Dispatch ran in the window, and those the search and the filters keep.
    fn rows(&self, cx: &App) -> (Vec<SpendRow>, Vec<SpendRow>) {
        let all = spent_rows(&agent_rows(&self.agents, &self.stats));
        let mut visible = all.clone();
        let filters = self.filters.read(cx).filters();
        visible.retain(|row| row_matches(row, &self.query, filters));
        (all, visible)
    }
}

impl Render for SpendView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.tick_motion(window);
        let theme = *Theme::global(cx);
        let (all, visible) = self.rows(cx);
        self.sync_fields(&all, cx);
        let enter = self.enter.value().clamp(0.0, 1.0);
        let filter_bar = FilterPopover::bar(&self.filters, window, cx);
        // The toolbar's inset, so the cards' edges line up with its buttons.
        let padding = theme.space_2;
        let top = page_under_inset(PAGE, filter_bar.is_some(), padding, window, cx);

        let content: AnyElement = if !self.loaded {
            // The cards, in bones, until the first answer comes back.
            page_skeleton(SkeletonLayout::Table)
                .without_toolbar()
                .into_any_element()
        } else if visible.is_empty() {
            let (title, mood) = if all.is_empty() {
                ("No Dispatch runs in this window", MascotMood::Sleeping)
            } else {
                ("No matches", MascotMood::Lost)
            };
            page_empty(title, mood, cx).into_any_element()
        } else {
            let cards = visible
                .iter()
                .map(|row| parts::agent_card(row, cx))
                .collect();
            let body = div()
                .w_full()
                .flex()
                .flex_col()
                .gap(px(CARD_GAP))
                .opacity(enter)
                .child(parts::totals_row(&spend_totals(&visible)))
                .child(card_grid(
                    "spend-cards",
                    CARD_MIN,
                    CARD_GAP,
                    3,
                    cards,
                    window,
                    cx,
                ));
            scroll_area_under("spend-scroll", top, body, window, cx)
        };

        // The `SearchBar` pill, then `FilterPopover`'s icon button, 8px apart.
        let end = div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(div().w(px(200.0)).child(self.search.clone()))
            .child(self.filters.clone());
        let scrolls = self.loaded && !visible.is_empty();
        div()
            .size_full()
            .flex()
            .flex_col()
            .text_color(theme.color_text_primary)
            // The cards run up under the toolbar, which fades them out on its trail.
            .child(page_under(
                PAGE,
                toolbar(div(), end, cx),
                filter_bar,
                padding,
                div()
                    .flex_1()
                    .min_h(px(0.0))
                    .flex()
                    .flex_col()
                    .when(!scrolls, |content| content.pt(px(top)))
                    .child(content),
                window,
                cx,
            ))
    }
}

fn load_agents(client: &CoreClient) -> Slot<Vec<DispatchAgent>> {
    match client.list_agents(&ListAgentsRequest::default()) {
        Ok(response) => Slot::Ready(response.agents),
        Err(_) => Slot::Keep,
    }
}

fn load_stats(client: &CoreClient, since: String) -> Slot<Vec<DispatchStatsRow>> {
    match client.stats(&StatsRequest {
        project_id: String::new(),
        by: "agent".into(),
        since,
    }) {
        Ok(response) => Slot::Ready(response.rows),
        Err(_) => Slot::Clear,
    }
}
