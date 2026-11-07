//! The conversation Usage dialog: what a conversation used in Dispatch, by agent and model,
//! and what the harness that called Dispatch used in its own session. It opens from the
//! sidebar's conversation menu, on the Composer card over a `modal_layer`; Close, Escape, or
//! a press on the scrim closes it. The figures are `GetConversationUsage`'s, the same ones
//! `dispatch usage --conversation` prints. Costs are API-equivalent prices, never a
//! subscription's bill.

use std::rc::Rc;

use gpui::{
    AnyElement, AppContext as _, Context, DispatchPhase, Entity, FontWeight, IntoElement,
    KeyDownEvent, Render, SharedString, Window, canvas, div, prelude::*, px,
};

use crate::avatar::agent_avatar;
use crate::client::{GetConversationUsageRequest, GetConversationUsageResponse};
use crate::models::conversation_usage::{USAGE_NOTE, UsageRow, caller_line, rows, total_row};
use crate::motion::{Animator, SpringTier, SpringValue};
use crate::state::AppState;
use crate::theme::Theme;
use crate::ui::patterns::{composer_card, modal_layer};
use crate::ui::{Icon, mono, rail_section, tooltip as plain_tooltip, wrap_text};

/// The columns every row keeps, in order after the agent's bot.
const RUNS: f32 = 44.0;
const INPUT: f32 = 56.0;
const CACHE: f32 = 56.0;
const OUTPUT: f32 = 56.0;
const COST: f32 = 72.0;
/// The bot's box, which the header skips to line its labels up.
const BOT: f32 = 26.0;
const GAP: f32 = 6.0;

pub struct ConversationUsageView {
    state: Entity<AppState>,
    conversation: String,
    usage: Option<GetConversationUsageResponse>,
    loading: bool,
    error: Option<String>,
    open: bool,
    opacity: SpringValue,
    rise: SpringValue,
    animator: Animator,
}

impl ConversationUsageView {
    pub fn new(state: Entity<AppState>, conversation: String, _cx: &mut Context<Self>) -> Self {
        Self {
            state,
            conversation,
            usage: None,
            loading: false,
            error: None,
            open: false,
            opacity: SpringValue::new(0.0, SpringTier::Fast),
            rise: SpringValue::new(8.0, SpringTier::Fast),
            animator: Animator::new(),
        }
    }

    pub fn conversation(&self) -> &str {
        &self.conversation
    }

    /// Opens the dialog and reads the conversation's figures.
    pub fn open(&mut self, cx: &mut Context<Self>) {
        self.open = true;
        self.fetch(cx);
        cx.notify();
    }

    pub fn close(&mut self, cx: &mut Context<Self>) {
        if self.open {
            self.open = false;
            cx.notify();
        }
    }

    pub fn is_open(&self) -> bool {
        self.open || self.opacity.value() > 0.01
    }

    fn fetch(&mut self, cx: &mut Context<Self>) {
        let Some(client) = self.state.read(cx).client() else {
            self.error = Some("Dispatch core is not running".to_string());
            self.loading = false;
            cx.notify();
            return;
        };
        self.loading = true;
        self.error = None;
        let request = GetConversationUsageRequest {
            conversation_id: self.conversation.clone(),
        };
        cx.spawn(async move |this, async_cx| {
            let result = async_cx
                .background_spawn(async move { client.conversation_usage(&request) })
                .await;
            let _ = this.update(async_cx, |this, cx| {
                this.loading = false;
                match result {
                    Ok(usage) => this.usage = Some(usage),
                    Err(error) => this.error = Some(crate::views::core_message(error)),
                }
                cx.notify();
            });
        })
        .detach();
    }
}

impl Render for ConversationUsageView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *Theme::global(cx);
        self.opacity.set_target(if self.open { 1.0 } else { 0.0 });
        self.rise.set_target(if self.open { 0.0 } else { 8.0 });
        let moving = !self.opacity.settled() || !self.rise.settled();
        let dt = self.animator.dt(window, moving);
        if dt > 0.0 {
            self.opacity.step(dt);
            self.rise.step(dt);
        }
        let t = self.opacity.value();
        if !self.open && t < 0.01 {
            return div().size_0();
        }
        let body = self.body(&theme, cx);
        let closer = cx.entity();
        let card = composer_card("Conversation usage", t, self.rise.value(), &theme)
            .heading()
            .on_close(Rc::new(move |_, cx| {
                closer.update(cx, |this, cx| this.close(cx));
            }))
            .child(body);
        let dismiss = cx.entity();
        let layer = modal_layer(
            t,
            self.open,
            Rc::new(move |_, cx| {
                dismiss.update(cx, |this, cx| this.close(cx));
            }),
            card,
            &theme,
        );
        // Escape closes the dialog wherever the focus is.
        let escape = self.open.then(|| {
            let view = cx.entity();
            canvas(
                |_, _, _| {},
                move |_, _, window, _| {
                    let view = view.clone();
                    window.on_key_event(move |event: &KeyDownEvent, phase, _, cx| {
                        if phase == DispatchPhase::Bubble && event.keystroke.key == "escape" {
                            view.update(cx, |this, cx| this.close(cx));
                        }
                    });
                },
            )
            .absolute()
            .size_0()
        });
        div().size_0().children(escape).child(layer)
    }
}

impl ConversationUsageView {
    /// The dialog's body: run usage, the calling agent's session, and the grand total.
    fn body(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let mut body = div()
            .flex()
            .flex_col()
            .gap(px(16.0))
            .px(px(8.0))
            .debug_selector(|| "conversation-usage".to_string());
        if self.loading && self.usage.is_none() {
            return body
                .child(note("Reading usage\u{2026}", theme))
                .into_any_element();
        }
        if let Some(error) = &self.error {
            return body.child(note(error.clone(), theme)).into_any_element();
        }
        let Some(usage) = &self.usage else {
            return body.into_any_element();
        };

        let runs = rows(&usage.runs);
        let mut run_section = rail_section("Dispatch runs", Icon::CreditCard, cx).child(table(
            &runs,
            total_row(usage.run_totals.as_ref()),
            theme,
            "runs",
        ));
        if runs.is_empty() {
            run_section = run_section.child(note("No runs started in this conversation.", theme));
        }
        body = body.child(run_section);

        let mut caller = rail_section("Calling agent", Icon::Bot, cx).child(
            div()
                .px(px(8.0))
                .text_size(px(12.0))
                .text_color(theme.color_text_muted)
                .child(caller_line(&usage.caller_agent, &usage.caller_session)),
        );
        if usage.caller_measured {
            caller = caller.child(table(
                &rows(&usage.caller),
                total_row(usage.caller_totals.as_ref()),
                theme,
                "caller",
            ));
        } else {
            let line = if usage.caller_note.trim().is_empty() {
                "Not measured for this agent.".to_string()
            } else {
                usage.caller_note.clone()
            };
            caller = caller.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .px(px(8.0))
                    .py(px(6.0))
                    .child(agent_avatar(&usage.caller_agent, "", 20.0))
                    .child(note(line, theme).px(px(0.0))),
            );
        }
        body = body.child(caller);

        let total = total_row(usage.total.as_ref());
        let mut sum = div()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .px(px(8.0))
            .pt(px(12.0))
            .border_t_1()
            .border_color(theme.color_border_subtle);
        if let Some(total) = &total {
            sum = sum.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(GAP))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .text_size(px(13.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Conversation total"),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(12.0))
                            .text_color(theme.color_text_secondary)
                            .child(format!(
                                "{} tokens",
                                crate::models::spend::format_tokens(total.tokens)
                            )),
                    )
                    .child(cell(total.cost.clone(), COST, theme, true)),
            );
        }
        sum = sum.child(
            div()
                .text_size(px(11.0))
                .line_height(px(16.0))
                .text_color(theme.color_text_muted)
                .when(
                    usage.total.as_ref().is_some_and(|total| total.cost_partial),
                    |note| {
                        note.child(wrap_text(
                            "Some tokens had no price, so the total is a floor.",
                        ))
                    },
                )
                .child(wrap_text(USAGE_NOTE)),
        );
        body.child(sum).into_any_element()
    }
}

/// One section's table: the columns' labels, a row per agent and model, and its totals.
fn table(rows: &[UsageRow], total: Option<UsageRow>, theme: &Theme, key: &str) -> AnyElement {
    let mut table = div().flex().flex_col().gap(px(2.0)).px(px(8.0));
    let mut header = div().flex().items_center().gap(px(GAP)).min_h(px(24.0));
    header = header.child(div().w(px(BOT)).flex_none()).child(
        div()
            .flex_1()
            .min_w(px(0.0))
            .text_size(px(11.0))
            .text_color(theme.color_text_muted)
            .child("Agent and model"),
    );
    for (label, width) in [
        ("Runs", RUNS),
        ("Input", INPUT),
        ("Cache", CACHE),
        ("Output", OUTPUT),
        ("Cost", COST),
    ] {
        header = header.child(cell(label, width, theme, false));
    }
    table = table.child(header);
    for row in rows {
        table = table.child(row_line(row, theme, false, key));
    }
    if let Some(total) = total {
        table = table
            .child(
                div()
                    .pt(px(4.0))
                    .border_t_1()
                    .border_color(theme.color_border_subtle),
            )
            .child(row_line(&total, theme, true, key));
    }
    table.into_any_element()
}

fn row_line(row: &UsageRow, theme: &Theme, strong: bool, key: &str) -> gpui::Div {
    let total = row.agent.trim().is_empty();
    let mut line = div().flex().items_center().gap(px(GAP)).min_h(px(30.0));
    line = line
        .child(
            div()
                .w(px(BOT))
                .flex_none()
                .flex()
                .items_center()
                .when(!total, |slot| {
                    slot.child(agent_avatar(&row.agent, "", 20.0))
                }),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .child(
                    div()
                        .text_size(px(13.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme.color_text_primary)
                        .child(SharedString::from(if total {
                            "Total".to_string()
                        } else {
                            row.agent_label()
                        })),
                )
                .when(!total, |cell| {
                    cell.child(
                        div()
                            .text_size(px(11.0))
                            .text_color(theme.color_text_muted)
                            .child(SharedString::from(row.model.clone())),
                    )
                }),
        )
        .child(cell(row.runs_label(), RUNS, theme, strong))
        .child(cell(row.input.clone(), INPUT, theme, strong))
        .child(cell(row.cache.clone(), CACHE, theme, strong))
        .child(cell(row.output.clone(), OUTPUT, theme, strong))
        .child(cost_cell(row, strong, theme, key));
    line
}

/// The cost cell, whose tooltip says where the price came from and whether it is a floor.
fn cost_cell(row: &UsageRow, strong: bool, theme: &Theme, key: &str) -> AnyElement {
    cell(row.cost.clone(), COST, theme, strong)
        .id(SharedString::from(format!(
            "conversation-usage-cost-{key}-{}",
            row.agent
        )))
        .tooltip(plain_tooltip(row.cost_title.clone()))
        .into_any_element()
}

fn cell(text: impl Into<SharedString>, width: f32, theme: &Theme, strong: bool) -> gpui::Div {
    div()
        .w(px(width))
        .flex_none()
        .text_right()
        .whitespace_nowrap()
        .text_size(px(12.0))
        .font(mono(theme, FontWeight::NORMAL))
        .when(strong, |cell| cell.font_weight(FontWeight::SEMIBOLD))
        .child(text.into())
}

fn note(text: impl Into<SharedString>, theme: &Theme) -> gpui::Div {
    div()
        .px(px(8.0))
        .py(px(4.0))
        .text_size(px(12.0))
        .line_height(px(18.0))
        .text_color(theme.color_text_muted)
        .child(wrap_text(text.into()))
}
