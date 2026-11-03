//! Environment filtering and output redaction for agent processes.

use std::collections::HashSet;
use std::io::{self, Write};
use std::sync::LazyLock;

use regex::Regex;

pub fn sanitize(src: &[String], extra: &[String]) -> Vec<String> {
    let mut allowed = HashSet::new();
    for entry in src {
        if let Some(("DISPATCH_ENV_ALLOW", value)) = entry.split_once('=') {
            allowed.extend(
                value
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned),
            );
        }
    }
    if allowed.is_empty()
        && let Ok(value) = std::env::var("DISPATCH_ENV_ALLOW")
    {
        allowed.extend(
            value
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned),
        );
    }
    let overridden: HashSet<_> = extra
        .iter()
        .filter_map(|s| s.split_once('=').map(|(k, _)| k))
        .collect();
    let mut out: Vec<_> = src
        .iter()
        .filter(|entry| {
            let Some((key, _)) = entry.split_once('=') else {
                return false;
            };
            !overridden.contains(key)
                && (matches!(
                    key,
                    "PATH" | "HOME" | "USER" | "LOGNAME" | "SHELL" | "TMPDIR" | "LANG" | "TERM"
                ) || ["LC_", "XDG_", "DISPATCH_"]
                    .iter()
                    .any(|p| key.starts_with(p))
                    || allowed.contains(key))
        })
        .cloned()
        .collect();
    out.extend_from_slice(extra);
    out
}

pub fn environ(extra: &[String]) -> Vec<String> {
    let src = std::env::vars_os()
        .map(|(k, v)| format!("{}={}", k.to_string_lossy(), v.to_string_lossy()))
        .collect::<Vec<_>>();
    sanitize(&src, extra)
}

static PATTERNS: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    [
        (r"(?s)-----BEGIN[^\r\n]*?PRIVATE KEY-----.*?-----END[^\r\n]*?-----", "[REDACTED]"),
        (r#"(?i)\b(Authorization:\s*)[^\r\n"']+"#, "${1}[REDACTED]"),
        (r#"(?i)\b(Bearer\s+)[^\s"',;]+"#, "${1}[REDACTED]"),
        (r#"(?i)\b([A-Za-z0-9_]*(?:TOKEN|SECRET|PASSWORD|API_KEY)[A-Za-z0-9_]*\s*=\s*)(?:"[^"\r\n]*"|'[^'\r\n]*'|[^\s"',;]+)"#, "${1}[REDACTED]"),
    ].into_iter().filter_map(|(p, r)| Regex::new(p).ok().map(|re| (re,r))).collect()
});

pub fn redact(input: &str) -> String {
    PATTERNS
        .iter()
        .fold(input.to_owned(), |s, (re, replacement)| {
            re.replace_all(&s, *replacement).into_owned()
        })
}

pub struct RedactingWriter<W>(pub W);
impl<W: Write> Write for RedactingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0
            .write_all(redact(&String::from_utf8_lossy(buf)).as_bytes())?;
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}
