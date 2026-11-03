//! Pure transcript logic, ported from `run-log.ts`.
//!
//! `build_transcript` reads every agent stream the desktop understands: Codex
//! app-server and `--json`, Claude Code `stream-json`, Pi `--mode json`, ACP
//! `session/update`, Cursor, Grok, and the core's `[kind]` lines. Usage lines
//! are kept for [`live_usage`] and are not drawn.

use std::collections::HashMap;

use serde_json::{Map, Value};

/// One line of a run's transcript, in the shape `buildTranscript` returns.
#[derive(Clone, Debug, PartialEq)]
pub enum TranscriptEntry {
    Step {
        text: String,
        detail: Option<String>,
        failed: Option<bool>,
        output: Option<String>,
    },
    Thinking {
        text: String,
    },
    Message {
        text: String,
    },
    Output {
        text: String,
    },
    Event {
        event: String,
        text: String,
    },
}

/// Context and tokens from the last `{"dispatch":"usage"}` line.
#[derive(Clone, Debug, PartialEq)]
pub struct LiveUsage {
    pub context_tokens: i64,
    pub context_window: i64,
    pub input_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub output_tokens: i64,
    pub reasoning_tokens: i64,
    pub cost_usd: Option<f64>,
}

/// A `WatchRun` frame reduced to what [`run_log`] needs.
#[derive(Clone, Debug, PartialEq)]
pub struct LogEvent {
    pub kind: String,
    pub text: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BlockKind {
    Message,
    Thinking,
}

#[derive(Clone, Copy, Debug)]
struct Delta {
    kind: BlockKind,
    index: usize,
}

/// Fields of a step update. `None` means the key was absent and the previous value stays.
#[derive(Clone, Debug)]
struct StepUpdate {
    text: String,
    failed: Option<Option<bool>>,
    detail: Option<Option<String>>,
    output: Option<Option<String>>,
}

impl StepUpdate {
    fn text(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            failed: None,
            detail: None,
            output: None,
        }
    }

    fn failed(mut self, failed: bool) -> Self {
        self.failed = Some(Some(failed));
        self
    }

    fn detail(mut self, detail: Option<String>) -> Self {
        self.detail = Some(detail);
        self
    }

    fn output(mut self, output: Option<String>) -> Self {
        self.output = Some(output);
        self
    }
}

/// Incremental reader. [`build_transcript`] feeds a whole log; the view appends
/// a suffix when the combined log grows on a newline boundary.
#[derive(Clone, Debug, Default)]
pub struct TranscriptBuilder {
    entries: Vec<TranscriptEntry>,
    items: HashMap<String, usize>,
    untargeted: HashMap<String, String>,
    text: Vec<String>,
    fenced: bool,
    streamed: bool,
    delta: Option<Delta>,
    plain_messages: Vec<usize>,
}

impl TranscriptBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Parse `chunk`. A later call is new text that begins on a line boundary:
    /// the previous chunk ended at a newline. A trailing empty split-segment is
    /// the newline itself, not an extra blank line, so it is dropped.
    pub fn feed(&mut self, chunk: &str) {
        if chunk.is_empty() {
            return;
        }
        if !self.streamed && chunk_has_json(chunk) {
            self.promote_plain();
            self.streamed = true;
        }
        let mut lines: Vec<&str> = chunk.split('\n').collect();
        if chunk.ends_with('\n') && lines.last() == Some(&"") {
            lines.pop();
        }
        let last = lines.len().saturating_sub(1);
        for (index, line) in lines.iter().enumerate() {
            if !self.fenced && line.trim_start().starts_with('{') {
                if let Some(ev) = parse_json(line) {
                    self.flush_text();
                    self.read_event(&ev, line);
                    continue;
                } else if index == last && !chunk.ends_with('\n') {
                    continue;
                }
            }
            if line.trim_start().starts_with("```") || line.trim_start().starts_with("~~~") {
                self.fenced = !self.fenced;
            }
            if !self.fenced
                && let Some((event, text)) = event_line(line)
            {
                self.flush_text();
                self.push(TranscriptEntry::Event { event, text });
                continue;
            }
            self.text.push((*line).to_string());
        }
    }

    pub fn finish(&self, answer: &str) -> Vec<TranscriptEntry> {
        self.finish_with_tail(answer, "")
    }

    /// [`finish`] plus a last line that has not reached its newline yet.
    /// A complete JSON object is applied. An unfinished `{` line is hidden,
    /// matching a live log whose last line is still arriving.
    pub fn finish_with_tail(&self, answer: &str, tail: &str) -> Vec<TranscriptEntry> {
        if !tail.is_empty() && !self.fenced && tail.trim_start().starts_with('{') {
            if parse_json(tail).is_some() {
                let mut clone = self.clone();
                let mut line = String::with_capacity(tail.len() + 1);
                line.push_str(tail);
                line.push('\n');
                clone.feed(&line);
                return clone.finish(answer);
            }
            return self.finish_pending(answer, "");
        }
        self.finish_pending(answer, tail)
    }

    fn finish_pending(&self, answer: &str, tail: &str) -> Vec<TranscriptEntry> {
        let mut result = Vec::with_capacity(self.entries.len() + 1);
        for entry in &self.entries {
            match entry {
                TranscriptEntry::Message { text } => {
                    let text = text.trim().to_string();
                    if !text.is_empty() {
                        result.push(TranscriptEntry::Message { text });
                    }
                }
                TranscriptEntry::Thinking { text } => {
                    let text = text.trim().to_string();
                    if !text.is_empty() {
                        result.push(TranscriptEntry::Thinking { text });
                    }
                }
                other => result.push(other.clone()),
            }
        }
        let mut pending = self.text.clone();
        if !tail.is_empty() {
            pending.push(tail.to_string());
        }
        if !pending.is_empty() {
            let joined = trim_flush(&pending.join("\n")).trim().to_string();
            if !joined.is_empty() {
                if self.streamed {
                    result.push(TranscriptEntry::Output { text: joined });
                } else {
                    result.push(TranscriptEntry::Message { text: joined });
                }
            }
        }
        if !answer.trim().is_empty()
            && !result
                .iter()
                .any(|entry| matches!(entry, TranscriptEntry::Message { .. }))
        {
            result.push(TranscriptEntry::Message {
                text: strip_ansi(answer).trim().to_string(),
            });
        }
        result
    }

    fn promote_plain(&mut self) {
        for index in self.plain_messages.drain(..) {
            if let Some(TranscriptEntry::Message { text }) = self.entries.get(index) {
                let text = text.clone();
                self.entries[index] = TranscriptEntry::Output { text };
            }
        }
    }

    fn flush_text(&mut self) {
        if self.text.is_empty() {
            return;
        }
        let joined = trim_flush(&self.text.join("\n"));
        self.text.clear();
        if joined.trim().is_empty() {
            return;
        }
        if self.streamed {
            self.push(TranscriptEntry::Output { text: joined });
        } else {
            let index = self.push(TranscriptEntry::Message { text: joined });
            self.plain_messages.push(index);
        }
    }

    fn push(&mut self, entry: TranscriptEntry) -> usize {
        self.delta = None;
        self.entries.push(entry);
        self.entries.len() - 1
    }

    fn append_block(&mut self, kind: BlockKind, chunk: &str) {
        if chunk.is_empty() {
            return;
        }
        if let Some(delta) = self.delta
            && delta.kind == kind
            && self.block_is(delta.index, kind)
        {
            self.block_text_mut(delta.index).push_str(chunk);
            return;
        }
        let index = self.push(block_entry(kind, chunk));
        self.delta = Some(Delta { kind, index });
    }

    fn complete_block(&mut self, kind: BlockKind, full: &str) {
        if full.trim().is_empty() {
            return;
        }
        if let Some(delta) = self.delta
            && delta.kind == kind
            && self.block_is(delta.index, kind)
        {
            *self.block_text_mut(delta.index) = full.to_string();
            self.delta = None;
            return;
        }
        self.push(block_entry(kind, full));
    }

    fn stream_item(&mut self, id: &str, kind: BlockKind, chunk: &str) {
        if chunk.is_empty() {
            return;
        }
        if !id.is_empty()
            && let Some(&index) = self.items.get(id)
            && self.block_is(index, kind)
        {
            self.block_text_mut(index).push_str(chunk);
            return;
        }
        let index = self.push(block_entry(kind, chunk));
        if !id.is_empty() {
            self.items.insert(id.to_string(), index);
        }
    }

    fn settle_item(&mut self, id: &str, kind: BlockKind, full: &str) {
        if full.trim().is_empty() {
            return;
        }
        if !id.is_empty()
            && let Some(&index) = self.items.get(id)
            && self.block_is(index, kind)
        {
            *self.block_text_mut(index) = full.to_string();
            return;
        }
        self.stream_item(id, kind, full);
    }

    fn apply_step(&mut self, id: &str, update: StepUpdate) {
        if !id.is_empty()
            && let Some(&index) = self.items.get(id)
        {
            self.merge_step(index, &update);
            return;
        }
        let index = self.push(TranscriptEntry::Step {
            text: update.text,
            detail: update.detail.flatten(),
            failed: update.failed.flatten(),
            output: update.output.flatten(),
        });
        if !id.is_empty() {
            self.items.insert(id.to_string(), index);
        }
    }

    fn merge_step(&mut self, index: usize, update: &StepUpdate) {
        let Some(TranscriptEntry::Step {
            text,
            detail,
            failed,
            output,
        }) = self.entries.get_mut(index)
        else {
            self.entries[index] = TranscriptEntry::Step {
                text: update.text.clone(),
                detail: update.detail.clone().flatten(),
                failed: update.failed.flatten(),
                output: update.output.clone().flatten(),
            };
            return;
        };
        *text = update.text.clone();
        if let Some(value) = update.failed {
            *failed = value;
        }
        if let Some(value) = update.detail.clone() {
            *detail = value;
        }
        if let Some(value) = update.output.clone() {
            *output = value;
        }
    }

    fn block_is(&self, index: usize, kind: BlockKind) -> bool {
        matches!(
            (&self.entries[index], kind),
            (TranscriptEntry::Message { .. }, BlockKind::Message)
                | (TranscriptEntry::Thinking { .. }, BlockKind::Thinking)
        )
    }

    fn block_text_mut(&mut self, index: usize) -> &mut String {
        match &mut self.entries[index] {
            TranscriptEntry::Message { text } | TranscriptEntry::Thinking { text } => text,
            _ => unreachable!("block kind checked"),
        }
    }

    fn read_event(&mut self, ev: &Map<String, Value>, line: &str) {
        if ev.contains_key("dispatch") {
            return;
        }
        if ev.get("method").and_then(Value::as_str) == Some("session/update") {
            self.read_acp(obj(obj(ev.get("params")).get("update")));
            return;
        }
        if ev.get("method").and_then(Value::as_str).is_some() {
            let method = ev.get("method").and_then(Value::as_str).unwrap_or("");
            self.read_notification(method, obj(ev.get("params")));
            return;
        }
        let type_name = json_str(ev.get("type"));
        if type_name == "stream_event" {
            self.read_claude_stream(obj(ev.get("event")));
            return;
        }
        if type_name.starts_with("message_") || type_name.starts_with("tool_execution_") {
            self.read_pi(&type_name, ev);
            return;
        }
        if type_name.starts_with("item.") {
            self.read_codex_json(&type_name, ev);
            return;
        }
        if type_name == "turn.failed" || type_name == "error" {
            let message = json_str(obj(ev.get("error")).get("message"));
            let message = if message.is_empty() {
                json_str(ev.get("message"))
            } else {
                message
            };
            if !message.is_empty() {
                self.push(TranscriptEntry::Event {
                    event: "failed".to_string(),
                    text: message,
                });
            }
            return;
        }
        if type_name == "thinking" {
            self.append_block(BlockKind::Thinking, json_str(ev.get("text")).as_str());
            return;
        }
        if type_name == "assistant" && ev.get("message").is_some() {
            self.read_assistant(ev);
            return;
        }
        if type_name == "user" && ev.get("message").is_some() {
            self.read_tool_results(ev);
            return;
        }
        if type_name == "tool_call" && ev.get("tool_call").is_some() {
            self.read_cursor_tool(ev, line);
            return;
        }
        if type_name == "result" {
            if ev.get("is_error") == Some(&Value::Bool(true))
                && !json_str(ev.get("result")).is_empty()
            {
                self.push(TranscriptEntry::Event {
                    event: "failed".to_string(),
                    text: json_str(ev.get("result")),
                });
            } else if !self
                .entries
                .iter()
                .any(|entry| matches!(entry, TranscriptEntry::Message { .. }))
            {
                self.complete_block(BlockKind::Message, json_str(ev.get("result")).as_str());
            }
            return;
        }
        if type_name == "text" && ev.get("data").and_then(Value::as_str).is_some() {
            self.append_block(BlockKind::Message, json_str(ev.get("data")).as_str());
            return;
        }
        if (type_name == "thought" || type_name == "reasoning")
            && ev.get("data").and_then(Value::as_str).is_some()
        {
            self.append_block(BlockKind::Thinking, json_str(ev.get("data")).as_str());
            return;
        }
        if type_name == "tool_call" && (ev.get("toolName").is_some() || ev.get("title").is_some()) {
            let tool = {
                let name = json_str(ev.get("toolName"));
                if name.is_empty() {
                    json_str(ev.get("title"))
                } else {
                    name
                }
            };
            let (text, detail) = step_for(&tool, obj(ev.get("rawInput")));
            let mut update = StepUpdate::text(text);
            if let Some(detail) = detail {
                update = update.detail(detail);
            }
            self.apply_step(json_str(ev.get("toolCallId")).as_str(), update);
            return;
        }
        if type_name == "tool_call_update" {
            let id = json_str(ev.get("toolCallId"));
            if json_str(ev.get("status")) == "failed"
                && let Some(&index) = self.items.get(&id)
                && let Some(TranscriptEntry::Step { failed, .. }) = self.entries.get_mut(index)
            {
                *failed = Some(true);
            }
            return;
        }
        if ev.get("part").is_some() {
            self.read_opencode(&type_name, ev);
            return;
        }
        if type_name.is_empty() {
            for key in ["response", "result", "output", "text"] {
                if let Some(text) = ev.get(key).and_then(Value::as_str) {
                    self.complete_block(BlockKind::Message, text);
                    return;
                }
            }
        }
    }

    fn read_notification(&mut self, method: &str, params: &Map<String, Value>) {
        let id = json_str(params.get("itemId"));
        if method == "item/agentMessage/delta" {
            self.stream_item(
                &id,
                BlockKind::Message,
                json_str(params.get("delta")).as_str(),
            );
            return;
        }
        if method == "item/reasoning/summaryTextDelta"
            || method == "item/reasoning/textDelta"
            || method == "item/plan/delta"
        {
            self.stream_item(
                &id,
                BlockKind::Thinking,
                json_str(params.get("delta")).as_str(),
            );
            return;
        }
        if method == "item/commandExecution/outputDelta" {
            if let Some(&index) = self.items.get(&id)
                && let Some(TranscriptEntry::Step { output, .. }) = self.entries.get_mut(index)
            {
                let next = strip_ansi(json_str(params.get("delta")).as_str());
                match output {
                    Some(existing) => existing.push_str(&next),
                    None => *output = Some(next),
                }
            }
            return;
        }
        if method == "error" {
            let message = json_str(obj(params.get("error")).get("message"));
            if !message.is_empty() && !js_truthy(params.get("willRetry")) {
                self.push(TranscriptEntry::Event {
                    event: "failed".to_string(),
                    text: message,
                });
            }
            return;
        }
        if method == "turn/completed" {
            let message = json_str(obj(obj(params.get("turn")).get("error")).get("message"));
            if !message.is_empty() {
                self.push(TranscriptEntry::Event {
                    event: "failed".to_string(),
                    text: message,
                });
            }
            return;
        }
        if method != "item/started" && method != "item/completed" {
            return;
        }
        let item = obj(params.get("item"));
        let item_id = json_str(item.get("id"));
        let done = method == "item/completed";
        match json_str(item.get("type")).as_str() {
            "agentMessage" => {
                if done {
                    self.settle_item(
                        &item_id,
                        BlockKind::Message,
                        json_str(item.get("text")).as_str(),
                    );
                }
            }
            "reasoning" | "plan" => {
                if done {
                    let parts = {
                        let summary = arr(item.get("summary"));
                        if summary.is_empty() {
                            arr(item.get("content"))
                        } else {
                            summary
                        }
                    };
                    let text = {
                        let direct = json_str(item.get("text"));
                        if direct.is_empty() {
                            parts
                                .iter()
                                .filter_map(Value::as_str)
                                .filter(|part| !part.is_empty())
                                .collect::<Vec<_>>()
                                .join("\n\n")
                        } else {
                            direct
                        }
                    };
                    self.settle_item(&item_id, BlockKind::Thinking, &text);
                }
            }
            "commandExecution" => {
                let exit = json_i64(item.get("exitCode"));
                let mut update = StepUpdate::text(format!(
                    "Running {}",
                    unwrap_shell(json_str(item.get("command")).as_str())
                ));
                if let Some(exit) = exit {
                    update = update.failed(exit != 0);
                    if exit != 0 {
                        update = update.detail(Some(format!("exit {exit}")));
                    }
                }
                let output =
                    trim_end_ws(&strip_ansi(json_str(item.get("aggregatedOutput")).as_str()));
                if !output.is_empty() {
                    update = update.output(Some(output));
                }
                self.apply_step(&item_id, update);
            }
            "fileChange" => {
                let paths = arr(item.get("changes"))
                    .iter()
                    .map(|change| json_str(obj_val(change).get("path")))
                    .filter(|path| !path.is_empty())
                    .collect::<Vec<_>>()
                    .join(", ");
                let text = if paths.is_empty() {
                    "Edited files".to_string()
                } else {
                    format!("Edited {paths}")
                };
                self.apply_step(
                    &item_id,
                    StepUpdate::text(text).failed(json_str(item.get("status")) == "failed"),
                );
            }
            "mcpToolCall" => {
                let called = [json_str(item.get("server")), json_str(item.get("tool"))]
                    .into_iter()
                    .filter(|part| !part.is_empty())
                    .collect::<Vec<_>>()
                    .join(".");
                self.apply_step(
                    &item_id,
                    StepUpdate::text(format!("Called {called}"))
                        .failed(json_str(item.get("status")) == "failed"),
                );
            }
            "dynamicToolCall" => {
                let (text, detail) = step_for(
                    json_str(item.get("tool")).as_str(),
                    obj(item.get("arguments")),
                );
                let mut update =
                    StepUpdate::text(text).failed(json_str(item.get("status")) == "failed");
                if let Some(detail) = detail {
                    update = update.detail(detail);
                }
                self.apply_step(&item_id, update);
            }
            "webSearch" => {
                self.apply_step(
                    &item_id,
                    StepUpdate::text(format!(
                        "Searching the web for {}",
                        json_str(item.get("query"))
                    )),
                );
            }
            _ => {}
        }
    }

    fn read_acp(&mut self, update: &Map<String, Value>) {
        let text = json_str(obj(update.get("content")).get("text"));
        match json_str(update.get("sessionUpdate")).as_str() {
            "agent_message_chunk" => self.append_block(BlockKind::Message, &text),
            "agent_thought_chunk" => self.append_block(BlockKind::Thinking, &text),
            "plan" => {
                let plan = arr(update.get("entries"))
                    .iter()
                    .map(obj_val)
                    .map(|entry| {
                        let mark = if json_str(entry.get("status")) == "completed" {
                            "- [x]"
                        } else {
                            "- [ ]"
                        };
                        format!("{mark} {}", json_str(entry.get("content")))
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                self.settle_item("acp-plan", BlockKind::Thinking, &plan);
            }
            "tool_call" | "tool_call_update" => self.read_acp_tool(update),
            _ => {}
        }
    }

    fn read_acp_tool(&mut self, update: &Map<String, Value>) {
        let id = json_str(update.get("toolCallId"));
        let mut args = Map::new();
        args.extend(obj(update.get("rawInput")).clone());
        args.extend(obj(update.get("args")).clone());
        let location = json_str(
            obj_val(arr(update.get("locations")).first().unwrap_or(&Value::Null)).get("path"),
        );
        if target(&args).is_empty() && !location.is_empty() {
            args.insert("path".to_string(), Value::String(location));
        }
        let known = self.items.get(&id).copied();
        let tool = {
            let name = json_str(update.get("name"));
            if !name.is_empty() {
                name
            } else {
                let kind = json_str(update.get("kind"));
                if !kind.is_empty() {
                    kind
                } else {
                    let title = json_str(update.get("title"));
                    if !title.is_empty() {
                        title
                    } else {
                        self.untargeted.get(&id).cloned().unwrap_or_default()
                    }
                }
            }
        };
        let status = json_str(update.get("status"));
        let from_content = content_text(update.get("content").unwrap_or(&Value::Null));
        let raw_output = if from_content.is_empty() {
            content_text(update.get("rawOutput").unwrap_or(&Value::Null))
        } else {
            from_content
        };
        let output = trim_end_ws(&strip_ansi(&raw_output));
        if let Some(index) = known
            && matches!(self.entries.get(index), Some(TranscriptEntry::Step { .. }))
        {
            if self.untargeted.contains_key(&id) && !target(&args).is_empty() {
                let (text, detail) = step_for(&tool, &args);
                if let Some(TranscriptEntry::Step {
                    text: slot,
                    detail: detail_slot,
                    ..
                }) = self.entries.get_mut(index)
                {
                    *slot = text;
                    if let Some(detail) = detail {
                        *detail_slot = detail;
                    }
                }
                self.untargeted.remove(&id);
            }
            if let Some(TranscriptEntry::Step {
                failed,
                output: slot,
                ..
            }) = self.entries.get_mut(index)
            {
                if status == "failed" {
                    *failed = Some(true);
                }
                if !output.is_empty() {
                    *slot = Some(output);
                }
            }
            return;
        }
        let (mut text, detail) = step_for(&tool, &args);
        let mut update_step = StepUpdate::text(text.clone());
        if let Some(detail) = detail.clone() {
            update_step = update_step.detail(detail);
        } else if !verb_matched(&tool) {
            update_step = update_step.detail(None);
        }
        if target(&args).is_empty() {
            self.untargeted.insert(id.clone(), tool.clone());
            let title = json_str(update.get("title"));
            if !title.is_empty() && tool != title {
                text = title;
                update_step.text = text;
            }
        }
        if status == "failed" {
            update_step = update_step.failed(true);
        }
        if !output.is_empty() {
            update_step = update_step.output(Some(output));
        }
        self.apply_step(&id, update_step);
    }

    fn read_claude_stream(&mut self, event: &Map<String, Value>) {
        if json_str(event.get("type")) != "content_block_delta" {
            return;
        }
        let delta = obj(event.get("delta"));
        match json_str(delta.get("type")).as_str() {
            "text_delta" => {
                self.append_block(BlockKind::Message, json_str(delta.get("text")).as_str())
            }
            "thinking_delta" => self.append_block(
                BlockKind::Thinking,
                json_str(delta.get("thinking")).as_str(),
            ),
            _ => {}
        }
    }

    fn read_pi(&mut self, type_name: &str, ev: &Map<String, Value>) {
        if type_name == "message_update" {
            let event = obj(ev.get("assistantMessageEvent"));
            match json_str(event.get("type")).as_str() {
                "text_delta" => {
                    self.append_block(BlockKind::Message, json_str(event.get("delta")).as_str())
                }
                "thinking_delta" => {
                    self.append_block(BlockKind::Thinking, json_str(event.get("delta")).as_str())
                }
                "text_end" => {
                    self.complete_block(BlockKind::Message, json_str(event.get("content")).as_str())
                }
                "thinking_end" => self
                    .complete_block(BlockKind::Thinking, json_str(event.get("content")).as_str()),
                _ => {}
            }
            return;
        }
        if type_name == "message_end" {
            let message = obj(ev.get("message"));
            if json_str(message.get("stopReason")) == "error" {
                let text = json_str(message.get("errorMessage"));
                if !text.is_empty() {
                    self.push(TranscriptEntry::Event {
                        event: "failed".to_string(),
                        text,
                    });
                }
            }
            return;
        }
        let id = json_str(ev.get("toolCallId"));
        if type_name == "tool_execution_start" {
            let (text, detail) =
                step_for(json_str(ev.get("toolName")).as_str(), obj(ev.get("args")));
            let mut update = StepUpdate::text(text);
            if let Some(detail) = detail {
                update = update.detail(detail);
            }
            self.apply_step(&id, update);
            return;
        }
        if type_name == "tool_execution_end" {
            let Some(&index) = self.items.get(&id) else {
                return;
            };
            let output = trim_end_ws(&strip_ansi(&content_text(
                obj(ev.get("result")).get("content").unwrap_or(&Value::Null),
            )));
            if let Some(TranscriptEntry::Step {
                output: slot,
                failed,
                ..
            }) = self.entries.get_mut(index)
            {
                if !output.is_empty() {
                    *slot = Some(output);
                }
                if ev.get("isError") == Some(&Value::Bool(true)) {
                    *failed = Some(true);
                }
            }
        }
    }

    fn read_codex_json(&mut self, type_name: &str, ev: &Map<String, Value>) {
        let item = obj(ev.get("item"));
        let id = json_str(item.get("id"));
        let item_type = {
            let kind = json_str(item.get("type"));
            if kind.is_empty() {
                json_str(item.get("item_type"))
            } else {
                kind
            }
        };
        let done = type_name == "item.completed";
        if item_type == "agent_message" || item_type == "assistant_message" {
            if done {
                self.complete_block(BlockKind::Message, json_str(item.get("text")).as_str());
            }
            return;
        }
        if item_type == "reasoning" {
            if done {
                self.complete_block(BlockKind::Thinking, json_str(item.get("text")).as_str());
            }
            return;
        }
        if item_type == "command_execution" {
            let exit = json_i64(item.get("exit_code"));
            let output = trim_end_ws(&strip_ansi(
                json_str(item.get("aggregated_output")).as_str(),
            ));
            let update = StepUpdate::text(format!(
                "Running {}",
                unwrap_shell(json_str(item.get("command")).as_str())
            ))
            .failed(exit.is_some_and(|code| code != 0))
            .detail(exit.and_then(|code| {
                if code == 0 {
                    None
                } else {
                    Some(format!("exit {code}"))
                }
            }))
            .output(if output.is_empty() {
                None
            } else {
                Some(output)
            });
            self.apply_step(&id, update);
            return;
        }
        if item_type == "file_change" {
            let paths = arr(item.get("changes"))
                .iter()
                .map(|change| json_str(obj_val(change).get("path")))
                .filter(|path| !path.is_empty())
                .collect::<Vec<_>>()
                .join(", ");
            let text = if paths.is_empty() {
                "Edited files".to_string()
            } else {
                format!("Edited {paths}")
            };
            self.apply_step(&id, StepUpdate::text(text));
            return;
        }
        if item_type == "mcp_tool_call" {
            let called = [json_str(item.get("server")), json_str(item.get("tool"))]
                .into_iter()
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
                .join(".");
            self.apply_step(
                &id,
                StepUpdate::text(format!("Called {called}"))
                    .failed(json_str(item.get("status")) == "failed"),
            );
            return;
        }
        if item_type == "web_search" {
            self.apply_step(
                &id,
                StepUpdate::text(format!(
                    "Searching the web for {}",
                    json_str(item.get("query"))
                )),
            );
            return;
        }
        if item_type == "error" {
            self.push(TranscriptEntry::Event {
                event: "failed".to_string(),
                text: json_str(item.get("message")),
            });
        }
    }

    fn read_assistant(&mut self, ev: &Map<String, Value>) {
        let parts: Vec<&Map<String, Value>> = arr(obj(ev.get("message")).get("content"))
            .iter()
            .map(obj_val)
            .collect();
        let chunk = parts
            .iter()
            .filter(|part| json_str(part.get("type")) == "text")
            .map(|part| json_str(part.get("text")))
            .collect::<String>();
        if ev.contains_key("timestamp_ms") {
            self.append_block(BlockKind::Message, &chunk);
        } else {
            self.complete_block(BlockKind::Message, &chunk);
        }
        for part in parts {
            if json_str(part.get("type")) == "thinking" {
                self.complete_block(BlockKind::Thinking, json_str(part.get("thinking")).as_str());
            }
            if json_str(part.get("type")) == "tool_use" {
                let (text, detail) =
                    step_for(json_str(part.get("name")).as_str(), obj(part.get("input")));
                let mut update = StepUpdate::text(text);
                if let Some(detail) = detail {
                    update = update.detail(detail);
                }
                self.apply_step(json_str(part.get("id")).as_str(), update);
            }
        }
    }

    fn read_tool_results(&mut self, ev: &Map<String, Value>) {
        for part in arr(obj(ev.get("message")).get("content"))
            .iter()
            .map(obj_val)
        {
            if json_str(part.get("type")) != "tool_result" {
                continue;
            }
            let id = json_str(part.get("tool_use_id"));
            let Some(&index) = self.items.get(&id) else {
                continue;
            };
            let output = trim_end_ws(&strip_ansi(&content_text(
                part.get("content").unwrap_or(&Value::Null),
            )));
            if let Some(TranscriptEntry::Step {
                output: slot,
                failed,
                ..
            }) = self.entries.get_mut(index)
            {
                if !output.is_empty() {
                    *slot = Some(output);
                }
                if part.get("is_error") == Some(&Value::Bool(true)) {
                    *failed = Some(true);
                }
            }
        }
    }

    fn read_cursor_tool(&mut self, ev: &Map<String, Value>, line: &str) {
        let call = obj(ev.get("tool_call"));
        let tool = first_tool_call_key(line, call);
        let body = obj(call.get(&tool));
        let failed = js_truthy(obj(body.get("result")).get("error"))
            || js_truthy(obj(body.get("result")).get("failure"));
        let (text, detail) = step_for(&tool, obj(body.get("args")));
        let mut update = StepUpdate::text(text).failed(failed);
        if let Some(detail) = detail {
            update = update.detail(detail);
        }
        self.apply_step(json_str(ev.get("call_id")).as_str(), update);
    }

    fn read_opencode(&mut self, type_name: &str, ev: &Map<String, Value>) {
        let part = obj(ev.get("part"));
        if type_name == "text" {
            self.append_block(BlockKind::Message, json_str(part.get("text")).as_str());
        } else if type_name == "reasoning" {
            self.append_block(BlockKind::Thinking, json_str(part.get("text")).as_str());
        } else if type_name == "tool_use" || type_name == "tool" {
            let state = obj(part.get("state"));
            let (text, detail) =
                step_for(json_str(part.get("tool")).as_str(), obj(state.get("input")));
            let mut update =
                StepUpdate::text(text).failed(json_str(state.get("status")) == "error");
            if let Some(detail) = detail {
                update = update.detail(detail);
            }
            let id = {
                let call = json_str(part.get("callID"));
                if call.is_empty() {
                    json_str(part.get("id"))
                } else {
                    call
                }
            };
            self.apply_step(&id, update);
        }
    }
}

/// Reads `log` the way the agent's CLI would show it.
/// `answer`, the stored final reply, closes a transcript whose stream holds none.
pub fn build_transcript(log: &str, answer: &str) -> Vec<TranscriptEntry> {
    let mut builder = TranscriptBuilder::new();
    builder.feed(&strip_ansi(log));
    builder.finish(answer)
}

/// The last `{"dispatch":"usage"}` line, or `None` when the log has none.
pub fn live_usage(log: &str) -> Option<LiveUsage> {
    for line in log.split('\n').rev() {
        if !line.contains("\"dispatch\":\"usage\"") {
            continue;
        }
        let Some(ev) = parse_json(line) else {
            continue;
        };
        if ev.get("dispatch").and_then(Value::as_str) != Some("usage") {
            continue;
        }
        return Some(LiveUsage {
            context_tokens: json_i64(ev.get("context")).unwrap_or(0),
            context_window: json_i64(ev.get("window")).unwrap_or(0),
            input_tokens: json_i64(ev.get("input")).unwrap_or(0),
            cache_read_tokens: json_i64(ev.get("cacheRead")).unwrap_or(0),
            cache_write_tokens: json_i64(ev.get("cacheWrite")).unwrap_or(0),
            output_tokens: json_i64(ev.get("output")).unwrap_or(0),
            reasoning_tokens: json_i64(ev.get("reasoning")).unwrap_or(0),
            cost_usd: ev
                .get("costUsd")
                .and_then(Value::as_f64)
                .filter(|n| n.is_finite()),
        });
    }
    None
}

/// Joins stored output with events streamed since, without repeating either.
pub fn run_log(stored: &str, events: &[LogEvent]) -> String {
    let mut streamed = String::new();
    let mut has_output = false;
    for event in events {
        if event.kind == "output" || event.kind == "agent" {
            streamed.push_str(&event.text);
            has_output = true;
        } else {
            let text = event.text.trim();
            if !text.is_empty() {
                streamed.push('[');
                streamed.push_str(&event.kind);
                streamed.push_str("] ");
                streamed.push_str(text);
                streamed.push('\n');
            }
        }
    }
    if stored.is_empty() {
        return streamed;
    }
    if streamed.is_empty() {
        return stored.to_string();
    }
    if streamed.contains(stored) || stored.contains(&streamed) {
        return if streamed.len() >= stored.len() {
            streamed
        } else {
            stored.to_string()
        };
    }
    if has_output {
        streamed
    } else {
        format!("{stored}\n{streamed}")
    }
}

/// `bash -lc 'go test ./...'` reads as `go test ./...`.
///
/// The optional path prefix is only the leading non-whitespace token, so a
/// `/` inside the quoted command stays part of the command.
pub fn unwrap_shell(command: &str) -> String {
    let command = command.trim();
    let token_end = command.find(char::is_whitespace).unwrap_or(command.len());
    let token = &command[..token_end];
    let mut starts: Vec<usize> = token
        .match_indices('/')
        .map(|(index, _)| index + 1)
        .collect();
    starts.push(0);
    starts.sort_unstable_by(|left, right| right.cmp(left));
    for start in starts {
        if let Some(body) = unwrap_at(&command[start..]) {
            return body;
        }
    }
    command.to_string()
}

fn unwrap_at(command: &str) -> Option<String> {
    let rest = if let Some(rest) = command.strip_prefix("bash ") {
        rest
    } else if let Some(rest) = command.strip_prefix("zsh ") {
        rest
    } else {
        command.strip_prefix("sh ")?
    };
    let rest = rest
        .strip_prefix("-lc ")
        .or_else(|| rest.strip_prefix("-c "))?;
    let quote = rest.chars().next()?;
    if quote != '\'' && quote != '"' {
        return None;
    }
    let body = &rest[quote.len_utf8()..];
    let quote_len = quote.len_utf8();
    if body.len() >= quote_len && body.ends_with(quote) {
        Some(body[..body.len() - quote_len].to_string())
    } else {
        None
    }
}

/// Drops terminal escapes and keeps what a carriage return left visible.
pub fn strip_ansi(text: &str) -> String {
    let mut cleaned = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\u{1b}' {
            cleaned.push(ch);
            continue;
        }
        match chars.peek().copied() {
            Some('[') => {
                chars.next();
                let mut params = true;
                for next in chars.by_ref() {
                    if params && matches!(next, '0'..='9' | ';' | '?') {
                        continue;
                    }
                    params = false;
                    if ('\u{20}'..='\u{2f}').contains(&next) {
                        continue;
                    }
                    if ('\u{40}'..='\u{7e}').contains(&next) {
                        break;
                    }
                    cleaned.push(next);
                    break;
                }
            }
            Some(']') => {
                chars.next();
                while let Some(next) = chars.next() {
                    if next == '\u{7}' {
                        break;
                    }
                    if next == '\u{1b}' && chars.peek() == Some(&'\\') {
                        chars.next();
                        break;
                    }
                }
            }
            Some(next) if ('@'..='Z').contains(&next) || matches!(next, '\\' | '_' | '-') => {
                chars.next();
            }
            _ => {}
        }
    }
    cleaned
        .split('\n')
        .map(|line| {
            let trimmed = line.strip_suffix('\r').unwrap_or(line);
            match trimmed.rfind('\r') {
                Some(index) => &trimmed[index + 1..],
                None => trimmed,
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A diff or patch: hunk headers, or most lines marked added or removed.
pub fn looks_like_diff(text: &str) -> bool {
    let lines: Vec<&str> = text.split('\n').filter(|line| !line.is_empty()).collect();
    if lines
        .iter()
        .any(|line| is_hunk(line) || line.starts_with("diff --git "))
    {
        return true;
    }
    let marked = lines.iter().filter(|line| is_diff_mark(line)).count();
    lines.len() >= 2 && marked >= lines.len().div_ceil(2)
}

fn is_hunk(line: &str) -> bool {
    let Some(rest) = line.strip_prefix("@@ ") else {
        return false;
    };
    rest.contains(" @@")
}

fn is_diff_mark(line: &str) -> bool {
    let mut chars = line.chars();
    match chars.next() {
        Some('+') | Some('-') => !matches!(chars.next(), Some('+' | '-')),
        _ => false,
    }
}

/// JSON object or array, parsed only when the text is small enough to be a blob.
pub fn looks_like_json(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.len() > 2_000_000 {
        return false;
    }
    let bytes = trimmed.as_bytes();
    if bytes.is_empty() {
        return false;
    }
    let open = bytes[0];
    let close = *bytes.last().unwrap_or(&0);
    if !matches!((open, close), (b'{', b'}') | (b'[', b']')) {
        return false;
    }
    serde_json::from_str::<Value>(trimmed).is_ok()
}

fn chunk_has_json(chunk: &str) -> bool {
    chunk
        .split('\n')
        .any(|line| line.trim_start().starts_with('{') && parse_json(line).is_some())
}

fn parse_json(line: &str) -> Option<Map<String, Value>> {
    let value: Value = serde_json::from_str(line.trim()).ok()?;
    match value {
        Value::Object(map) => Some(map),
        _ => None,
    }
}

fn event_line(line: &str) -> Option<(String, String)> {
    let rest = line.strip_prefix('[')?;
    let (kind, after) = rest.split_once(']')?;
    let mut chars = kind.chars();
    let first = chars.next()?;
    if !first.is_ascii_lowercase() {
        return None;
    }
    if !chars.all(|ch| ch.is_ascii_lowercase() || ch == '_' || ch == '-') {
        return None;
    }
    let text = after.strip_prefix(' ')?;
    Some((kind.to_string(), text.trim().to_string()))
}

fn block_entry(kind: BlockKind, text: &str) -> TranscriptEntry {
    match kind {
        BlockKind::Message => TranscriptEntry::Message {
            text: text.to_string(),
        },
        BlockKind::Thinking => TranscriptEntry::Thinking {
            text: text.to_string(),
        },
    }
}

fn trim_flush(text: &str) -> String {
    let mut start = 0;
    let bytes = text.as_bytes();
    while start < bytes.len() && bytes[start] == b'\n' {
        start += 1;
    }
    let mut end = text.len();
    while end > start {
        let ch = text[..end].chars().next_back().unwrap();
        if ch.is_whitespace() {
            end -= ch.len_utf8();
        } else {
            break;
        }
    }
    text[start..end].to_string()
}

fn trim_end_ws(text: &str) -> String {
    text.trim_end().to_string()
}

fn empty_map() -> &'static Map<String, Value> {
    use std::sync::OnceLock;
    static EMPTY: OnceLock<Map<String, Value>> = OnceLock::new();
    EMPTY.get_or_init(Map::new)
}

fn obj<'a>(value: Option<&'a Value>) -> &'a Map<String, Value> {
    match value.and_then(Value::as_object) {
        Some(map) => map,
        None => empty_map(),
    }
}

fn obj_val<'a>(value: &'a Value) -> &'a Map<String, Value> {
    match value.as_object() {
        Some(map) => map,
        None => empty_map(),
    }
}

fn arr(value: Option<&Value>) -> &[Value] {
    value
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

fn json_str(value: Option<&Value>) -> String {
    value.and_then(Value::as_str).unwrap_or("").to_string()
}

fn json_i64(value: Option<&Value>) -> Option<i64> {
    let value = value?;
    if let Some(number) = value.as_i64() {
        return Some(number);
    }
    if let Some(number) = value.as_u64() {
        return i64::try_from(number).ok();
    }
    let number = value.as_f64()?;
    if number.is_finite() && number.fract() == 0.0 {
        Some(number as i64)
    } else {
        None
    }
}

fn js_truthy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => false,
        Some(Value::Bool(flag)) => *flag,
        Some(Value::Number(number)) => number.as_f64().is_some_and(|n| n != 0.0),
        Some(Value::String(text)) => !text.is_empty(),
        Some(Value::Array(_) | Value::Object(_)) => true,
    }
}

fn target(args: &Map<String, Value>) -> String {
    for key in [
        "command",
        "cmd",
        "path",
        "Path",
        "file_path",
        "target_file",
        "filePath",
        "pattern",
        "query",
        "url",
        "glob",
    ] {
        let Some(value) = args.get(key) else {
            continue;
        };
        if let Some(text) = value.as_str()
            && !text.trim().is_empty()
        {
            return text.trim().to_string();
        }
        if let Some(parts) = value.as_array()
            && parts.iter().all(|part| part.as_str().is_some())
            && !parts.is_empty()
        {
            return parts
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(" ");
        }
    }
    String::new()
}

fn step_for(tool: &str, args: &Map<String, Value>) -> (String, Option<Option<String>>) {
    let name = tool
        .strip_suffix("ToolCall")
        .unwrap_or(tool)
        .replace(['_', '-'], " ");
    let name = name.trim().to_string();
    let lower = name.to_lowercase().replace(' ', "");
    let on = target(args);
    let verb = if contains_any(&lower, &["shell", "bash", "terminal", "command", "exec"]) {
        "Running"
    } else if contains_any(&lower, &["read", "view", "cat"]) {
        "Reading"
    } else if contains_any(&lower, &["write", "create"]) {
        "Writing"
    } else if contains_any(&lower, &["edit", "replace", "patch", "update", "delete"]) {
        "Editing"
    } else if contains_any(&lower, &["grep", "glob", "search", "find", "ls", "list"]) {
        "Searching"
    } else if contains_any(&lower, &["fetch", "web"]) {
        "Fetching"
    } else {
        ""
    };
    if !verb.is_empty() {
        let text = if on.is_empty() {
            format!("{verb} ({name})")
        } else {
            format!("{verb} {on}")
        };
        (text, None)
    } else {
        let text = if name.is_empty() {
            "Tool call".to_string()
        } else {
            name
        };
        let detail = if on.is_empty() { None } else { Some(on) };
        (text, Some(detail))
    }
}

fn verb_matched(tool: &str) -> bool {
    let (_, detail) = step_for(tool, empty_map());
    detail.is_none() && !tool.is_empty()
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

fn content_text(value: &Value) -> String {
    if let Some(text) = value.as_str() {
        return text.to_string();
    }
    let parts = arr(Some(value))
        .iter()
        .map(obj_val)
        .map(|part| {
            let inner = obj(part.get("content"));
            let text = json_str(part.get("text"));
            if !text.is_empty() {
                return text;
            }
            let inner_text = json_str(inner.get("text"));
            if !inner_text.is_empty() {
                return inner_text;
            }
            if json_str(part.get("type")) == "diff" {
                format!("Edited {}", json_str(part.get("path")))
            } else {
                String::new()
            }
        })
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    if !parts.is_empty() {
        return parts;
    }
    let raw = obj_val(value);
    for key in ["content", "output", "stdout", "result"] {
        let text = json_str(raw.get(key));
        if !text.is_empty() {
            return text;
        }
    }
    String::new()
}

/// `serde_json` maps are ordered by key. Scan the raw line so the first
/// `*ToolCall` key wins, matching `Object.keys`.
fn first_tool_call_key(line: &str, call: &Map<String, Value>) -> String {
    let start = line.find("\"tool_call\"").unwrap_or(0);
    let mut rest = &line[start..];
    while let Some(open) = rest.find('"') {
        rest = &rest[open + 1..];
        let Some(close) = rest.find('"') else {
            break;
        };
        let key = &rest[..close];
        rest = &rest[close + 1..];
        if key.ends_with("ToolCall") && call.contains_key(key) {
            return key.to_string();
        }
    }
    call.keys()
        .find(|key| key.ends_with("ToolCall"))
        .cloned()
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::{
        LiveUsage, LogEvent, TranscriptBuilder, TranscriptEntry, build_transcript, live_usage,
        looks_like_diff, run_log, strip_ansi, unwrap_shell,
    };

    fn message(text: &str) -> TranscriptEntry {
        TranscriptEntry::Message { text: text.into() }
    }

    fn thinking(text: &str) -> TranscriptEntry {
        TranscriptEntry::Thinking { text: text.into() }
    }

    fn output(text: &str) -> TranscriptEntry {
        TranscriptEntry::Output { text: text.into() }
    }

    fn event(event: &str, text: &str) -> TranscriptEntry {
        TranscriptEntry::Event {
            event: event.into(),
            text: text.into(),
        }
    }

    fn step(
        text: &str,
        failed: Option<bool>,
        detail: Option<&str>,
        output: Option<&str>,
    ) -> TranscriptEntry {
        TranscriptEntry::Step {
            text: text.into(),
            detail: detail.map(str::to_string),
            failed,
            output: output.map(str::to_string),
        }
    }

    fn note(method: &str, params: serde_json::Value) -> String {
        serde_json::json!({ "method": method, "params": params }).to_string()
    }

    const CURSOR_FIXTURE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../crates/dispatch-core/tests/fixtures/adapters/cursor/stream.ndjson"
    ));
    const GROK_FIXTURE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../crates/dispatch-core/tests/fixtures/adapters/grok/stream.ndjson"
    ));

    #[test]
    fn reads_a_plain_reply_between_core_event_lines() {
        let log = "## Plan\n\n- Read the store\n- Add the handler\n[merged] into main\nDone.\n";
        assert_eq!(
            build_transcript(log, ""),
            vec![
                message("## Plan\n\n- Read the store\n- Add the handler"),
                event("merged", "into main"),
                message("Done."),
            ]
        );
        assert_eq!(
            build_transcript("```\n[note] not an event\n```", ""),
            vec![message("```\n[note] not an event\n```")]
        );
        assert_eq!(build_transcript("  \n", ""), Vec::<TranscriptEntry>::new());
    }

    #[test]
    fn streams_codex_app_server_notifications() {
        let live = [
            note("item/reasoning/summaryTextDelta", serde_json::json!({"itemId": "r1", "delta": "**Plan"})),
            note("item/reasoning/summaryTextDelta", serde_json::json!({"itemId": "r1", "delta": "ning**"})),
            note("item/agentMessage/delta", serde_json::json!({"itemId": "m1", "delta": "Reading "})),
            note("item/agentMessage/delta", serde_json::json!({"itemId": "m1", "delta": "the file."})),
            note(
                "item/started",
                serde_json::json!({"item": {"type": "commandExecution", "id": "c1", "command": "/bin/zsh -lc 'go test ./...'", "status": "inProgress"}}),
            ),
            note("item/commandExecution/outputDelta", serde_json::json!({"itemId": "c1", "delta": "ok  a\n"})),
            note("item/agentMessage/delta", serde_json::json!({"itemId": "m2", "delta": "Al"})),
        ]
        .join("\n");
        assert_eq!(
            build_transcript(&live, ""),
            vec![
                thinking("**Planning**"),
                message("Reading the file."),
                step("Running go test ./...", None, None, Some("ok  a\n")),
                message("Al"),
            ]
        );
        let done = [
            live,
            note(
                "item/completed",
                serde_json::json!({"item": {"type": "commandExecution", "id": "c1", "command": "go test ./...", "exitCode": 1, "aggregatedOutput": "FAIL a\n"}}),
            ),
            note(
                "item/completed",
                serde_json::json!({"item": {"type": "agentMessage", "id": "m2", "text": "All done.", "phase": "final_answer"}}),
            ),
            note(
                "item/completed",
                serde_json::json!({"item": {"type": "fileChange", "id": "f1", "changes": [{"path": "a.go", "kind": "update"}], "status": "completed"}}),
            ),
            note(
                "turn/completed",
                serde_json::json!({"turn": {"id": "t1", "status": "failed", "error": {"message": "usage limit"}}}),
            ),
        ]
        .join("\n");
        assert_eq!(
            build_transcript(&done, ""),
            vec![
                thinking("**Planning**"),
                message("Reading the file."),
                step(
                    "Running go test ./...",
                    Some(true),
                    Some("exit 1"),
                    Some("FAIL a")
                ),
                message("All done."),
                step("Edited a.go", Some(false), None, None),
                event("failed", "usage limit"),
            ]
        );
    }

    #[test]
    fn reads_codex_json_as_steps_reasoning_and_reply() {
        let log = [
            serde_json::json!({"type": "thread.started", "thread_id": "t1"}).to_string(),
            serde_json::json!({"type": "item.completed", "item": {"id": "i0", "type": "reasoning", "text": "**Planning**"}}).to_string(),
            serde_json::json!({"type": "item.started", "item": {"id": "i1", "type": "command_execution", "command": "go test ./...", "status": "in_progress"}}).to_string(),
            serde_json::json!({"type": "item.completed", "item": {"id": "i1", "type": "command_execution", "command": "go test ./...", "exit_code": 1}}).to_string(),
            serde_json::json!({"type": "item.completed", "item": {"id": "i2", "type": "file_change", "changes": [{"path": "a.go", "kind": "update"}]}}).to_string(),
            serde_json::json!({"type": "item.completed", "item": {"id": "i3", "type": "agent_message", "text": "Fixed **it**."}}).to_string(),
            serde_json::json!({"type": "turn.completed", "usage": {}}).to_string(),
            "warning: something on stderr".to_string(),
            "{\"type\":\"item.sta".to_string(),
        ]
        .join("\n");
        assert_eq!(
            build_transcript(&log, ""),
            vec![
                thinking("**Planning**"),
                step("Running go test ./...", Some(true), Some("exit 1"), None),
                step("Edited a.go", None, None, None),
                message("Fixed **it**."),
                output("warning: something on stderr"),
            ]
        );
    }

    #[test]
    fn joins_cursor_and_grok_deltas() {
        let cursor = build_transcript(CURSOR_FIXTURE, "");
        let steps: Vec<_> = cursor
            .iter()
            .filter(|entry| matches!(entry, TranscriptEntry::Step { .. }))
            .cloned()
            .collect();
        assert_eq!(
            steps,
            vec![step("Reading /tmp/ws/a.txt", Some(false), None, None)]
        );
        let messages: Vec<_> = cursor
            .iter()
            .filter(|entry| matches!(entry, TranscriptEntry::Message { .. }))
            .cloned()
            .collect();
        assert_eq!(
            messages,
            vec![message("The file `a.txt` contains only the word “hello”.")]
        );
        assert_eq!(
            cursor.first(),
            Some(&thinking("Reading a.txt to prepare a one-sentence reply."))
        );

        let grok = build_transcript(GROK_FIXTURE, "");
        assert_eq!(
            grok.first(),
            Some(&step("Reading /tmp/ws/a.txt", None, None, None))
        );
        assert_eq!(
            grok.iter()
                .find(|entry| matches!(entry, TranscriptEntry::Message { .. })),
            Some(&message("The file says hello."))
        );
    }

    #[test]
    fn reads_acp_session_updates() {
        let update = |body: serde_json::Value| {
            serde_json::json!({"method": "session/update", "params": {"update": body}}).to_string()
        };
        let log = [
            update(serde_json::json!({"sessionUpdate": "user_message_chunk", "content": {"type": "text", "text": "Read README.md"}})),
            update(serde_json::json!({"sessionUpdate": "agent_thought_chunk", "content": {"type": "text", "text": "Looking at "}})),
            update(serde_json::json!({"sessionUpdate": "agent_thought_chunk", "content": {"type": "text", "text": "the readme."}})),
            update(serde_json::json!({"sessionUpdate": "plan", "entries": [{"content": "Read the file", "status": "in_progress"}]})),
            update(serde_json::json!({"sessionUpdate": "tool_call", "toolCallId": "t1", "name": "read_file", "args": {"Path": "/ws/README.md"}, "status": "in_progress", "kind": "read"})),
            update(serde_json::json!({"sessionUpdate": "tool_call_update", "toolCallId": "t1", "status": "completed", "content": [{"type": "content", "content": {"type": "text", "text": "# Probe\n"}}]})),
            update(serde_json::json!({"sessionUpdate": "tool_call", "toolCallId": "t2", "title": "bash", "kind": "execute", "status": "pending"})),
            update(serde_json::json!({"sessionUpdate": "tool_call_update", "toolCallId": "t2", "status": "failed", "rawInput": {"command": "rm -rf x"}, "rawOutput": {"output": "denied"}})),
            update(serde_json::json!({"sessionUpdate": "plan", "entries": [{"content": "Read the file", "status": "completed"}]})),
            update(serde_json::json!({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "It holds a "}})),
            update(serde_json::json!({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "probe heading."}})),
            serde_json::json!({"dispatch": "usage", "context": 16270, "window": 200000, "input": 10187, "cacheRead": 5873, "output": 210}).to_string(),
        ]
        .join("\n");
        assert_eq!(
            build_transcript(&log, ""),
            vec![
                thinking("Looking at the readme."),
                thinking("- [x] Read the file"),
                step("Reading /ws/README.md", None, None, Some("# Probe")),
                step("Running rm -rf x", Some(true), None, Some("denied")),
                message("It holds a probe heading."),
            ]
        );
    }

    #[test]
    fn reads_claude_stream_json() {
        let line = |value: serde_json::Value| value.to_string();
        let log = [
            line(serde_json::json!({"type": "system", "subtype": "init", "session_id": "s1"})),
            line(serde_json::json!({"type": "stream_event", "event": {"type": "content_block_delta", "index": 0, "delta": {"type": "thinking_delta", "thinking": "Need the file."}}})),
            line(serde_json::json!({"type": "assistant", "message": {"content": [{"type": "thinking", "thinking": "Need the file."}]}})),
            line(serde_json::json!({"type": "assistant", "message": {"content": [{"type": "tool_use", "id": "tu1", "name": "Read", "input": {"file_path": "/ws/a.txt"}}]}})),
            line(serde_json::json!({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": "tu1", "content": [{"type": "text", "text": "hello"}]}]}})),
            line(serde_json::json!({"type": "stream_event", "event": {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "It says "}}})),
            line(serde_json::json!({"type": "stream_event", "event": {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "hello."}}})),
            line(serde_json::json!({"type": "assistant", "message": {"content": [{"type": "text", "text": "It says hello."}]}})),
            line(serde_json::json!({"type": "rate_limit_event", "rate_limit_info": {}})),
            line(serde_json::json!({"type": "result", "subtype": "success", "is_error": false, "result": "It says hello.", "total_cost_usd": 0.01})),
        ]
        .join("\n");
        assert_eq!(
            build_transcript(&log, ""),
            vec![
                thinking("Need the file."),
                step("Reading /ws/a.txt", None, None, Some("hello")),
                message("It says hello."),
            ]
        );
        assert_eq!(
            build_transcript(
                &line(
                    serde_json::json!({"type": "result", "is_error": true, "result": "Credit balance is too low"})
                ),
                "",
            ),
            vec![event("failed", "Credit balance is too low")]
        );
    }

    #[test]
    fn reads_pi_json() {
        let line = |value: serde_json::Value| value.to_string();
        let log = [
            line(serde_json::json!({"type": "session", "id": "p1", "cwd": "/ws"})),
            line(serde_json::json!({"type": "agent_start"})),
            line(serde_json::json!({"type": "message_update", "assistantMessageEvent": {"type": "thinking_delta", "delta": "Checking."}})),
            line(serde_json::json!({"type": "tool_execution_start", "toolCallId": "c1", "toolName": "bash", "args": {"command": "ls"}})),
            line(serde_json::json!({"type": "tool_execution_end", "toolCallId": "c1", "toolName": "bash", "result": {"content": [{"type": "text", "text": "a.txt"}]}, "isError": false})),
            line(serde_json::json!({"type": "message_update", "assistantMessageEvent": {"type": "text_delta", "delta": "One "}})),
            line(serde_json::json!({"type": "message_update", "assistantMessageEvent": {"type": "text_delta", "delta": "file."}})),
            line(serde_json::json!({"type": "message_end", "message": {"role": "assistant", "stopReason": "stop"}})),
            line(serde_json::json!({"type": "agent_end"})),
        ]
        .join("\n");
        assert_eq!(
            build_transcript(&log, ""),
            vec![
                thinking("Checking."),
                step("Running ls", None, None, Some("a.txt")),
                message("One file."),
            ]
        );
    }

    #[test]
    fn keeps_the_last_usage_line() {
        let log = [
            serde_json::json!({"dispatch": "usage", "context": 1000, "window": 272000, "input": 900, "output": 100}).to_string(),
            "plain output".to_string(),
            serde_json::json!({"dispatch": "usage", "context": 46200, "window": 272000, "input": 9000, "cacheRead": 36000, "output": 1200, "reasoning": 300, "costUsd": null}).to_string(),
        ]
        .join("\n");
        assert_eq!(
            live_usage(&log),
            Some(LiveUsage {
                context_tokens: 46200,
                context_window: 272000,
                input_tokens: 9000,
                cache_read_tokens: 36000,
                cache_write_tokens: 0,
                output_tokens: 1200,
                reasoning_tokens: 300,
                cost_usd: None,
            })
        );
        assert_eq!(live_usage("no usage here"), None);
        assert_eq!(build_transcript(&log, ""), vec![output("plain output")]);
    }

    #[test]
    fn falls_back_to_the_stored_answer_and_strips_escapes() {
        assert_eq!(
            build_transcript(
                &serde_json::json!({"type": "turn.started"}).to_string(),
                "Done."
            ),
            vec![message("Done.")]
        );
        assert_eq!(
            strip_ansi("\u{1b}[32mok\u{1b}[0m\n10%\r50%\r100%"),
            "ok\n100%"
        );
    }

    #[test]
    fn unwraps_a_login_shell_and_keeps_command_output() {
        assert_eq!(unwrap_shell("bash -lc 'go test ./...'"), "go test ./...");
        assert_eq!(unwrap_shell("/bin/zsh -lc \"rg -n x\""), "rg -n x");
        assert_eq!(unwrap_shell("go vet"), "go vet");
        let log = serde_json::json!({
            "type": "item.completed",
            "item": {
                "id": "c1",
                "type": "command_execution",
                "command": "bash -lc 'ls'",
                "aggregated_output": "a\nb\n",
                "exit_code": 0
            }
        })
        .to_string();
        assert_eq!(
            build_transcript(&log, ""),
            vec![step("Running ls", Some(false), None, Some("a\nb"))]
        );
    }

    #[test]
    fn run_log_appends_core_lines_and_prefers_the_longer_copy() {
        let events = vec![
            LogEvent {
                kind: "output".into(),
                text: "hello\n".into(),
            },
            LogEvent {
                kind: "merged".into(),
                text: "into main".into(),
            },
        ];
        assert_eq!(run_log("", &events), "hello\n[merged] into main\n");
        assert_eq!(run_log("stored", &[]), "stored");
        // An output event replaces the stored copy instead of appending it.
        assert_eq!(run_log("stored", &events), "hello\n[merged] into main\n");
        assert_eq!(run_log("hello\n", &events[..1]), "hello\n");
        let merged_only = [LogEvent {
            kind: "merged".into(),
            text: "into main".into(),
        }];
        assert_eq!(
            run_log("stored", &merged_only),
            "stored\n[merged] into main\n"
        );
    }

    #[test]
    fn appending_on_a_newline_matches_a_full_parse() {
        let log = "## Plan\n\n- Read the store\n[merged] into main\nDone.\n";
        let full = build_transcript(log, "");
        let split = log.find('\n').unwrap();
        let mut builder = TranscriptBuilder::new();
        builder.feed(&log[..=split]);
        builder.feed(&log[split + 1..]);
        assert_eq!(builder.finish(""), full);
        let mut tail = TranscriptBuilder::new();
        let stable = log.rfind('\n').map(|index| &log[..=index]).unwrap_or("");
        tail.feed(stable);
        assert_eq!(tail.finish_with_tail("", &log[stable.len()..]), full);
    }

    #[test]
    fn recognises_a_diff() {
        assert!(looks_like_diff("@@ -1 +1 @@\n-a\n+b"));
        assert!(!looks_like_diff("just two\nplain lines"));
    }
}
