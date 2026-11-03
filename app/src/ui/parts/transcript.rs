//! Transcript rows, terminal Markdown, and syntax color for the run view.
//!
//! Highlighting is one line at a time with a fresh syntect parse, so a 50k-line
//! log only colors the rows on screen. Scope colors use theme tokens: the app
//! has no chart palette, and accent and info are the same blue.

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::{HashSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, OnceLock};

use gpui::{
    App, Font, FontFallbacks, FontFeatures, FontStyle, FontWeight, HighlightStyle, Hsla,
    IntoElement, SharedString, StyledText, Window, div, font, prelude::*, px,
};
use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use syntect::easy::ScopeRegionIterator;
use syntect::parsing::{ParseState, ScopeStack, SyntaxSet};

use crate::models::transcript::{TranscriptEntry, looks_like_diff, looks_like_json};
use crate::theme::{StatusTone, Theme, status_tone};

pub const OUTPUT_LINES: usize = 6;
pub const PREVIEW_ENTRIES: usize = 60;
const TERM_FONT_PX: f32 = 12.5;
const TERM_LINE_PX: f32 = 20.0;
const TERM_GAP_PX: f32 = 10.0;
const HIGHLIGHT_CAP: usize = 16_384;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenRole {
    Keyword,
    String,
    Number,
    Title,
    /// `hljs-attr`, `-property`, `-type`, `-name`: a key, an attribute, a tag.
    Attribute,
    Deletion,
    Comment,
    Punctuation,
    Addition,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HlSpan {
    pub start: usize,
    pub end: usize,
    pub role: TokenRole,
}

#[derive(Clone, Debug, PartialEq)]
pub struct InlineSpan {
    pub text: String,
    pub code: bool,
    pub strong: bool,
    pub emphasis: bool,
    pub strike: bool,
    pub link: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum RowBody {
    Gap,
    Muted(String),
    Live,
    Step {
        entry: usize,
        verb: String,
        target: Option<String>,
        shell: bool,
        detail: Option<String>,
        failed: bool,
    },
    Output {
        entry: usize,
        text: String,
        gutter: bool,
        elbow: bool,
        language: String,
        fade: bool,
    },
    More {
        entry: usize,
        hidden: usize,
        gutter: bool,
    },
    Thinking(String),
    Heading {
        level: u8,
        spans: Vec<InlineSpan>,
    },
    Item {
        indent: u32,
        marker: String,
        spans: Vec<InlineSpan>,
    },
    Quote {
        spans: Vec<InlineSpan>,
    },
    Rule,
    Blank,
    Code {
        language: String,
        text: String,
        first: bool,
        last: bool,
    },
    Text {
        spans: Vec<InlineSpan>,
    },
    Event {
        glyph: char,
        kind: String,
        text: String,
        tone: StatusTone,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct TranscriptRow {
    pub signature: u64,
    pub body: RowBody,
}

pub struct HighlightCache {
    map: std::collections::HashMap<u64, Vec<HlSpan>>,
    order: VecDeque<u64>,
}

impl HighlightCache {
    pub fn new() -> Self {
        Self {
            map: std::collections::HashMap::new(),
            order: VecDeque::new(),
        }
    }

    pub fn spans(&mut self, line: &str, language: &str) -> Vec<HlSpan> {
        if line.is_empty() || language.is_empty() || line.len() > HIGHLIGHT_CAP {
            return Vec::new();
        }
        let key = cache_key(language, line);
        if let Some(hit) = self.map.get(&key) {
            return hit.clone();
        }
        let spans = highlight_line(line, language);
        self.order.push_back(key);
        self.map.insert(key, spans.clone());
        while self.order.len() > 512 {
            if let Some(old) = self.order.pop_front() {
                self.map.remove(&old);
            }
        }
        spans
    }
}

impl Default for HighlightCache {
    fn default() -> Self {
        Self::new()
    }
}

pub struct RowFrame {
    pub theme: Theme,
    pub index: usize,
    pub count: usize,
    pub enter: f32,
    pub reveal: f32,
    pub cursor: f32,
    pub cache: Rc<RefCell<HighlightCache>>,
    pub on_expand: Rc<dyn Fn(usize, &mut Window, &mut App)>,
}

/// JetBrains Mono when the machine or the asset source has it, else Menlo.
///
/// `IconAssets` only serves SVG icons, so `fonts/JetBrainsMono-Regular.ttf`
/// misses unless a later asset source learns that path. System font files are
/// tried next; main already loads the same paths at startup.
pub fn terminal_font(cx: &App) -> Font {
    if family_present(cx, "JetBrains Mono") {
        return mono_font("JetBrains Mono");
    }
    if let Ok(Some(bytes)) = cx.asset_source().load("fonts/JetBrainsMono-Regular.ttf") {
        let _ = cx.text_system().add_fonts(vec![bytes]);
        if family_present(cx, "JetBrains Mono") {
            return mono_font("JetBrains Mono");
        }
    }
    for path in jetbrains_paths() {
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let _ = cx.text_system().add_fonts(vec![Cow::Owned(bytes)]);
        if family_present(cx, "JetBrains Mono") {
            return mono_font("JetBrains Mono");
        }
    }
    let mut font = mono_font("Menlo");
    font.fallbacks = Some(FontFallbacks::from_fonts(vec![
        "Menlo".to_string(),
        "monospace".to_string(),
    ]));
    font
}

pub fn flatten_transcript(
    entries: &[TranscriptEntry],
    expanded: &HashSet<usize>,
    reveal_entry: Option<usize>,
) -> Vec<TranscriptRow> {
    let mut rows = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        if index > 0 {
            push_row(&mut rows, RowBody::Gap);
        }
        match entry {
            TranscriptEntry::Step {
                text,
                detail,
                failed,
                output,
            } => push_step(
                &mut rows,
                index,
                text,
                detail.as_deref(),
                *failed,
                output.as_deref(),
                expanded,
                reveal_entry,
            ),
            TranscriptEntry::Thinking { text } => {
                push_row(&mut rows, RowBody::Thinking(strip_bold(text)));
            }
            TranscriptEntry::Message { text } => push_markdown(&mut rows, text),
            TranscriptEntry::Output { text } => {
                push_output_lines(&mut rows, index, text, false, false, None);
            }
            TranscriptEntry::Event { event, text } => {
                let tone = status_tone(event);
                push_row(
                    &mut rows,
                    RowBody::Event {
                        glyph: tone_glyph(tone),
                        kind: event.clone(),
                        text: text.clone(),
                        tone,
                    },
                );
            }
        }
    }
    rows
}

pub fn render_row(
    frame: &RowFrame,
    row: &TranscriptRow,
    _window: &mut Window,
    cx: &mut App,
) -> gpui::AnyElement {
    let theme = frame.theme;
    let last = frame.index + 1 == frame.count;
    let fade = row_fade(&row.body);
    let mut opacity = if fade {
        frame.reveal.clamp(0.0, 1.0)
    } else {
        1.0
    };
    if last {
        opacity *= frame.enter.clamp(0.0, 1.0);
    }
    let child = match &row.body {
        RowBody::Gap => div().h(px(TERM_GAP_PX)).w_full().into_any_element(),
        RowBody::Blank => div().h(px(TERM_GAP_PX)).w_full().into_any_element(),
        RowBody::Muted(text) => div()
            .w_full()
            .min_h(px(TERM_LINE_PX))
            .text_color(theme.color_text_muted)
            .child(text.clone())
            .into_any_element(),
        RowBody::Live => live_row(theme, frame.cursor).into_any_element(),
        RowBody::Step {
            entry,
            verb,
            target,
            shell,
            detail,
            failed,
        } => step_row(
            frame,
            *entry,
            verb,
            target.as_deref(),
            *shell,
            detail.as_deref(),
            *failed,
        ),
        RowBody::Output {
            text,
            gutter,
            elbow,
            language,
            ..
        } => output_row(frame, text, *gutter, *elbow, language, theme).into_any_element(),
        RowBody::More {
            entry,
            hidden,
            gutter,
        } => more_row(frame, *entry, *hidden, *gutter, theme),
        RowBody::Thinking(text) => div()
            .w_full()
            .min_h(px(TERM_LINE_PX))
            .italic()
            .text_color(theme.color_text_muted)
            .whitespace_normal()
            .child(text.clone())
            .into_any_element(),
        RowBody::Heading { level, spans } => {
            inline_row(theme, spans, heading_color(theme, *level), true, false).into_any_element()
        }
        RowBody::Item {
            indent,
            marker,
            spans,
        } => item_row(theme, *indent, marker, spans).into_any_element(),
        RowBody::Quote { spans } => quote_row(theme, spans).into_any_element(),
        RowBody::Rule => div()
            .w_full()
            .h(px(TERM_LINE_PX))
            .text_color(theme.color_text_disabled)
            .child("────────")
            .into_any_element(),
        RowBody::Code {
            language,
            text,
            first,
            last,
        } => code_row(frame, theme, language, text, *first, *last).into_any_element(),
        RowBody::Text { spans } => {
            inline_row(theme, spans, theme.color_text_primary, false, false).into_any_element()
        }
        RowBody::Event {
            glyph,
            kind,
            text,
            tone,
        } => event_row(theme, *glyph, kind, text, *tone).into_any_element(),
    };
    let _ = cx;
    let mut wrap = div().w_full();
    if opacity < 0.999 {
        wrap = wrap.opacity(opacity);
    }
    wrap.child(child).into_any_element()
}

pub fn highlight_line(line: &str, language: &str) -> Vec<HlSpan> {
    if line.is_empty() || language.is_empty() || line.len() > HIGHLIGHT_CAP {
        return Vec::new();
    }
    let set = syntaxes();
    let Some(syntax) = find_syntax(set, language) else {
        return Vec::new();
    };
    let mut state = ParseState::new(syntax);
    let ops = state.parse_line(line, set).unwrap_or_default();
    let mut stack = ScopeStack::new();
    let mut spans = Vec::new();
    let mut byte = 0;
    for (text, op) in ScopeRegionIterator::new(&ops, line) {
        let _ = stack.apply(op);
        if text.is_empty() {
            continue;
        }
        let start = byte;
        byte += text.len();
        let Some(role) = role_of(&stack.to_string()) else {
            continue;
        };
        if let Some(HlSpan {
            end, role: prev, ..
        }) = spans.last_mut()
            && *prev == role
            && *end == start
        {
            *end = byte;
            continue;
        }
        spans.push(HlSpan {
            start,
            end: byte,
            role,
        });
    }
    spans
}

fn push_step(
    rows: &mut Vec<TranscriptRow>,
    entry: usize,
    text: &str,
    detail: Option<&str>,
    failed: Option<bool>,
    output: Option<&str>,
    expanded: &HashSet<usize>,
    reveal_entry: Option<usize>,
) {
    let (verb, target, shell) = split_step(text);
    push_row(
        rows,
        RowBody::Step {
            entry,
            verb,
            target,
            shell,
            detail: detail.map(str::to_string),
            failed: failed == Some(true),
        },
    );
    if let Some(output) = output {
        let open = expanded.contains(&entry);
        let reveal = open && reveal_entry == Some(entry);
        push_output_lines(rows, entry, output, true, open, reveal.then_some(entry));
    }
}

fn push_output_lines(
    rows: &mut Vec<TranscriptRow>,
    entry: usize,
    text: &str,
    elbow: bool,
    open: bool,
    reveal: Option<usize>,
) {
    let trimmed = text.trim_end();
    if trimmed.trim().is_empty() {
        return;
    }
    let lines: Vec<&str> = trimmed.split('\n').collect();
    let language = output_language(trimmed);
    let hidden = if elbow && !open {
        lines.len().saturating_sub(OUTPUT_LINES)
    } else {
        0
    };
    let shown = lines.len() - hidden;
    for (index, line) in lines.iter().take(shown).enumerate() {
        push_row(
            rows,
            RowBody::Output {
                entry,
                text: (*line).to_string(),
                gutter: elbow,
                elbow: elbow && index == 0,
                language: language.clone(),
                fade: reveal.is_some() && index >= OUTPUT_LINES,
            },
        );
    }
    if hidden > 0 {
        push_row(
            rows,
            RowBody::More {
                entry,
                hidden,
                gutter: elbow,
            },
        );
    }
}

fn push_markdown(rows: &mut Vec<TranscriptRow>, markdown: &str) {
    let mut builder = MdBuilder::default();
    let options = Options::ENABLE_STRIKETHROUGH;
    for event in Parser::new_ext(markdown, options) {
        builder.event(event);
    }
    builder.finish(rows);
}

fn push_row(rows: &mut Vec<TranscriptRow>, body: RowBody) {
    let signature = body_signature(&body);
    rows.push(TranscriptRow { signature, body });
}

fn split_step(text: &str) -> (String, Option<String>, bool) {
    const VERBS: [&str; 9] = [
        "Searching the web for",
        "Running",
        "Reading",
        "Writing",
        "Editing",
        "Edited",
        "Searching",
        "Fetching",
        "Called",
    ];
    for verb in VERBS {
        if let Some(rest) = text.strip_prefix(verb)
            && let Some(target) = rest.strip_prefix(' ')
            && !target.is_empty()
        {
            return (
                verb.to_string(),
                Some(target.to_string()),
                verb == "Running",
            );
        }
    }
    (text.to_string(), None, false)
}

fn output_language(text: &str) -> String {
    if looks_like_diff(text) {
        "diff".to_string()
    } else if looks_like_json(text) {
        "json".to_string()
    } else {
        String::new()
    }
}

fn strip_bold(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("**") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        if let Some(end) = after.find("**")
            && end > 0
        {
            out.push_str(&after[..end]);
            rest = &after[end + 2..];
            continue;
        }
        out.push_str("**");
        rest = after;
    }
    out.push_str(rest);
    out
}

fn tone_glyph(tone: StatusTone) -> char {
    match tone {
        StatusTone::Ok | StatusTone::Healthy => '✓',
        StatusTone::Danger | StatusTone::Down => '✗',
        _ => '•',
    }
}

fn row_fade(body: &RowBody) -> bool {
    matches!(body, RowBody::Output { fade: true, .. })
}

fn heading_color(theme: Theme, level: u8) -> gpui::Rgba {
    if level <= 2 {
        theme.chart_cyan
    } else {
        theme.color_text_primary
    }
}

fn event_color(theme: Theme, tone: StatusTone) -> gpui::Rgba {
    crate::theme::tone_color(&theme, tone)
}

fn step_row(
    frame: &RowFrame,
    entry: usize,
    verb: &str,
    target: Option<&str>,
    shell: bool,
    detail: Option<&str>,
    failed: bool,
) -> gpui::AnyElement {
    let theme = frame.theme;
    let bullet = if failed {
        theme.color_danger
    } else {
        theme.chart_cyan
    };
    let verb_color = if failed {
        theme.color_danger
    } else {
        theme.color_text_primary
    };
    let detail_color = if failed {
        theme.color_danger
    } else {
        theme.color_text_muted
    };
    let target_node = target.map(|target| {
        if shell {
            highlighted_text(frame, target, "bash", theme.chart_cyan)
        } else {
            div()
                .text_color(theme.chart_cyan)
                .child(target.to_string())
                .into_any_element()
        }
    });
    div()
        .id(("term-step", entry))
        .w_full()
        .min_h(px(TERM_LINE_PX))
        .flex()
        .gap(px(TERM_CH_PX))
        .child(div().flex_none().text_color(bullet).child("›"))
        .child({
            let mut line = div()
                .min_w(px(0.0))
                .flex_1()
                .flex()
                .flex_wrap()
                .whitespace_normal()
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(verb_color)
                        .child(verb.to_string()),
                );
            if target_node.is_some() {
                line = line.child(div().text_color(theme.color_text_primary).child(" "));
            }
            if let Some(target_node) = target_node {
                line = line.child(target_node);
            }
            if let Some(detail) = detail {
                line = line.child(div().text_color(detail_color).child(format!(" {detail}")));
            }
            line
        })
        .into_any_element()
}

fn output_row(
    frame: &RowFrame,
    text: &str,
    gutter: bool,
    elbow: bool,
    language: &str,
    theme: Theme,
) -> gpui::Div {
    let body = highlighted_text(frame, text, language, theme.color_text_muted);
    let mut row = div()
        .w_full()
        .min_h(px(TERM_LINE_PX))
        .flex()
        .gap(px(TERM_CH_PX))
        .text_color(theme.color_text_muted);
    if gutter {
        row = row.pl(px(2.0 * TERM_CH_PX)).child(
            div()
                .w(px(TERM_CH_PX))
                .flex_none()
                .text_color(theme.color_text_disabled)
                .child(if elbow { "└" } else { "" }),
        );
    }
    row.child(div().min_w(px(0.0)).flex_1().child(body))
}

fn more_row(
    frame: &RowFrame,
    entry: usize,
    hidden: usize,
    gutter: bool,
    theme: Theme,
) -> gpui::AnyElement {
    let on_expand = frame.on_expand.clone();
    let label = format!("… +{hidden} lines");
    let mut row = div().w_full().min_h(px(TERM_LINE_PX)).flex();
    if gutter {
        row = row
            .pl(px(2.0 * TERM_CH_PX))
            .gap(px(TERM_CH_PX))
            .child(div().w(px(TERM_CH_PX)).flex_none());
    }
    row.child(
        div()
            .id(("term-more", entry))
            .cursor_pointer()
            .text_color(theme.color_text_disabled)
            .hover(move |style| style.text_color(theme.color_text_secondary))
            .on_click(move |_, window, cx| on_expand(entry, window, cx))
            .child(label),
    )
    .into_any_element()
}

fn live_row(theme: Theme, cursor: f32) -> gpui::Div {
    div()
        .w_full()
        .min_h(px(TERM_LINE_PX))
        .flex()
        .items_center()
        .gap(px(TERM_CH_PX))
        .child(
            div()
                .w(px(7.5))
                .h(px(16.0))
                .flex_none()
                .bg(theme.color_text_primary)
                .opacity(cursor.clamp(0.0, 1.0)),
        )
        .child(
            div()
                .text_color(theme.color_text_muted)
                .child("The agent is still writing."),
        )
}

fn code_row(
    frame: &RowFrame,
    theme: Theme,
    language: &str,
    text: &str,
    first: bool,
    last: bool,
) -> gpui::Div {
    let body = highlighted_text(frame, text, language, theme.color_text_secondary);
    let mut row = div()
        .w_full()
        .min_h(px(TERM_LINE_PX))
        .border_l_1()
        .border_color(theme.color_border_strong)
        .pl(px(2.0 * TERM_CH_PX))
        .text_color(theme.color_text_secondary)
        .child(body);
    if first {
        row = row.mt(px(6.0)).pt(px(2.0));
    }
    if last {
        row = row.mb(px(6.0)).pb(px(2.0));
    }
    row
}

fn item_row(theme: Theme, indent: u32, marker: &str, spans: &[InlineSpan]) -> gpui::Div {
    div()
        .w_full()
        .min_h(px(TERM_LINE_PX))
        .flex()
        .gap(px(TERM_CH_PX))
        .pl(px(indent as f32 * 2.0 * TERM_CH_PX))
        .text_color(theme.color_text_primary)
        .child(
            div()
                .flex_none()
                .text_color(theme.color_text_muted)
                .child(marker.to_string()),
        )
        .child(inline_row(theme, spans, theme.color_text_primary, false, false).flex_1())
}

fn quote_row(theme: Theme, spans: &[InlineSpan]) -> gpui::Div {
    div()
        .w_full()
        .min_h(px(TERM_LINE_PX))
        .flex()
        .italic()
        .text_color(theme.color_text_secondary)
        .child(
            div()
                .flex_none()
                .text_color(theme.color_text_disabled)
                .child("│ "),
        )
        .child(inline_row(theme, spans, theme.color_text_secondary, false, true).flex_1())
}

fn event_row(theme: Theme, glyph: char, kind: &str, text: &str, tone: StatusTone) -> gpui::Div {
    let color = event_color(theme, tone);
    div()
        .w_full()
        .min_h(px(TERM_LINE_PX))
        .flex()
        .gap(px(TERM_CH_PX))
        .whitespace_normal()
        .text_color(theme.color_text_muted)
        .child(div().flex_none().text_color(color).child(glyph.to_string()))
        .child(
            div()
                .flex_none()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(color)
                .child(kind.to_string()),
        )
        .child(text.to_string())
}

/// One `ch` of the terminal face (JetBrains Mono advances 0.6em).
const TERM_CH_PX: f32 = TERM_FONT_PX * 0.6;

/// A line of inline Markdown as one run of text, so code, bold, and links flow with the words
/// around them (`Inline` in `run-terminal.tsx`): code in cyan, bold in the primary color at
/// 700, a link in blue and underlined with its target muted after it.
fn inline_row(
    theme: Theme,
    spans: &[InlineSpan],
    color: gpui::Rgba,
    strong_heading: bool,
    quote: bool,
) -> gpui::Div {
    let mut row = div()
        .min_w(px(0.0))
        .min_h(px(TERM_LINE_PX))
        .whitespace_normal()
        .text_color(color);
    if strong_heading {
        row = row.font_weight(FontWeight::BOLD);
    }
    if spans.is_empty() {
        return row.child(" ");
    }
    let underline = gpui::UnderlineStyle {
        thickness: px(1.0),
        color: Some(Hsla::from(theme.color_border_strong)),
        wavy: false,
    };
    let mut text = String::new();
    let mut highlights: Vec<(std::ops::Range<usize>, HighlightStyle)> = Vec::new();
    for span in spans {
        let start = text.len();
        text.push_str(&span.text);
        let mut style = HighlightStyle::default();
        if span.strong {
            style.font_weight = Some(FontWeight::BOLD);
            style.color = Some(Hsla::from(theme.color_text_primary));
        }
        if span.code {
            style.color = Some(Hsla::from(theme.chart_cyan));
        } else if span.link.is_some() {
            style.color = Some(Hsla::from(theme.chart_blue));
            style.underline = Some(underline);
        }
        if span.emphasis {
            style.font_style = Some(FontStyle::Italic);
        } else if quote {
            style.font_style = Some(FontStyle::Italic);
        }
        if span.strike {
            style.strikethrough = Some(gpui::StrikethroughStyle {
                thickness: px(1.0),
                color: None,
            });
        }
        if style != HighlightStyle::default() && start < text.len() {
            highlights.push((start..text.len(), style));
        }
        if let Some(href) = &span.link {
            let start = text.len();
            text.push_str(&format!(" ({href})"));
            highlights.push((
                start..text.len(),
                HighlightStyle {
                    color: Some(Hsla::from(theme.color_text_muted)),
                    underline: Some(underline),
                    ..HighlightStyle::default()
                },
            ));
        }
    }
    row.child(StyledText::new(SharedString::from(text)).with_highlights(highlights))
}

fn highlighted_text(
    frame: &RowFrame,
    text: &str,
    language: &str,
    color: gpui::Rgba,
) -> gpui::AnyElement {
    if text.is_empty() {
        return div().min_h(px(TERM_LINE_PX)).child(" ").into_any_element();
    }
    let spans = frame.cache.borrow_mut().spans(text, language);
    if spans.is_empty() {
        return div()
            .whitespace_normal()
            .text_color(color)
            .child(text.to_string())
            .into_any_element();
    }
    let highlights = spans.into_iter().filter_map(|span| {
        if !text.is_char_boundary(span.start)
            || !text.is_char_boundary(span.end)
            || span.start >= span.end
        {
            return None;
        }
        Some((
            span.start..span.end,
            role_highlight(&frame.theme, span.role),
        ))
    });
    StyledText::new(SharedString::from(text.to_string()))
        .with_highlights(highlights)
        .into_any_element()
}

/// highlight.js tokens on the app's opaque chart palette, as `run-terminal.css` sets them.
fn role_highlight(theme: &Theme, role: TokenRole) -> HighlightStyle {
    let (color, italic) = match role {
        TokenRole::Keyword => (theme.chart_violet, false),
        TokenRole::String | TokenRole::Addition => (theme.chart_green, false),
        TokenRole::Number => (theme.chart_amber, false),
        TokenRole::Title => (theme.chart_blue, false),
        TokenRole::Attribute => (theme.chart_cyan, false),
        TokenRole::Deletion => (theme.chart_red, false),
        TokenRole::Comment => (theme.color_text_disabled, true),
        TokenRole::Punctuation => (theme.color_text_secondary, false),
    };
    HighlightStyle {
        color: Some(Hsla::from(color)),
        font_style: italic.then_some(FontStyle::Italic),
        ..HighlightStyle::default()
    }
}

fn role_of(stack: &str) -> Option<TokenRole> {
    // A key is `hljs-attr` even though its scope is also a string.
    if stack.contains("property-name")
        || stack.contains("mapping.key")
        || stack.contains("dictionary.key")
    {
        return Some(TokenRole::Attribute);
    }
    for scope in stack.split_whitespace().rev() {
        if scope.contains("comment") {
            return Some(TokenRole::Comment);
        }
        if scope.contains("markup.deleted") || scope.contains("deleted.diff") {
            return Some(TokenRole::Deletion);
        }
        if scope.contains("markup.inserted") || scope.contains("inserted.diff") {
            return Some(TokenRole::Addition);
        }
        if scope.contains("string") || scope.contains("regexp") {
            return Some(TokenRole::String);
        }
        if scope.contains("constant.numeric") {
            return Some(TokenRole::Number);
        }
        if scope.contains("variable") {
            return Some(TokenRole::Number);
        }
        if scope.contains("keyword.operator") || scope.contains("punctuation") {
            return Some(TokenRole::Punctuation);
        }
        if scope.contains("keyword")
            || scope.contains("storage")
            || scope.contains("constant.language")
            || scope.contains("support.function.builtin")
        {
            return Some(TokenRole::Keyword);
        }
        if scope.contains("entity.other.attribute")
            || scope.contains("entity.name.tag")
            || scope.contains("support.type")
        {
            return Some(TokenRole::Attribute);
        }
        if scope.contains("entity.name")
            || scope.contains("support.function")
            || scope.contains("support.class")
        {
            return Some(TokenRole::Title);
        }
    }
    None
}

fn syntaxes() -> &'static SyntaxSet {
    static SET: OnceLock<SyntaxSet> = OnceLock::new();
    SET.get_or_init(SyntaxSet::load_defaults_newlines)
}

fn find_syntax<'a>(
    set: &'a SyntaxSet,
    language: &str,
) -> Option<&'a syntect::parsing::SyntaxReference> {
    let canonical = canonical_language(language);
    set.find_syntax_by_token(&canonical)
        .or_else(|| set.find_syntax_by_token(language))
        .or_else(|| set.find_syntax_by_extension(&canonical))
}

fn canonical_language(language: &str) -> String {
    let lower = language.trim().to_ascii_lowercase();
    match lower.as_str() {
        "sh" | "shell" | "zsh" | "console" | "bash" => "bash",
        "ts" | "tsx" | "typescript" => "typescript",
        "js" | "jsx" | "mjs" | "javascript" => "javascript",
        "html" | "svg" | "xml" => "xml",
        "yml" | "yaml" => "yaml",
        "toml" | "ini" => "ini",
        "patch" | "diff" => "diff",
        "md" | "markdown" => "markdown",
        "py" | "python" => "python",
        "rs" | "rust" => "rust",
        "json" => "json",
        "go" => "go",
        "css" => "css",
        "sql" => "sql",
        other => other,
    }
    .to_string()
}

pub fn resolve_fence_language(language: &str, code: &str, guess: bool) -> String {
    if !language.is_empty() && find_syntax(syntaxes(), language).is_some() {
        return canonical_language(language);
    }
    if looks_like_diff(code) {
        return "diff".to_string();
    }
    if looks_like_json(code) {
        return "json".to_string();
    }
    if guess && code.len() < 20_000 {
        let first = code.lines().next().unwrap_or("");
        if let Some(syntax) = syntaxes().find_syntax_by_first_line(first) {
            return syntax
                .file_extensions
                .first()
                .cloned()
                .unwrap_or_else(|| syntax.name.to_ascii_lowercase());
        }
    }
    String::new()
}

fn cache_key(language: &str, line: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    language.hash(&mut hasher);
    line.hash(&mut hasher);
    hasher.finish()
}

fn body_signature(body: &RowBody) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    match body {
        RowBody::Gap => 1u8.hash(&mut hasher),
        RowBody::Blank => 2u8.hash(&mut hasher),
        RowBody::Muted(text) => {
            3u8.hash(&mut hasher);
            text.hash(&mut hasher);
        }
        RowBody::Live => 4u8.hash(&mut hasher),
        RowBody::Step {
            entry,
            verb,
            target,
            shell,
            detail,
            failed,
        } => {
            5u8.hash(&mut hasher);
            entry.hash(&mut hasher);
            verb.hash(&mut hasher);
            target.hash(&mut hasher);
            shell.hash(&mut hasher);
            detail.hash(&mut hasher);
            failed.hash(&mut hasher);
        }
        RowBody::Output {
            entry,
            text,
            gutter,
            elbow,
            language,
            fade,
        } => {
            6u8.hash(&mut hasher);
            entry.hash(&mut hasher);
            text.hash(&mut hasher);
            gutter.hash(&mut hasher);
            elbow.hash(&mut hasher);
            language.hash(&mut hasher);
            fade.hash(&mut hasher);
        }
        RowBody::More {
            entry,
            hidden,
            gutter,
        } => {
            7u8.hash(&mut hasher);
            entry.hash(&mut hasher);
            hidden.hash(&mut hasher);
            gutter.hash(&mut hasher);
        }
        RowBody::Thinking(text) => {
            8u8.hash(&mut hasher);
            text.hash(&mut hasher);
        }
        RowBody::Heading { level, spans } => {
            9u8.hash(&mut hasher);
            level.hash(&mut hasher);
            hash_spans(&mut hasher, spans);
        }
        RowBody::Item {
            indent,
            marker,
            spans,
        } => {
            10u8.hash(&mut hasher);
            indent.hash(&mut hasher);
            marker.hash(&mut hasher);
            hash_spans(&mut hasher, spans);
        }
        RowBody::Quote { spans } => {
            11u8.hash(&mut hasher);
            hash_spans(&mut hasher, spans);
        }
        RowBody::Rule => 12u8.hash(&mut hasher),
        RowBody::Code {
            language,
            text,
            first,
            last,
        } => {
            13u8.hash(&mut hasher);
            language.hash(&mut hasher);
            text.hash(&mut hasher);
            first.hash(&mut hasher);
            last.hash(&mut hasher);
        }
        RowBody::Text { spans } => {
            14u8.hash(&mut hasher);
            hash_spans(&mut hasher, spans);
        }
        RowBody::Event {
            glyph,
            kind,
            text,
            tone,
        } => {
            15u8.hash(&mut hasher);
            glyph.hash(&mut hasher);
            kind.hash(&mut hasher);
            text.hash(&mut hasher);
            tone_tag(*tone).hash(&mut hasher);
        }
    }
    hasher.finish()
}

fn hash_spans(hasher: &mut impl Hasher, spans: &[InlineSpan]) {
    for span in spans {
        span.text.hash(hasher);
        span.code.hash(hasher);
        span.strong.hash(hasher);
        span.emphasis.hash(hasher);
        span.link.hash(hasher);
    }
}

fn tone_tag(tone: StatusTone) -> u8 {
    match tone {
        StatusTone::Healthy => 0,
        StatusTone::Degraded => 1,
        StatusTone::Down => 2,
        StatusTone::Unknown => 3,
        StatusTone::Syncing => 4,
        StatusTone::Ok => 5,
        StatusTone::Warn => 6,
        StatusTone::Danger => 7,
        StatusTone::Muted => 8,
    }
}

fn mix(from: gpui::Rgba, to: gpui::Rgba, t: f32) -> gpui::Rgba {
    let t = t.clamp(0.0, 1.0);
    gpui::Rgba {
        r: from.r + (to.r - from.r) * t,
        g: from.g + (to.g - from.g) * t,
        b: from.b + (to.b - from.b) * t,
        a: from.a + (to.a - from.a) * t,
    }
}

fn mono_font(family: &str) -> Font {
    let mut face = font(family.to_string());
    face.features = FontFeatures(Arc::new(vec![
        ("calt".to_string(), 0),
        ("liga".to_string(), 0),
    ]));
    face
}

fn family_present(cx: &App, family: &str) -> bool {
    cx.text_system()
        .all_font_names()
        .iter()
        .any(|name| name.eq_ignore_ascii_case(family))
}

fn jetbrains_paths() -> Vec<PathBuf> {
    let mut paths = vec![
        PathBuf::from("/Library/Fonts/JetBrainsMono-Regular.ttf"),
        PathBuf::from("/Library/Fonts/JetBrainsMono-Regular.otf"),
        PathBuf::from("/Library/Fonts/JetBrains Mono/JetBrainsMono-Regular.ttf"),
    ];
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        for name in [
            "Library/Fonts/JetBrainsMono-Regular.ttf",
            "Library/Fonts/JetBrainsMono-Regular.otf",
            "Library/Fonts/JetBrains Mono/JetBrainsMono-Regular.ttf",
        ] {
            paths.push(home.join(name));
        }
    }
    paths
}

#[derive(Clone, Copy, Default)]
struct Marks {
    strong: bool,
    emphasis: bool,
    strike: bool,
}

#[derive(Clone)]
struct ListFrame {
    ordered: Option<u64>,
    #[allow(dead_code)]
    indent: u32,
}

#[derive(Default)]
struct MdBuilder {
    rows: Vec<RowBody>,
    spans: Vec<InlineSpan>,
    marks: Vec<Marks>,
    link: Vec<Option<String>>,
    lists: Vec<ListFrame>,
    item_marker: Vec<String>,
    heading: Option<u8>,
    quote: usize,
    code: Option<(String, String)>,
    need_blank: bool,
}

impl MdBuilder {
    fn event(&mut self, event: Event<'_>) {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(text) => self.text(&text),
            Event::Code(text) => self.push_span(text.to_string(), true),
            Event::SoftBreak | Event::HardBreak => self.break_line(),
            Event::Rule => {
                self.break_block();
                self.rows.push(RowBody::Rule);
                self.need_blank = true;
            }
            Event::TaskListMarker(checked) => {
                self.push_span(
                    if checked {
                        "[x] ".to_string()
                    } else {
                        "[ ] ".to_string()
                    },
                    false,
                );
            }
            Event::Html(text) | Event::InlineHtml(text) => self.text(&text),
            Event::InlineMath(text) | Event::DisplayMath(text) | Event::FootnoteReference(text) => {
                self.text(&text);
            }
        }
    }

    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph => self.break_block(),
            Tag::Heading { level, .. } => {
                self.break_block();
                self.heading = Some(level as u8);
            }
            Tag::BlockQuote(_) => {
                self.break_block();
                self.quote += 1;
            }
            Tag::CodeBlock(kind) => {
                self.break_block();
                let language = match kind {
                    CodeBlockKind::Fenced(lang) => lang.trim().to_string(),
                    CodeBlockKind::Indented => String::new(),
                };
                self.code = Some((language, String::new()));
            }
            Tag::List(start) => self.lists.push(ListFrame {
                ordered: start,
                indent: self.lists.len() as u32,
            }),
            Tag::Item => {
                self.break_block();
                let marker = if let Some(frame) = self.lists.last_mut() {
                    if let Some(number) = frame.ordered.as_mut() {
                        let marker = format!("{number}.");
                        *number += 1;
                        marker
                    } else {
                        "•".to_string()
                    }
                } else {
                    "•".to_string()
                };
                self.item_marker.push(marker);
            }
            Tag::Strong => self.marks.push(Marks {
                strong: true,
                ..self.mark()
            }),
            Tag::Emphasis => self.marks.push(Marks {
                emphasis: true,
                ..self.mark()
            }),
            Tag::Strikethrough => self.marks.push(Marks {
                strike: true,
                ..self.mark()
            }),
            Tag::Link { dest_url, .. } => self.link.push(Some(dest_url.to_string())),
            Tag::Image { dest_url, .. } => self.link.push(Some(dest_url.to_string())),
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph | TagEnd::Heading(_) => {
                self.flush_line();
                self.heading = None;
                self.need_blank = true;
            }
            TagEnd::BlockQuote(_) => {
                self.flush_line();
                self.quote = self.quote.saturating_sub(1);
                self.need_blank = true;
            }
            TagEnd::CodeBlock => self.finish_code(),
            TagEnd::List(_) => {
                self.lists.pop();
            }
            TagEnd::Item => {
                self.flush_line();
                self.item_marker.pop();
            }
            TagEnd::Strong | TagEnd::Emphasis | TagEnd::Strikethrough => {
                self.marks.pop();
            }
            TagEnd::Link | TagEnd::Image => {
                self.link.pop();
            }
            _ => {}
        }
    }

    fn text(&mut self, text: &str) {
        if self.code.is_some() {
            if let Some((_, buf)) = self.code.as_mut() {
                buf.push_str(text);
            }
            return;
        }
        let mut lines = text.split('\n');
        if let Some(first) = lines.next() {
            if !first.is_empty() {
                self.push_span(first.to_string(), false);
            }
        }
        for line in lines {
            self.break_line();
            if !line.is_empty() {
                self.push_span(line.to_string(), false);
            }
        }
    }

    fn push_span(&mut self, text: String, code: bool) {
        if text.is_empty() {
            return;
        }
        let mark = self.mark();
        let link = self.link.last().cloned().unwrap_or(None);
        if let Some(last) = self.spans.last_mut()
            && !code
            && !last.code
            && last.strong == mark.strong
            && last.emphasis == mark.emphasis
            && last.strike == mark.strike
            && last.link == link
        {
            last.text.push_str(&text);
            return;
        }
        self.spans.push(InlineSpan {
            text,
            code,
            strong: mark.strong,
            emphasis: mark.emphasis,
            strike: mark.strike,
            link,
        });
    }

    fn mark(&self) -> Marks {
        self.marks.last().copied().unwrap_or_default()
    }

    fn break_block(&mut self) {
        self.flush_line();
        if self.need_blank
            && !self.rows.is_empty()
            && !matches!(self.rows.last(), Some(RowBody::Blank))
        {
            self.rows.push(RowBody::Blank);
        }
        self.need_blank = false;
    }

    fn break_line(&mut self) {
        self.flush_line();
    }

    fn flush_line(&mut self) {
        if self.spans.is_empty() {
            return;
        }
        let spans = std::mem::take(&mut self.spans);
        let row = if let Some(level) = self.heading {
            RowBody::Heading { level, spans }
        } else if let Some(marker) = self.item_marker.last() {
            RowBody::Item {
                indent: self.lists.len().saturating_sub(1) as u32,
                marker: marker.clone(),
                spans,
            }
        } else if self.quote > 0 {
            RowBody::Quote { spans }
        } else {
            RowBody::Text { spans }
        };
        self.rows.push(row);
    }

    fn finish_code(&mut self) {
        let Some((language, code)) = self.code.take() else {
            return;
        };
        let language = resolve_fence_language(&language, &code, true);
        let mut lines: Vec<&str> = code.split('\n').collect();
        if code.ends_with('\n') && lines.last() == Some(&"") {
            lines.pop();
        }
        if lines.is_empty() {
            self.need_blank = true;
            return;
        }
        let last = lines.len() - 1;
        for (index, line) in lines.into_iter().enumerate() {
            self.rows.push(RowBody::Code {
                language: language.clone(),
                text: line.to_string(),
                first: index == 0,
                last: index == last,
            });
        }
        self.need_blank = true;
    }

    fn finish(&mut self, out: &mut Vec<TranscriptRow>) {
        self.flush_line();
        while matches!(self.rows.last(), Some(RowBody::Blank)) {
            self.rows.pop();
        }
        for body in self.rows.drain(..) {
            push_row(out, body);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{OUTPUT_LINES, TokenRole, flatten_transcript, highlight_line};
    use crate::models::transcript::TranscriptEntry;
    use std::collections::HashSet;

    fn step_output(lines: usize) -> TranscriptEntry {
        let text = (1..=lines)
            .map(|index| format!("line {index}"))
            .collect::<Vec<_>>()
            .join("\n");
        TranscriptEntry::Step {
            text: "Running go test ./...".into(),
            detail: None,
            failed: Some(false),
            output: Some(text),
        }
    }

    #[test]
    fn folds_step_output_after_six_lines_and_opens_the_rest() {
        let rows = flatten_transcript(&[step_output(9)], &HashSet::new(), None);
        let shown = rows
            .iter()
            .filter(|row| matches!(row.body, super::RowBody::Output { .. }))
            .count();
        assert_eq!(shown, OUTPUT_LINES);
        let hidden = rows.iter().find_map(|row| match &row.body {
            super::RowBody::More { hidden, .. } => Some(*hidden),
            _ => None,
        });
        assert_eq!(hidden, Some(3));
        let mut open = HashSet::new();
        open.insert(0);
        let rows = flatten_transcript(&[step_output(9)], &open, Some(0));
        assert_eq!(
            rows.iter()
                .filter(|row| matches!(row.body, super::RowBody::Output { .. }))
                .count(),
            9
        );
        assert!(
            rows.iter()
                .any(|row| matches!(row.body, super::RowBody::Output { fade: true, .. }))
        );
        assert!(
            rows.iter()
                .all(|row| !matches!(row.body, super::RowBody::More { .. }))
        );
    }

    #[test]
    fn reads_a_reply_as_terminal_markdown() {
        let rows = flatten_transcript(
            &[TranscriptEntry::Message {
                text: "## Plan\n\n- one\n  1. two\n```go\nfunc a() {}\n```".into(),
            }],
            &HashSet::new(),
            None,
        );
        assert!(rows.iter().any(|row| matches!(
            &row.body,
            super::RowBody::Heading { level: 2, spans } if spans.iter().any(|span| span.text == "Plan")
        )));
        assert!(rows.iter().any(|row| matches!(
            &row.body,
            super::RowBody::Item { marker, spans, .. } if marker == "•" && spans.iter().any(|span| span.text == "one")
        )));
        assert!(rows.iter().any(|row| matches!(
            &row.body,
            super::RowBody::Code { language, text, .. } if language == "go" && text.contains("func a")
        )));
        let thinking = flatten_transcript(
            &[TranscriptEntry::Thinking {
                text: "**Planning**".into(),
            }],
            &HashSet::new(),
            None,
        );
        assert!(thinking.iter().any(|row| matches!(
            &row.body,
            super::RowBody::Thinking(text) if text == "Planning"
        )));
    }

    #[test]
    fn colors_a_rust_keyword_and_a_diff_addition() {
        let keyword = highlight_line("fn main() {}", "rs");
        assert!(keyword.iter().any(|span| {
            span.role == TokenRole::Keyword && &"fn main() {}"[span.start..span.end] == "fn"
        }));
        let diff = highlight_line("+added", "diff");
        assert!(diff.iter().any(|span| span.role == TokenRole::Addition));
    }

    #[test]
    fn colors_a_json_key_as_an_attribute_and_its_value_as_a_string() {
        let line = r#"{"name": "value"}"#;
        let spans = highlight_line(line, "json");
        let role_at = |needle: &str| {
            let start = line.find(needle).unwrap() + 1;
            spans
                .iter()
                .find(|span| span.start <= start && start < span.end)
                .map(|span| span.role)
        };
        assert_eq!(role_at("\"name"), Some(TokenRole::Attribute));
        assert_eq!(role_at("\"value"), Some(TokenRole::String));
    }
}
