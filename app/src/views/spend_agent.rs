//! Spend detail: every run one agent spent, as a table. The Spend page's card opens it with
//! [`Route::SpendAgent`]. The totals over the rows on show sit on top, as tiles; below them the
//! table lists each run (date, conversation, repository, model, tier, input, cache, and output
//! tokens, cost and where it came from, status), newest first. The shared `FilterPopover`'s one
//! field is Group by, whose None keeps a row per run and whose other picks fold the runs into
//! sections (conversation, repository, model, or day), each ending in its subtotal; the search
//! narrows the rows, and a column's header sorts them. A click on a run's row opens it.
//!
//! The rows come from the runs the app already holds, with the usage the Spend page sums.

use std::rc::Rc;

use gpui::{
    AnyElement, App, Context, Entity, FontWeight, IntoElement, Render, Window, div, prelude::*, px,
};

use crate::avatar::agent_avatar;
use crate::models::sort::{SortDirection, next_sort};
use crate::models::spend_agent::{
    COLUMNS, DEFAULT_SORT, EntryTotals, GroupBy, Layout, SpendColumn, SpendEntry, entries,
    group_spec, group_value, layout, visible_entries,
};
use crate::state::{AppState, Route};
use crate::theme::Theme;
use crate::ui::parts::spend as spend_parts;
use crate::ui::parts::spend_agent as parts;
use crate::ui::{
    Button, ButtonKind, ButtonSize, FilterField, FilterPopover, FiltersChanged, Icon, MascotMood,
    TextInput, page_empty, page_under, page_under_inset, toolbar_center,
};

/// The page's chrome state.
const PAGE: &str = "spend-agent-page";
const SEARCH_WIDTH: f32 = 200.0;
/// The agent's bot in the toolbar.
const TITLE_BOT: f32 = 20.0;

/// What the table was built from, so a poll that moves nothing keeps the rows it has.
struct Cache {
    agent: String,
    revision: u64,
    query: String,
    sort: Option<(SpendColumn, SortDirection)>,
    group: GroupBy,
    entries: Rc<Vec<SpendEntry>>,
    visible: Rc<Vec<SpendEntry>>,
    totals: EntryTotals,
    layout: Rc<Layout>,
}

pub struct SpendAgentView {
    state: Entity<AppState>,
    search: Entity<TextInput>,
    filters: Entity<FilterPopover>,
    query: String,
    sort: Option<(SpendColumn, SortDirection)>,
    cache: Option<Cache>,
}

impl SpendAgentView {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| TextInput::new(cx).placeholder("Search runs\u{2026}").pill());
        cx.observe(&search, |this, input, cx| {
            let text = input.read(cx).text().to_string();
            if text != this.query {
                this.query = text;
                cx.notify();
            }
        })
        .detach();
        let filters =
            cx.new(|cx| FilterPopover::new("spend-agent-filters", "Spend filters", true, cx));
        filters.update(cx, |popover, _| {
            popover.set_fields(vec![FilterField::new(group_spec(), Some(Icon::Layers))]);
        });
        cx.subscribe(&filters, |_, _, _: &FiltersChanged, cx| cx.notify())
            .detach();
        cx.observe(&filters, |_, _, cx| cx.notify()).detach();
        // The rows follow the runs as the app's poll brings them.
        cx.observe(&state, |_, _, cx| cx.notify()).detach();
        Self {
            state,
            search,
            filters,
            query: String::new(),
            sort: Some(DEFAULT_SORT),
            cache: None,
        }
    }

    /// The F key's control, while this page shows.
    pub fn filters(&self) -> &Entity<FilterPopover> {
        &self.filters
    }

    /// The agent whose runs the page lists, when it is the open route.
    fn agent(&self, cx: &App) -> Option<String> {
        match self.state.read(cx).route() {
            Route::SpendAgent(agent) => Some(agent.clone()),
            _ => None,
        }
    }

    /// The rows on show and their layout, rebuilt only when the runs, the route, the search,
    /// the filters, or the sort move.
    fn ensure_cache(&mut self, cx: &App) -> Option<&Cache> {
        let agent = self.agent(cx)?;
        let revision = self.state.read(cx).runs_revision();
        let group = group_value(self.filters.read(cx).filters());
        if self.cache.as_ref().is_some_and(|cache| {
            cache.agent == agent
                && cache.revision == revision
                && cache.query == self.query
                && cache.sort == self.sort
                && cache.group == group
        }) {
            return self.cache.as_ref();
        }
        let entries = {
            let state = self.state.read(cx);
            entries(&agent, state.runs(), state.workspaces(), &|id| {
                state.conversation_name(id).map(str::to_string)
            })
        };
        let visible = visible_entries(&entries, &self.query, self.sort);
        let totals = EntryTotals::of(visible.iter());
        let table = layout(&visible, group);
        self.cache = Some(Cache {
            agent,
            revision,
            query: self.query.clone(),
            sort: self.sort,
            group,
            entries: Rc::new(entries),
            visible: Rc::new(visible),
            totals,
            layout: Rc::new(table),
        });
        self.cache.as_ref()
    }

    /// The toolbar's left: Back to the Spend page.
    fn back_button(&self) -> AnyElement {
        let state = self.state.clone();
        Button::new("spend-agent-back", ButtonKind::Secondary)
            .size(ButtonSize::Icon)
            .icon(Icon::Back)
            .tooltip("Back")
            .on_click(move |_, cx| {
                state.update(cx, |state, cx| {
                    state.navigate(Route::Spend);
                    cx.notify();
                });
            })
            .into_any_element()
    }

    /// The toolbar's middle: the agent's bot and name, then how many runs it spent.
    fn title(&self, agent: &str, count: usize, cx: &App) -> AnyElement {
        let theme = *Theme::global(cx);
        let runs = if count == 1 {
            "1 run".to_string()
        } else {
            format!("{count} runs")
        };
        div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(agent_avatar(agent, "", TITLE_BOT))
            .child(
                div()
                    .text_size(px(12.5))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.color_text_primary)
                    .child(agent.to_string()),
            )
            .child(
                div()
                    .text_size(px(12.5))
                    .text_color(theme.color_text_muted)
                    .child(runs),
            )
            .into_any_element()
    }
}

impl Render for SpendAgentView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *Theme::global(cx);
        if self.agent(cx).is_none() {
            return div().size_full().into_any_element();
        }
        let (agent, run_count, group, visible, totals, table) = {
            let Some(cache) = self.ensure_cache(cx) else {
                return div().size_full().into_any_element();
            };
            (
                cache.agent.clone(),
                cache.entries.len(),
                cache.group,
                Rc::clone(&cache.visible),
                cache.totals.clone(),
                Rc::clone(&cache.layout),
            )
        };
        let empty = visible.is_empty();
        let filter_bar = FilterPopover::bar(&self.filters, window, cx);
        // The toolbar's inset, so the tiles and the table line up with its buttons.
        let padding = theme.space_2;
        let top = page_under_inset(PAGE, filter_bar.is_some(), padding, window, cx);
        let view = cx.entity();

        let content: AnyElement = if empty {
            let (title, mood) = if run_count == 0 {
                ("No runs for this agent", MascotMood::Sleeping)
            } else {
                ("No matches", MascotMood::Lost)
            };
            div()
                .flex_1()
                .flex()
                .flex_col()
                .pt(px(top))
                .child(page_empty(title, mood, cx))
                .into_any_element()
        } else {
            let sorted = self.sort.and_then(|(column, direction)| {
                COLUMNS
                    .iter()
                    .position(|item| *item == column)
                    .map(|index| (index, direction))
            });
            let sorted_view = view.clone();
            let table = parts::spend_agent_table(
                "spend-agent-rows".into(),
                group,
                visible,
                table,
                sorted,
                move |index, _, cx| {
                    sorted_view.update(cx, |this, cx| {
                        this.sort = next_sort(this.sort, COLUMNS[index]);
                        cx.notify();
                    });
                },
                Rc::new(move |run_id, _, cx| {
                    view.update(cx, |this, cx| {
                        this.state.update(cx, |state, cx| {
                            state.navigate(Route::Run(run_id.to_string()));
                            cx.notify();
                        });
                        cx.notify();
                    });
                }),
            )
            .fades(theme.color_surface);
            div()
                .flex_1()
                .min_h(px(0.0))
                .flex()
                .flex_col()
                .child(
                    div()
                        .flex_none()
                        .pt(px(top))
                        .pb(px(theme.space_2))
                        .child(spend_parts::totals_row(&totals.spend_totals())),
                )
                .child(
                    div()
                        .flex_1()
                        .min_h(px(0.0))
                        .flex()
                        .flex_col()
                        .overflow_hidden()
                        .child(table),
                )
                .into_any_element()
        };

        let end = div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(div().w(px(SEARCH_WIDTH)).child(self.search.clone()))
            .child(self.filters.clone());
        div()
            .size_full()
            .flex()
            .flex_col()
            .text_color(theme.color_text_primary)
            .child(page_under(
                PAGE,
                toolbar_center(
                    self.back_button(),
                    self.title(&agent, run_count, cx),
                    end,
                    cx,
                ),
                filter_bar,
                padding,
                content,
                window,
                cx,
            ))
            .into_any_element()
    }
}
