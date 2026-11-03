use super::stream::{events, text};
use serde_json::{Value, json};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Usage {
    pub input_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub output_tokens: i64,
    pub reasoning_tokens: i64,
    pub cost_usd: Option<f64>,
    pub context_tokens: i64,
    pub context_window: i64,
}

pub fn usage_line(u: &Usage) -> String {
    format!(
        "{}\n",
        json!({"dispatch":"usage", "context":u.context_tokens, "window":u.context_window, "input":u.input_tokens, "cacheRead":u.cache_read_tokens, "cacheWrite":u.cache_write_tokens, "output":u.output_tokens, "reasoning":u.reasoning_tokens, "costUsd":u.cost_usd})
    )
}
fn num(v: &Value, key: &str) -> i64 {
    v.get(key).and_then(Value::as_i64).unwrap_or(0)
}
fn grok_add(u: &mut Usage, x: &Value) {
    u.input_tokens += num(x, "input_tokens");
    u.cache_read_tokens += num(x, "cache_read_input_tokens");
    u.cache_write_tokens += num(x, "cache_creation_input_tokens");
    u.output_tokens += num(x, "output_tokens");
    u.reasoning_tokens += num(x, "reasoning_tokens");
}
pub fn usage_cursor(stdout: &str) -> Option<Usage> {
    let mut out = None;
    for ev in events(stdout) {
        if text(&ev, "type") == "result" && ev["usage"].is_object() {
            let x = &ev["usage"];
            out = Some(Usage {
                input_tokens: num(x, "inputTokens"),
                output_tokens: num(x, "outputTokens"),
                cache_read_tokens: num(x, "cacheReadTokens"),
                cache_write_tokens: num(x, "cacheWriteTokens"),
                ..Usage::default()
            });
        }
    }
    out
}
pub fn usage_grok(stdout: &str) -> Option<Usage> {
    let (mut end, mut calls, mut last) = (None, None::<Usage>, 0);
    for ev in events(stdout) {
        let x = &ev["usage"];
        if !x.is_object() {
            continue;
        }
        match text(&ev, "type") {
            "usage" => {
                grok_add(calls.get_or_insert_with(Usage::default), x);
                last = num(x, "input_tokens")
                    + num(x, "cache_read_input_tokens")
                    + num(x, "cache_creation_input_tokens")
                    + num(x, "output_tokens");
            }
            "end" => {
                let mut u = Usage::default();
                grok_add(&mut u, x);
                u.cost_usd = ev["total_cost_usd"].as_f64();
                end = Some(u);
            }
            _ => {}
        }
    }
    let mut out = end.or(calls);
    if let Some(u) = &mut out {
        u.context_tokens = last;
    }
    out
}
pub fn usage_pi(stdout: &str) -> Option<Usage> {
    let mut out = None::<Usage>;
    for ev in events(stdout) {
        let msg = &ev["message"];
        let x = &msg["usage"];
        if text(&ev, "type") != "message_end" || text(msg, "role") != "assistant" || !x.is_object()
        {
            continue;
        }
        let u = out.get_or_insert_with(Usage::default);
        let (input, read, write, output) = (
            num(x, "input"),
            num(x, "cacheRead"),
            num(x, "cacheWrite"),
            num(x, "output"),
        );
        u.input_tokens += input;
        u.cache_read_tokens += read;
        u.cache_write_tokens += write;
        u.output_tokens += output;
        u.context_tokens = input + read + write + output;
        if let Some(cost) = x["cost"]["total"].as_f64() {
            u.cost_usd = Some(u.cost_usd.unwrap_or(0.0) + cost);
        }
    }
    out
}

fn claude_add(u: &mut Usage, x: &Value) {
    u.input_tokens += num(x, "input_tokens");
    u.cache_read_tokens += num(x, "cache_read_input_tokens");
    u.cache_write_tokens += num(x, "cache_creation_input_tokens");
    u.output_tokens += num(x, "output_tokens");
    u.reasoning_tokens += num(&x["output_tokens_details"], "thinking_tokens");
}
fn claude_context(x: &Value) -> i64 {
    num(x, "input_tokens")
        + num(x, "cache_read_input_tokens")
        + num(x, "cache_creation_input_tokens")
        + num(x, "output_tokens")
}
pub fn usage_claude(stdout: &str) -> Option<Usage> {
    let mut calls = std::collections::HashMap::<String, Value>::new();
    let mut order = Vec::new();
    let mut context = 0;
    let mut final_usage = None;
    for ev in events(stdout) {
        let event = &ev["event"];
        if text(&ev, "type") == "stream_event"
            && text(event, "type") == "message_delta"
            && event["usage"].is_object()
        {
            let id = text(&ev, "api_message_id");
            let id = if id.is_empty() {
                order.len().to_string()
            } else {
                id.to_owned()
            };
            if !calls.contains_key(&id) {
                order.push(id.clone());
            }
            calls.insert(id, event["usage"].clone());
            if text(&ev, "parent_tool_use_id").is_empty() {
                context = claude_context(&event["usage"]);
            }
        }
        let x = &ev["usage"];
        if text(&ev, "type") != "result" || !x.is_object() {
            continue;
        }
        let mut u = Usage {
            cost_usd: ev["total_cost_usd"].as_f64(),
            ..Usage::default()
        };
        let models = ev["modelUsage"].as_object();
        if models.is_none_or(|m| m.is_empty()) {
            claude_add(&mut u, x);
        }
        let mut most = -1;
        for model in models.into_iter().flat_map(|m| m.values()) {
            u.input_tokens += num(model, "inputTokens");
            u.cache_read_tokens += num(model, "cacheReadInputTokens");
            u.cache_write_tokens += num(model, "cacheCreationInputTokens");
            u.output_tokens += num(model, "outputTokens");
            u.reasoning_tokens += num(model, "thinkingTokens");
            let output = num(model, "outputTokens");
            if output > most {
                most = output;
                u.context_window = num(model, "contextWindow");
            }
        }
        u.context_tokens = x["iterations"]
            .as_array()
            .and_then(|a| a.last())
            .map_or(context, claude_context);
        final_usage = Some(u);
    }
    if final_usage.is_some() {
        return final_usage;
    }
    if order.is_empty() {
        return None;
    }
    let mut u = Usage {
        context_tokens: context,
        ..Usage::default()
    };
    for id in order {
        if let Some(x) = calls.get(&id) {
            claude_add(&mut u, x);
        }
    }
    Some(u)
}

#[derive(Clone, Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AcpUsage {
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cached_read_tokens: i64,
    pub cached_write_tokens: i64,
    pub thought_tokens: i64,
    pub total_tokens: i64,
}
impl AcpUsage {
    pub fn normalize(&self) -> Usage {
        let cache = self.cached_read_tokens + self.cached_write_tokens;
        let mut input = self.input_tokens;
        if cache > 0
            && self.total_tokens > 0
            && self.total_tokens < input + self.output_tokens + cache + self.thought_tokens
        {
            input = (input - cache).max(0);
        }
        let mut output = self.output_tokens;
        if self.thought_tokens > 0
            && self.total_tokens >= input + cache + output + self.thought_tokens
        {
            output += self.thought_tokens;
        }
        Usage {
            input_tokens: input,
            cache_read_tokens: self.cached_read_tokens,
            cache_write_tokens: self.cached_write_tokens,
            output_tokens: output,
            reasoning_tokens: self.thought_tokens,
            ..Usage::default()
        }
    }
}
