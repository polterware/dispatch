//! Commander (⌘K): a centered opaque palette over the pages, Settings, and recent runs.

use gpui::{
    App, Context, Entity, FocusHandle, Focusable, IntoElement, KeyBinding, KeyDownEvent,
    MouseButton, Render, ScrollHandle, SharedString, Window, actions, div, point, prelude::*, px,
};

use crate::models::commander::{
    COMMAND_ROW_H, CommandEffect, GROUP_GAP, PLACEHOLDER, PaletteItem, QueryEditor, RecentRun,
    TOP_PAD, effect_for, filter_items, group_spans, highlight_y, step_index, with_recent,
};
use crate::motion::{Animator, Spring, SpringTier, SpringValue};
use crate::state::AppState;
use crate::theme::Theme;
use crate::ui::ToastTone;
use crate::ui::parts::commander::{
    command_label, command_mark, empty_results, group_heading, highlight, search_value,
};
use crate::ui::patterns::{commander_panel, commander_size};

actions!(
    commander,
    [
        ToggleCommander,
        MoveUp,
        MoveDown,
        ConfirmCommand,
        DismissCommander,
    ]
);

/// Bindings installed from `main` (`cmd-k` is bound in `app.rs`). Arrows, Enter, and Escape apply while the
/// palette is focused (`Commander` context). `CommanderView::key_bindings` is
/// the same list, reachable while `views::commander` stays private.
pub fn key_bindings() -> Vec<KeyBinding> {
    vec![
        KeyBinding::new("up", MoveUp, Some("Commander")),
        KeyBinding::new("down", MoveDown, Some("Commander")),
        KeyBinding::new("enter", ConfirmCommand, Some("Commander")),
        KeyBinding::new("escape", DismissCommander, Some("Commander")),
    ]
}

pub struct CommanderView {
    state: Entity<AppState>,
    focus: FocusHandle,
    editor: QueryEditor,
    active: Option<usize>,
    result_ids: Vec<String>,
    reveal: bool,
    tracked_open: bool,
    enter: SpringValue,
    press: SpringValue,
    indicator: Spring,
    animator: Animator,
    scroll: ScrollHandle,
}

impl CommanderView {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let state_for_toggle = state.clone();
        App::on_action(cx, move |_: &ToggleCommander, app| {
            state_for_toggle.update(app, |app_state, cx| {
                app_state.toggle_commander();
                cx.notify();
            });
        });
        let state_for_watch = state.clone();
        cx.observe(&state, move |this, _, cx| {
            let open = state_for_watch.read(cx).commander_open();
            if this.tracked_open && !open {
                this.reset_for_close();
            }
            if open && !this.tracked_open {
                this.arm_open();
            }
            this.tracked_open = open;
            cx.notify();
        })
        .detach();
        Self {
            state,
            focus: cx.focus_handle(),
            editor: QueryEditor::new(),
            active: None,
            result_ids: Vec::new(),
            reveal: false,
            tracked_open: false,
            enter: SpringValue::new(0.0, SpringTier::Slow),
            press: SpringValue::new(0.0, SpringTier::Fast),
            indicator: Spring::selection(0.0),
            animator: Animator::new(),
            scroll: ScrollHandle::new(),
        }
    }

    pub fn key_bindings() -> Vec<KeyBinding> {
        key_bindings()
    }

    /// Closing plays the enter spring back to zero; the query is cleared on the next open.
    fn reset_for_close(&mut self) {
        self.enter.set_target(0.0);
    }

    /// How far the palette is presented, 0 to 1, for the overlay behind it.
    pub fn presence(&self) -> f32 {
        self.enter.value().clamp(0.0, 1.0)
    }

    /// Whether the overlay must stay mounted: open, or still springing out.
    pub fn is_presented(&self) -> bool {
        self.tracked_open || !self.enter.settled() || self.enter.value() > 0.001
    }

    fn arm_open(&mut self) {
        self.editor.clear();
        self.active = None;
        self.result_ids.clear();
        self.reveal = false;
        self.indicator.snap(0.0);
        self.press.snap(0.0);
        self.scroll.set_offset(point(px(0.0), px(0.0)));
        self.enter.snap(0.0);
        self.enter.set_target(1.0);
        self.enter.step(1.0 / 60.0);
    }

    fn moving(&self) -> bool {
        !self.enter.settled() || !self.press.settled() || !self.indicator.is_settled()
    }

    fn sync_results(&mut self, filtered: &[PaletteItem]) {
        let ids: Vec<String> = filtered.iter().map(|item| item.id.clone()).collect();
        let lengths = group_lengths(filtered);
        if ids != self.result_ids {
            self.result_ids = ids;
            self.active = if filtered.is_empty() { None } else { Some(0) };
            let y = self
                .active
                .map(|index| highlight_y(&lengths, index))
                .unwrap_or(0.0);
            self.indicator.snap(y);
            self.scroll.set_offset(point(px(0.0), px(0.0)));
            self.reveal = false;
        } else if let Some(index) = self.active {
            let y = highlight_y(&lengths, index);
            self.indicator.set_target(y);
            if self.reveal {
                reveal_row(&self.scroll, y);
                self.reveal = false;
            }
        }
    }

    fn move_by(&mut self, step: isize, cx: &mut Context<Self>) {
        let next = step_index(self.active, self.result_ids.len(), step);
        if next.is_none() {
            return;
        }
        self.active = next;
        self.reveal = true;
        cx.notify();
    }

    fn confirm_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.active else {
            return;
        };
        self.confirm_index(index, window, cx);
    }

    fn confirm_index(&mut self, index: usize, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.result_ids.get(index).cloned() else {
            return;
        };
        let Some(effect) = effect_for(&id) else {
            return;
        };
        let state = self.state.clone();
        match effect {
            CommandEffect::Reload => {
                state.update(cx, |app, cx| {
                    app.reload_all(cx);
                    app.push_toast(crate::models::commander::RELOAD_TOAST, ToastTone::Success);
                    app.set_commander_open(false);
                    cx.notify();
                });
            }
            CommandEffect::Navigate(route) => {
                state.update(cx, |app, cx| {
                    app.navigate(route);
                    app.request_refresh(cx);
                    app.set_commander_open(false);
                    cx.notify();
                });
            }
        }
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        self.state.update(cx, |app, cx| {
            app.set_commander_open(false);
            cx.notify();
        });
    }

    fn hover_index(&mut self, index: usize, cx: &mut Context<Self>) {
        if self.active != Some(index) {
            self.active = Some(index);
            self.reveal = false;
            cx.notify();
        }
    }

    fn press_index(&mut self, index: usize, cx: &mut Context<Self>) {
        self.active = Some(index);
        self.reveal = false;
        self.press.set_target(1.0);
        cx.notify();
    }

    fn release_press(&mut self, cx: &mut Context<Self>) {
        if self.press.target() != 0.0 || !self.press.settled() {
            self.press.set_target(0.0);
            cx.notify();
        }
    }

    fn on_query_changed(&mut self, cx: &mut Context<Self>) {
        cx.notify();
    }

    fn handle_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if event.is_held {
            return;
        }
        let keystroke = &event.keystroke;
        let modified = keystroke.modifiers.platform
            || keystroke.modifiers.control
            || keystroke.modifiers.alt
            || keystroke.modifiers.function;
        match keystroke.key.as_str() {
            "up" if !keystroke.modifiers.modified() => {
                self.move_by(-1, cx);
                return;
            }
            "down" if !keystroke.modifiers.modified() => {
                self.move_by(1, cx);
                return;
            }
            "enter" if !modified && !keystroke.modifiers.shift => {
                self.confirm_active(window, cx);
                return;
            }
            "escape" if !keystroke.modifiers.modified() => {
                self.close(cx);
                return;
            }
            "backspace" if !modified => {
                self.editor.backspace();
                self.on_query_changed(cx);
                return;
            }
            "delete" if !modified => {
                self.editor.delete_forward();
                self.on_query_changed(cx);
                return;
            }
            "left" if !modified => {
                self.editor.move_left();
                cx.notify();
                return;
            }
            "right" if !modified => {
                self.editor.move_right();
                cx.notify();
                return;
            }
            "home" if !modified => {
                self.editor.move_to_start();
                cx.notify();
                return;
            }
            "end" if !modified => {
                self.editor.move_to_end();
                cx.notify();
                return;
            }
            "a" if keystroke.modifiers.platform
                && !keystroke.modifiers.shift
                && !keystroke.modifiers.alt =>
            {
                self.editor.select_all();
                cx.notify();
                return;
            }
            "v" if keystroke.modifiers.platform
                && !keystroke.modifiers.shift
                && !keystroke.modifiers.alt =>
            {
                if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                    self.editor.insert(&text);
                    self.on_query_changed(cx);
                }
                return;
            }
            _ => {}
        }
        if modified {
            return;
        }
        if let Some(text) = keystroke.key_char.as_deref()
            && !text.is_empty()
            && text.chars().all(|ch| !ch.is_control())
        {
            self.editor.insert(text);
            self.on_query_changed(cx);
        }
    }

    fn results_column(
        &self,
        filtered: &[PaletteItem],
        theme: Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let press = self.press.value().clamp(0.0, 1.0);
        let mut column = div().relative().w_full().pb(px(theme.space_3));
        if self.active.is_some() && !filtered.is_empty() {
            column = column.child(highlight(self.indicator.value(), press, theme));
        }
        column = column.child(div().h(px(TOP_PAD)).w_full());
        if filtered.is_empty() {
            return column.child(empty_results(theme));
        }
        let spans = group_spans(filtered);
        for (group_index, (group, range)) in spans.into_iter().enumerate() {
            if group_index > 0 {
                column = column.child(div().h(px(GROUP_GAP)).w_full());
            }
            column = column.child(group_heading(group.label(), theme));
            for index in range {
                let item = &filtered[index];
                let active = self.active == Some(index);
                let opacity = if active { 1.0 - 0.18 * press } else { 1.0 };
                column = column.child(self.command_row(index, item, active, opacity, theme, cx));
            }
        }
        column
    }

    fn command_row(
        &self,
        index: usize,
        item: &PaletteItem,
        active: bool,
        opacity: f32,
        theme: Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let mark = command_mark(&item.icon, cx);
        let label = command_label(item, active, theme);
        div()
            .id(("commander-row", index))
            .h(px(COMMAND_ROW_H))
            .w_full()
            .px(px(theme.space_2))
            .flex()
            .items_center()
            .gap(px(theme.space_2))
            .cursor_pointer()
            .opacity(opacity)
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered {
                    this.hover_index(index, cx);
                }
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    this.press_index(index, cx);
                }),
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                this.confirm_index(index, window, cx);
            }))
            .child(mark)
            .child(label)
    }
}

impl Focusable for CommanderView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for CommanderView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *Theme::global(cx);
        let filtered = {
            let runs = self.state.read(cx).runs();
            let recent: Vec<RecentRun> = runs
                .iter()
                .map(|run| RecentRun {
                    id: run.id.clone(),
                    prompt: run.prompt.clone(),
                    created_at: run.created_at.clone(),
                    agent: run.agent.clone(),
                    status: run.status.clone(),
                })
                .collect();
            filter_items(&with_recent(&recent), self.editor.text())
        };
        self.sync_results(&filtered);
        let dt = self.animator.dt(window, self.moving());
        if dt > 0.0 {
            self.enter.step(dt);
            self.press.step(dt);
            self.indicator.step(dt);
        }
        if self.moving() {
            let _ = self.animator.dt(window, true);
        }
        if self.tracked_open && !self.focus.is_focused(window) {
            self.focus.focus(window);
        }

        let size = commander_size(window);
        let t = self.enter.value().clamp(0.0, 1.0);
        let state = self.state.clone();
        let before = SharedString::from(self.editor.before_cursor().to_string());
        let after = SharedString::from(self.editor.after_cursor().to_string());
        let full = SharedString::from(self.editor.text().to_string());
        let selected = self.editor.selected();
        let results = self.results_column(&filtered, theme, cx);

        let input = search_value(before, after, full, selected, PLACEHOLDER, theme);
        let close: crate::ui::Handler = std::rc::Rc::new(move |_window, app| {
            state.update(app, |app_state, cx| {
                app_state.set_commander_open(false);
                cx.notify();
            });
        });
        commander_panel(
            "commander-palette",
            size,
            t,
            input,
            results,
            &self.scroll,
            close,
            &theme,
            cx,
        )
        .key_context("Commander")
        .track_focus(&self.focus)
        .on_action(cx.listener(|this, _: &MoveUp, _, cx| this.move_by(-1, cx)))
        .on_action(cx.listener(|this, _: &MoveDown, _, cx| this.move_by(1, cx)))
        .on_action(cx.listener(|this, _: &ConfirmCommand, window, cx| {
            this.confirm_active(window, cx);
        }))
        .on_action(cx.listener(|this, _: &DismissCommander, _, cx| this.close(cx)))
        .on_key_down(cx.listener(Self::handle_key))
        .on_mouse_up(
            MouseButton::Left,
            cx.listener(|this, _, _, cx| this.release_press(cx)),
        )
        .on_mouse_up_out(
            MouseButton::Left,
            cx.listener(|this, _, _, cx| this.release_press(cx)),
        )
    }
}

fn group_lengths(items: &[PaletteItem]) -> Vec<usize> {
    group_spans(items)
        .into_iter()
        .map(|(_, range)| range.len())
        .collect()
}

fn reveal_row(scroll: &ScrollHandle, y: f32) {
    let view_h = f32::from(scroll.bounds().size.height);
    if view_h < 1.0 {
        return;
    }
    let top = -f32::from(scroll.offset().y);
    let bottom = top + view_h;
    let row_bottom = y + COMMAND_ROW_H;
    if y < top {
        scroll.set_offset(point(px(0.0), px(-y)));
    } else if row_bottom > bottom {
        scroll.set_offset(point(px(0.0), px(-(row_bottom - view_h))));
    }
}

#[cfg(test)]
mod tests {
    use super::key_bindings;

    #[test]
    fn key_bindings_cover_the_list() {
        let bindings = key_bindings();
        let names: Vec<&str> = bindings
            .iter()
            .map(|binding| binding.action().name())
            .collect();
        assert!(names.contains(&"commander::MoveUp"));
        assert!(names.contains(&"commander::MoveDown"));
        assert!(names.contains(&"commander::ConfirmCommand"));
        assert!(names.contains(&"commander::DismissCommander"));
        assert_eq!(bindings.len(), 4);
        let via_view: Vec<&str> = super::CommanderView::key_bindings()
            .iter()
            .map(|binding| binding.action().name())
            .collect();
        assert_eq!(via_view, names);
    }
}
