use crate::{
    Error,
    difficulty::Difficulty,
    router::valid_priority,
    tiers::{Table, table_from_rows, valid_effort},
    types::Tier,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

pub const GENERAL_ROLE: &str = "general";
pub const GENERAL_WHEN: &str = "None of the other roles fits well: a mixed or unusual task, or one that names no clear kind of work.";
pub const PICK_MIN: f64 = 0.5;

fn is_false(v: &bool) -> bool {
    !*v
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Profile {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub provider: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub model: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub effort: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub agent: String,
    #[serde(skip_serializing_if = "is_false")]
    pub relax: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub priority: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub tier: String,
    #[serde(rename = "readonly", skip_serializing_if = "is_false")]
    pub read_only: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub instructions: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub when: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tiers: Vec<RoleEntry>,
    #[serde(skip_serializing_if = "is_false")]
    pub manual: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub prefer: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub avoid: Vec<String>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub timeout: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RoleEntry {
    pub tier: String,
    pub agent: String,
    pub model: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub provider: String,
}

impl Profile {
    pub fn pickable(&self) -> bool {
        !self.manual && !self.when.trim().is_empty()
    }

    pub fn board(&self) -> Table {
        let rows: Vec<Tier> = self
            .tiers
            .iter()
            .enumerate()
            .map(|(i, e)| Tier {
                name: e.tier.clone(),
                agent: e.agent.clone(),
                model: e.model.clone(),
                provider: e.provider.clone(),
                position: i as i64,
                ..Default::default()
            })
            .collect();
        table_from_rows(&rows)
    }
}

pub fn builtin_profiles() -> BTreeMap<String, Profile> {
    [
        (
            "explorer",
            "easy",
            true,
            "10m",
            "Find where something is in the code or explain how existing code works. A question to answer, not a change to make.",
            "Answer from the code. Name the files and lines that matter. Do not change anything.",
        ),
        (
            "implementer",
            "normal",
            false,
            "",
            "Make a change: build a feature, fix a known bug, or refactor code, with the approach already clear.",
            "Make the change, keep it focused, and run the checks and tests that cover it.",
        ),
        (
            "tester",
            "normal",
            false,
            "",
            "Write or fix tests for code that exists, or run the tests and report what fails.",
            "Write and run tests for the described change. Report what fails and why.",
        ),
        (
            "reviewer",
            "hard",
            true,
            "15m",
            "Review a diff, a branch, or a pull request for bugs. Judging work already done, not doing it.",
            "Review the diff. Point out only real bugs, with file and line.",
        ),
        (
            "planner",
            "hard",
            true,
            "20m",
            "Design how to build something before building it: weigh approaches, plan the steps, or find the cause of a hard bug.",
            "Plan the work. Weigh the approaches, name the files involved, and list the steps. Do not change anything.",
        ),
    ]
    .into_iter()
    .map(|(name, tier, read_only, timeout, when, instructions)| {
        (
            name.into(),
            Profile {
                tier: tier.into(),
                read_only,
                timeout: timeout.into(),
                when: when.into(),
                instructions: instructions.into(),
                ..Default::default()
            },
        )
    })
    .collect()
}

pub fn is_builtin_profile(name: &str) -> bool {
    matches!(
        name,
        "explorer" | "implementer" | "tester" | "reviewer" | "planner"
    )
}

pub fn global_profiles_path(state_dir: &Path) -> PathBuf {
    state_dir.join("roles.json")
}

pub fn project_profiles_path(dir: &Path) -> PathBuf {
    dir.join(".dispatch").join("roles.json")
}

pub fn profile_names(profiles: &BTreeMap<String, Profile>) -> Vec<String> {
    profiles.keys().cloned().collect()
}

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct RolesFile {
    #[serde(skip_serializing_if = "Option::is_none")]
    roles: Option<BTreeMap<String, Profile>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    agents: Option<BTreeMap<String, Profile>>,
}

pub fn parse_profiles(input: &str) -> Result<BTreeMap<String, Profile>, Error> {
    let f: RolesFile = serde_json::from_str(input)?;
    Ok(f.roles.or(f.agents).unwrap_or_default())
}

pub fn merge_profiles(
    user: BTreeMap<String, Profile>,
    project: BTreeMap<String, Profile>,
) -> BTreeMap<String, Profile> {
    let builtin = builtin_profiles();
    let mut out = builtin.clone();
    for (name, mut p) in user.into_iter().chain(project) {
        if p.when.is_empty()
            && let Some(b) = builtin.get(&name)
        {
            p.when = b.when.clone();
        }
        out.insert(name, p);
    }
    out
}

pub fn valid_profile_name(name: &str) -> bool {
    let b = name.as_bytes();
    !b.is_empty()
        && b.len() <= 40
        && name != GENERAL_ROLE
        && name != "none"
        && b[0].is_ascii_lowercase()
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}

pub fn pick_roles(profiles: &BTreeMap<String, Profile>) -> BTreeMap<String, String> {
    let mut out: BTreeMap<_, _> = profiles
        .iter()
        .filter(|(name, p)| name.as_str() != GENERAL_ROLE && p.pickable())
        .map(|(name, p)| (name.clone(), p.when.trim().to_owned()))
        .collect();
    if !out.is_empty() {
        out.insert(GENERAL_ROLE.into(), GENERAL_WHEN.into());
    }
    out
}

pub fn pick_role(
    role: &str,
    probs: &BTreeMap<String, f64>,
    roles: &BTreeMap<String, String>,
) -> String {
    if roles.is_empty() {
        return String::new();
    }
    if probs.is_empty() {
        return if roles.contains_key(role) {
            role
        } else {
            GENERAL_ROLE
        }
        .into();
    }
    let mut best = GENERAL_ROLE;
    let mut best_p = -1.0;
    for (name, p) in probs {
        if *p > best_p {
            best = name;
            best_p = *p;
        }
    }
    if !roles.contains_key(best) || best_p < PICK_MIN {
        GENERAL_ROLE.into()
    } else {
        best.into()
    }
}

pub fn validate_profile(name: &str, p: &Profile, known: &HashSet<String>) -> Vec<String> {
    let mut out = Vec::new();
    let mut add = |s: String| out.push(format!("{name}: {s}"));
    if !p.effort.is_empty() && !valid_effort(&p.effort) {
        add(format!("invalid effort {:?}", p.effort));
    }
    if !p.priority.is_empty() && !valid_priority(&p.priority) {
        add(format!(
            "invalid priority {:?} (want speed|balanced|quality)",
            p.priority
        ));
    }
    if !p.agent.is_empty() && !known.contains(&p.agent) {
        add(format!("unknown agent {:?}", p.agent));
    }
    let valid_tier = |s: &str| matches!(s, "easy" | "normal" | "hard");
    if !p.tier.is_empty() && !valid_tier(&p.tier) {
        add(format!("invalid tier {:?} (want easy|normal|hard)", p.tier));
    }
    if !p.timeout.is_empty() && parse_duration(&p.timeout).is_none() {
        add(format!("invalid timeout {:?}", p.timeout));
    }
    for (kind, list) in [("prefer", &p.prefer), ("avoid", &p.avoid)] {
        for a in list {
            if !known.contains(a) {
                add(format!("{kind}: unknown agent {a:?}"));
            }
        }
    }
    for a in &p.prefer {
        for b in &p.avoid {
            if a == b {
                add(format!("agent {a:?} is both preferred and avoided"));
            }
        }
    }
    for (i, e) in p.tiers.iter().enumerate() {
        if !valid_tier(&e.tier) {
            add(format!(
                "tiers[{i}]: invalid tier {:?} (want easy|normal|hard)",
                e.tier
            ));
        } else if !known.contains(&e.agent) {
            add(format!("tiers[{i}]: unknown agent {:?}", e.agent));
        } else if e.model.is_empty() {
            add(format!("tiers[{i}]: model is required"));
        }
    }
    if !p.tiers.is_empty() && (!p.model.is_empty() || !p.provider.is_empty()) {
        add("its board and a model or provider both pick the model; keep one".into());
    }
    if p.provider.is_empty()
        && p.model.is_empty()
        && p.agent.is_empty()
        && p.effort.is_empty()
        && p.priority.is_empty()
        && p.instructions.is_empty()
        && p.tier.is_empty()
        && p.when.is_empty()
        && p.tiers.is_empty()
        && !p.manual
        && p.prefer.is_empty()
        && p.avoid.is_empty()
    {
        add("empty role".into());
    }
    out
}

pub fn raise_to_floor(diff: &mut Difficulty, floor: &str, role: &str) -> String {
    let idx = |s: &str| match s {
        "easy" => Some(0),
        "normal" => Some(1),
        "hard" => Some(2),
        _ => None,
    };
    if let (Some(f), Some(d)) = (idx(floor), idx(crate::pass_model::difficulty_tier(diff.d)))
        && d < f
    {
        diff.d = crate::difficulty::BAND_MID[f];
        diff.tier = floor.into();
        return format!("role {role}: raised to its {floor} tier");
    }
    String::new()
}

fn parse_duration(value: &str) -> Option<i64> {
    if value == "0" || value == "+0" || value == "-0" {
        return Some(0);
    }
    let negative = value.starts_with('-');
    let text = value.strip_prefix(['+', '-']).unwrap_or(value);
    if text.is_empty() {
        return None;
    }
    let mut rest = text;
    let mut seconds = 0.0;
    while !rest.is_empty() {
        let end = rest
            .find(|c: char| !c.is_ascii_digit() && c != '.')
            .unwrap_or(rest.len());
        if end == 0 {
            return None;
        }
        let number: f64 = rest[..end].parse().ok()?;
        rest = &rest[end..];
        let (unit, scale) = [
            ("ns", 1e-9),
            ("us", 1e-6),
            ("µs", 1e-6),
            ("μs", 1e-6),
            ("ms", 1e-3),
            ("s", 1.0),
            ("m", 60.0),
            ("h", 3600.0),
        ]
        .into_iter()
        .find(|(u, _)| rest.starts_with(u))?;
        rest = &rest[unit.len()..];
        seconds += number * scale;
    }
    let nanos = seconds * 1e9 * if negative { -1.0 } else { 1.0 };
    if !nanos.is_finite() || nanos < i64::MIN as f64 || nanos >= -(i64::MIN as f64) {
        return None;
    }
    Some(nanos as i64)
}

pub fn load_profiles(path: &Path) -> Result<BTreeMap<String, Profile>, Error> {
    if path.as_os_str().is_empty() {
        return Ok(BTreeMap::new());
    }
    let mut path = path.to_path_buf();
    let mut content = std::fs::read_to_string(&path);
    if content
        .as_ref()
        .is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound)
        && path.file_name().is_some_and(|n| n == "roles.json")
    {
        path.set_file_name("agents.json");
        content = std::fs::read_to_string(&path);
    }
    match content {
        Ok(text) => {
            parse_profiles(&text).map_err(|e| Error::Message(format!("{}: {e}", path.display())))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
        Err(e) => Err(e.into()),
    }
}

pub fn load_all_profiles(user: &Path, project: &Path) -> Result<BTreeMap<String, Profile>, Error> {
    Ok(merge_profiles(
        load_profiles(user)?,
        load_profiles(project)?,
    ))
}

pub fn save_profile(path: &Path, name: &str, profile: Profile, remove: bool) -> Result<(), Error> {
    let mut profiles = load_profiles(path)?;
    if remove {
        if profiles.remove(name).is_none() {
            return Err(Error::Message(format!(
                "no role {name:?} in {}",
                path.display()
            )));
        }
    } else {
        profiles.insert(name.into(), profile);
    }
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    let doc = RolesFile {
        roles: (!profiles.is_empty()).then_some(profiles),
        agents: None,
    };
    let mut data = serde_json::to_string_pretty(&doc)?;
    data.push('\n');
    std::fs::write(path, data)?;
    Ok(())
}

#[derive(Clone, Debug, Default)]
pub struct ProfileOptions {
    pub tier: String,
    pub filters: crate::router::Filters,
    pub tier_floor: String,
    pub read_only: bool,
    pub timeout_ns: i64,
    pub prompt: String,
    pub resume: String,
    pub profile: String,
    pub prefer: Vec<String>,
    pub avoid: Vec<String>,
    pub role_board: Table,
}

pub fn apply_profile(options: &mut ProfileOptions, name: &str, p: &Profile) -> Result<(), Error> {
    if options.filters.provider.is_empty() {
        options.filters.provider.clone_from(&p.provider);
    }
    if options.filters.model.is_empty() {
        options.filters.model.clone_from(&p.model);
    }
    if options.filters.effort.is_empty() {
        options.filters.effort.clone_from(&p.effort);
    }
    if options.filters.agent.is_empty() {
        options.filters.agent.clone_from(&p.agent);
    }
    if !options.filters.relax {
        options.filters.relax = p.relax;
    }
    if options.filters.priority.is_empty() {
        options.filters.priority.clone_from(&p.priority);
    }
    if matches!(p.tier.as_str(), "easy" | "normal" | "hard") {
        options.tier_floor.clone_from(&p.tier);
    }
    options.read_only |= p.read_only;
    if options.timeout_ns == 0 && !p.timeout.is_empty() {
        options.timeout_ns = parse_duration(&p.timeout).ok_or_else(|| {
            Error::Message(format!("role {name}: invalid timeout {:?}", p.timeout))
        })?;
    }
    if !p.instructions.is_empty() && options.resume.is_empty() {
        options.prompt = format!("{}\n\n{}", p.instructions, options.prompt);
    }
    options.profile = name.into();
    options.prefer.clone_from(&p.prefer);
    options.avoid.clone_from(&p.avoid);
    options.role_board = p.board();
    Ok(())
}

pub fn apply_picked(
    options: &mut ProfileOptions,
    name: &str,
    profile: &Profile,
) -> Result<(), Error> {
    let mut p = profile.clone();
    p.read_only = false;
    p.model.clear();
    p.provider.clear();
    apply_profile(options, name, &p)
}
