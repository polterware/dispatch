//! Root view: sidebar, the routed page, the Commander overlay, and toasts.

use std::rc::Rc;

use gpui::{
    AnyElement, AnyView, App, Context, CursorStyle, FocusHandle, IntoElement, KeyBinding,
    MouseButton, Render, StyleRefinement, Window, actions, div, prelude::*, px,
};

use crate::state::{AppState, Route, observe};
use crate::theme::Theme;
use crate::ui::patterns::{main_surface, modal_layer};
use crate::ui::toast_stack;
use crate::views::{
    CommanderView, IntelligenceView, RolesView, RunPageView, RunsView, SettingsView, SidebarView,
    SpendView, StatsView,
};

actions!(
    dispatch_app,
    [Quit, CloseWindow, GoBack, ToggleCommander, CheckForUpdates]
);

pub fn register_key_bindings(cx: &mut App) {
    cx.on_action(|_: &Quit, cx| cx.quit());
    cx.on_action(|_: &CheckForUpdates, _| crate::updater::check());
    cx.bind_keys([
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("cmd-w", CloseWindow, None),
        KeyBinding::new("cmd-[", GoBack, None),
        KeyBinding::new("cmd-k", ToggleCommander, None),
    ]);
}

pub struct AppView {
    focus_handle: FocusHandle,
    state: gpui::Entity<AppState>,
    sidebar: gpui::Entity<SidebarView>,
    roles: gpui::Entity<RolesView>,
    intelligence: gpui::Entity<IntelligenceView>,
    spend: gpui::Entity<SpendView>,
    stats: gpui::Entity<StatsView>,
    settings: gpui::Entity<SettingsView>,
    run_page: gpui::Entity<RunPageView>,
    runs: gpui::Entity<RunsView>,
    commander: gpui::Entity<CommanderView>,
    /// While the sidebar's edge is dragged: the pointer's x and the width at the press.
    resizing: Option<(f32, f32)>,
}

/// `ResizableSidebar`'s handle: 12px wide, centered on the sidebar's edge.
const RESIZE_HANDLE_WIDTH: f32 = 12.0;

impl AppView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        let state = AppState::open(cx);
        observe(&state, cx);
        let commander = cx.new(|cx| CommanderView::new(state.clone(), cx));
        // The overlay stays mounted while the palette springs out.
        cx.observe(&commander, |_, _, cx| cx.notify()).detach();
        let view = Self {
            focus_handle: focus_handle.clone(),
            sidebar: cx.new(|cx| SidebarView::new(state.clone(), cx)),
            roles: cx.new(|cx| RolesView::new(state.clone(), cx)),
            intelligence: cx.new(|cx| IntelligenceView::new(state.clone(), cx)),
            spend: cx.new(|cx| SpendView::new(state.clone(), cx)),
            stats: cx.new(|cx| StatsView::new(state.clone(), cx)),
            settings: cx.new(|cx| SettingsView::new(state.clone(), cx)),
            run_page: cx.new(|cx| RunPageView::new(state.clone(), cx)),
            runs: cx.new(|cx| RunsView::new(state.clone(), cx)),
            commander,
            state,
            resizing: None,
        };
        let width = view.state.read(cx).sidebar_width();
        cx.update_global::<Theme, _>(|theme, _| theme.sidebar_width = width);
        focus_handle.focus(window);
        if let Some(dir) = crate::shots::shots_dir() {
            let filters = vec![
                (
                    "03-intelligence",
                    view.intelligence.read(cx).filters().clone(),
                ),
                ("04-spend", view.spend.read(cx).filters().clone()),
                ("12-runs", view.runs.read(cx).filters().clone()),
            ];
            crate::shots::start_tour(dir, view.state.clone(), filters, window, cx);
        }
        view
    }
}

impl AppView {
    /// Follows the pointer while the sidebar's edge is dragged, as `ResizableSidebar` does:
    /// the width moves with it between the minimum and the maximum, and is kept on release.
    fn drag_sidebar(&mut self, x: f32, persist: bool, cx: &mut Context<Self>) {
        let Some((origin, start)) = self.resizing else {
            return;
        };
        // A press that ends where it began (a click on the handle) leaves the width alone.
        let moved = (x - origin).abs() >= 1.0;
        let target = if moved { start + (x - origin) } else { start };
        let width = self.state.update(cx, |state, _| {
            state.set_sidebar_width(target, persist && moved);
            state.sidebar_width()
        });
        // The pages and the sidebar's panels read the width from the theme.
        cx.update_global::<Theme, _>(|theme, _| theme.sidebar_width = width);
        if persist {
            self.resizing = None;
        }
        cx.notify();
    }
}

impl Render for AppView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *Theme::global(cx);
        let route = self.state.read(cx).route().clone();
        let commander_open = self.state.read(cx).commander_open();
        let commander_shown = commander_open || self.commander.read(cx).is_presented();
        let toasts = self.state.read(cx).toasts().to_vec();
        let page = page_for_route(
            &route,
            &self.roles,
            &self.intelligence,
            &self.spend,
            &self.stats,
            &self.settings,
            &self.run_page,
            &self.runs,
        );
        let state = self.state.clone();
        let dismiss = Rc::new(move |id, _window: &mut Window, app: &mut App| {
            state.update(app, |state, cx| {
                state.dismiss_toast(id);
                cx.notify();
            });
        });

        let sidebar_width = theme.sidebar_width;
        let resizing = self.resizing.is_some();
        let surface = main_surface(page, &theme);

        div()
            .id("dispatch-root")
            .track_focus(&self.focus_handle)
            .relative()
            .size_full()
            .flex()
            .flex_row()
            .bg(theme.color_shell_glass)
            .text_color(theme.color_text_primary)
            .font_family(theme.font_sans)
            .text_size(px(theme.font_size_md))
            .when(resizing, |root| root.cursor(CursorStyle::ResizeLeftRight))
            .on_mouse_move(cx.listener(|this, event: &gpui::MouseMoveEvent, _, cx| {
                if this.resizing.is_none() {
                    return;
                }
                let x = f32::from(event.position.x);
                let released = event.pressed_button != Some(MouseButton::Left);
                this.drag_sidebar(x, released, cx);
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, event: &gpui::MouseUpEvent, _, cx| {
                    this.drag_sidebar(f32::from(event.position.x), true, cx);
                }),
            )
            // `FilterPopover`'s F key: the page's filter menu, while nothing else has focus.
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                let keystroke = &event.keystroke;
                if keystroke.key != "f"
                    || keystroke.modifiers.modified()
                    || !this.focus_handle.is_focused(window)
                    || this.state.read(cx).commander_open()
                {
                    return;
                }
                let filters = match this.state.read(cx).route() {
                    Route::Runs => this.runs.read(cx).filters().clone(),
                    Route::Intelligence => this.intelligence.read(cx).filters().clone(),
                    Route::Spend => this.spend.read(cx).filters().clone(),
                    _ => return,
                };
                if filters.update(cx, |filters, cx| filters.open_from_shortcut(cx)) {
                    cx.stop_propagation();
                }
            }))
            .on_action(cx.listener(|_this, _: &CloseWindow, window, _cx| {
                window.remove_window();
            }))
            .on_action(cx.listener(|this, _: &GoBack, _window, cx| {
                this.state.update(cx, |state, cx| {
                    if state.go_back() {
                        state.request_refresh(cx);
                    }
                    cx.notify();
                });
            }))
            .on_action(cx.listener(|this, _: &ToggleCommander, _window, cx| {
                this.state.update(cx, |state, cx| {
                    state.toggle_commander();
                    cx.notify();
                });
            }))
            // Cached views: a bot animating on one side re-renders that side
            // only, not the other one.
            .child(
                AnyView::from(self.sidebar.clone()).cached(
                    StyleRefinement::default()
                        .w(px(sidebar_width))
                        .h_full()
                        .flex_none(),
                ),
            )
            // `.dispatch-main.pw-shell-main-frame`: padding 8px 8px 8px 0.
            .child(
                div()
                    .flex_1()
                    .h_full()
                    .min_w(px(0.0))
                    .pt(px(theme.space_2))
                    .pr(px(theme.space_2))
                    .pb(px(theme.space_2))
                    .child(surface),
            )
            .child(
                div()
                    .id("sidebar-resize")
                    .absolute()
                    .top(px(0.0))
                    .bottom(px(0.0))
                    .left(px(sidebar_width - RESIZE_HANDLE_WIDTH / 2.0))
                    .w(px(RESIZE_HANDLE_WIDTH))
                    .cursor(CursorStyle::ResizeLeftRight)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &gpui::MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            let width = this.state.read(cx).sidebar_width();
                            this.resizing = Some((f32::from(event.position.x), width));
                            cx.notify();
                        }),
                    ),
            )
            .when(commander_shown, |this| {
                let state = self.state.clone();
                this.child(modal_layer(
                    self.commander.read(cx).presence(),
                    true,
                    Rc::new(move |_window, app| {
                        state.update(app, |state, cx| {
                            state.set_commander_open(false);
                            cx.notify();
                        });
                    }),
                    self.commander.clone(),
                    &theme,
                ))
            })
            .child(toast_stack(&toasts, dismiss, cx))
    }
}

fn page_for_route(
    route: &Route,
    roles: &gpui::Entity<RolesView>,
    intelligence: &gpui::Entity<IntelligenceView>,
    spend: &gpui::Entity<SpendView>,
    stats: &gpui::Entity<StatsView>,
    settings: &gpui::Entity<SettingsView>,
    run_page: &gpui::Entity<RunPageView>,
    runs: &gpui::Entity<RunsView>,
) -> AnyElement {
    let view: AnyView = match route {
        Route::Roles | Route::RoleBoard(_) => roles.clone().into(),
        Route::Intelligence => intelligence.clone().into(),
        Route::Spend => spend.clone().into(),
        Route::Stats => stats.clone().into(),
        Route::Settings(_) => settings.clone().into(),
        Route::Run(_) => run_page.clone().into(),
        Route::Runs => runs.clone().into(),
    };
    view.cached(StyleRefinement::default().size_full())
        .into_any_element()
}
