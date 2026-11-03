//! Intelligence: how able Auto takes every model it can route to be, from the catalog alone
//! (`ListModelCatalog`). A `DataTable` of Score, Model, Skill (θ), and P(pass) at each band,
//! sorted by Score, high to low, until a header sorts it; the note under it names the bar and
//! the catalog's sources. The toolbar holds the search and the shared `FilterPopover`, whose
//! pills sit in the `ActiveFilterBar` under it.

use std::rc::Rc;

use gpui::{
    AppContext as _, Context, Entity, FontWeight, IntoElement, Render, SharedString, Window, div,
    prelude::*, px,
};

use crate::client::{CoreClient, CoreError, ListModelCatalogRequest};
use crate::models::intelligence::{
    COLUMNS, Catalog, CatalogEntry, DEFAULT_SORT, SkillColumn, SkillSort, band_of, empty_catalog,
    empty_title, map_dispatch_catalog, model_filter_fields, percent_text, policy_bar, row_id,
    skill_digits, theta_source_label, theta_text, tier_label, visible_entries,
};
use crate::motion::{Animator, SpringTier, SpringValue};
use crate::state::{AppState, ConnectionState, ToastTone};
use crate::theme::{Theme, tw};
use crate::ui::parts::intelligence::{
    band_cell, catalog_footer, model_cell, score_cell, theta_cell,
};
use crate::ui::{
    Column, DataTable, FilterField, FilterPopover, FiltersChanged, Icon, MascotMood, TextInput,
    mono, next_sort, page_empty, page_fill_bar, sans, text_width, toolbar,
};

/// `SearchBar`'s `w-[200px]`, beside the filters' icon button, `gap-2` apart.
const SEARCH_WIDTH: f32 = 200.0;
const ACTIONS_GAP: f32 = 8.0;

pub struct IntelligenceView {
    state: Entity<AppState>,
    search: Entity<TextInput>,
    filters: Entity<FilterPopover>,
    /// The option ids the filter fields were last built from.
    fields_key: String,
    catalog: Catalog,
    loaded: bool,
    /// The last paint had no core client. A later client starts a real load.
    awaiting_core: bool,
    loading: bool,
    load_generation: u64,
    sort: SkillSort,
    empty_enter: SpringValue,
    animator: Animator,
}

impl IntelligenceView {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| {
            TextInput::new(cx)
                .placeholder("Search models\u{2026}")
                .pill()
        });
        cx.observe(&search, |_, _, cx| cx.notify()).detach();
        let filters =
            cx.new(|cx| FilterPopover::new("intelligence-filters", "Model filters", true, cx));
        cx.subscribe(&filters, |_, _, _: &FiltersChanged, cx| cx.notify())
            .detach();
        cx.observe(&filters, |_, _, cx| cx.notify()).detach();
        cx.observe(&state, |this, _, cx| {
            this.ensure_load(cx);
            cx.notify();
        })
        .detach();
        let mut view = Self {
            state,
            search,
            filters,
            fields_key: String::new(),
            catalog: empty_catalog(),
            loaded: false,
            awaiting_core: false,
            loading: false,
            load_generation: 0,
            sort: DEFAULT_SORT,
            empty_enter: SpringValue::new(0.0, SpringTier::Moderate),
            animator: Animator::new(),
        };
        view.ensure_load(cx);
        view
    }

    fn ensure_load(&mut self, cx: &mut Context<Self>) {
        if self.loading {
            return;
        }
        let (client, connection) = {
            let state = self.state.read(cx);
            (state.client(), state.connection().clone())
        };
        if client.is_none() {
            if matches!(connection, ConnectionState::Offline { .. }) && !self.loaded {
                self.loaded = true;
                self.awaiting_core = true;
                self.toast("The Dispatch core is not running.", ToastTone::Danger, cx);
            }
            return;
        }
        if self.loaded && !self.awaiting_core {
            return;
        }
        self.awaiting_core = false;
        self.spawn_load(cx);
    }

    fn spawn_load(&mut self, cx: &mut Context<Self>) {
        let Some(client) = self.state.read(cx).client() else {
            self.loading = false;
            return;
        };
        self.load_generation = self.load_generation.wrapping_add(1);
        let generation = self.load_generation;
        self.loading = true;
        cx.spawn(async move |this, cx| {
            let catalog = cx
                .background_spawn(async move { fetch_catalog(client) })
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.load_generation != generation {
                    return;
                }
                view.loading = false;
                view.loaded = true;
                match catalog {
                    Ok(json) => view.catalog = map_dispatch_catalog(&json),
                    Err(error) => view.toast(
                        error_text(&error, "Could not load the model catalog."),
                        ToastTone::Danger,
                        cx,
                    ),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn toast(&self, message: impl Into<SharedString>, tone: ToastTone, cx: &mut Context<Self>) {
        self.state.update(cx, |state, cx| {
            state.push_toast(message, tone);
            cx.notify();
        });
    }

    /// The F key's control, while this page shows.
    pub fn filters(&self) -> &Entity<FilterPopover> {
        &self.filters
    }

    /// The fields follow the catalog's values, rebuilt only when those change.
    fn sync_fields(&mut self, cx: &mut Context<Self>) {
        let specs = model_filter_fields(&self.catalog.entries, self.catalog.discover);
        let key = specs
            .iter()
            .map(|spec| {
                spec.options
                    .iter()
                    .map(|option| option.id.as_str())
                    .collect::<Vec<_>>()
                    .join("\u{1f}")
            })
            .collect::<Vec<_>>()
            .join("\u{1e}");
        if key == self.fields_key {
            return;
        }
        self.fields_key = key;
        let icons = [Icon::Bot, Icon::Gauge, Icon::LibraryBig, Icon::ShieldCheck];
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
}

impl Render for IntelligenceView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_fields(cx);
        let theme = *Theme::global(cx);
        let query = self.search.read(cx).text().to_string();
        let discover = self.catalog.discover;
        let bar = policy_bar(&self.catalog);
        let entries = &self.catalog.entries;
        let rows = {
            let filters = self.filters.read(cx).filters();
            visible_entries(entries, &query, filters, bar, discover, self.sort)
        };
        let filtered = !query.trim().is_empty() || !self.filters.read(cx).filters().is_empty();
        let has_entries = !entries.is_empty();
        self.empty_enter
            .set_target(if rows.is_empty() { 1.0 } else { 0.0 });
        let moving = !self.empty_enter.settled();
        let dt = self.animator.dt(window, moving);
        if dt > 0.0 {
            self.empty_enter.step(dt);
        }

        let view = cx.entity();
        let mut content = div()
            .flex_1()
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .gap(px(tw(3.0)));
        if rows.is_empty() {
            let mood = if filtered {
                MascotMood::Lost
            } else {
                MascotMood::Sleeping
            };
            content = content.child(
                page_empty(empty_title(filtered, has_entries, self.loaded), mood, cx)
                    .opacity(self.empty_enter.value().clamp(0.0, 1.0)),
            );
        } else {
            content = content
                .child(
                    div()
                        .flex_1()
                        .min_h(px(0.0))
                        .flex()
                        .flex_col()
                        .overflow_hidden()
                        .child(models_table(rows, discover, bar, self.sort, view)),
                )
                .child(catalog_footer(&self.catalog, cx));
        }

        let end = div()
            .flex()
            .items_center()
            .gap(px(ACTIONS_GAP))
            .child(div().w(px(SEARCH_WIDTH)).child(self.search.clone()))
            .child(self.filters.clone());
        let filter_bar = FilterPopover::bar(&self.filters, window, cx);
        div()
            .size_full()
            .flex()
            .flex_col()
            .text_color(theme.color_text_primary)
            .child(page_fill_bar(
                toolbar(div(), end, cx),
                filter_bar,
                tw(4.0),
                content,
                cx,
            ))
    }
}

/// The models `DataTable`: Score at 96px, Model and Skill (θ) at their content's width, and
/// the bands right-aligned, the last taking what is left.
fn models_table(
    rows: Vec<CatalogEntry>,
    discover: bool,
    bar: f64,
    sort: SkillSort,
    view: Entity<IntelligenceView>,
) -> DataTable {
    let rows = Rc::new(rows);
    let columns = COLUMNS
        .iter()
        .map(|column| {
            let header = Column::new(column.title()).sortable();
            match column {
                SkillColumn::Score => header.width(96.0).min_width(88.0),
                SkillColumn::Model => header.fixed().min_width(200.0),
                SkillColumn::Theta => header.fixed().min_width(150.0),
                SkillColumn::Easy | SkillColumn::Normal | SkillColumn::Hard => header.end(),
            }
        })
        .collect();
    let sorted = sort.and_then(|(column, direction)| {
        COLUMNS
            .iter()
            .position(|item| *item == column)
            .map(|index| (index, direction))
    });
    let cells = rows.clone();
    let measured = rows.clone();
    DataTable::new(
        "intelligence-models",
        columns,
        rows.len(),
        move |row, _window, cx| {
            let entry = &cells[row.index];
            vec![
                score_cell(&row_id(entry), entry.theta, cx),
                model_cell(entry, discover, cx),
                theta_cell(entry, cx),
                band_cell(entry, "easy", bar, cx),
                band_cell(entry, "normal", bar, cx),
                band_cell(entry, "hard", bar, cx),
            ]
        },
    )
    .measure(move |range, window, cx| {
        let theme = *Theme::global(cx);
        let end = range.end.min(measured.len());
        let start = range.start.min(end);
        let mut widths = vec![0.0_f32; COLUMNS.len()];
        for entry in &measured[start..end] {
            for (index, column) in COLUMNS.iter().enumerate() {
                let width = content_width(entry, *column, discover, &theme, window);
                widths[index] = widths[index].max(width);
            }
        }
        widths
    })
    .sort(sorted, move |index, _window, cx| {
        view.update(cx, |this, cx| {
            this.sort = next_sort(this.sort, COLUMNS[index]);
            cx.notify();
        });
    })
}

/// A cell's content at its natural width, as the browser measures it at `max-content`.
fn content_width(
    entry: &CatalogEntry,
    column: SkillColumn,
    discover: bool,
    theme: &Theme,
    window: &Window,
) -> f32 {
    let code =
        |text: &str, size: f32| text_width(text, mono(theme, FontWeight::NORMAL), size, window);
    match column {
        // 10px glyphs, 0.14 × 18 apart.
        SkillColumn::Score => {
            let glyphs = skill_digits(entry.theta).chars().count() as f32;
            glyphs * 10.0 + (glyphs - 1.0).max(0.0) * 18.0 * 0.14
        }
        // The avatar, the model, and the tier chip, 10px apart.
        SkillColumn::Model => {
            let title = text_width(&entry.model, mono(theme, FontWeight::MEDIUM), 12.5, window);
            let tier = tier_label(&entry.tier, discover);
            let chip = text_width(&tier, sans(theme, FontWeight::MEDIUM), 11.0, window) + 20.0;
            24.0 + 10.0 + title + 10.0 + chip
        }
        SkillColumn::Theta => {
            let source = theta_source_label(&entry.theta_source, entry.score);
            code(&theta_text(entry.theta), 12.5)
                + 6.5
                + text_width(&source, sans(theme, FontWeight::NORMAL), 12.0, window)
        }
        SkillColumn::Easy | SkillColumn::Normal | SkillColumn::Hard => {
            match band_of(entry, column.band().unwrap_or_default()) {
                Some(pass) => {
                    code(&format!("{} ", percent_text(pass.p_pass)), 12.5)
                        + code(&format!("\u{2265}{}", percent_text(pass.low)), 11.0)
                }
                None => text_width("\u{2014}", sans(theme, FontWeight::NORMAL), 12.5, window),
            }
        }
    }
}

fn fetch_catalog(client: CoreClient) -> Result<String, CoreError> {
    client
        .list_model_catalog(&ListModelCatalogRequest::default())
        .map(|response| response.json)
}

fn error_text(error: &CoreError, fallback: &str) -> String {
    match error {
        CoreError::NotRunning => "The Dispatch core is not running.".to_string(),
        CoreError::Rpc(message) if message.trim().is_empty() => fallback.to_string(),
        CoreError::Rpc(message) => message.clone(),
    }
}
