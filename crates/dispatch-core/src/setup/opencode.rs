use std::collections::BTreeMap;
use std::fs::{self, Permissions};
use std::io::{ErrorKind, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::{Map, Value};

use super::jsonc::{JsoncDoc, Span, parse_jsonc, strip_jsonc};
use super::{CURRENT, MISSING, OUTDATED, join_errors, lossy};
use crate::Error;
use crate::hook::{create_temp, gojson, parent_dir};

// OPENCODE_FILES is the Dispatch plugin for OpenCode: its server part swaps
// OpenCode's subagents for Dispatch runs while delegation is on, and its
// OpenCode Desktop part shows the switch and the conversation's runs. Sorted
// by name.
const OPENCODE_FILES: [(&str, &[u8]); 4] = [
    ("gui-experimental.cjs", GUI_EXPERIMENTAL_CJS),
    ("package.json", PACKAGE_JSON),
    ("rpc.js", RPC_JS),
    ("server.js", SERVER_JS),
];
const GUI_EXPERIMENTAL_CJS: &[u8] = include_bytes!("opencode/gui-experimental.cjs");
const PACKAGE_JSON: &[u8] = include_bytes!("opencode/package.json");
const RPC_JS: &[u8] = include_bytes!("opencode/rpc.js");
const SERVER_JS: &[u8] = include_bytes!("opencode/server.js");

/// The file in dispatch's state folder that holds the delegation switch; the
/// plugin reads it on each model call.
pub const OPENCODE_SWITCH: &str = "opencode-plugin.json";

/// The Dispatch plugin: its folder, the global OpenCode config that loads it,
/// Current, Outdated (other files or options), or Missing, and whether
/// delegation is on.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct OpenCode {
    pub path: String,
    pub config: String,
    pub state: String,
    pub on: bool,
}

/// The plugin's entry in the global config's plugins list.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OpenCodeEntry {
    pub package: String,
    pub options: BTreeMap<String, String>,
}

impl OpenCodeEntry {
    fn to_json(&self) -> String {
        let options = self
            .options
            .iter()
            .map(|(k, v)| (k.clone(), Value::String(v.clone())))
            .collect::<Map<_, _>>();
        // Field order as the Go struct writes it.
        format!(
            "{{\"package\":{},\"options\":{}}}",
            gojson::marshal(&Value::String(self.package.clone())),
            gojson::marshal(&Value::Object(options))
        )
    }
}

/// Where the plugin is installed: in dispatch's state folder, outside the
/// folders OpenCode loads by itself, so the global config entry is its only
/// load and OpenCode Desktop loads its GUI.
pub fn opencode_plugin_dir(state_dir: &Path) -> PathBuf {
    state_dir.join("opencode-plugin")
}

/// The global OpenCode config: opencode.jsonc when it exists, else
/// opencode.json, under $XDG_CONFIG_HOME or ~/.config.
pub fn opencode_config_path(home: &Path) -> PathBuf {
    let mut base = home.join(".config");
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from)
        && xdg.is_absolute()
    {
        base = xdg;
    }
    let dir = base.join("opencode");
    let jsonc = dir.join("opencode.jsonc");
    if fs::metadata(&jsonc).is_ok() {
        return jsonc;
    }
    dir.join("opencode.json")
}

fn opencode_want(binary: &str, state_dir: &Path) -> OpenCodeEntry {
    let state = lossy(&state_dir.join(OPENCODE_SWITCH));
    let options = BTreeMap::from([
        ("binary".to_string(), binary.to_string()),
        ("state".to_string(), state),
    ]);
    OpenCodeEntry {
        package: lossy(&opencode_plugin_dir(state_dir)),
        options,
    }
}

pub(crate) fn read_opencode(home: &Path, binary: &str, state_dir: &Path) -> OpenCode {
    let mut o = OpenCode {
        path: lossy(&opencode_plugin_dir(state_dir)),
        config: lossy(&opencode_config_path(home)),
        state: MISSING.into(),
        on: opencode_on(state_dir),
    };
    let entry = find_opencode_entry(Path::new(&o.config), &o.path);
    let (files_found, files_current) = opencode_files_state(Path::new(&o.path));
    if entry.is_none() && !files_found {
        return o;
    }
    o.state = OUTDATED.into();
    let want = opencode_want(binary, state_dir);
    if let Some(entry) = entry
        && files_current
        && entry.package == want.package
        && same_options(&entry.options, &want.options)
    {
        o.state = CURRENT.into();
    }
    o
}

fn same_options(a: &BTreeMap<String, String>, b: &BTreeMap<String, String>) -> bool {
    let get = |k: &String| b.get(k).map_or("", String::as_str);
    a.len() == b.len() && a.iter().all(|(k, v)| get(k) == v)
}

fn opencode_files_state(dir: &Path) -> (bool, bool) {
    let mut found = false;
    let mut current = true;
    for (name, want) in OPENCODE_FILES {
        match fs::read(dir.join(name)) {
            Ok(got) => {
                found = true;
                if got != want {
                    current = false;
                }
            }
            Err(_) => current = false,
        }
    }
    (found, current)
}

// field finds a struct field the way Go's encoding/json does: the exact name,
// else a case-insensitive match.
fn field<'a>(obj: &'a Map<String, Value>, name: &str) -> Option<&'a Value> {
    obj.get(name).or_else(|| {
        obj.iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v)
    })
}

fn opencode_on(state_dir: &Path) -> bool {
    let Ok(b) = fs::read(state_dir.join(OPENCODE_SWITCH)) else {
        return true;
    };
    match serde_json::from_slice::<Value>(&b) {
        Ok(Value::Object(body)) => match field(&body, "enabled") {
            Some(Value::Bool(on)) => *on,
            _ => true,
        },
        _ => true,
    }
}

// find_opencode_entry finds the plugins entry that loads dir, as a path string
// or as an object's package.
fn find_opencode_entry(config: &Path, dir: &str) -> Option<OpenCodeEntry> {
    let src = fs::read(config).ok()?;
    let doc = parse_jsonc(&src).ok()?;
    doc.plugins
        .iter()
        .filter_map(|el| decode_opencode_entry(&src[el.start..el.end]))
        .find(|entry| entry.package == dir)
}

fn decode_opencode_entry(raw: &[u8]) -> Option<OpenCodeEntry> {
    let clean = strip_jsonc(raw);
    let value = serde_json::from_slice::<Value>(&clean).ok()?;
    let obj = match value {
        Value::Null => return Some(OpenCodeEntry::default()),
        Value::String(path) => {
            return Some(OpenCodeEntry {
                package: path,
                options: BTreeMap::new(),
            });
        }
        Value::Object(obj) => obj,
        _ => return None,
    };
    let mut entry = OpenCodeEntry::default();
    match field(&obj, "package") {
        None | Some(Value::Null) => {}
        Some(Value::String(p)) => entry.package = p.clone(),
        Some(_) => return None,
    }
    match field(&obj, "options") {
        None | Some(Value::Null) => {}
        Some(Value::Object(options)) => {
            for (k, v) in options {
                let v = match v {
                    Value::String(s) => s.clone(),
                    Value::Null => String::new(),
                    _ => return None,
                };
                entry.options.insert(k.clone(), v);
            }
        }
        Some(_) => return None,
    }
    if entry.package.is_empty() {
        return None;
    }
    Some(entry)
}

/// Writes the plugin over any older copy and loads it from the global
/// OpenCode config, which OpenCode reloads by itself. It returns the plugin
/// folder.
pub fn install_opencode_plugin(
    home: &Path,
    binary: &str,
    state_dir: &Path,
) -> Result<PathBuf, Error> {
    let dir = opencode_plugin_dir(state_dir);
    fs::create_dir_all(&dir)?;
    for (name, body) in OPENCODE_FILES {
        write_atomic(&dir.join(name), body)?;
    }
    let entry = opencode_want(binary, state_dir).to_json();
    edit_opencode_config(
        &opencode_config_path(home),
        &lossy(&dir),
        Some(entry.as_bytes()),
    )?;
    Ok(dir)
}

/// Drops the plugin's config entry, its folder, and the switch.
pub fn remove_opencode_plugin(home: &Path, state_dir: &Path) -> Result<(), Error> {
    let dir = opencode_plugin_dir(state_dir);
    let mut errs = Vec::new();
    if let Err(e) = edit_opencode_config(&opencode_config_path(home), &lossy(&dir), None) {
        errs.push(e);
    }
    if let Err(e) = remove_all(&dir) {
        errs.push(e.into());
    }
    if let Err(e) = fs::remove_file(state_dir.join(OPENCODE_SWITCH))
        && e.kind() != ErrorKind::NotFound
    {
        errs.push(e.into());
    }
    join_errors(errs)
}

// remove_all is Go's os.RemoveAll: a missing path is no error.
fn remove_all(path: &Path) -> std::io::Result<()> {
    let result = match fs::symlink_metadata(path) {
        Ok(info) if info.is_dir() => fs::remove_dir_all(path),
        Ok(_) => fs::remove_file(path),
        Err(e) => Err(e),
    };
    match result {
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

/// Switches delegation through Dispatch on or off for every OpenCode session.
pub fn set_opencode_delegation(state_dir: &Path, on: bool) -> Result<(), Error> {
    let body = format!("{{\n  \"enabled\": {on}\n}}\n");
    fs::create_dir_all(state_dir)?;
    write_atomic(&state_dir.join(OPENCODE_SWITCH), body.as_bytes())
}

/// Puts entry in the config's plugins list in place of the one that loads
/// dir, or adds it; a None entry removes that one. It edits only that list,
/// so the rest of the file, comments included, stays as it was.
pub fn edit_opencode_config(config: &Path, dir: &str, entry: Option<&[u8]>) -> Result<(), Error> {
    // A linked config, such as one from a dotfiles repository, is edited where it lives.
    let config = match fs::canonicalize(config) {
        Ok(resolved) => resolved,
        Err(_) => config.to_path_buf(),
    };
    let src = match fs::read(&config) {
        Ok(src) => src,
        Err(e) if e.kind() == ErrorKind::NotFound => {
            if entry.is_none() {
                return Ok(());
            }
            b"{\n  \"$schema\": \"https://opencode.ai/config.json\"\n}\n".to_vec()
        }
        Err(e) => return Err(e.into()),
    };
    let doc = match parse_jsonc(&src) {
        Ok(doc) => doc,
        Err(e) => return Err(Error::Message(format!("{}: {e}", config.display()))),
    };
    let loads_dir = |el: &Span| {
        let entry = decode_opencode_entry(&src[el.start..el.end]);
        entry.is_some_and(|e| e.package == dir)
    };
    let at = doc.plugins.iter().position(loads_dir);
    let out = match (entry, at) {
        (None, None) => return Ok(()),
        (None, Some(i)) => remove_element(&src, &doc, i),
        (Some(entry), Some(i)) => splice(&src, doc.plugins[i].start, doc.plugins[i].end, entry),
        (Some(entry), None) => match doc.plugins_array {
            None if doc.plugins_key => {
                return Err(Error::Message(format!(
                    "{}: \"plugins\" is not a list",
                    config.display()
                )));
            }
            None => {
                let mut member = b"\n  \"plugins\": [\n    ".to_vec();
                member.extend_from_slice(entry);
                member.extend_from_slice(b"\n  ]");
                member.push(if doc.members > 0 { b',' } else { b'\n' });
                splice(&src, doc.open + 1, doc.open + 1, &member)
            }
            Some(arr) if doc.plugins.is_empty() => {
                let mut inner = b"\n    ".to_vec();
                inner.extend_from_slice(entry);
                inner.extend_from_slice(b"\n  ");
                splice(&src, arr.start + 1, arr.end - 1, &inner)
            }
            Some(_) => {
                let last = doc.plugins[doc.plugins.len() - 1];
                let mut with = b",\n    ".to_vec();
                with.extend_from_slice(entry);
                splice(&src, last.end, last.end, &with)
            }
        },
    };
    fs::create_dir_all(parent_dir(&config))?;
    write_atomic(&config, &out)
}

fn splice(src: &[u8], start: usize, end: usize, with: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(src.len() - (end - start) + with.len());
    out.extend_from_slice(&src[..start]);
    out.extend_from_slice(with);
    out.extend_from_slice(&src[end..]);
    out
}

// remove_element cuts plugins element i with the comma that joined it to its
// neighbor and the space before it.
fn remove_element(src: &[u8], doc: &JsoncDoc, i: usize) -> Vec<u8> {
    let els = &doc.plugins;
    let Span { mut start, mut end } = els[i];
    if i + 1 < els.len() {
        end = els[i + 1].start;
    } else if i > 0 {
        start = els[i - 1].end;
    } else if let Some(arr) = doc.plugins_array {
        (start, end) = (arr.start + 1, arr.end - 1);
    }
    splice(src, start, end, &[])
}

// write_atomic replaces path through a temporary file of its own beside it, so
// the app and the CLI writing at once never share one, and keeps the mode the
// file had (a user's 0600 config stays 0600).
fn write_atomic(path: &Path, body: &[u8]) -> Result<(), Error> {
    let mode = fs::metadata(path)
        .map(|info| info.permissions().mode() & 0o777)
        .unwrap_or(0o644);
    let base = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let (tmp_path, mut tmp) = create_temp(parent_dir(path), &format!(".{base}."), ".tmp")?;
    let mut errs: Vec<Error> = Vec::new();
    if let Err(e) = tmp.write_all(body) {
        errs.push(e.into());
    }
    if let Err(e) = tmp.set_permissions(Permissions::from_mode(mode)) {
        errs.push(e.into());
    }
    drop(tmp);
    let mut result = join_errors(errs);
    if result.is_ok() {
        result = fs::rename(&tmp_path, path).map_err(Error::from);
    }
    if result.is_err() {
        let _ = fs::remove_file(&tmp_path);
    }
    result
}
