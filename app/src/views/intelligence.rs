//! Intelligence: how able Auto takes every model it can route to be, from the catalog alone
//! (`ListModelCatalog`). A `DataTable` of Score, Model, and P(pass) at each band, one row per
//! model@effort however many agents offer it, sorted by Score, high to low, until a header
//! sorts it. The Score's tooltip says how it was read (θ, its source, the lower bound's
//! margin, the catalog's date and sources), the model's lists the agents here that offer it.
//! A click on a row opens the model's dialog (`composer_card` with its heading, on a
//! `modal_layer`, as Roles opens its Composer), with all of that and its pass chance at each
//! band; Close, Escape, or a press on the scrim closes it. The toolbar holds the search and
//! the shared `FilterPopover`, whose pills sit in the `ActiveFilterBar` under it.

use std::rc::Rc;

use gpui::{
    AppContext as _, Context, DispatchPhase, Entity, FontWeight, IntoElement, KeyDownEvent, Render,
    SharedString, Window, canvas, div, prelude::*, px,
};

use crate::client::{CoreClient, CoreError, ListModelCatalogRequest};
use crate::models::intelligence::{
    COLUMNS, Catalog, DEFAULT_SORT, ModelRow, SkillColumn, SkillSort, band_of, bar_note,
    empty_catalog, empty_title, group_models, map_dispatch_catalog, model_filter_fields,
    percent_text, policy_bar, score_detail, skill_digits, visible_rows,
};
use crate::motion::{Animator, SpringTier, SpringValue};
use crate::state::{AppState, ConnectionState, ToastTone};
use crate::theme::{Theme, tw};
use crate::ui::parts::intelligence::{band_cell, model_cell, model_details, score_cell};
use crate::ui::patterns::{composer_card, modal_layer};
use crate::ui::{
    Column, DataTable, FilterField, FilterPopover, FiltersChanged, Icon, MascotMood, TextInput,
    mono, next_sort, page_empty, page_under, page_under_inset, sans, text_width, toolbar,
};
use crate::ui::{SkeletonLayout, page_skeleton};

/// `SearchBar`'s `w-[200px]`, beside the filters' icon button, `gap-2` apart.
const SEARCH_WIDTH: f32 = 200.0;
const ACTIONS_GAP: f32 = 8.0;
/// The page's chrome state.
const PAGE: &str = "intelligence-page";

pub struct IntelligenceView {
    state: Entity<AppState>,
    search: Entity<TextInput>,
    filters: Entity<FilterPopover>,
    /// The option ids the filter fields were last built from.
    fields_key: String,
    catalog: Catalog,
    /// The catalog's entries, one per model@effort.
    rows: Vec<ModelRow>,
    loaded: bool,
    /// The last paint had no core client. A later client starts a real load.
    awaiting_core: bool,
    loading: bool,
    load_generation: u64,
    sort: SkillSort,
    empty_enter: SpringValue,
    /// The model whose dialog is open, kept while it fades out after it closes.
    detail: Option<ModelRow>,
    detail_open: bool,
    detail_opacity: SpringValue,
    detail_y: SpringValue,
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
            rows: Vec::new(),
            loaded: false,
            awaiting_core: false,
            loading: false,
            load_generation: 0,
            sort: DEFAULT_SORT,
            empty_enter: SpringValue::new(0.0, SpringTier::Moderate),
            detail: None,
            detail_open: false,
            detail_opacity: SpringValue::new(0.0, SpringTier::Fast),
            detail_y: SpringValue::new(8.0, SpringTier::Fast),
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
                    Ok(json) => {
                        view.catalog = map_dispatch_catalog(&json);
                        view.rows = group_models(&view.catalog.entries);
                    }
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

    fn open_detail(&mut self, row: ModelRow, cx: &mut Context<Self>) {
        self.detail = Some(row);
        self.detail_open = true;
        cx.notify();
    }

    /// The first model's dialog, in the table's order (the screenshot tour).
    pub(crate) fn open_first_detail(&mut self, cx: &mut Context<Self>) {
        let bar = policy_bar(&self.catalog);
        let filters = self.filters.read(cx).filters().clone();
        let query = self.search.read(cx).text().to_string();
        let first = visible_rows(
            &self.rows,
            &query,
            &filters,
            bar,
            self.catalog.discover,
            self.sort,
        )
        .into_iter()
        .next();
        if let Some(row) = first {
            self.open_detail(row, cx);
        }
    }

    pub(crate) fn close_detail(&mut self, cx: &mut Context<Self>) {
        if self.detail_open {
            self.detail_open = false;
            cx.notify();
        }
    }

    /// The model's dialog, while it is open or fading out.
    fn detail_layer(&self, theme: &Theme, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let row = self.detail.as_ref()?;
        let t = self.detail_opacity.value();
        let detail = score_detail(row, &self.catalog);
        let body = model_details(
            row,
            &detail,
            self.catalog.discover,
            policy_bar(&self.catalog),
            &bar_note(&self.catalog),
            cx,
        );
        let closer = cx.entity();
        let card = composer_card(row.name.clone(), t, self.detail_y.value(), theme)
            .heading()
            .on_close(Rc::new(move |_, cx| {
                closer.update(cx, |this, cx| this.close_detail(cx));
            }))
            .child(body);
        let dismiss = cx.entity();
        Some(modal_layer(
            t,
            self.detail_open,
            Rc::new(move |_, cx| {
                dismiss.update(cx, |this, cx| this.close_detail(cx));
            }),
            card,
            theme,
        ))
    }

    /// The fields follow the catalog's values, rebuilt only when those change.
    fn sync_fields(&mut self, cx: &mut Context<Self>) {
        let specs = model_filter_fields(&self.rows, self.catalog.discover);
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
        let rows = {
            let filters = self.filters.read(cx).filters();
            visible_rows(&self.rows, &query, filters, bar, discover, self.sort)
        };
        let filtered = !query.trim().is_empty() || !self.filters.read(cx).filters().is_empty();
        let has_entries = !self.rows.is_empty();
        self.empty_enter
            .set_target(if rows.is_empty() { 1.0 } else { 0.0 });
        self.detail_opacity
            .set_target(if self.detail_open { 1.0 } else { 0.0 });
        self.detail_y
            .set_target(if self.detail_open { 0.0 } else { 8.0 });
        let moving = !self.empty_enter.settled()
            || !self.detail_opacity.settled()
            || !self.detail_y.settled();
        let dt = self.animator.dt(window, moving);
        if dt > 0.0 {
            self.empty_enter.step(dt);
            self.detail_opacity.step(dt);
            self.detail_y.step(dt);
        }
        if !self.detail_open && self.detail_opacity.settled() && self.detail_opacity.value() < 0.02
        {
            self.detail = None;
        }

        let view = cx.entity();
        let filter_bar = FilterPopover::bar(&self.filters, window, cx);
        // The toolbar's inset, so the table's edges line up with its buttons.
        let padding = theme.space_2;
        let top = page_under_inset(PAGE, filter_bar.is_some(), padding, window, cx);
        let mut content = div()
            .flex_1()
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .gap(px(tw(3.0)));
        if !self.loaded || rows.is_empty() {
            content = content.pt(px(top));
        }
        if !self.loaded {
            // The models table, in bones, until the first answer comes back.
            content = content.child(page_skeleton(SkeletonLayout::Table).without_toolbar());
        } else if rows.is_empty() {
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
            content = content.child(
                div()
                    .flex_1()
                    .min_h(px(0.0))
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .child(
                        models_table(rows, &self.catalog, bar, self.sort, view)
                            .inset_top(top)
                            .fades(theme.color_surface),
                    ),
            );
        }

        let end = div()
            .flex()
            .items_center()
            .gap(px(ACTIONS_GAP))
            .child(div().w(px(SEARCH_WIDTH)).child(self.search.clone()))
            .child(self.filters.clone());
        let layer = self.detail_layer(&theme, cx);
        // Escape closes the dialog wherever the focus is.
        let escape = self.detail_open.then(|| {
            let view = cx.entity();
            canvas(
                |_, _, _| {},
                move |_, _, window, _| {
                    let view = view.clone();
                    window.on_key_event(move |event: &KeyDownEvent, phase, _, cx| {
                        if phase == DispatchPhase::Bubble && event.keystroke.key == "escape" {
                            view.update(cx, |this, cx| this.close_detail(cx));
                        }
                    });
                },
            )
            .absolute()
            .size_0()
        });
        div()
            .size_full()
            .flex()
            .flex_col()
            .text_color(theme.color_text_primary)
            // The table runs up under the toolbar, which fades it out on its trail.
            .child(page_under(
                PAGE,
                toolbar(div(), end, cx),
                filter_bar,
                padding,
                content,
                window,
                cx,
            ))
            .children(escape)
            .children(layer)
    }
}

/// The models `DataTable`: Score at 96px, Model at its content's width, and the bands
/// right-aligned, the last taking what is left.
fn models_table(
    rows: Vec<ModelRow>,
    catalog: &Catalog,
    bar: f64,
    sort: SkillSort,
    view: Entity<IntelligenceView>,
) -> DataTable {
    let discover = catalog.discover;
    let details: Vec<_> = rows.iter().map(|row| score_detail(row, catalog)).collect();
    let details = Rc::new(details);
    let rows = Rc::new(rows);
    let columns = COLUMNS
        .iter()
        .map(|column| {
            let header = Column::new(column.title()).sortable();
            match column {
                SkillColumn::Score => header.width(96.0).min_width(88.0),
                SkillColumn::Model => header.fixed().min_width(200.0).shrinks(),
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
    let opened = rows.clone();
    let opener = view.clone();
    DataTable::new(
        "intelligence-models",
        columns,
        rows.len(),
        move |row, _window, cx| {
            let model = &cells[row.index];
            vec![
                score_cell(model, details[row.index].clone(), cx),
                model_cell(model, discover, cx),
                band_cell(model, "easy", bar, cx),
                band_cell(model, "normal", bar, cx),
                band_cell(model, "hard", bar, cx),
            ]
        },
    )
    .measure(move |range, window, cx| {
        let theme = *Theme::global(cx);
        let end = range.end.min(measured.len());
        let start = range.start.min(end);
        let mut widths = vec![0.0_f32; COLUMNS.len()];
        for model in &measured[start..end] {
            for (index, column) in COLUMNS.iter().enumerate() {
                let width = content_width(model, *column, &theme, window);
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
    .on_activate(move |index, _window, cx| {
        if let Some(row) = opened.get(index).cloned() {
            opener.update(cx, |this, cx| this.open_detail(row, cx));
        }
    })
}

/// A cell's content at its natural width, as the browser measures it at `max-content`.
fn content_width(model: &ModelRow, column: SkillColumn, theme: &Theme, window: &Window) -> f32 {
    let entry = &model.entry;
    let code =
        |text: &str, size: f32| text_width(text, mono(theme, FontWeight::NORMAL), size, window);
    match column {
        // 10px glyphs, 0.14 × 18 apart.
        SkillColumn::Score => {
            let glyphs = skill_digits(entry.theta).chars().count() as f32;
            glyphs * 10.0 + (glyphs - 1.0).max(0.0) * 18.0 * 0.14
        }
        // The avatar and the model, 10px apart. Layout rounds text up to a whole pixel, so
        // the measure does too, or the name would fade.
        SkillColumn::Model => {
            let title = text_width(&model.name, mono(theme, FontWeight::MEDIUM), 12.5, window);
            24.0 + 10.0 + title.ceil()
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
