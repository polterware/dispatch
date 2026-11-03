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
    pub speed: i32,
    pub strength: i32,
}

impl TierCardInput {
    pub fn from_tier(tier: &DispatchTier) -> Self {
        Self {
            agent: tier.agent.clone(),
            model: tier.model.clone(),
            provider: tier.provider.clone(),
            speed: tier.speed,
            strength: tier.strength,
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
    let mut columns: Vec<(String, Vec<DispatchTier>)> = TIER_COLUMNS
        .iter()
        .map(|(id, _)| ((*id).to_string(), Vec::new()))
        .collect();
    for tier in tiers {
        if let Some((_, list)) = columns.iter_mut().find(|(id, _)| id == &tier.name) {
            list.push(tier.clone());
        } else {
            columns.push((tier.name.clone(), vec![tier.clone()]));
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
            speed: card.speed,
            strength: card.strength,
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
    let tallest = TIER_COLUMNS
        .iter()
        .map(|(id, _)| columns.get(id).len())
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RolesList {
    pub rows: Vec<RoleRow>,
    pub builtin: Vec<String>,
    /// `pick` defaults to on when the core omits it.
    pub pick: bool,
}

/// The roles of a checkout, from the core's Subagents `list` JSON body.
pub fn parse_roles_list(json: &str) -> Result<RolesList, String> {
    let value: Value = serde_json::from_str(json).map_err(|err| err.to_string())?;
    let pick = value.get("pick").and_then(Value::as_bool).unwrap_or(true);
    let builtin = string_list(value.get("builtin"));
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
        pick,
    })
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
                speed: 0,
                strength: 0,
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
    next.tiers = board_to_role_tiers(tiers);
    next
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

/// Why a pick does or does not happen, under the switch.
pub fn pick_note(classifier: &str) -> &'static str {
    if classifier.is_empty() {
        "Needs a classifier: set one in Settings › Classifier. Until then a run takes a role only with --role."
    } else if classifier == "openai" {
        "OpenAI Decisions judges difficulty only, so runs stay general. Jev or the agent CLI model picks a role."
    } else {
        "On an Auto run that names no role, the classifier picks the one whose When fits, at P 0.5 or more, else General. The task's difficulty picks the column of that role's board."
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

/// A rename saves the new name, then drops the old one. A built-in name is never dropped, only its copy.
pub fn rename_drops_previous(previous: &str, next: &str, source: RoleSource) -> bool {
    !previous.is_empty() && previous != next && source == RoleSource::User
}

pub fn toggle_name(list: &mut Vec<String>, name: &str) {
    if let Some(index) = list.iter().position(|item| item == name) {
        list.remove(index);
    } else {
        list.push(name.to_string());
    }
}

/// The classifier string inside a Config `get` body.
pub fn classifier_from_config(json: &str) -> String {
    let Ok(value) = serde_json::from_str::<Value>(json) else {
        return String::new();
    };
    value
        .get("classifier")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
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

pub fn palette_card(model: &PaletteModel) -> TierCardInput {
    TierCardInput {
        agent: model.agent.clone(),
        model: model.model.clone(),
        provider: model.provider.clone(),
        speed: 0,
        strength: 0,
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
    use super::*;

    fn row(name: &str, position: i32, agent: &str, model: &str) -> DispatchTier {
        DispatchTier {
            name: name.to_string(),
            position,
            agent: agent.to_string(),
            model: model.to_string(),
            provider: String::new(),
            speed: 0,
            strength: 0,
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
                speed: 0,
                strength: 0,
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
    fn explains_the_pick_switch() {
        assert!(pick_note("").contains("Needs a classifier"));
        assert!(pick_note("openai").contains("stay general"));
        assert!(pick_note("jev").contains("P 0.5 or more"));
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
            },
            PaletteModel {
                agent: "codex".into(),
                model: "gpt-mini".into(),
                provider: "openai".into(),
                tier: "easy".into(),
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
