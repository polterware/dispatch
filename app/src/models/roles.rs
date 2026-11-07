//! Pure logic for Roles: the tier board, role profiles, and build-from-model prices.
//!
//! Ports `tier-board-model.ts`, the pure helpers in `roles-page.tsx`, and
//! `builtPrice` / `mapDispatchTierBuild` from the Electron app.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::client::DispatchTier;

/// The general role: the tier table every run uses where its role has no models.
pub const GENERAL_ROLE: &str = "general";

/// Routing tiers, in board column order.
pub const TIER_COLUMNS: &[(&str, &str)] =
    &[("easy", "Easy"), ("normal", "Normal"), ("hard", "Hard")];

/// A board cell: a tier column and a 0-based order row (row 0 is the primary candidate).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TierSlot {
    pub tier: String,
    pub index: usize,
}

/// What a card carries, from the board or from the model palette.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TierCardInput {
    pub agent: String,
    pub model: String,
    pub provider: String,
}

impl TierCardInput {
    pub fn from_tier(tier: &DispatchTier) -> Self {
        Self {
            agent: tier.agent.clone(),
            model: tier.model.clone(),
            provider: tier.provider.clone(),
        }
    }
}

/// Candidates of each tier, ordered by position. Unknown tiers keep first-seen order after the three columns.
#[derive(Clone, Debug, PartialEq)]
pub struct TierColumns {
    pub columns: Vec<(String, Vec<DispatchTier>)>,
}

impl TierColumns {
    pub fn get(&self, name: &str) -> &[DispatchTier] {
        self.columns
            .iter()
            .find(|(id, _)| id == name)
            .map(|(_, list)| list.as_slice())
            .unwrap_or(&[])
    }
}

pub fn tier_columns(tiers: &[DispatchTier]) -> TierColumns {
    let all: Vec<String> = TIER_COLUMNS
        .iter()
        .map(|(id, _)| (*id).to_string())
        .collect();
    tier_columns_in(tiers, &all)
}

/// Candidates of the given tiers, in that order; a card in another tier is not drawn.
pub fn tier_columns_in(tiers: &[DispatchTier], kept: &[String]) -> TierColumns {
    let mut columns: Vec<(String, Vec<DispatchTier>)> =
        kept.iter().map(|id| (id.clone(), Vec::new())).collect();
    for tier in tiers {
        if let Some((_, list)) = columns.iter_mut().find(|(id, _)| id == &tier.name) {
            list.push(tier.clone());
        }
    }
    for (_, list) in &mut columns {
        list.sort_by(|a, b| a.position.cmp(&b.position));
    }
    TierColumns { columns }
}

fn flatten(columns: &TierColumns) -> Vec<DispatchTier> {
    let mut out = Vec::new();
    for (name, list) in &columns.columns {
        for (position, tier) in list.iter().enumerate() {
            let mut next = tier.clone();
            next.name = name.clone();
            next.position = position as i32;
            out.push(next);
        }
    }
    out
}

/// Places a card at a slot and shifts the cards below it down.
/// With `from`, the card moves (it leaves its old slot first); without it, the card is new.
/// A new card whose agent and model are already in the target tier is refused.
pub fn place_tier_card(
    tiers: &[DispatchTier],
    card: &TierCardInput,
    to: &TierSlot,
    from: Option<&TierSlot>,
) -> Vec<DispatchTier> {
    let mut columns = tier_columns(tiers);
    if !columns.columns.iter().any(|(id, _)| id == &to.tier) {
        columns.columns.push((to.tier.clone(), Vec::new()));
    }
    if from.is_none()
        && columns
            .get(&to.tier)
            .iter()
            .any(|item| item.agent == card.agent && item.model == card.model)
    {
        return tiers.to_vec();
    }
    if let Some(from) = from {
        let Some(source_at) = columns.columns.iter().position(|(id, _)| id == &from.tier) else {
            return tiers.to_vec();
        };
        if from.index >= columns.columns[source_at].1.len() {
            return tiers.to_vec();
        }
        columns.columns[source_at].1.remove(from.index);
    }
    let target_at = columns
        .columns
        .iter()
        .position(|(id, _)| id == &to.tier)
        .expect("target tier was inserted");
    let target = &mut columns.columns[target_at].1;
    let index = to.index.min(target.len());
    target.insert(
        index,
        DispatchTier {
            name: to.tier.clone(),
            position: index as i32,
            agent: card.agent.clone(),
            model: card.model.clone(),
            provider: card.provider.clone(),
        },
    );
    flatten(&columns)
}

/// Removes the card at a slot.
pub fn remove_tier_card(tiers: &[DispatchTier], slot: &TierSlot) -> Vec<DispatchTier> {
    let mut columns = tier_columns(tiers);
    if let Some((_, list)) = columns.columns.iter_mut().find(|(id, _)| id == &slot.tier) {
        if slot.index < list.len() {
            list.remove(slot.index);
        }
    }
    flatten(&columns)
}

/// How many rows the board draws: the tallest column, plus one empty row when it is editable.
pub fn board_row_count(columns: &TierColumns, editable: bool) -> usize {
    let tallest = columns
        .columns
        .iter()
        .map(|(_, list)| list.len())
        .max()
        .unwrap_or(0);
    tallest.max(1) + usize::from(editable)
}

pub fn tier_column_label(id: &str) -> String {
    TIER_COLUMNS
        .iter()
        .find(|(key, _)| *key == id)
        .map(|(_, label)| (*label).to_string())
        .unwrap_or_else(|| {
            let mut chars = id.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
}

/// Stable identity for springing one card between cells. Agent and model stay put when the card moves.
pub fn card_id(agent: &str, model: &str, provider: &str) -> String {
    format!("{agent}\u{1}{model}\u{1}{provider}")
}

/// Lowercased text match over the fields a list shows. An empty query matches everything.
pub fn matches_query(query: &str, values: &[&str]) -> bool {
    let needle = query.trim().to_lowercase();
    needle.is_empty()
        || values
            .iter()
            .any(|value| value.to_lowercase().contains(&needle))
}

pub fn tier_matches(query: &str, tier: &DispatchTier) -> bool {
    matches_query(
        query,
        &[&tier.name, &tier.agent, &tier.provider, &tier.model],
    )
}

/// Where an effective role comes from: the install, the user's roles.json, or the repository's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoleSource {
    Builtin,
    User,
    Repository,
}

impl RoleSource {
    pub fn parse(value: &str) -> Self {
        match value {
            "builtin" => Self::Builtin,
            "repository" => Self::Repository,
            _ => Self::User,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Builtin => "Built in",
            Self::User => "Yours",
            Self::Repository => "From the repository",
        }
    }
}

/// One card of a role's board, as roles.json holds it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleEntry {
    pub tier: String,
    pub agent: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub provider: String,
}

/// A role as roles.json holds it. Empty strings and false flags are omitted, matching the core's omitempty.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleProfile {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub provider: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub model: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub effort: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub agent: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub relax: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub priority: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tier: String,
    #[serde(default, rename = "readonly", skip_serializing_if = "is_false")]
    pub readonly: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub instructions: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub when: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tiers: Vec<RoleEntry>,
    /// The tier columns the role keeps, easiest first. Absent keeps all three.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub columns: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub manual: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub prefer: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub avoid: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub timeout: String,
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoleRow {
    pub name: String,
    pub profile: RoleProfile,
    pub source: RoleSource,
}

/// A built-in role offered as a template to create a role from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoleTemplate {
    pub name: String,
    pub profile: RoleProfile,
    pub in_use: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RolesList {
    pub rows: Vec<RoleRow>,
    pub builtin: Vec<String>,
    pub templates: Vec<RoleTemplate>,
    /// `pick` defaults to on when the core omits it.
    pub pick: bool,
}

/// The roles of a checkout, from the core's Subagents `list` JSON body.
pub fn parse_roles_list(json: &str) -> Result<RolesList, String> {
    let value: Value = serde_json::from_str(json).map_err(|err| err.to_string())?;
    let pick = value.get("pick").and_then(Value::as_bool).unwrap_or(true);
    let builtin = string_list(value.get("builtin"));
    let templates = value
        .get("templates")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    let name = item.get("name").and_then(Value::as_str)?.to_string();
                    let profile = item
                        .get("profile")
                        .map(|profile| {
                            serde_json::from_value::<RoleProfile>(profile.clone())
                                .unwrap_or_default()
                        })
                        .unwrap_or_default();
                    let in_use = item
                        .get("in_use")
                        .or_else(|| item.get("inUse"))
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    Some(RoleTemplate {
                        name,
                        profile,
                        in_use,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let source = value.get("source").and_then(Value::as_object);
    let mut names: Vec<String> = value
        .get("roles")
        .and_then(Value::as_object)
        .map(|roles| roles.keys().cloned().collect())
        .unwrap_or_default();
    names.sort();
    let roles_obj = value.get("roles").and_then(Value::as_object);
    let mut rows = Vec::new();
    for name in names {
        let profile = roles_obj
            .and_then(|roles| roles.get(&name))
            .map(|item| serde_json::from_value::<RoleProfile>(item.clone()).unwrap_or_default())
            .unwrap_or_default();
        let source = source
            .and_then(|map| map.get(&name))
            .and_then(Value::as_str)
            .map(RoleSource::parse)
            .unwrap_or(RoleSource::User);
        rows.push(RoleRow {
            name,
            profile,
            source,
        });
    }
    Ok(RolesList {
        rows,
        builtin,
        templates,
        pick,
    })
}

/// The templates New role offers: the ones no role of that name uses now.
pub fn available_templates(templates: &[RoleTemplate]) -> Vec<RoleTemplate> {
    templates
        .iter()
        .filter(|template| !template.in_use)
        .cloned()
        .collect()
}

fn string_list(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// A role's board as the tier board draws it: each tier's cards in order.
pub fn role_board_tiers(profile: &RoleProfile) -> Vec<DispatchTier> {
    let mut counts: BTreeMap<String, i32> = BTreeMap::new();
    profile
        .tiers
        .iter()
        .map(|entry| {
            let next = counts.get(&entry.tier).copied().unwrap_or(-1) + 1;
            counts.insert(entry.tier.clone(), next);
            DispatchTier {
                name: entry.tier.clone(),
                agent: entry.agent.clone(),
                model: entry.model.clone(),
                provider: entry.provider.clone(),
                position: next,
            }
        })
        .collect()
}

/// The tier board's cards back into a role's board, tier by tier in board order.
/// Tiers outside easy/normal/hard sort first, matching `indexOf` of -1 in the original.
pub fn board_to_role_tiers(tiers: &[DispatchTier]) -> Vec<RoleEntry> {
    let mut ordered: Vec<&DispatchTier> = tiers.iter().collect();
    ordered.sort_by(|a, b| {
        tier_sort_key(&a.name)
            .cmp(&tier_sort_key(&b.name))
            .then(a.position.cmp(&b.position))
    });
    ordered
        .into_iter()
        .map(|tier| RoleEntry {
            tier: tier.name.clone(),
            agent: tier.agent.clone(),
            model: tier.model.clone(),
            provider: tier.provider.clone(),
        })
        .collect()
}

fn tier_sort_key(name: &str) -> isize {
    TIER_COLUMNS
        .iter()
        .position(|(id, _)| *id == name)
        .map(|index| index as isize)
        .unwrap_or(-1)
}

pub fn profile_with_board(profile: &RoleProfile, tiers: &[DispatchTier]) -> RoleProfile {
    let mut next = profile.clone();
    let kept = kept_columns(profile);
    next.tiers = board_to_role_tiers(tiers)
        .into_iter()
        .filter(|entry| kept.iter().any(|column| column == &entry.tier))
        .collect();
    next
}

/// The tier columns a role keeps, easiest first. An absent setting keeps all three.
pub fn kept_columns(profile: &RoleProfile) -> Vec<String> {
    let Some(columns) = profile.columns.as_ref().filter(|c| !c.is_empty()) else {
        return TIER_COLUMNS
            .iter()
            .map(|(id, _)| (*id).to_string())
            .collect();
    };
    let kept: Vec<String> = TIER_COLUMNS
        .iter()
        .filter(|(id, _)| columns.iter().any(|c| c == id))
        .map(|(id, _)| (*id).to_string())
        .collect();
    if kept.is_empty() {
        TIER_COLUMNS
            .iter()
            .map(|(id, _)| (*id).to_string())
            .collect()
    } else {
        kept
    }
}

/// The columns a role stores for `kept`: none when they are all three, matching an absent field.
pub fn stored_columns(kept: &[String]) -> Option<Vec<String>> {
    (kept.len() < TIER_COLUMNS.len()).then(|| kept.to_vec())
}

/// `kept` without `tier`. The last remaining column stays: a role always keeps one.
pub fn remove_column(kept: &[String], tier: &str) -> Vec<String> {
    if kept.len() <= 1 {
        return kept.to_vec();
    }
    kept.iter().filter(|c| *c != tier).cloned().collect()
}

/// `kept` with `tier`, in board order. A column it already has leaves it as it was.
pub fn add_column(kept: &[String], tier: &str) -> Vec<String> {
    if !TIER_COLUMNS.iter().any(|(id, _)| *id == tier) {
        return kept.to_vec();
    }
    TIER_COLUMNS
        .iter()
        .filter(|(id, _)| *id == tier || kept.iter().any(|c| c == id))
        .map(|(id, _)| (*id).to_string())
        .collect()
}

/// The lowest tier a role runs at, as its card says it.
pub fn role_tier(profile: &RoleProfile) -> String {
    if profile.tier.is_empty() {
        "Any".to_string()
    } else {
        format!("{} or harder", tier_column_label(&profile.tier))
    }
}

pub fn when_display(when: &str) -> &str {
    if when.is_empty() {
        "Not picked: it does not say when it fits."
    } else {
        when
    }
}

pub fn access_label(readonly: bool) -> &'static str {
    if readonly {
        "Read only"
    } else {
        "Can change files"
    }
}

pub fn models_label(name: &str, count: usize) -> String {
    if count == 0 {
        if name == GENERAL_ROLE {
            "0".to_string()
        } else {
            "General's".to_string()
        }
    } else {
        count.to_string()
    }
}

pub fn source_text(source: Option<RoleSource>, manual: bool) -> String {
    let label = source.map(RoleSource::label).unwrap_or("Tier table");
    if manual {
        format!("{label} · Only with --role")
    } else {
        label.to_string()
    }
}

pub fn instructions_preview(instructions: &str) -> String {
    let first = instructions.split('\n').next().unwrap_or("").trim();
    if first.is_empty() {
        "None".to_string()
    } else {
        first.to_string()
    }
}

/// A row of the roles list: General first, then each role.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoleListRow {
    pub name: String,
    pub label: String,
    pub when: String,
    pub tier: String,
    pub access: String,
    pub models: usize,
    pub source: Option<RoleSource>,
    pub manual: bool,
}

pub fn role_list_rows(rows: &[RoleRow], general_models: usize) -> Vec<RoleListRow> {
    let mut list = vec![RoleListRow {
        name: GENERAL_ROLE.to_string(),
        label: "General".to_string(),
        when: "Every run whose role has no models for its tier, and every run with no role."
            .to_string(),
        tier: "Any".to_string(),
        access: "As the run asks".to_string(),
        models: general_models,
        source: None,
        manual: false,
    }];
    for row in rows {
        list.push(RoleListRow {
            name: row.name.clone(),
            label: row.name.clone(),
            when: row.profile.when.clone(),
            tier: role_tier(&row.profile),
            access: access_label(row.profile.readonly).to_string(),
            models: row.profile.tiers.len(),
            source: Some(row.source),
            manual: row.profile.manual,
        });
    }
    list
}

/// Composer fields that are not the name or the instructions text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoleDraft {
    pub tier: String,
    pub readonly: bool,
    pub manual: bool,
    pub prefer: Vec<String>,
    pub avoid: Vec<String>,
    pub instructions: String,
    pub when: String,
    pub timeout: String,
}

/// The saved profile: the previous one, with the composer's fields written over it.
/// Empty text and off switches are stored empty so serde omits them.
pub fn compose_profile(previous: Option<&RoleProfile>, draft: &RoleDraft) -> RoleProfile {
    let mut profile = previous.cloned().unwrap_or_default();
    profile.tier = draft.tier.trim().to_string();
    profile.readonly = draft.readonly;
    profile.instructions = draft.instructions.trim().to_string();
    profile.when = draft.when.trim().to_string();
    profile.manual = draft.manual;
    profile.prefer = draft.prefer.clone();
    profile.avoid = draft.avoid.clone();
    profile.timeout = draft.timeout.trim().to_string();
    profile
}

/// A name a run can take with `--role`: lowercase letters, digits, and dashes, starting with a letter.
/// `general` and `none` are reserved. Matches the core's `ValidProfileName`.
pub fn valid_profile_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if name == GENERAL_ROLE || name == "none" || name.chars().count() > 40 {
        return false;
    }
    if !first.is_ascii_lowercase() {
        return false;
    }
    chars.all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
}

pub fn role_name_error(name: &str) -> Option<String> {
    let name = name.trim();
    if name == GENERAL_ROLE {
        Some("General is the tier table; name the role something else.".to_string())
    } else if !valid_profile_name(name) {
        Some(format!(
            "invalid role name \"{name}\" (lowercase letters, digits, and dashes, starting with a letter)"
        ))
    } else {
        None
    }
}

/// What the core would refuse in `profile`, said for the Composer before it is sent: a time
/// limit that is no duration, an agent both preferred and avoided, or a role with nothing in it.
pub fn profile_error(profile: &RoleProfile) -> Option<String> {
    let timeout = profile.timeout.trim();
    if !timeout.is_empty() && !is_duration(timeout) {
        return Some(format!(
            "Time limit \"{timeout}\" is not a duration; use one such as 15m or 1h30m."
        ));
    }
    if let Some(agent) = profile
        .prefer
        .iter()
        .find(|agent| profile.avoid.contains(agent))
    {
        return Some(format!(
            "{agent} is both preferred and avoided; keep it in one list."
        ));
    }
    let empty = profile.provider.is_empty()
        && profile.model.is_empty()
        && profile.agent.is_empty()
        && profile.effort.is_empty()
        && profile.priority.is_empty()
        && profile.instructions.trim().is_empty()
        && profile.tier.is_empty()
        && profile.when.trim().is_empty()
        && profile.tiers.is_empty()
        && !profile.manual
        && profile.prefer.is_empty()
        && profile.avoid.is_empty();
    empty.then(|| "Give the role instructions or a When to use.".to_string())
}

/// A Go duration, as the core reads a time limit: numbers each with a unit (ns, us, µs, ms,
/// s, m, h), such as 15m or 1h30m, or 0.
pub fn is_duration(value: &str) -> bool {
    let text = value.strip_prefix(['+', '-']).unwrap_or(value);
    if text == "0" {
        return true;
    }
    if text.is_empty() {
        return false;
    }
    let mut rest = text;
    while !rest.is_empty() {
        let end = rest
            .find(|c: char| !c.is_ascii_digit() && c != '.')
            .unwrap_or(rest.len());
        if end == 0 || rest[..end].parse::<f64>().is_err() {
            return false;
        }
        rest = &rest[end..];
        let Some(unit) = ["ns", "us", "µs", "μs", "ms", "s", "m", "h"]
            .into_iter()
            .find(|unit| rest.starts_with(unit))
        else {
            return false;
        };
        rest = &rest[unit.len()..];
    }
    true
}

/// A rename saves the new name, then drops the old one. A built-in name is never dropped, only its copy.
pub fn rename_drops_previous(previous: &str, next: &str, source: RoleSource) -> bool {
    !previous.is_empty() && previous != next && source == RoleSource::User
}

#[derive(Clone, Debug, PartialEq)]
pub struct BuiltEntry {
    pub agent: String,
    pub model: String,
    pub provider: String,
    pub effort: String,
    pub p_pass: f64,
    pub p_pass_low: f64,
    pub effective_cost: Option<f64>,
    pub qualified: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TierBuild {
    pub priority: String,
    pub applied: bool,
    pub problems: Vec<String>,
    pub tiers: BTreeMap<String, Vec<BuiltEntry>>,
}

/// One proposed row's price: P(pass) with its bound and the effective cost.
pub fn built_price(entry: &BuiltEntry) -> String {
    let mut text = format!("P {:.2} ≥ {:.2}", entry.p_pass, entry.p_pass_low);
    if let Some(cost) = entry.effective_cost {
        text.push_str(&format!(" · ${cost:.2}"));
    }
    text
}

/// The proposal as board rows, primary first in each tier, an effort written into its model
/// as the tier table stores it.
pub fn build_board(build: &TierBuild) -> Vec<DispatchTier> {
    let mut rows = Vec::new();
    for (tier, _) in TIER_COLUMNS {
        for (position, entry) in build.tiers.get(*tier).into_iter().flatten().enumerate() {
            let model = if entry.effort.is_empty()
                || entry.model.ends_with(&format!("-{}", entry.effort))
            {
                entry.model.clone()
            } else {
                format!("{}-{}", entry.model, entry.effort)
            };
            rows.push(DispatchTier {
                name: tier.to_string(),
                provider: entry.provider.clone(),
                agent: entry.agent.clone(),
                model,
                position: position as i32,
                ..DispatchTier::default()
            });
        }
    }
    rows
}

/// A prompt for a coding agent to pick a role's models and save them with the CLI, its other
/// fields kept as they are (`dispatch roles save` writes the whole role).
pub fn role_models_prompt(name: &str, profile: &RoleProfile) -> String {
    let general = name == "general";
    let mut lines = Vec::new();
    if general {
        lines.push("Pick the models of Dispatch's tier table, General: the models every run whose role has no models of its own falls back on.".to_string());
    } else {
        lines.push(format!(
            "Pick the models for the Dispatch role \"{name}\": the agent and model that run its tasks at each difficulty tier."
        ));
        lines.push(String::new());
        lines.push(format!("When to use: {}", when_display(&profile.when)));
        if !profile.instructions.trim().is_empty() {
            lines.push(format!("Instructions: {}", profile.instructions.trim()));
        }
        lines.push(format!("Access: {}", access_label(profile.readonly)));
        lines.push(format!("Lowest tier: {}", role_tier(profile)));
    }
    lines.push(String::new());
    lines.push("Read what there is to pick from: `dispatch models` lists every model the installed agent CLIs report, with its skill and price, and `dispatch tiers build --role <role>` shows what the catalog proposes. Choose up to three per tier (easy, normal, hard), primary first, as agent/model with the effort in the model's name where it has one (claude/claude-opus-5-5-high). Weigh how well each model does this kind of work against what it costs: a dearer model only earns its price where the work needs it.".to_string());
    lines.push(String::new());
    if general {
        // The CLI saves only the catalog's proposal (`tiers build --apply`), not a pick.
        lines.push("Then answer with the three lists, one line per tier (easy: agent/model, agent/model), for the board in Dispatch's Roles page, General.".to_string());
    } else {
        lines.push("Then save them with this command, filling in the three lists; it writes the whole role, so keep the other flags as they are:".to_string());
        lines.push(String::new());
        lines.push(roles_save_command(name, profile));
    }
    lines.join("\n")
}

/// `dispatch roles save` with every field the role holds, and the three model lists to fill.
fn roles_save_command(name: &str, profile: &RoleProfile) -> String {
    let quote = |text: &str| format!("'{}'", text.replace('\'', "'\\''"));
    let mut parts = vec!["dispatch roles save".to_string(), name.to_string()];
    let mut text = |flag: &str, value: &str| {
        if !value.trim().is_empty() {
            parts.push(format!("--{flag} {}", quote(value.trim())));
        }
    };
    text("when", &profile.when);
    text("instructions", &profile.instructions);
    text("tier", &profile.tier);
    text("timeout", &profile.timeout);
    text("prefer", &profile.prefer.join(","));
    text("avoid", &profile.avoid.join(","));
    if profile.readonly {
        parts.push("--readonly".to_string());
    }
    if profile.manual {
        parts.push("--manual".to_string());
    }
    let kept = kept_columns(profile);
    if kept.len() < TIER_COLUMNS.len() {
        parts.push(format!("--columns {}", kept.join(",")));
    }
    for tier in kept {
        parts.push(format!("--{tier} '<agent/model,…>'"));
    }
    parts.join(" ")
}

pub fn is_build_empty(build: &TierBuild) -> bool {
    TIER_COLUMNS
        .iter()
        .all(|(id, _)| build.tiers.get(*id).map(Vec::len).unwrap_or(0) == 0)
}

/// The tier table the catalog proposes, from a BuildTiers JSON body.
pub fn parse_tier_build(json: &str) -> Result<TierBuild, String> {
    let value: Value = serde_json::from_str(json).map_err(|err| err.to_string())?;
    let mut tiers = BTreeMap::new();
    if let Some(map) = value.get("tiers").and_then(Value::as_object) {
        for (tier, rows) in map {
            let entries = rows
                .as_array()
                .map(|items| items.iter().map(parse_built_entry).collect())
                .unwrap_or_default();
            tiers.insert(tier.clone(), entries);
        }
    }
    Ok(TierBuild {
        priority: value
            .get("priority")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        applied: value
            .get("applied")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        problems: string_list(value.get("problems")),
        tiers,
    })
}

fn parse_built_entry(value: &Value) -> BuiltEntry {
    let auto = value.get("auto").cloned().unwrap_or(Value::Null);
    BuiltEntry {
        agent: value
            .get("agent")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        model: value
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        provider: value
            .get("provider")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        effort: value
            .get("effort")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        p_pass: json_f64(auto.get("pPass")),
        p_pass_low: json_f64(auto.get("pPassLow")),
        effective_cost: auto.get("effectiveCost").and_then(Value::as_f64),
        qualified: auto
            .get("qualified")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    }
}

fn json_f64(value: Option<&Value>) -> f64 {
    value.and_then(Value::as_f64).unwrap_or(0.0)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaletteModel {
    pub agent: String,
    pub model: String,
    pub provider: String,
    pub tier: String,
    /// The efforts the agent takes beside this model, as the core lists them.
    pub efforts: Vec<String>,
}

/// Models grouped by agent, filtered by the palette search, agents sorted.
pub fn palette_groups(models: &[PaletteModel], query: &str) -> Vec<(String, Vec<PaletteModel>)> {
    let mut by_agent: BTreeMap<String, Vec<PaletteModel>> = BTreeMap::new();
    for model in models {
        if !matches_query(query, &[&model.agent, &model.model, &model.provider]) {
            continue;
        }
        by_agent
            .entry(model.agent.clone())
            .or_default()
            .push(model.clone());
    }
    by_agent.into_iter().collect()
}

/// `model`'s palette card at `effort`, or at the agent's default when `effort` is empty.
pub fn palette_card_at(model: &PaletteModel, effort: &str) -> TierCardInput {
    let mut card = palette_card(model);
    if !effort.is_empty() {
        card.model = format!("{}-{effort}", card.model);
    }
    card
}

/// `tiers` with the card at `slot` swapped for `card`, in place. A card already elsewhere in
/// that tier leaves the board as it was.
pub fn replace_tier_card(
    tiers: &[DispatchTier],
    card: &TierCardInput,
    slot: &TierSlot,
) -> Vec<DispatchTier> {
    let mut columns = tier_columns(tiers);
    let Some(at) = columns.columns.iter().position(|(id, _)| id == &slot.tier) else {
        return tiers.to_vec();
    };
    let column = &mut columns.columns[at].1;
    if slot.index >= column.len()
        || column.iter().enumerate().any(|(index, item)| {
            index != slot.index && item.agent == card.agent && item.model == card.model
        })
    {
        return tiers.to_vec();
    }
    let item = &mut column[slot.index];
    item.agent = card.agent.clone();
    item.model = card.model.clone();
    item.provider = card.provider.clone();
    flatten(&columns)
}

pub fn palette_card(model: &PaletteModel) -> TierCardInput {
    TierCardInput {
        agent: model.agent.clone(),
        model: model.model.clone(),
        provider: model.provider.clone(),
    }
}

/// One card placed on the three-column board, with a stable id for springs.
#[derive(Clone, Debug)]
pub struct PlacedCard {
    pub id: String,
    pub col: usize,
    pub row: usize,
    pub tier: DispatchTier,
}

/// Column and row of each card, in board order. Duplicate agent/model pairs get a numeric suffix.
pub fn placed_cards(tiers: &[DispatchTier]) -> Vec<PlacedCard> {
    let columns = tier_columns(tiers);
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut out = Vec::new();
    for (col, (id, _)) in TIER_COLUMNS.iter().enumerate() {
        for (row, tier) in columns.get(id).iter().enumerate() {
            let base = card_id(&tier.agent, &tier.model, &tier.provider);
            let seen = counts.entry(base.clone()).or_insert(0);
            let key = if *seen == 0 {
                base
            } else {
                format!("{base}\u{1}{seen}")
            };
            *seen += 1;
            out.push(PlacedCard {
                id: key,
                col,
                row,
                tier: (*tier).clone(),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use crate::client::DispatchTier;

    fn tier(name: &str, agent: &str, model: &str, position: i32) -> DispatchTier {
        DispatchTier {
            name: name.into(),
            agent: agent.into(),
            model: model.into(),
            provider: String::new(),
            position,
        }
    }

    #[test]
    fn a_role_keeps_at_least_one_column_and_can_add_the_missing_ones() {
        let profile = super::RoleProfile {
            columns: Some(vec!["easy".into(), "normal".into()]),
            ..super::RoleProfile::default()
        };
        assert_eq!(super::kept_columns(&profile), vec!["easy", "normal"]);
        assert_eq!(
            super::kept_columns(&super::RoleProfile::default()),
            vec!["easy", "normal", "hard"]
        );
        assert_eq!(
            super::remove_column(&["easy".to_string()], "easy"),
            vec!["easy".to_string()],
            "the last remaining column cannot be deleted"
        );
        assert_eq!(
            super::remove_column(&["easy".to_string(), "hard".to_string()], "easy"),
            vec!["hard".to_string()]
        );
        assert_eq!(
            super::add_column(&["hard".to_string()], "easy"),
            vec!["easy".to_string(), "hard".to_string()]
        );
        assert_eq!(
            super::add_column(&["hard".to_string()], "hard"),
            vec!["hard".to_string()]
        );
        assert_eq!(
            super::stored_columns(&["easy".into(), "hard".into()]),
            Some(vec!["easy".into(), "hard".into()])
        );
        assert_eq!(
            super::stored_columns(&["easy".into(), "normal".into(), "hard".into()]),
            None,
            "all three is the absent setting"
        );

        // Deleting a column drops its models from the role.
        let tiers = vec![
            tier("easy", "codex", "small", 0),
            tier("normal", "codex", "mid", 0),
            tier("hard", "codex", "deep", 0),
        ];
        let kept = super::kept_columns(&profile);
        let next = super::profile_with_board(&profile, &tiers);
        assert!(
            next.tiers.iter().all(|entry| entry.tier != "hard"),
            "a deleted column's cards go: {next:?}"
        );
        assert_eq!(super::tier_columns_in(&tiers, &kept).columns.len(), 2);
        assert!(super::tier_columns_in(&tiers, &kept).get("hard").is_empty());
    }

    #[test]
    fn a_swap_puts_the_new_model_in_the_same_slot() {
        let tiers = vec![
            tier("easy", "claude", "claude-haiku-5-5-low", 0),
            tier("easy", "codex", "gpt-6-luna-medium", 1),
        ];
        let card = super::TierCardInput {
            agent: "claude".into(),
            model: "claude-sonnet-5-5-high".into(),
            provider: "anthropic".into(),
        };
        let slot = super::TierSlot {
            tier: "easy".into(),
            index: 1,
        };
        let next = super::replace_tier_card(&tiers, &card, &slot);
        assert_eq!(next.len(), 2);
        assert_eq!(next[0].model, "claude-haiku-5-5-low");
        assert_eq!(
            (next[1].agent.as_str(), next[1].model.as_str()),
            ("claude", "claude-sonnet-5-5-high")
        );
        // A model already elsewhere in the tier leaves the board alone.
        let dup = super::TierCardInput {
            agent: "claude".into(),
            model: "claude-haiku-5-5-low".into(),
            provider: String::new(),
        };
        assert_eq!(super::replace_tier_card(&tiers, &dup, &slot), tiers);
    }

    #[test]
    fn a_palette_effort_goes_into_the_models_name() {
        let model = super::PaletteModel {
            agent: "claude".into(),
            model: "claude-opus-5-5".into(),
            provider: "anthropic".into(),
            tier: String::new(),
            efforts: Vec::new(),
        };
        assert_eq!(
            super::palette_card_at(&model, "max").model,
            "claude-opus-5-5-max"
        );
        assert_eq!(super::palette_card_at(&model, "").model, "claude-opus-5-5");
    }
    #[test]
    fn a_built_board_names_each_model_at_its_effort_once() {
        let entry = |model: &str, effort: &str| super::BuiltEntry {
            agent: "claude".into(),
            model: model.into(),
            provider: "anthropic".into(),
            effort: effort.into(),
            p_pass: 0.9,
            p_pass_low: 0.8,
            effective_cost: None,
            qualified: true,
        };
        let build = super::TierBuild {
            priority: "balanced".into(),
            applied: false,
            problems: Vec::new(),
            tiers: [(
                "hard".to_string(),
                vec![
                    entry("claude-opus-5-5", "high"),
                    entry("claude-sonnet-5-5-high", "high"),
                    entry("claude-haiku-5-5", ""),
                ],
            )]
            .into_iter()
            .collect(),
        };
        let board = super::build_board(&build);
        let models: Vec<_> = board
            .iter()
            .map(|row| (row.name.as_str(), row.model.as_str(), row.position))
            .collect();
        assert_eq!(
            models,
            [
                ("hard", "claude-opus-5-5-high", 0),
                ("hard", "claude-sonnet-5-5-high", 1),
                ("hard", "claude-haiku-5-5", 2),
            ]
        );
    }

    #[test]
    fn the_models_prompt_ends_in_a_command_that_keeps_the_role() {
        let profile = super::RoleProfile {
            when: "Review a diff".into(),
            instructions: "Don't edit files.".into(),
            readonly: true,
            ..Default::default()
        };
        let prompt = super::role_models_prompt("reviewer", &profile);
        assert!(prompt.contains("dispatch roles save reviewer"));
        assert!(prompt.contains("--instructions 'Don'\\''t edit files.'"));
        assert!(prompt.contains("--readonly"));
        // General has no CLI to save a pick: the agent answers with the lists.
        let general = super::role_models_prompt("general", &super::RoleProfile::default());
        assert!(!general.contains("roles save"));
        assert!(general.contains("answer with the three lists"));
    }

    #[test]
    fn a_time_limit_reads_as_the_core_reads_it() {
        for good in ["15m", "1h30m", "90s", "1.5h", "0", "250ms"] {
            assert!(super::is_duration(good), "{good}");
        }
        for bad in ["gdsg", "15", "m", "15 m", "1h30", ""] {
            assert!(!super::is_duration(bad), "{bad}");
        }
    }

    #[test]
    fn the_composer_says_what_the_core_would_refuse() {
        let mut profile = super::RoleProfile {
            when: "tests".into(),
            timeout: "gdsg".into(),
            ..Default::default()
        };
        assert!(
            super::profile_error(&profile)
                .unwrap()
                .contains("not a duration")
        );
        profile.timeout = "15m".into();
        assert_eq!(super::profile_error(&profile), None);
        profile.prefer = vec!["claude".into()];
        profile.avoid = vec!["claude".into()];
        assert!(
            super::profile_error(&profile)
                .unwrap()
                .contains("both preferred and avoided")
        );
        let empty = super::RoleProfile::default();
        assert!(
            super::profile_error(&empty)
                .unwrap()
                .contains("instructions")
        );
    }

    use super::*;

    fn row(name: &str, position: i32, agent: &str, model: &str) -> DispatchTier {
        DispatchTier {
            name: name.to_string(),
            position,
            agent: agent.to_string(),
            model: model.to_string(),
            provider: String::new(),
        }
    }

    fn table() -> Vec<DispatchTier> {
        vec![
            row("easy", 0, "codex", "a"),
            row("easy", 1, "grok", "b"),
            row("hard", 0, "cursor", "c"),
        ]
    }

    fn order(tiers: &[DispatchTier], name: &str) -> Vec<String> {
        tier_columns(tiers)
            .get(name)
            .iter()
            .map(|tier| format!("{}:{}", tier.agent, tier.position))
            .collect()
    }

    #[test]
    fn moves_a_card_within_a_tier_and_renumbers_positions() {
        let tiers = table();
        let next = place_tier_card(
            &tiers,
            &TierCardInput::from_tier(&tiers[1]),
            &TierSlot {
                tier: "easy".into(),
                index: 0,
            },
            Some(&TierSlot {
                tier: "easy".into(),
                index: 1,
            }),
        );
        assert_eq!(order(&next, "easy"), vec!["grok:0", "codex:1"]);
    }

    #[test]
    fn moves_a_card_across_tiers_shifting_the_cards_below_the_slot() {
        let tiers = table();
        let next = place_tier_card(
            &tiers,
            &TierCardInput::from_tier(&tiers[0]),
            &TierSlot {
                tier: "hard".into(),
                index: 0,
            },
            Some(&TierSlot {
                tier: "easy".into(),
                index: 0,
            }),
        );
        assert_eq!(order(&next, "easy"), vec!["grok:0"]);
        assert_eq!(order(&next, "hard"), vec!["codex:0", "cursor:1"]);
    }

    #[test]
    fn appends_a_new_card_past_the_end_and_refuses_a_duplicate_in_the_same_tier() {
        let tiers = table();
        let added = place_tier_card(
            &tiers,
            &TierCardInput {
                agent: "codex".into(),
                model: "z".into(),
                provider: "openai".into(),
            },
            &TierSlot {
                tier: "normal".into(),
                index: 99,
            },
            None,
        );
        assert_eq!(order(&added, "normal"), vec!["codex:0"]);
        let placed = added
            .iter()
            .find(|tier| tier.model == "z")
            .expect("new card");
        assert_eq!(placed.provider, "openai");
        let refused = place_tier_card(
            &tiers,
            &TierCardInput::from_tier(&tiers[0]),
            &TierSlot {
                tier: "easy".into(),
                index: 2,
            },
            None,
        );
        assert_eq!(refused.len(), tiers.len());
    }

    #[test]
    fn removes_a_card() {
        assert_eq!(
            order(
                &remove_tier_card(
                    &table(),
                    &TierSlot {
                        tier: "easy".into(),
                        index: 0
                    }
                ),
                "easy"
            ),
            vec!["grok:0"]
        );
    }

    #[test]
    fn round_trips_a_role_board_tier_by_tier_in_order() {
        let profile = RoleProfile {
            tiers: vec![
                RoleEntry {
                    tier: "hard".into(),
                    agent: "codex".into(),
                    model: "deep".into(),
                    provider: String::new(),
                },
                RoleEntry {
                    tier: "normal".into(),
                    agent: "grok".into(),
                    model: "first".into(),
                    provider: String::new(),
                },
                RoleEntry {
                    tier: "normal".into(),
                    agent: "codex".into(),
                    model: "second".into(),
                    provider: String::new(),
                },
            ],
            ..RoleProfile::default()
        };
        let board = role_board_tiers(&profile);
        let positions: Vec<(String, i32)> = board
            .iter()
            .map(|card| (card.name.clone(), card.position))
            .collect();
        assert_eq!(
            positions,
            vec![
                ("hard".into(), 0),
                ("normal".into(), 0),
                ("normal".into(), 1)
            ]
        );
        assert_eq!(
            board_to_role_tiers(&board),
            vec![
                RoleEntry {
                    tier: "normal".into(),
                    agent: "grok".into(),
                    model: "first".into(),
                    provider: String::new()
                },
                RoleEntry {
                    tier: "normal".into(),
                    agent: "codex".into(),
                    model: "second".into(),
                    provider: String::new()
                },
                RoleEntry {
                    tier: "hard".into(),
                    agent: "codex".into(),
                    model: "deep".into(),
                    provider: String::new()
                },
            ]
        );
    }

    #[test]
    fn reads_the_lowest_tier() {
        assert_eq!(
            role_tier(&RoleProfile {
                tier: "hard".into(),
                ..RoleProfile::default()
            }),
            "Hard or harder"
        );
        assert_eq!(role_tier(&RoleProfile::default()), "Any");
    }

    #[test]
    fn lists_general_before_roles_and_counts_models() {
        let rows = vec![RoleRow {
            name: "reviewer".into(),
            source: RoleSource::Builtin,
            profile: RoleProfile {
                tier: "hard".into(),
                readonly: true,
                when: "Review a diff.".into(),
                tiers: vec![RoleEntry {
                    tier: "hard".into(),
                    agent: "codex".into(),
                    model: "deep-model".into(),
                    provider: String::new(),
                }],
                ..RoleProfile::default()
            },
        }];
        let listed = role_list_rows(&rows, 5);
        let pairs: Vec<(&str, usize)> = listed
            .iter()
            .map(|row| (row.name.as_str(), row.models))
            .collect();
        assert_eq!(pairs, vec![("general", 5), ("reviewer", 1)]);
        assert_eq!(
            listed[0].when,
            "Every run whose role has no models for its tier, and every run with no role."
        );
        assert_eq!(listed[0].access, "As the run asks");
        assert_eq!(models_label("general", 0), "0");
        assert_eq!(models_label("reviewer", 0), "General's");
        assert_eq!(source_text(None, false), "Tier table");
        assert_eq!(
            source_text(Some(RoleSource::User), true),
            "Yours · Only with --role"
        );
        assert_eq!(
            when_display(""),
            "Not picked: it does not say when it fits."
        );
    }

    #[test]
    fn parses_the_roles_list_and_defaults_pick_on() {
        let listed = parse_roles_list(
            r#"{"roles":{"reviewer":{"tier":"hard","readonly":true,"when":"Review a diff.","tiers":[{"tier":"hard","agent":"codex","model":"deep-model"}]},"planner":{"when":"Plan."}},"source":{"reviewer":"builtin","planner":"user"},"builtin":["reviewer"],"pick":true}"#,
        )
        .unwrap();
        assert!(listed.pick);
        assert_eq!(
            listed
                .rows
                .iter()
                .map(|row| row.name.as_str())
                .collect::<Vec<_>>(),
            vec!["planner", "reviewer"]
        );
        assert_eq!(listed.rows[1].source, RoleSource::Builtin);
        assert!(listed.rows[1].profile.readonly);
        assert_eq!(listed.rows[1].profile.tiers.len(), 1);
        let omitted = parse_roles_list(r#"{"roles":{}}"#).unwrap();
        assert!(omitted.pick);
        assert!(
            !parse_roles_list(r#"{"pick":false,"roles":{}}"#)
                .unwrap()
                .pick
        );
    }

    #[test]
    fn lists_new_role_templates_and_only_the_ones_not_in_use() {
        let listed = parse_roles_list(
            r#"{"roles":{"explorer":{"when":"Find it."}},"templates":[{"name":"explorer","profile":{"tier":"easy","readonly":true,"when":"Find it.","instructions":"Answer from the code."},"in_use":true},{"name":"reviewer","profile":{"tier":"hard","readonly":true,"when":"Review it."},"in_use":false}],"pick":true}"#,
        )
        .unwrap();
        assert_eq!(listed.templates.len(), 2);
        assert_eq!(listed.templates[0].name, "explorer");
        assert!(listed.templates[0].in_use);
        assert_eq!(
            listed.templates[0].profile.instructions,
            "Answer from the code."
        );
        let available = available_templates(&listed.templates);
        assert_eq!(
            available
                .iter()
                .map(|template| template.name.as_str())
                .collect::<Vec<_>>(),
            vec!["reviewer"]
        );
        assert!(available_templates(&[]).is_empty());
    }

    #[test]
    fn composes_a_profile_and_rejects_a_reserved_name() {
        let previous = RoleProfile {
            tiers: vec![RoleEntry {
                tier: "easy".into(),
                agent: "grok".into(),
                model: "g".into(),
                provider: String::new(),
            }],
            provider: "keep".into(),
            ..RoleProfile::default()
        };
        let draft = RoleDraft {
            tier: "hard".into(),
            readonly: true,
            manual: false,
            prefer: vec!["codex".into()],
            avoid: Vec::new(),
            instructions: " Review. ".into(),
            when: "  ".into(),
            timeout: "15m".into(),
        };
        let profile = compose_profile(Some(&previous), &draft);
        assert_eq!(profile.tier, "hard");
        assert!(profile.readonly);
        assert!(!profile.manual);
        assert_eq!(profile.instructions, "Review.");
        assert!(profile.when.is_empty());
        assert_eq!(profile.tiers.len(), 1);
        assert_eq!(profile.provider, "keep");
        assert_eq!(
            serde_json::to_string(&compose_profile(
                None,
                &RoleDraft {
                    tier: String::new(),
                    readonly: false,
                    manual: false,
                    prefer: Vec::new(),
                    avoid: Vec::new(),
                    instructions: String::new(),
                    when: String::new(),
                    timeout: String::new(),
                }
            ))
            .unwrap(),
            "{}"
        );
        assert_eq!(
            role_name_error("general").as_deref(),
            Some("General is the tier table; name the role something else.")
        );
        assert!(role_name_error("reviewer").is_none());
        assert!(role_name_error("None").is_some());
        assert!(rename_drops_previous("reviewer", "audit", RoleSource::User));
        assert!(!rename_drops_previous(
            "reviewer",
            "audit",
            RoleSource::Builtin
        ));
    }

    #[test]
    fn reads_a_proposed_tier_table_and_prices_a_row() {
        let view = parse_tier_build(
            r#"{"priority":"balanced","applied":true,"tiers":{"easy":[{"agent":"grok","model":"g","auto":{"pPass":0.9,"pPassLow":0.85,"effectiveCost":0.01,"qualified":true}}]}}"#,
        )
        .unwrap();
        assert!(view.applied);
        assert_eq!(view.priority, "balanced");
        let entry = &view.tiers["easy"][0];
        assert_eq!(entry.agent, "grok");
        assert_eq!(entry.model, "g");
        assert!(entry.qualified);
        assert_eq!(entry.effective_cost, Some(0.01));
        assert_eq!(built_price(entry), "P 0.90 ≥ 0.85 · $0.01");
        assert!(!is_build_empty(&view));
    }

    #[test]
    fn groups_palette_models_by_the_search() {
        let models = vec![
            PaletteModel {
                agent: "grok".into(),
                model: "grok-4".into(),
                provider: "xai".into(),
                tier: "hard".into(),
                efforts: Vec::new(),
            },
            PaletteModel {
                agent: "codex".into(),
                model: "gpt-mini".into(),
                provider: "openai".into(),
                tier: "easy".into(),
                efforts: Vec::new(),
            },
        ];
        let groups = palette_groups(&models, "grok");
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].0, "grok");
        assert!(palette_groups(&models, "missing").is_empty());
        assert!(tier_matches("", &row("easy", 0, "codex", "a")));
        assert!(!tier_matches("cursor", &row("easy", 0, "codex", "a")));
    }
}
