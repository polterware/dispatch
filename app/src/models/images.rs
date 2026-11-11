//! The Images page's tiles: every artifact newest first, where it came from, and the rules
//! its search, filters, and day groups follow.
//!
//! An artifact's origin is read off its run's caller, as the core reads it: the desktop app
//! (`dispatch`) made it, or an agent did through the CLI, in which case its bot and the
//! shortened session it was called from are shown. The date is the artifact's own, on this
//! computer's clock.

use super::filter::{FilterOption, FilterSpec, Filters};
use crate::client::DispatchArtifact;
use crate::format::{agent_display_name, prompt_first_line};

pub const ORIGIN_APP: &str = "app";
pub const ORIGIN_DELEGATED: &str = "delegated";

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// Who made an artifact: the desktop app, or an agent that delegated a run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    App,
    Delegated,
}

impl Origin {
    /// The id the Origin filter keeps it under.
    pub fn key(self) -> &'static str {
        match self {
            Origin::App => ORIGIN_APP,
            Origin::Delegated => ORIGIN_DELEGATED,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Origin::App => "App",
            Origin::Delegated => "Delegated",
        }
    }
}

/// One image on the gallery, with the run it came from in words.
#[derive(Clone, Debug, PartialEq)]
pub struct ImageTile {
    pub id: String,
    pub run_id: String,
    /// The copy's absolute path.
    pub path: String,
    pub rel_path: String,
    /// png, jpeg, webp, or gif.
    pub kind: String,
    pub prompt: String,
    /// The run's prompt's first line, which names it.
    pub title: String,
    pub origin: Origin,
    /// The calling agent of a delegated run, as the core recorded it.
    pub caller_agent: String,
    /// Its session id, shortened for the tile.
    pub session: String,
    pub agent: String,
    pub model: String,
    /// The repository the run worked in, and its folder's name.
    pub repo: String,
    pub repo_name: String,
    pub created_at: String,
    /// The date on this computer's clock, "Nov 11, 2026".
    pub date: String,
    pub width: i32,
    pub height: i32,
}

impl From<&DispatchArtifact> for ImageTile {
    fn from(artifact: &DispatchArtifact) -> Self {
        let origin = if artifact.origin.trim().eq_ignore_ascii_case(ORIGIN_APP)
            || artifact.caller_agent.trim() == "dispatch"
        {
            Origin::App
        } else {
            Origin::Delegated
        };
        Self {
            id: artifact.id.clone(),
            run_id: artifact.run_id.clone(),
            path: artifact.path.clone(),
            rel_path: artifact.rel_path.clone(),
            kind: artifact.kind.clone(),
            prompt: artifact.prompt.clone(),
            title: prompt_first_line(&artifact.prompt, &artifact.run_id),
            origin,
            caller_agent: artifact.caller_agent.trim().to_string(),
            session: short_session(&artifact.conversation_id),
            agent: artifact.agent.clone(),
            model: artifact.model.clone(),
            repo: artifact.repo.trim_end_matches('/').to_string(),
            repo_name: repo_name(&artifact.repo),
            created_at: artifact.created_at.clone(),
            date: local_date(&artifact.created_at),
            width: artifact.width,
            height: artifact.height,
        }
    }
}

/// A session id as a tile shows it: its first 8 characters, then an ellipsis when there is
/// more. An app run's own `dispatch:<id>` loses the prefix.
pub fn short_session(id: &str) -> String {
    let id = id.trim();
    let id = id.strip_prefix("dispatch:").unwrap_or(id);
    let mut chars = id.chars();
    let head: String = chars.by_ref().take(8).collect();
    if chars.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}

/// A repository's folder name.
pub fn repo_name(path: &str) -> String {
    path.trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(path)
        .to_string()
}

/// Where an artifact came from, in words: "App", or "Delegated · Claude · 1a2b3c4d…".
pub fn origin_text(tile: &ImageTile) -> String {
    match tile.origin {
        Origin::App => "App".to_string(),
        Origin::Delegated => {
            let agent = agent_display_name(&tile.caller_agent);
            if tile.session.is_empty() {
                format!("Delegated · {agent}")
            } else {
                format!("Delegated · {agent} · {}", tile.session)
            }
        }
    }
}

/// The agent and model a run used, "Codex · gpt-6-astra".
pub fn agent_text(tile: &ImageTile) -> String {
    let agent = agent_display_name(&tile.agent);
    if tile.model.trim().is_empty() {
        agent
    } else {
        format!("{agent} · {}", tile.model.trim())
    }
}

/// Origin, Agent, and Repository: the fields the gallery filters by, built from the tiles on
/// hand so each offers only what is here.
pub fn filter_specs(tiles: &[ImageTile]) -> [FilterSpec; 3] {
    [origin_spec(), agent_spec(tiles), repository_spec(tiles)]
}

pub fn origin_spec() -> FilterSpec {
    FilterSpec::multi(
        "origin",
        "Origin",
        vec![
            FilterOption::new(ORIGIN_APP, "App"),
            FilterOption::new(ORIGIN_DELEGATED, "Delegated"),
        ],
    )
    .plural("origins")
}

fn agent_spec(tiles: &[ImageTile]) -> FilterSpec {
    let mut seen: Vec<&str> = Vec::new();
    for tile in tiles {
        let agent = tile.agent.trim();
        if !agent.is_empty() && !seen.contains(&agent) {
            seen.push(agent);
        }
    }
    seen.sort_unstable();
    let options = seen
        .into_iter()
        .map(|agent| FilterOption::new(agent, agent_display_name(agent)))
        .collect();
    FilterSpec::multi("agent", "Agent", options).plural("agents")
}

fn repository_spec(tiles: &[ImageTile]) -> FilterSpec {
    let mut repos: Vec<&str> = Vec::new();
    for tile in tiles {
        if !tile.repo.is_empty() && !repos.contains(&tile.repo.as_str()) {
            repos.push(tile.repo.as_str());
        }
    }
    repos.sort_unstable();
    let names: Vec<String> = repos.iter().map(|repo| repo_name(repo)).collect();
    let options = repos
        .iter()
        .zip(names.iter())
        .map(|(repo, name)| {
            // Two folders of the same name keep their whole path, so the rows tell them apart.
            let label = if names.iter().filter(|other| *other == name).count() > 1 {
                (*repo).to_string()
            } else {
                name.clone()
            };
            FilterOption::new(*repo, label)
        })
        .collect();
    FilterSpec::multi("repository", "Repository", options).plural("repositories")
}

/// The tiles the search and the filters keep, newest first: the search reads the run's
/// prompt alone, and each clause keeps the tiles holding any of its values.
pub fn visible_tiles<'a>(
    tiles: &'a [ImageTile],
    query: &str,
    filters: &Filters,
) -> Vec<&'a ImageTile> {
    let needle = query.trim().to_lowercase();
    tiles
        .iter()
        .filter(|tile| {
            let searched = needle.is_empty() || tile.prompt.to_lowercase().contains(&needle);
            searched
                && filters.matches(|field| match field {
                    "origin" => Some(vec![tile.origin.key()]),
                    "agent" => Some(vec![tile.agent.as_str()]),
                    "repository" => Some(vec![tile.repo.as_str()]),
                    _ => None,
                })
        })
        .collect()
}

/// A gallery section: the tiles of one day on this computer's clock.
#[derive(Clone, Debug, PartialEq)]
pub struct ImageGroup<'a> {
    pub label: String,
    pub tiles: Vec<&'a ImageTile>,
}

/// The tiles under their day's heading, in the order they came (newest first).
pub fn group_by_day<'a>(tiles: &[&'a ImageTile]) -> Vec<ImageGroup<'a>> {
    let mut groups: Vec<ImageGroup<'a>> = Vec::new();
    let mut key = String::new();
    for tile in tiles {
        let day = local_day(&tile.created_at);
        if groups.is_empty() || day != key {
            key = day;
            groups.push(ImageGroup {
                label: local_date(&tile.created_at),
                tiles: Vec::new(),
            });
        }
        if let Some(group) = groups.last_mut() {
            group.tiles.push(tile);
        }
    }
    groups
}

/// The artifact's date on this computer's clock, "Nov 11, 2026", or its stamp when it does
/// not parse.
pub fn local_date(value: &str) -> String {
    match timestamp_ms(value).map(local_parts) {
        Some((year, month, day, _, _)) if (1..=12).contains(&month) => {
            format!("{} {day}, {year}", MONTHS[(month - 1) as usize])
        }
        _ => value.trim().to_string(),
    }
}

/// The artifact's date and time on this computer's clock, "Nov 11, 2026 · 14:32".
pub fn local_datetime(value: &str) -> String {
    match timestamp_ms(value).map(local_parts) {
        Some((year, month, day, hour, minute)) if (1..=12).contains(&month) => format!(
            "{} {day}, {year} · {hour:02}:{minute:02}",
            MONTHS[(month - 1) as usize]
        ),
        _ => value.trim().to_string(),
    }
}

/// The local day a stamp falls on, "2026-11-11", for grouping.
fn local_day(value: &str) -> String {
    match timestamp_ms(value).map(local_parts) {
        Some((year, month, day, _, _)) => format!("{year:04}-{month:02}-{day:02}"),
        None => String::new(),
    }
}

/// A stamp's local year, month, day, hour, and minute.
fn local_parts(ms: i64) -> (i32, i32, i32, i32, i32) {
    let secs = ms.div_euclid(1000) as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let parsed = unsafe { libc::localtime_r(&secs, &mut tm) };
    if parsed.is_null() {
        (1970, 1, 1, 0, 0)
    } else {
        (
            tm.tm_year + 1900,
            tm.tm_mon + 1,
            tm.tm_mday,
            tm.tm_hour,
            tm.tm_min,
        )
    }
}

/// `Date.parse` for the RFC 3339 stamps the core sends.
fn timestamp_ms(value: &str) -> Option<i64> {
    let value = value.trim();
    let bytes = value.as_bytes();
    if bytes.len() < 19 {
        return None;
    }
    let year = digits(&bytes[0..4])?;
    let month = digits(&bytes[5..7])?;
    let day = digits(&bytes[8..10])?;
    let hour = digits(&bytes[11..13])?;
    let minute = digits(&bytes[14..16])?;
    let second = digits(&bytes[17..19])?;
    if bytes[4] != b'-'
        || bytes[7] != b'-'
        || !matches!(bytes[10], b'T' | b't' | b' ')
        || bytes[13] != b':'
        || bytes[16] != b':'
        || !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
    {
        return None;
    }
    let mut index = 19;
    let mut millis = 0i64;
    if bytes.get(index) == Some(&b'.') {
        index += 1;
        let start = index;
        while index < bytes.len() && bytes[index].is_ascii_digit() {
            index += 1;
        }
        if start == index {
            return None;
        }
        let mut fraction = value[start..index].to_string();
        while fraction.len() < 3 {
            fraction.push('0');
        }
        millis = fraction[..3].parse().ok()?;
    }
    let offset = match bytes.get(index) {
        None | Some(b'Z' | b'z') => 0,
        Some(sign @ (b'+' | b'-')) => {
            let rest = value.get(index + 1..)?;
            let (hours, minutes) = rest.split_once(':')?;
            let sign = if *sign == b'-' { -1 } else { 1 };
            sign * (hours.parse::<i64>().ok()? * 3600 + minutes.parse::<i64>().ok()? * 60)
        }
        Some(_) => return None,
    };
    let days = days_from_civil(year, month, day);
    let secs = days * 86_400 + i64::from(hour) * 3600 + i64::from(minute) * 60 + i64::from(second);
    Some((secs - offset) * 1000 + millis)
}

fn digits(bytes: &[u8]) -> Option<i32> {
    if bytes.is_empty() || !bytes.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let mut value = 0i32;
    for byte in bytes {
        value = value * 10 + i32::from(byte - b'0');
    }
    Some(value)
}

fn days_from_civil(year: i32, month: i32, day: i32) -> i64 {
    let year = year as i64 - i64::from(month <= 2);
    let era = year.div_euclid(400);
    let year_of_era = (year - era * 400) as u64;
    let month_prime = if month > 2 { month - 3 } else { month + 9 };
    let day_of_year = (153 * month_prime + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year as u64;
    era * 146097 + day_of_era as i64 - 719468
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::ProtoI64;

    fn artifact(
        run: &str,
        prompt: &str,
        origin: &str,
        agent: &str,
        repo: &str,
        at: &str,
    ) -> ImageTile {
        let artifact = DispatchArtifact {
            id: format!("art-{run}"),
            run_id: run.into(),
            workspace_id: "ws-1".into(),
            conversation_id: if origin == ORIGIN_APP {
                "dispatch:abcdef0123456789".into()
            } else {
                "claude-session-1234".into()
            },
            caller_agent: if origin == ORIGIN_APP {
                "dispatch".into()
            } else {
                "claude".into()
            },
            agent: agent.into(),
            model: "gpt-6-astra".into(),
            repo: repo.into(),
            path: format!("/tmp/{run}.png"),
            rel_path: "chart.png".into(),
            kind: "png".into(),
            bytes: ProtoI64(10),
            width: 1024,
            height: 768,
            created_at: at.into(),
            prompt: prompt.into(),
            origin: origin.into(),
        };
        ImageTile::from(&artifact)
    }

    fn tiles() -> Vec<ImageTile> {
        vec![
            artifact(
                "a",
                "draw a chart",
                ORIGIN_DELEGATED,
                "codex",
                "/code/api",
                "2026-11-11T12:00:00Z",
            ),
            artifact(
                "b",
                "make a logo",
                ORIGIN_APP,
                "fake",
                "/code/web",
                "2026-11-10T09:00:00Z",
            ),
            artifact(
                "c",
                "draw a diagram",
                ORIGIN_DELEGATED,
                "codex",
                "/code/api/",
                "2026-11-09T18:00:00Z",
            ),
        ]
    }

    #[test]
    fn reads_where_a_tile_came_from() {
        let tiles = tiles();
        assert_eq!(tiles[0].origin, Origin::Delegated);
        assert_eq!(origin_text(&tiles[0]), "Delegated · Claude · claude-s…");
        assert_eq!(tiles[1].origin, Origin::App);
        assert_eq!(origin_text(&tiles[1]), "App");
        assert_eq!(tiles[1].session, "abcdef01…");
        assert_eq!(tiles[0].repo_name, "api");
        assert_eq!(tiles[0].title, "draw a chart");
        assert_eq!(agent_text(&tiles[0]), "Codex · gpt-6-astra");
        assert_eq!(short_session("app-e2e"), "app-e2e");
        assert_eq!(short_session(""), "");
    }

    #[test]
    fn keeps_what_the_search_and_the_filters_hold() {
        let tiles = tiles();
        let specs = filter_specs(&tiles);
        assert_eq!(
            specs[0]
                .options
                .iter()
                .map(|option| option.label.as_str())
                .collect::<Vec<_>>(),
            vec!["App", "Delegated"]
        );
        assert_eq!(
            specs[1]
                .options
                .iter()
                .map(|option| option.label.as_str())
                .collect::<Vec<_>>(),
            vec!["Codex", "fake"]
        );
        assert_eq!(
            specs[2]
                .options
                .iter()
                .map(|option| option.id.as_str())
                .collect::<Vec<_>>(),
            vec!["/code/api", "/code/web"]
        );

        let mut filters = Filters::default();
        assert_eq!(visible_tiles(&tiles, "", &filters).len(), 3);
        assert_eq!(visible_tiles(&tiles, "logo", &filters).len(), 1);
        assert_eq!(visible_tiles(&tiles, "diagram", &filters)[0].id, "art-c");
        assert!(visible_tiles(&tiles, "no such prompt", &filters).is_empty());

        filters.add(&specs[0], &[ORIGIN_DELEGATED.into()]);
        let delegated = visible_tiles(&tiles, "", &filters);
        assert_eq!(
            delegated
                .iter()
                .map(|tile| tile.id.as_str())
                .collect::<Vec<_>>(),
            vec!["art-a", "art-c"]
        );

        filters.add(&specs[1], &["codex".into()]);
        assert_eq!(visible_tiles(&tiles, "draw", &filters).len(), 2);
        assert!(visible_tiles(&tiles, "logo", &filters).is_empty());

        let mut by_repo = Filters::default();
        by_repo.add(&specs[2], &["/code/api".into()]);
        assert_eq!(visible_tiles(&tiles, "", &by_repo).len(), 2);
    }

    #[test]
    fn groups_tiles_under_their_day() {
        let tiles = tiles();
        let all: Vec<&ImageTile> = tiles.iter().collect();
        let groups = group_by_day(&all);
        assert_eq!(groups.len(), 3);
        assert_eq!(groups[0].tiles[0].id, "art-a");
        assert_eq!(groups[2].tiles[0].id, "art-c");
        assert_eq!(groups[0].label, local_date("2026-11-11T12:00:00Z"));

        let same_day = vec![&tiles[0], &tiles[0]];
        assert_eq!(group_by_day(&same_day).len(), 1);
        assert!(group_by_day(&[]).is_empty());
    }

    #[test]
    fn shows_dates_on_this_computers_clock() {
        assert_eq!(
            local_date("2026-11-11T12:00:00Z"),
            local_date("2026-11-11T12:00:00Z")
        );
        assert!(local_date("2026-11-11T12:00:00Z").contains("2026"));
        assert_eq!(local_date("not a stamp"), "not a stamp");
        assert_eq!(local_datetime(""), "");
    }
}
