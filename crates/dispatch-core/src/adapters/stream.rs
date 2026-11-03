use serde_json::Value;

pub(crate) fn events(stdout: &str) -> impl Iterator<Item = Value> + '_ {
    stdout.lines().filter_map(|line| {
        let line = line.trim();
        if !line.starts_with('{') {
            return None;
        }
        serde_json::from_str::<Value>(line)
            .ok()
            .filter(Value::is_object)
    })
}

pub(crate) fn text<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}

pub fn parse_cursor_stream(stdout: &str) -> (String, String) {
    let (mut result, mut last, mut session, mut found) =
        (String::new(), String::new(), String::new(), false);
    for ev in events(stdout) {
        let kind = text(&ev, "type");
        if kind.is_empty() {
            continue;
        }
        found = true;
        if !text(&ev, "session_id").is_empty() {
            session = text(&ev, "session_id").to_owned();
        }
        if kind == "result" {
            result = text(&ev, "result").to_owned();
        }
        if kind == "assistant" && ev.get("timestamp_ms").is_none_or(Value::is_null) {
            let content = message_text(&ev["message"]["content"]);
            if !content.trim().is_empty() {
                last = content;
            }
        }
    }
    if !found {
        return (stdout.trim().to_owned(), String::new());
    }
    if result.trim().is_empty() {
        result = last;
    }
    (result.trim().to_owned(), session)
}

pub fn parse_grok_stream(stdout: &str) -> (String, String) {
    let (mut reply, mut prev, mut session, mut found) =
        (String::new(), String::new(), String::new(), false);
    for ev in events(stdout) {
        let kind = text(&ev, "type");
        if kind.is_empty() {
            continue;
        }
        found = true;
        match kind {
            "text" => reply.push_str(text(&ev, "data")),
            "tool_call" => {
                if !reply.trim().is_empty() {
                    prev = reply.clone();
                }
                reply.clear();
            }
            "end" => session = text(&ev, "sessionId").to_owned(),
            _ => {}
        }
    }
    if !found {
        return (stdout.trim().to_owned(), String::new());
    }
    (
        if reply.trim().is_empty() {
            prev.trim().to_owned()
        } else {
            reply.trim().to_owned()
        },
        session,
    )
}

pub(crate) fn message_text(content: &Value) -> String {
    content
        .as_array()
        .into_iter()
        .flatten()
        .filter(|p| text(p, "type") == "text")
        .map(|p| text(p, "text"))
        .collect()
}

pub fn parse_claude_stream(stdout: &str) -> (String, String) {
    let (mut result, mut last, mut session, mut found) =
        (String::new(), String::new(), String::new(), false);
    for ev in events(stdout) {
        let kind = text(&ev, "type");
        if kind.is_empty() {
            continue;
        }
        found = true;
        if !text(&ev, "session_id").is_empty() {
            session = text(&ev, "session_id").to_owned();
        }
        if kind == "result" {
            result = text(&ev, "result").to_owned();
        }
        if kind == "assistant" && text(&ev, "parent_tool_use_id").is_empty() {
            let value = message_text(&ev["message"]["content"]);
            if !value.trim().is_empty() {
                last = value;
            }
        }
    }
    if !found {
        return (stdout.trim().to_owned(), String::new());
    }
    if result.trim().is_empty() {
        result = last;
    }
    (result.trim().to_owned(), session)
}

pub fn parse_pi_stream(stdout: &str) -> (String, String) {
    let (mut last, mut session, mut found) = (String::new(), String::new(), false);
    for ev in events(stdout) {
        let kind = text(&ev, "type");
        if kind.is_empty() {
            continue;
        }
        found = true;
        if kind == "session" {
            session = text(&ev, "id").to_owned();
        }
        if kind == "message_end" && text(&ev["message"], "role") == "assistant" {
            let value = message_text(&ev["message"]["content"]);
            if !value.trim().is_empty() {
                last = value;
            }
        }
    }
    if !found {
        return (stdout.trim().to_owned(), String::new());
    }
    (last.trim().to_owned(), session)
}
