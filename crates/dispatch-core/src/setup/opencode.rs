//! The one-time cleanup of the plugin an earlier Dispatch installed in
//! OpenCode. OpenCode reaches Dispatch through the `dispatch` command and the
//! skill, like every other agent, so what an earlier version installed is
//! removed: its `plugins` entry in OpenCode's global config, its folder in
//! dispatch's state folder, and its delegation switch.

use std::fs::{self, Permissions};
use std::io::{ErrorKind, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::jsonc::{Span, parse_jsonc, strip_jsonc};
use super::{join_errors, lossy};
use crate::Error;
use crate::hook::{create_temp, parent_dir};

/// The switch file the plugin read on each model call, beside its folder.
const OPENCODE_SWITCH: &str = "opencode-plugin.json";

/// The folder an earlier Dispatch installed its OpenCode plugin into, inside
/// dispatch's state folder; the global config's `plugins` entry pointed here.
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

/// Removes the plugin an earlier Dispatch installed in OpenCode, when it is
/// still there: the global config's `plugins` entry that loads the
/// `opencode-plugin` folder of dispatch's state folder, that folder, and its
/// switch. Only that entry goes, so the rest of the file, comments included,
/// stays as it was. Answers whether anything was there.
pub fn remove_legacy_opencode_plugin(home: &Path, state_dir: &Path) -> Result<bool, Error> {
    let dir = lossy(&opencode_plugin_dir(state_dir));
    let entry = remove_legacy_entry(&opencode_config_path(home), &dir)?;
    let folder = remove_all(&opencode_plugin_dir(state_dir))?;
    let switch = match fs::remove_file(state_dir.join(OPENCODE_SWITCH)) {
        Ok(()) => true,
        Err(e) if e.kind() == ErrorKind::NotFound => false,
        Err(e) => return Err(e.into()),
    };
    Ok(entry || folder || switch)
}

// remove_legacy_entry drops the plugins entry that loads dir from config.
fn remove_legacy_entry(config: &Path, dir: &str) -> Result<bool, Error> {
    // A linked config, such as one from a dotfiles repository, is edited where it lives.
    let config = match fs::canonicalize(config) {
        Ok(resolved) => resolved,
        Err(_) => config.to_path_buf(),
    };
    let src = match fs::read(&config) {
        Ok(src) => src,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e.into()),
    };
    let doc = match parse_jsonc(&src) {
        Ok(doc) => doc,
        Err(e) => return Err(Error::Message(format!("{}: {e}", config.display()))),
    };
    let loads_dir = |el: &Span| {
        let raw = &src[el.start..el.end];
        entry_path(raw).is_some_and(|package| package == dir)
    };
    let Some(at) = doc.plugins.iter().position(loads_dir) else {
        return Ok(false);
    };
    write_atomic(&config, &remove_element(&src, &doc, at))?;
    Ok(true)
}

// entry_path reads a plugins element as the package it loads: a path string,
// or the package of an object entry.
fn entry_path(raw: &[u8]) -> Option<String> {
    let value = serde_json::from_slice::<Value>(&strip_jsonc(raw)).ok()?;
    match value {
        Value::String(path) => Some(path),
        Value::Object(obj) => obj
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("package"))
            .and_then(|(_, v)| v.as_str())
            .map(str::to_string),
        _ => None,
    }
}

// remove_all is Go's os.RemoveAll: a missing path is no error, and it answers
// whether there was one.
fn remove_all(path: &Path) -> std::io::Result<bool> {
    let result = match fs::symlink_metadata(path) {
        Ok(info) if info.is_dir() => fs::remove_dir_all(path),
        Ok(_) => fs::remove_file(path),
        Err(e) => Err(e),
    };
    match result {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

// remove_element cuts plugins element i with the comma that joined it to its
// neighbor and the space before it.
fn remove_element(src: &[u8], doc: &super::jsonc::JsoncDoc, i: usize) -> Vec<u8> {
    let els = &doc.plugins;
    let Span { mut start, mut end } = els[i];
    if i + 1 < els.len() {
        end = els[i + 1].start;
    } else if i > 0 {
        start = els[i - 1].end;
    } else if let Some(arr) = doc.plugins_array {
        (start, end) = (arr.start + 1, arr.end - 1);
    }
    let mut out = Vec::with_capacity(src.len() - (end - start));
    out.extend_from_slice(&src[..start]);
    out.extend_from_slice(&src[end..]);
    out
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
