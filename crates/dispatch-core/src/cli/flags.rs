//! Command-line flags read the way Go's flag package reads them, so every
//! invocation agents and Polterware already use keeps working: `-name` or
//! `--name`, `=value` or the next argument, `-name=false` for a bool, and
//! `--` to end the flags.

use std::collections::BTreeMap;
use std::time::Duration;

#[derive(Clone, Debug)]
enum Value {
    Str(String),
    Bool(bool),
    Int(i64),
    Dur(Duration),
}

struct Def {
    usage: &'static str,
    default: Value,
}

/// A set of flags for one command.
pub(crate) struct Flags {
    name: String,
    defs: BTreeMap<&'static str, Def>,
    values: BTreeMap<&'static str, Value>,
}

/// Why the arguments could not be read; the message is Go's.
#[derive(Debug)]
pub(crate) enum FlagError {
    Help,
    Bad(String),
}

impl std::fmt::Display for FlagError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FlagError::Help => f.write_str("flag: help requested"),
            FlagError::Bad(msg) => f.write_str(msg),
        }
    }
}

impl Flags {
    pub(crate) fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            defs: BTreeMap::new(),
            values: BTreeMap::new(),
        }
    }

    fn def(&mut self, name: &'static str, default: Value, usage: &'static str) -> &mut Self {
        self.values.insert(name, default.clone());
        self.defs.insert(name, Def { usage, default });
        self
    }

    pub(crate) fn string(
        &mut self,
        name: &'static str,
        default: &str,
        usage: &'static str,
    ) -> &mut Self {
        self.def(name, Value::Str(default.into()), usage)
    }

    pub(crate) fn bool(
        &mut self,
        name: &'static str,
        default: bool,
        usage: &'static str,
    ) -> &mut Self {
        self.def(name, Value::Bool(default), usage)
    }

    pub(crate) fn int(
        &mut self,
        name: &'static str,
        default: i64,
        usage: &'static str,
    ) -> &mut Self {
        self.def(name, Value::Int(default), usage)
    }

    pub(crate) fn duration(
        &mut self,
        name: &'static str,
        default: Duration,
        usage: &'static str,
    ) -> &mut Self {
        self.def(name, Value::Dur(default), usage)
    }

    pub(crate) fn str(&self, name: &str) -> String {
        match self.values.get(name) {
            Some(Value::Str(s)) => s.clone(),
            _ => String::new(),
        }
    }

    pub(crate) fn flag(&self, name: &str) -> bool {
        matches!(self.values.get(name), Some(Value::Bool(true)))
    }

    pub(crate) fn int_value(&self, name: &str) -> i64 {
        match self.values.get(name) {
            Some(Value::Int(n)) => *n,
            _ => 0,
        }
    }

    pub(crate) fn dur(&self, name: &str) -> Duration {
        match self.values.get(name) {
            Some(Value::Dur(d)) => *d,
            _ => Duration::ZERO,
        }
    }

    /// Reads flags up to the first positional argument and returns the rest.
    pub(crate) fn parse(&mut self, args: &[String]) -> Result<Vec<String>, FlagError> {
        let r = self.parse_inner(args);
        if let Err(e) = &r {
            if let FlagError::Bad(msg) = e {
                eprintln!("{msg}");
            }
            self.print_usage();
        }
        r
    }

    /// Reads flags anywhere among the positional arguments.
    pub(crate) fn parse_interspersed(&mut self, args: &[String]) -> Result<Vec<String>, FlagError> {
        let mut positional = Vec::new();
        let mut args = args.to_vec();
        loop {
            let rest = self.parse(&args)?;
            let Some((first, tail)) = rest.split_first() else {
                return Ok(positional);
            };
            positional.push(first.clone());
            args = tail.to_vec();
        }
    }

    /// Reads flags like [`Flags::parse`] without printing anything on error.
    pub(crate) fn parse_silent(&mut self, args: &[String]) -> Result<Vec<String>, FlagError> {
        self.parse_inner(args)
    }

    fn parse_inner(&mut self, args: &[String]) -> Result<Vec<String>, FlagError> {
        let mut i = 0;
        while i < args.len() {
            let s = &args[i];
            if s.len() < 2 || !s.starts_with('-') {
                break;
            }
            let mut minuses = 1;
            if s.as_bytes()[1] == b'-' {
                minuses = 2;
                if s.len() == 2 {
                    i += 1;
                    break;
                }
            }
            let name = &s[minuses..];
            if name.is_empty() || name.starts_with('-') || name.starts_with('=') {
                return Err(FlagError::Bad(format!("bad flag syntax: {s}")));
            }
            i += 1;
            let (name, value) = match name.split_once('=') {
                Some((n, v)) => (n, Some(v.to_owned())),
                None => (name, None),
            };
            let Some((&key, def)) = self.defs.get_key_value(name) else {
                if name == "help" || name == "h" {
                    return Err(FlagError::Help);
                }
                return Err(FlagError::Bad(format!(
                    "flag provided but not defined: -{name}"
                )));
            };
            let parsed = match &def.default {
                Value::Bool(_) => match value {
                    None => Value::Bool(true),
                    Some(v) => Value::Bool(parse_bool(&v).ok_or_else(|| {
                        FlagError::Bad(format!(
                            "invalid boolean value {v:?} for -{name}: parse error"
                        ))
                    })?),
                },
                other => {
                    let v = match value {
                        Some(v) => v,
                        None if i < args.len() => {
                            i += 1;
                            args[i - 1].clone()
                        }
                        None => {
                            return Err(FlagError::Bad(format!("flag needs an argument: -{name}")));
                        }
                    };
                    let invalid = || {
                        FlagError::Bad(format!("invalid value {v:?} for flag -{name}: parse error"))
                    };
                    match other {
                        Value::Str(_) => Value::Str(v.clone()),
                        Value::Int(_) => Value::Int(v.parse().map_err(|_| invalid())?),
                        Value::Dur(_) => Value::Dur(parse_duration(&v).ok_or_else(invalid)?),
                        Value::Bool(_) => unreachable!(),
                    }
                }
            };
            self.values.insert(key, parsed);
        }
        Ok(args[i..].to_vec())
    }

    fn print_usage(&self) {
        eprintln!("Usage of {}:", self.name);
        for (name, def) in &self.defs {
            let (kind, default) = match &def.default {
                Value::Str(s) => ("string", (!s.is_empty()).then(|| format!("{s:?}"))),
                Value::Bool(b) => ("", b.then(|| "true".to_owned())),
                Value::Int(n) => ("int", (*n != 0).then(|| n.to_string())),
                Value::Dur(d) => ("duration", (!d.is_zero()).then(|| format_duration(*d))),
            };
            let head = if kind.is_empty() {
                format!("  -{name}")
            } else {
                format!("  -{name} {kind}")
            };
            let mut line = format!("{head}\n    \t{}", def.usage);
            if let Some(d) = default {
                line.push_str(&format!(" (default {d})"));
            }
            eprintln!("{line}");
        }
    }
}

fn parse_bool(s: &str) -> Option<bool> {
    match s {
        "1" | "t" | "T" | "TRUE" | "true" | "True" => Some(true),
        "0" | "f" | "F" | "FALSE" | "false" | "False" => Some(false),
        _ => None,
    }
}

/// Reads a duration as Go's time.ParseDuration does: "90s", "1h30m", "1.5h".
/// A negative one is read as zero, which every caller treats as unset.
pub(crate) fn parse_duration(s: &str) -> Option<Duration> {
    let (neg, mut rest) = match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    if rest == "0" {
        return Some(Duration::ZERO);
    }
    if rest.is_empty() {
        return None;
    }
    let mut total = 0f64;
    while !rest.is_empty() {
        let num_end = rest
            .find(|c: char| !(c.is_ascii_digit() || c == '.'))
            .unwrap_or(rest.len());
        let num = &rest[..num_end];
        if num.is_empty() || num == "." {
            return None;
        }
        let value: f64 = num.parse().ok()?;
        rest = &rest[num_end..];
        let unit_end = rest
            .find(|c: char| c.is_ascii_digit() || c == '.')
            .unwrap_or(rest.len());
        let nanos = match &rest[..unit_end] {
            "ns" => 1.0,
            "us" | "µs" | "μs" => 1e3,
            "ms" => 1e6,
            "s" => 1e9,
            "m" => 60e9,
            "h" => 3600e9,
            _ => return None,
        };
        rest = &rest[unit_end..];
        total += value * nanos;
    }
    if neg {
        return Some(Duration::ZERO);
    }
    Some(Duration::from_nanos(total.round() as u64))
}

/// Writes a duration as Go's time.Duration.String does: "1h2m3.4s", "500ms".
pub(crate) fn format_duration(d: Duration) -> String {
    let nanos = d.as_nanos();
    if nanos == 0 {
        return "0s".into();
    }
    if nanos < 1_000_000_000 {
        let (unit, div) = if nanos < 1_000 {
            ("ns", 1u128)
        } else if nanos < 1_000_000 {
            ("µs", 1_000)
        } else {
            ("ms", 1_000_000)
        };
        return format!("{}{unit}", trim_fraction(nanos, div));
    }
    let secs = nanos / 1_000_000_000;
    let h = secs / 3600;
    let m = (secs / 60) % 60;
    let s = trim_fraction(nanos % 60_000_000_000, 1_000_000_000);
    if h > 0 {
        format!("{h}h{m}m{s}s")
    } else if m > 0 {
        format!("{m}m{s}s")
    } else {
        format!("{s}s")
    }
}

fn trim_fraction(value: u128, div: u128) -> String {
    let whole = value / div;
    let frac = value % div;
    if frac == 0 {
        return whole.to_string();
    }
    let width = div.ilog10() as usize;
    let digits = format!("{frac:0width$}");
    format!("{whole}.{}", digits.trim_end_matches('0'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn reads_go_style_flags_among_positionals() {
        let mut f = Flags::new("t");
        f.string("tier", "", "")
            .bool("json", false, "")
            .int("limit", 20, "")
            .duration("timeout", Duration::ZERO, "");
        let pos = f
            .parse_interspersed(&args(&[
                "fix",
                "--tier",
                "easy",
                "-json",
                "it",
                "-limit=5",
                "--timeout=1m30s",
            ]))
            .unwrap();
        assert_eq!(pos, ["fix", "it"]);
        assert_eq!(f.str("tier"), "easy");
        assert!(f.flag("json"));
        assert_eq!(f.int_value("limit"), 5);
        assert_eq!(f.dur("timeout"), Duration::from_secs(90));
    }

    #[test]
    fn bool_takes_only_an_equals_value() {
        let mut f = Flags::new("t");
        f.bool("json", true, "");
        let pos = f
            .parse_interspersed(&args(&["--json=false", "id"]))
            .unwrap();
        assert_eq!(pos, ["id"]);
        assert!(!f.flag("json"));
    }

    #[test]
    fn rejects_unknown_flags() {
        let mut f = Flags::new("t");
        assert!(
            matches!(f.parse(&args(&["-x"])), Err(FlagError::Bad(m)) if m == "flag provided but not defined: -x")
        );
    }

    #[test]
    fn durations_round_trip_like_go() {
        assert_eq!(parse_duration("1.5h"), Some(Duration::from_secs(5400)));
        assert_eq!(parse_duration("300ms"), Some(Duration::from_millis(300)));
        assert_eq!(parse_duration("5"), None);
        assert_eq!(format_duration(Duration::from_millis(1500)), "1.5s");
        assert_eq!(format_duration(Duration::from_millis(3_605_000)), "1h0m5s");
        assert_eq!(format_duration(Duration::from_millis(65_000)), "1m5s");
        assert_eq!(format_duration(Duration::from_millis(250)), "250ms");
    }
}
