//! Runs, the sidebar's View all ([`Route::Runs`]): every workspace's runs as a grouped stack
//! of full-width rows (grouped by Status by default, with running work on top), searched from
//! the toolbar, filtered by Repository, Status, Agent, and Tier through the shared
//! `FilterPopover` (`ActiveFilterBar` under the toolbar), and grouped/sorted through the
//! toolbar's Display menu.

use std::rc::Rc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gpui::{
    AnyElement, AppContext as _, Context, Entity, IntoElement, Render, Window, div, prelude::*, px,
};

use crate::client::{CancelRunRequest, CoreError};
use crate::models::filter::Filters;
use crate::models::runs::{
    COLLAPSED_PREFERENCE, DISPLAY_PREFERENCE, RunGroup, RunRow, all_rows, filter_specs,
    group_run_rows, parse_runs_display, runs_default_display, visible_rows,
};
use crate::models::sidebar::{
    Direction, GroupBy, Show, SidebarDisplay, SortBy, conversation_ref,
};
use crate::state::{AppState, ConnectionState, Route};
use crate::theme::Theme;
use crate::ui::parts::runs::{RunAction, group_header, run_stack_row};
use crate::ui::{
    Button, ButtonKind, ButtonSize, FilterField, FilterPopover, FiltersChanged, Icon, MascotMood,
    Menu, MenuItem, TextInput, ToastTone, page_empty, page_under, page_under_inset,
    scroll_area_under, toolbar,
};

/// The page's chrome state.
const PAGE: &str = "runs-page";
/// `formatAgo` labels move on the page's 15s clock.
const CLOCK: Duration = Duration::from_secs(15);
/// The Display menu's width (`min-w-48`, 12rem).
const DISPLAY_MENU_WIDTH: f32 = 12.0 * crate::theme::REM;

pub struct RunsView {
    state: Entity<AppState>,
    search: Entity<TextInput>,
    filters: Entity<FilterPopover>,
    display: SidebarDisplay,
    collapsed: Vec<String>,
    display_menu: Entity<Menu>,
    query: String,
    /// The option ids the filter fields were last built from.
    fields_key: String,
    cancelling: Option<String>,
    now_ms: i64,
    /// Every workspace's rows, built once per runs revision: a scroll or a hover redraws the
    /// page every frame, and rebuilding every run's row each time made it stutter.
    rows: Rc<Vec<RunRow>>,
    rows_key: Option<u64>,
    /// The rows the filters and the search keep, for the inputs that kept them.
    visible: Rc<Vec<RunRow>>,
    visible_key: Option<VisibleKey>,
    /// The grouped rows shown on the page.
    groups: Rc<Vec<RunGroup>>,
}

/// What the visible rows depend on: the rows, the filters, and the search.
type VisibleKey = (u64, Filters, String);

impl RunsView {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let display = match state
            .read(cx)
            .read_preference::<serde_json::Value>(DISPLAY_PREFERENCE)
        {
            Ok(Some(value)) => parse_runs_display(&value.to_string()),
            _ => runs_default_display(),
        };
        let collapsed = state
            .read(cx)
            .read_preference::<Vec<String>>(COLLAPSED_PREFERENCE)
            .ok()
            .flatten()
            .unwrap_or_default();
        let display_menu = cx.new(Menu::new);
        cx.observe(&display_menu, |_, _, cx| cx.notify()).detach();
        let search = cx.new(|cx| TextInput::new(cx).placeholder("Search runs…").pill());
        cx.observe(&search, |this, input, cx| {
            let text = input.read(cx).text().to_string();
            if text != this.query {
                this.query = text;
                cx.notify();
            }
        })
        .detach();
        let filters = cx.new(|cx| FilterPopover::new("run-filters", "Run filters", true, cx));
        cx.subscribe(&filters, |_, _, _: &FiltersChanged, cx| cx.notify())
            .detach();
        cx.observe(&filters, |_, _, cx| cx.notify()).detach();
        cx.observe(&state, |_, _, cx| cx.notify()).detach();
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(CLOCK).await;
                let alive = this
                    .update(cx, |view, cx| {
                        view.now_ms = unix_ms();
                        cx.notify();
                    })
                    .is_ok();
                if !alive {
                    break;
                }
            }
        })
        .detach();
        Self {
            state,
            search,
            filters,
            display,
            collapsed,
            display_menu,
            query: String::new(),
            fields_key: String::new(),
            cancelling: None,
            now_ms: unix_ms(),
            rows: Rc::default(),
            rows_key: None,
            visible: Rc::default(),
            visible_key: None,
            groups: Rc::default(),
        }
    }

    /// The F key's control, while this page shows.
    pub fn filters(&self) -> &Entity<FilterPopover> {
        &self.filters
    }

    /// Every workspace's rows and the ones on show, rebuilt only when what they come from
    /// moved, and the filter fields with them. A workspace's name or path only changes with a
    /// refresh, which moves the runs revision too.
    fn refresh_rows(&mut self, cx: &mut Context<Self>) {
        let revision = self.state.read(cx).runs_revision();
        if self.rows_key != Some(revision) {
            let rows = {
                let state = self.state.read(cx);
                all_rows(state.workspaces(), state.runs())
            };
            self.sync_fields(&rows, cx);
            self.rows = Rc::new(rows);
            self.rows_key = Some(revision);
        }
        let filters = self.filters.read(cx).filters().clone();
        let visible_key = (revision, filters, self.query.clone());
        if self.visible_key.as_ref() != Some(&visible_key) {
            self.visible = Rc::new(visible_rows(
                &self.rows,
                &visible_key.1,
                &self.query,
                None,
            ));
            self.visible_key = Some(visible_key);
        }
        let mut groups = group_run_rows(&self.visible, &self.display, self.now_ms);
        let state = self.state.read(cx);
        for group in &mut groups {
            if let Some(name) =
                conversation_ref(&group.id).and_then(|id| state.conversation_name(id))
            {
                group.title = name.to_string();
            }
        }
        self.groups = Rc::new(groups);
    }

    /// The fields follow the runs' distinct values, rebuilt only when those change.
    fn sync_fields(&mut self, rows: &[RunRow], cx: &mut Context<Self>) {
        let [repository, status, agent, tier] = filter_specs(rows);
        let key = [&repository, &status, &agent, &tier]
            .iter()
            .map(|spec| {
                spec.options
                    .iter()
                    .map(|option| format!("{}\u{1d}{}", option.id, option.label))
                    .collect::<Vec<_>>()
                    .join("\u{1f}")
            })
            .collect::<Vec<_>>()
            .join("\u{1e}");
        if key == self.fields_key {
            return;
        }
        self.fields_key = key;
        self.filters.update(cx, |popover, _| {
            popover.set_fields(vec![
                FilterField::new(repository, Some(Icon::Folder)),
                FilterField::new(status, None),
                FilterField::new(agent, Some(Icon::Bot)),
                FilterField::new(tier, Some(Icon::Gauge)),
            ]);
        });
    }

    fn is_collapsed(&self, id: &str) -> bool {
        self.collapsed.iter().any(|item| item == id)
    }

    fn toggle_group(&mut self, id: String, cx: &mut Context<Self>) {
        if let Some(pos) = self.collapsed.iter().position(|item| item == &id) {
            self.collapsed.remove(pos);
        } else {
            self.collapsed.push(id);
        }
        self.persist_collapsed(cx);
        cx.notify();
    }

    fn persist_display(&self, cx: &gpui::App) {
        let _ = self
            .state
            .read(cx)
            .write_preference(DISPLAY_PREFERENCE, &self.display);
    }

    fn persist_collapsed(&self, cx: &gpui::App) {
        let _ = self
            .state
            .read(cx)
            .write_preference(COLLAPSED_PREFERENCE, &self.collapsed);
    }

    fn toggle_display(&mut self, cx: &mut Context<Self>) {
        if self.display_menu.read(cx).is_open() {
            self.display_menu.update(cx, |menu, cx| {
                menu.set_open(false);
                cx.notify();
            });
            cx.notify();
            return;
        }
        let items = self.display_items(cx);
        self.display_menu.update(cx, |menu, cx| {
            menu.set_menu(items);
            menu.set_width(DISPLAY_MENU_WIDTH);
            menu.set_open(true);
            cx.notify();
        });
        cx.notify();
    }

    fn display_items(&self, cx: &mut Context<Self>) -> Vec<MenuItem> {
        let view = cx.entity();
        let radios = |labels: &[&str],
                      picked: usize,
                      pick: fn(&mut Self, usize, &mut Context<Self>)|
         -> Vec<MenuItem> {
            labels
                .iter()
                .enumerate()
                .map(|(index, label)| {
                    let view = view.clone();
                    MenuItem::radio(
                        label.to_string(),
                        index == picked,
                        Rc::new(move |_, cx| {
                            let _ = view.update(cx, |this, cx| pick(this, index, cx));
                        }),
                    )
                })
                .collect()
        };
        let group_index = match self.display.group_by {
            GroupBy::Conversation => 0,
            GroupBy::Repository => 1,
            GroupBy::Date => 2,
            GroupBy::Status => 3,
            GroupBy::Agent => 4,
            GroupBy::None => 5,
        };
        let sort_index = match self.display.sort_by {
            SortBy::Updated => 0,
            SortBy::Created => 1,
            SortBy::Name => 2,
        };
        let direction_index = match self.display.direction {
            Direction::Desc => 0,
            Direction::Asc => 1,
        };
        let show_index = match self.display.show {
            Show::All => 0,
            Show::Unmerged => 1,
            Show::Active => 2,
        };
        let mut sort = radios(
            &["Last updated", "Date created", "Name"],
            sort_index,
            Self::pick_sort,
        );
        sort.push(MenuItem::Separator);
        sort.extend(radios(
            &["Descending", "Ascending"],
            direction_index,
            Self::pick_direction,
        ));
        let reset = view;
        vec![
            MenuItem::submenu(
                "Group by",
                Some(Icon::Layers),
                radios(
                    &[
                        "Conversation",
                        "Repository",
                        "Date",
                        "Status",
                        "Agent",
                        "None",
                    ],
                    group_index,
                    Self::pick_group,
                ),
            ),
            MenuItem::submenu("Sort by", Some(Icon::ListOrdered), sort),
            MenuItem::submenu(
                "Show",
                Some(Icon::ListFilter),
                radios(
                    &["All runs", "Hide merged", "Running and needs merge"],
                    show_index,
                    Self::pick_show,
                ),
            ),
            MenuItem::Separator,
            MenuItem::action(
                "Reset to defaults",
                Some(Icon::RefreshCw),
                Rc::new(move |_, cx| {
                    let _ = reset.update(cx, |this, cx| this.reset_display(cx));
                }),
            ),
        ]
    }

    fn reset_display(&mut self, cx: &mut Context<Self>) {
        self.display = runs_default_display();
        self.persist_display(cx);
        cx.notify();
    }

    fn pick_group(&mut self, index: usize, cx: &mut Context<Self>) {
        let value = match index {
            0 => GroupBy::Conversation,
            1 => GroupBy::Repository,
            2 => GroupBy::Date,
            3 => GroupBy::Status,
            4 => GroupBy::Agent,
            _ => GroupBy::None,
        };
        if self.display.group_by != value {
            self.display.group_by = value;
            self.persist_display(cx);
            cx.notify();
        }
    }

    fn pick_sort(&mut self, index: usize, cx: &mut Context<Self>) {
        let value = match index {
            1 => SortBy::Created,
            2 => SortBy::Name,
            _ => SortBy::Updated,
        };
        if self.display.sort_by != value {
            self.display.sort_by = value;
            self.persist_display(cx);
            cx.notify();
        }
    }

    fn pick_direction(&mut self, index: usize, cx: &mut Context<Self>) {
        let value = if index == 0 {
            Direction::Desc
        } else {
            Direction::Asc
        };
        if self.display.direction != value {
            self.display.direction = value;
            self.persist_display(cx);
            cx.notify();
        }
    }

    fn pick_show(&mut self, index: usize, cx: &mut Context<Self>) {
        let value = match index {
            1 => Show::Unmerged,
            2 => Show::Active,
            _ => Show::All,
        };
        if self.display.show != value {
            self.display.show = value;
            self.persist_display(cx);
            cx.notify();
        }
    }

    fn open_run(&mut self, id: String, cx: &mut Context<Self>) {
        let state = self.state.clone();
        cx.defer(move |cx| {
            state.update(cx, |state, cx| {
                state.navigate(Route::Run(id));
                state.request_refresh(cx);
                cx.notify();
            });
        });
    }

    fn cancel_run(&mut self, id: String, cx: &mut Context<Self>) {
        let Some(client) = self.state.read(cx).client() else {
            return;
        };
        self.cancelling = Some(id.clone());
        cx.notify();
        cx.spawn(async move |this, cx| {
            let request = CancelRunRequest { id: id.clone() };
            let result = cx
                .background_spawn(async move { client.cancel_run(&request).map(|_| ()) })
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.cancelling.as_deref() == Some(id.as_str()) {
                    view.cancelling = None;
                }
                let state = view.state.clone();
                state.update(cx, |state, cx| {
                    if let Err(error) = result {
                        let message = match error {
                            CoreError::NotRunning => {
                                "Dispatch cannot reach its local core.".to_string()
                            }
                            CoreError::Rpc(message) => message,
                        };
                        state.push_toast(message, ToastTone::Danger);
                    }
                    state.request_refresh(cx);
                    cx.notify();
                });
                cx.notify();
            });
        })
        .detach();
    }
}

impl Render for RunsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *Theme::global(cx);
        self.refresh_rows(cx);
        let groups = self.groups.clone();
        let connecting = matches!(
            self.state.read(cx).connection(),
            ConnectionState::Connecting
        );
        let bar = FilterPopover::bar(&self.filters, window, cx);
        let padding = theme.space_2;
        let top = page_under_inset(PAGE, bar.is_some(), padding, window, cx);
        let scrolls = !groups.is_empty();

        let content: AnyElement = if scrolls {
            let body = runs_stack(&groups, self, cx);
            scroll_area_under("runs-scroll", top, body, window, cx)
        } else if connecting {
            div().into_any_element()
        } else {
            let (title, mood) = if self.rows.is_empty() {
                ("No runs yet", MascotMood::Sleeping)
            } else {
                ("No matches", MascotMood::Lost)
            };
            page_empty(title, mood, cx).into_any_element()
        };

        let display_open = self.display_menu.read(cx).is_open();
        let view = cx.entity();
        let display_button = Button::new("runs-display", ButtonKind::Secondary)
            .size(ButtonSize::Icon)
            .icon(Icon::Layers)
            .selected(display_open)
            .on_click(move |_, cx| {
                let _ = view.update(cx, |this, cx| this.toggle_display(cx));
            });
        let display_button = if display_open {
            display_button
        } else {
            display_button.tooltip("Display")
        };

        let end = div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(div().w(px(200.0)).child(self.search.clone()))
            .child(self.filters.clone())
            .child(display_button);
        div()
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .text_color(theme.color_text_primary)
            // The groups run up under the toolbar, which fades them out on its trail.
            .child(page_under(
                PAGE,
                toolbar(div(), end, cx),
                bar,
                padding,
                div()
                    .flex_1()
                    .min_h(px(0.0))
                    .flex()
                    .flex_col()
                    .when(!scrolls, |slot| slot.pt(px(top)))
                    .child(content),
                window,
                cx,
            ))
            .child(
                div()
                    .absolute()
                    .top(px(theme.space_2))
                    .right(px(DISPLAY_MENU_WIDTH * 2.0 + theme.space_4))
                    .w(px(DISPLAY_MENU_WIDTH))
                    .child(self.display_menu.clone()),
            )
    }
}

fn runs_stack(groups: &[RunGroup], view: &RunsView, cx: &mut Context<RunsView>) -> AnyElement {
    let theme = *Theme::global(cx);
    let entity = cx.entity();
    let on_toggle: RunAction = {
        let entity = entity.clone();
        Rc::new(move |id: &str, _, cx| {
            let id = id.to_string();
            entity.update(cx, |this, cx| this.toggle_group(id, cx));
        })
    };
    let on_open: RunAction = {
        let entity = entity.clone();
        Rc::new(move |id: &str, _, cx| {
            let id = id.to_string();
            entity.update(cx, |this, cx| this.open_run(id, cx));
        })
    };
    let on_cancel: RunAction = {
        let entity = entity.clone();
        Rc::new(move |id: &str, _, cx| {
            let id = id.to_string();
            entity.update(cx, |this, cx| this.cancel_run(id, cx));
        })
    };

    let mut stack = div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(theme.space_3))
        .pb(px(theme.space_4));
    let mut row_index = 0usize;
    for group in groups {
        let collapsed = view.is_collapsed(&group.id);
        let mut section = div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(theme.sidebar_row_gap))
            .child(group_header(group, collapsed, on_toggle.clone(), cx));
        if !collapsed {
            for row in &group.runs {
                let busy = view.cancelling.as_deref() == Some(row.id.as_str());
                section = section.child(run_stack_row(
                    row_index,
                    row,
                    busy,
                    view.now_ms,
                    on_open.clone(),
                    on_cancel.clone(),
                    cx,
                ));
                row_index += 1;
            }
        }
        stack = stack.child(section);
    }
    stack.into_any_element()
}

fn unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}
