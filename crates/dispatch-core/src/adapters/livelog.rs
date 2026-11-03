use crate::sandbox::redact;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::Path;
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
pub struct LiveLog(Arc<Mutex<State>>);
#[derive(Default)]
struct State {
    file: Option<File>,
    line: Vec<u8>,
}
impl LiveLog {
    pub fn open(path: &str) -> Self {
        let mut state = State::default();
        if !path.is_empty() {
            let path = Path::new(path);
            if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
                let _ = std::fs::DirBuilder::new()
                    .recursive(true)
                    .mode(0o700)
                    .create(parent);
            }
            state.file = OpenOptions::new()
                .create(true)
                .append(true)
                .mode(0o600)
                .open(path)
                .ok();
        }
        Self(Arc::new(Mutex::new(state)))
    }
    pub fn close(&self) {
        if let Ok(mut state) = self.0.lock() {
            if !state.line.is_empty() {
                let mut line = std::mem::take(&mut state.line);
                line.push(b'\n');
                emit(&mut state, &line);
            }
            state.file = None;
        }
    }
}
fn emit(state: &mut State, line: &[u8]) {
    if let Some(file) = &mut state.file
        && file
            .write_all(redact(&String::from_utf8_lossy(line)).as_bytes())
            .is_err()
    {
        state.file = None;
    }
}
impl Write for LiveLog {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if let Ok(mut state) = self.0.lock() {
            if state.file.is_none() {
                return Ok(buf.len());
            }
            state.line.extend_from_slice(buf);
            while let Some(i) = state.line.iter().position(|b| *b == b'\n') {
                let line: Vec<_> = state.line.drain(..=i).collect();
                emit(&mut state, &line);
            }
            if state.line.len() > 1 << 20 {
                let mut line = std::mem::take(&mut state.line);
                line.push(b'\n');
                emit(&mut state, &line);
            }
        }
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub struct StreamFilter {
    name: String,
    stream: String,
}
impl StreamFilter {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.into(),
            stream: String::new(),
        }
    }
    pub fn line(&mut self, line: &str) -> String {
        use serde_json::{Value, json};
        let Ok(mut ev) = serde_json::from_str::<Value>(line) else {
            return format!("{line}\n");
        };
        self.stream.push_str(line);
        self.stream.push('\n');
        let kind = ev["type"].as_str().unwrap_or("");
        if self.name == "claude" {
            if kind == "rate_limit_event" {
                if ev["rate_limit_info"].is_object() {
                    super::claude_limits::remember(super::claude_limits::samples(
                        &ev["rate_limit_info"],
                        chrono::Utc::now(),
                    ));
                }
                return String::new();
            }
            if kind == "system" {
                return String::new();
            }
            if kind == "stream_event" {
                let event = &ev["event"];
                if event["type"] == "message_delta" {
                    return super::usage::usage_claude(&self.stream)
                        .map_or_else(String::new, |u| super::usage::usage_line(&u));
                }
                if event["type"] != "content_block_delta"
                    || !matches!(
                        event["delta"]["type"].as_str(),
                        Some("text_delta" | "thinking_delta")
                    )
                {
                    return String::new();
                }
                ev = json!({"type":"stream_event", "event": {"type":"content_block_delta", "delta": event["delta"]}});
            } else if matches!(kind, "assistant" | "user") {
                if ev["parent_tool_use_id"]
                    .as_str()
                    .is_some_and(|s| !s.is_empty())
                {
                    return String::new();
                }
                let mut parts = ev["message"]["content"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                for p in &mut parts {
                    if let Some(m) = p.as_object_mut() {
                        m.remove("signature");
                        m.remove("caller");
                    }
                }
                ev = json!({"type": kind, "message": {"role":ev["message"]["role"], "content":parts}});
            } else if kind == "result" {
                ev = json!({"type":"result", "subtype":ev["subtype"], "is_error":ev["is_error"], "result":ev["result"]});
            }
        } else if self.name == "pi" {
            if kind == "message_update" {
                let delta = &ev["assistantMessageEvent"];
                let ty = delta["type"].as_str().unwrap_or("");
                if matches!(ty, "text_delta" | "thinking_delta") {
                    ev = json!({"type":kind, "assistantMessageEvent":{"type":ty,"delta":delta["delta"]}});
                } else if matches!(ty, "text_end" | "thinking_end") {
                    ev = json!({"type":kind, "assistantMessageEvent":{"type":ty,"content":delta["content"]}});
                } else {
                    return String::new();
                }
            } else if kind == "message_end" {
                let msg = &ev["message"];
                let mut out = format!(
                    "{}\n",
                    json!({"type":kind,"message":{"role":msg["role"],"stopReason":msg["stopReason"],"errorMessage":msg["errorMessage"]}})
                );
                if msg["role"] == "assistant"
                    && msg["usage"].is_object()
                    && let Some(u) = super::usage::usage_pi(&self.stream)
                {
                    out.push_str(&super::usage::usage_line(&u));
                }
                return out;
            } else if !matches!(kind, "tool_execution_start" | "tool_execution_end") {
                return String::new();
            }
        }
        format!("{ev}\n")
    }
}
