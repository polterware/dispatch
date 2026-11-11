//! The images a run makes: its folder under the data directory's `artifacts`, the images it
//! wrote there or changed in its worktree, and the copies a gallery reads.

use crate::{Error, types};
use std::io::Read;
use std::path::{Path, PathBuf};

/// The folder under the data directory that holds one folder per run.
pub const DIR: &str = "artifacts";
/// The environment variable naming a run's folder to its agent.
pub const ARTIFACTS_ENV: &str = "DISPATCH_ARTIFACTS_DIR";
/// Where a changed file's copy goes, inside the run's folder.
const CHANGED_DIR: &str = "worktree";
/// How much of an image is read to find its dimensions: enough for a JPEG's markers and any
/// extension segment before them.
const HEADER_BYTES: u64 = 1024 * 1024;
const EXTERNAL_MAX_DEPTH: usize = 6;
const EXTERNAL_MAX_ENTRIES: usize = 20_000;

/// A run's artifacts folder, whether or not it exists yet.
pub fn run_dir(base: &Path, run_id: &str) -> PathBuf {
    base.join(run_id)
}

/// Creates `run`'s folder, for the agent to write images into.
pub fn prepare(base: &Path, run_id: &str) -> Option<PathBuf> {
    if base.as_os_str().is_empty() || run_id.is_empty() {
        return None;
    }
    let dir = run_dir(base, run_id);
    std::fs::create_dir_all(&dir).ok().map(|()| dir)
}

/// What a run whose image mode is on is told about its folder; nothing otherwise.
pub fn brief_note(images: bool, dir: &Path) -> String {
    if !images || dir.as_os_str().is_empty() {
        return String::new();
    }
    format!(
        "\n\nWrite any images you make in {}, with a name that says what each is: Dispatch collects them into the run's gallery.",
        dir.display()
    )
}

/// An image's kind and pixel size, read from its header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Image {
    /// png, jpeg, webp, or gif.
    pub kind: &'static str,
    pub width: i32,
    pub height: i32,
}

impl Image {
    fn new(kind: &'static str, width: u32, height: u32) -> Self {
        Self {
            kind,
            width: i32::try_from(width).unwrap_or(i32::MAX),
            height: i32::try_from(height).unwrap_or(i32::MAX),
        }
    }
}

fn u16_le(b: &[u8]) -> u32 {
    u32::from(b[0]) | u32::from(b[1]) << 8
}

fn u16_be(b: &[u8]) -> u32 {
    u32::from(b[1]) | u32::from(b[0]) << 8
}

fn u24_le(b: &[u8]) -> u32 {
    u32::from(b[0]) | u32::from(b[1]) << 8 | u32::from(b[2]) << 16
}

fn u32_be(b: &[u8]) -> u32 {
    u32::from(b[3]) | u32::from(b[2]) << 8 | u32::from(b[1]) << 16 | u32::from(b[0]) << 24
}

/// Reads a file's first bytes and returns the image it is, by its magic bytes.
pub fn image(path: impl AsRef<Path>) -> Option<Image> {
    let file = std::fs::File::open(path.as_ref()).ok()?;
    let mut head = Vec::new();
    file.take(HEADER_BYTES).read_to_end(&mut head).ok()?;
    header(&head)
}

/// The image `bytes` start with, if they are one: png, jpeg, webp, or gif.
pub fn header(bytes: &[u8]) -> Option<Image> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return png(bytes);
    }
    if bytes.starts_with(&[0xFF, 0xD8]) {
        return jpeg(bytes);
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return gif(bytes);
    }
    if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return webp(bytes);
    }
    None
}

fn png(b: &[u8]) -> Option<Image> {
    // IHDR always comes first: its length, the type, the width, and the height.
    if b.len() < 24 || &b[12..16] != b"IHDR" {
        return None;
    }
    Some(Image::new("png", u32_be(&b[16..20]), u32_be(&b[20..24])))
}

fn gif(b: &[u8]) -> Option<Image> {
    if b.len() < 10 {
        return None;
    }
    Some(Image::new("gif", u16_le(&b[6..8]), u16_le(&b[8..10])))
}

fn webp(b: &[u8]) -> Option<Image> {
    let chunk = b.get(12..16)?;
    match chunk {
        // Extended format: the canvas size, one less than the pixel size, is 24 bits each.
        b"VP8X" if b.len() >= 30 => Some(Image::new(
            "webp",
            u24_le(&b[24..27]) + 1,
            u24_le(&b[27..30]) + 1,
        )),
        // Lossy: a frame tag and the start code, then 14-bit width and height.
        b"VP8 " if b.len() >= 30 && b[23..26] == [0x9D, 0x01, 0x2A] => Some(Image::new(
            "webp",
            u16_le(&b[26..28]) & 0x3FFF,
            u16_le(&b[28..30]) & 0x3FFF,
        )),
        // Lossless: a signature byte, then 14-bit width and height.
        b"VP8L" if b.len() >= 25 && b[20] == 0x2F => {
            let w = u32::from(b[21]) | u32::from(b[22] & 0x3F) << 8;
            let h = u32::from(b[23]) << 2
                | u32::from(b[22] & 0xC0) >> 6
                | u32::from(b[24] & 0x0F) << 10;
            Some(Image::new("webp", w + 1, h + 1))
        }
        _ => None,
    }
}

/// A JPEG's size is in whichever frame header comes after its other segments.
fn jpeg(b: &[u8]) -> Option<Image> {
    let mut i = 2;
    while i + 4 <= b.len() {
        if b[i] != 0xFF {
            i += 1;
            continue;
        }
        let marker = b[i + 1];
        // Fill bytes and markers with no length field.
        if marker == 0xFF {
            i += 1;
            continue;
        }
        if marker == 0x01 || (0xD0..=0xD8).contains(&marker) {
            i += 2;
            continue;
        }
        if marker == 0xD9 || marker == 0xDA {
            return None;
        }
        let len = u16_be(&b[i + 2..i + 4]);
        let frame = (0xC0..=0xCF).contains(&marker) && !matches!(marker, 0xC4 | 0xC8 | 0xCC);
        if frame {
            if i + 9 > b.len() || len < 7 {
                return None;
            }
            return Some(Image::new(
                "jpeg",
                u16_be(&b[i + 7..i + 9]),
                u16_be(&b[i + 5..i + 7]),
            ));
        }
        i += 2 + len as usize;
    }
    None
}

/// Whether a path's name ends in an image's extension; its header still decides.
fn is_image_name(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    [".png", ".jpg", ".jpeg", ".webp", ".gif"]
        .iter()
        .any(|ext| lower.ends_with(ext))
}

/// A relative path inside a run's folder: not absolute and never climbing out of it.
fn safe_rel(rel: &str) -> bool {
    !rel.is_empty()
        && !rel.starts_with('/')
        && rel
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}

/// Where an artifact's copy lives: `<base>/<run id>/<rel path>`, when the parts are safe.
pub fn copy_path(base: &Path, run_id: &str, rel: &str) -> Option<PathBuf> {
    let run = run_id.trim();
    if run.is_empty() || !run.bytes().all(|b| b.is_ascii_alphanumeric()) || !safe_rel(rel) {
        return None;
    }
    Some(base.join(run).join(rel))
}

/// Removes a run's whole artifacts folder, for a run deleted for good.
pub fn remove_run_dir(base: &Path, run_id: &str) {
    let run = run_id.trim();
    if base.as_os_str().is_empty()
        || run.is_empty()
        || !run.bytes().all(|b| b.is_ascii_alphanumeric())
    {
        return;
    }
    let _ = std::fs::remove_dir_all(base.join(run));
}

/// Indexes the images `run` made: what it wrote into its folder, and the images among the
/// files it changed in its worktree or, for image runs outside git, its edited folder. Answers
/// how many rows were added.
pub fn index_run(store: &dyn types::Store, base: &Path, run: &types::Run) -> Result<usize, Error> {
    if base.as_os_str().is_empty() || run.id.is_empty() {
        return Ok(0);
    }
    let dir = run_dir(base, &run.id);
    let mut added = 0;
    if dir.is_dir() {
        added += index_folder(store, run, &dir, "");
    }
    let folder = changed_folder(run);
    if !folder.is_empty() && !run.base_sha.is_empty() {
        added += index_changed(store, run, &dir, &folder);
    } else if run.images && !folder.is_empty() && run.base_sha.is_empty() {
        added += index_external(store, run, &dir, Path::new(&folder));
    }
    if added == 0 {
        // Nothing to keep: the folder a run was handed goes with the empty run.
        let _ = std::fs::remove_dir(&dir);
    }
    Ok(added)
}

fn index_external(store: &dyn types::Store, run: &types::Run, dest: &Path, root: &Path) -> usize {
    let mut walk = ExternalWalk {
        root,
        start: parse_run_start(run),
        visited: 0,
    };
    index_external_at(store, run, dest, root, 0, &mut walk)
}

fn parse_run_start(run: &types::Run) -> Option<std::time::SystemTime> {
    use std::time::SystemTime;
    [run.started_at.as_str(), run.created_at.as_str()]
        .into_iter()
        .filter(|value| !value.trim().is_empty())
        .find_map(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
        .and_then(|time| {
            let secs = time.timestamp();
            let nanos = time.timestamp_subsec_nanos();
            if secs < 0 {
                None
            } else {
                SystemTime::UNIX_EPOCH.checked_add(std::time::Duration::new(secs as u64, nanos))
            }
        })
}

struct ExternalWalk<'a> {
    root: &'a Path,
    start: Option<std::time::SystemTime>,
    visited: usize,
}

fn index_external_at(
    store: &dyn types::Store,
    run: &types::Run,
    dest: &Path,
    folder: &Path,
    depth: usize,
    walk: &mut ExternalWalk<'_>,
) -> usize {
    if depth > EXTERNAL_MAX_DEPTH || walk.visited >= EXTERNAL_MAX_ENTRIES {
        return 0;
    }
    let Ok(entries) = std::fs::read_dir(folder) else {
        return 0;
    };
    let mut added = 0;
    for entry in entries.filter_map(Result::ok) {
        if walk.visited >= EXTERNAL_MAX_ENTRIES {
            break;
        }
        walk.visited += 1;
        let name = entry.file_name().to_string_lossy().into_owned();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
            if !name.starts_with('.') && !matches!(name.as_str(), "node_modules" | "target") {
                added += index_external_at(store, run, dest, &entry.path(), depth + 1, walk);
            }
            continue;
        }
        if !kind.is_file() || !is_image_name(&name) {
            continue;
        }
        let path = entry.path();
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if walk
            .start
            .is_some_and(|start| metadata.modified().map_or(true, |mtime| mtime < start))
        {
            continue;
        }
        let Ok(rel) = path.strip_prefix(walk.root) else {
            continue;
        };
        let rel = rel.to_string_lossy().replace('\\', "/");
        let artifact_rel = format!("{CHANGED_DIR}/{rel}");
        if !safe_rel(&artifact_rel) {
            continue;
        }
        let copy = dest.join(&artifact_rel);
        if std::fs::create_dir_all(copy.parent().unwrap_or(dest)).is_err()
            || image(&path).is_none()
            || std::fs::copy(&path, &copy).is_err()
        {
            continue;
        }
        if save(store, run, &copy, &artifact_rel).is_ok() {
            added += 1;
        }
    }
    added
}

/// The folder whose changed files belong to `run`: its worktree, else the folder it edited.
fn changed_folder(run: &types::Run) -> String {
    if !run.worktree.is_empty() {
        return run.worktree.clone();
    }
    run.dir.clone()
}

fn index_folder(store: &dyn types::Store, run: &types::Run, root: &Path, rel: &str) -> usize {
    let folder = if rel.is_empty() {
        root.to_path_buf()
    } else {
        root.join(rel)
    };
    let Ok(entries) = std::fs::read_dir(&folder) else {
        return 0;
    };
    let mut added = 0;
    for entry in entries.filter_map(Result::ok) {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let name = entry.file_name().to_string_lossy().into_owned();
        let child = if rel.is_empty() {
            name.clone()
        } else {
            format!("{rel}/{name}")
        };
        // A link could point anywhere; only the run's own files count.
        if kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
            if name != ".git" {
                added += index_folder(store, run, root, &child);
            }
            continue;
        }
        if !kind.is_file() || !is_image_name(&name) || !safe_rel(&child) {
            continue;
        }
        let path = root.join(&child);
        if save(store, run, &path, &child).is_ok() {
            added += 1;
        }
    }
    added
}

fn index_changed(store: &dyn types::Store, run: &types::Run, dir: &Path, folder: &str) -> usize {
    let Ok(files) = crate::worktree::file_changes(folder, &run.base_sha) else {
        return 0;
    };
    let mut added = 0;
    for file in files {
        if !is_image_name(&file.path) {
            continue;
        }
        let rel = format!("{CHANGED_DIR}/{}", file.path);
        if !safe_rel(&rel) {
            continue;
        }
        let source = Path::new(folder).join(&file.path);
        let copy = dir.join(&rel);
        if std::fs::create_dir_all(dir).is_err() {
            return added;
        }
        let Some(parent) = copy.parent() else {
            continue;
        };
        if std::fs::create_dir_all(parent).is_err() {
            continue;
        }
        // A changed file that is not an image by its bytes is not copied at all.
        if image(&source).is_none() || std::fs::copy(&source, &copy).is_err() {
            continue;
        }
        if save(store, run, &copy, &rel).is_ok() {
            added += 1;
        }
    }
    added
}

/// Stores one copied image, read from `path`, as `rel` in the run's folder.
fn save(store: &dyn types::Store, run: &types::Run, path: &Path, rel: &str) -> Result<(), Error> {
    let Some(image) = image(path) else {
        return Err(Error::Message(format!(
            "{} is not an image Dispatch knows",
            path.display()
        )));
    };
    let bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let repo = if run.repo.is_empty() {
        run.dir.clone()
    } else {
        run.repo.clone()
    };
    let created = if run.finished_at.is_empty() {
        chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Nanos, true)
    } else {
        run.finished_at.clone()
    };
    store.save_artifact(&types::Artifact {
        id: types::new_id(),
        run_id: run.id.clone(),
        project_id: run.project_id.clone(),
        conversation_id: run.conversation_id.clone(),
        caller_agent: run.caller_agent.clone(),
        agent: run.agent.clone(),
        model: run.model.clone(),
        repo,
        rel_path: rel.to_owned(),
        kind: image.kind.to_owned(),
        bytes: i64::try_from(bytes).unwrap_or(i64::MAX),
        width: image.width,
        height: image.height,
        created_at: created,
        prompt: String::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct ArtifactStore(Mutex<Vec<types::Artifact>>);

    impl types::Store for ArtifactStore {
        fn save_run(&self, _: &types::Run) -> Result<(), Error> {
            Ok(())
        }
        fn get_run(&self, _: &str) -> Result<types::Run, Error> {
            Err(Error::Message("missing".into()))
        }
        fn list_runs(&self, _: &str, _: i64, _: i64, _: &str) -> Result<Vec<types::Run>, Error> {
            Ok(Vec::new())
        }
        fn append_event(&self, _: &types::RunEvent) -> Result<(), Error> {
            Ok(())
        }
        fn list_events(&self, _: &str, _: i64, _: i64) -> Result<Vec<types::RunEvent>, Error> {
            Ok(Vec::new())
        }
        fn list_history(
            &self,
            _: &str,
            _: &str,
            _: &str,
            _: i64,
            _: i64,
        ) -> Result<Vec<types::RunEvent>, Error> {
            Ok(Vec::new())
        }
        fn save_worktree(&self, _: &types::WorktreeInfo) -> Result<(), Error> {
            Ok(())
        }
        fn list_worktrees(&self, _: &str) -> Result<Vec<types::WorktreeInfo>, Error> {
            Ok(Vec::new())
        }
        fn save_quota(&self, _: &types::QuotaSnapshot) -> Result<(), Error> {
            Ok(())
        }
        fn list_quota(&self, _: &str, _: &str) -> Result<Vec<types::QuotaSnapshot>, Error> {
            Ok(Vec::new())
        }
        fn list_tiers(&self) -> Result<Vec<types::Tier>, Error> {
            Ok(Vec::new())
        }
        fn replace_tiers(&self, _: &[types::Tier]) -> Result<(), Error> {
            Ok(())
        }
        fn upsert_agent(&self, _: &types::Agent) -> Result<(), Error> {
            Ok(())
        }
        fn list_agents(&self) -> Result<Vec<types::Agent>, Error> {
            Ok(Vec::new())
        }
        fn set_cooldown(&self, _: &str, _: &str) -> Result<(), Error> {
            Ok(())
        }
        fn clear_cooldown(&self, _: &str) -> Result<(), Error> {
            Ok(())
        }
        fn save_route_decision(&self, _: &types::RouteDecision) -> Result<(), Error> {
            Ok(())
        }
        fn get_route_decision(&self, _: &str) -> Result<Option<types::RouteDecision>, Error> {
            Ok(None)
        }
        fn list_route_decisions(&self, _: &str) -> Result<Vec<types::RouteDecision>, Error> {
            Ok(Vec::new())
        }
        fn save_artifact(&self, artifact: &types::Artifact) -> Result<(), Error> {
            self.0.lock().unwrap().push(artifact.clone());
            Ok(())
        }
    }

    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut b = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        b.extend_from_slice(&width.to_be_bytes());
        b.extend_from_slice(&height.to_be_bytes());
        b.extend_from_slice(&[8, 6, 0, 0, 0]);
        b
    }

    #[test]
    fn reads_png_gif_jpeg_and_webp_headers() {
        assert_eq!(
            header(&png(800, 600)),
            Some(Image {
                kind: "png",
                width: 800,
                height: 600
            })
        );

        let mut gif = b"GIF89a".to_vec();
        gif.extend_from_slice(&[0x40, 0x01, 0xF0, 0x00, 0, 0, 0]);
        assert_eq!(
            header(&gif),
            Some(Image {
                kind: "gif",
                width: 320,
                height: 240
            })
        );

        // A JFIF header, then a baseline frame header: height 2, width 3.
        let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10];
        jpeg.extend_from_slice(b"JFIF\0\x01\x01\0\0\x01\0\x01\0\0");
        jpeg.extend_from_slice(&[0xFF, 0xC0, 0x00, 0x11, 0x08, 0x00, 0x02, 0x00, 0x03, 0x03]);
        assert_eq!(
            header(&jpeg),
            Some(Image {
                kind: "jpeg",
                width: 3,
                height: 2
            })
        );

        let mut webp = b"RIFF\x24\0\0\0WEBPVP8X\x0a\0\0\0\0\0\0\0".to_vec();
        webp.extend_from_slice(&[0x9F, 0x02, 0x00, 0x37, 0x01, 0x00]);
        assert_eq!(
            header(&webp),
            Some(Image {
                kind: "webp",
                width: 672,
                height: 312
            })
        );

        assert_eq!(header(b"not an image"), None);
        assert_eq!(header(b"\x89PNG\r\n\x1a\nshort"), None);
    }

    #[test]
    fn reads_only_names_a_run_may_write() {
        assert!(is_image_name("shot.PNG"));
        assert!(is_image_name("a/b/final.jpeg"));
        assert!(!is_image_name("notes.txt"));
        assert!(safe_rel("out/chart.png"));
        assert!(!safe_rel("../secrets.png"));
        assert!(!safe_rel("/etc/passwd.png"));
        assert!(!safe_rel("a//b.png"));
    }

    #[test]
    fn indexes_recent_images_from_non_git_image_run_only() {
        let temp = tempfile::tempdir().unwrap();
        let folder = temp.path().join("plain");
        std::fs::create_dir_all(folder.join("node_modules/pkg")).unwrap();
        let old = folder.join("old.png");
        std::fs::write(&old, png(1, 1)).unwrap();
        let old_time = filetime::FileTime::from_unix_time(1, 0);
        filetime::set_file_mtime(&old, old_time).unwrap();
        std::fs::write(folder.join("new.png"), png(640, 480)).unwrap();
        std::fs::write(folder.join("fake.png"), b"not an image").unwrap();
        std::fs::write(folder.join("node_modules/pkg/hidden.png"), png(2, 2)).unwrap();
        let store = ArtifactStore::default();
        let mut run = types::Run::default();
        run.id = "run123".into();
        run.dir = folder.to_string_lossy().into_owned();
        run.images = true;
        run.started_at = "2026-01-01T00:00:00Z".into();
        run.created_at = run.started_at.clone();
        let dest = temp.path().join("artifacts");
        assert_eq!(index_external(&store, &run, &dest, &folder), 1);
        let rows = store.0.lock().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].rel_path, "worktree/new.png");
        assert_eq!((rows[0].width, rows[0].height), (640, 480));
        assert!(dest.join("worktree/new.png").is_file());
    }
}
