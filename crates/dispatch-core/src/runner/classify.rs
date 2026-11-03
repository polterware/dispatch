use super::{
    Candidate, Classification, Deps, ERR_AUTO_OFF, Options, RoleAnswer, TIER_AUTO, Task,
    apply_picked, join_note, msg, validate_options,
};
use crate::{
    Error,
    adapters::valid_tier,
    difficulty::Difficulty,
    pass_model::difficulty_tier,
    roles::{GENERAL_ROLE, pick_role, pick_roles, raise_to_floor},
    types::{RolePick, Run},
};
use std::collections::BTreeMap;
use std::time::Duration;

/// Bounds one classification, the CLI classifier included.
const CLASSIFY_TIMEOUT: Duration = Duration::from_secs(90);

fn tier_index(t: &str) -> i32 {
    match t {
        "easy" => 0,
        "normal" => 1,
        "hard" => 2,
        _ => -1,
    }
}

impl Deps {
    /// Settles a new request's route before it is recorded. A request with no
    /// tier, model, or provider is Auto when a classifier is on, else the
    /// normal tier; an Auto one asks the classifier, and a failed
    /// classification falls back to the normal tier. The note says why the
    /// route changed.
    pub(crate) async fn resolve_auto(&self, o: &mut Options) -> Result<String, Error> {
        if o.tier.is_empty() && o.filters.model.is_empty() && o.filters.provider.is_empty() {
            if self.classifier.is_none() {
                o.tier = "normal".into();
                if tier_index(&o.tier_floor) > tier_index(&o.tier) {
                    o.tier.clone_from(&o.tier_floor);
                    return Ok(format!(
                        "auto is off (no classifier): role {}'s {} tier",
                        o.profile, o.tier
                    ));
                }
                return Ok("auto is off (no classifier): normal tier".into());
            }
            o.tier = TIER_AUTO.into();
        }
        if o.tier != TIER_AUTO || o.difficulty.is_some() {
            return Ok(String::new());
        }
        let Some(classifier) = self.classifier.clone() else {
            return Err(msg(ERR_AUTO_OFF));
        };
        validate_options(o)?;
        let roles = self.pick_roles(o);
        let task = Task {
            text: task_text(&o.card_text, &o.prompt),
            roles: roles.clone(),
        };
        let answer = match tokio::time::timeout(CLASSIFY_TIMEOUT, classifier.classify(task)).await {
            Ok(r) => r,
            Err(_) => Err(msg("context deadline exceeded")),
        };
        let mut diff = match answer {
            Ok(d) => d,
            Err(e) => {
                o.tier = "normal".into();
                return Ok(format!(
                    "classifier {} failed ({e}): normal tier",
                    classifier.name()
                ));
            }
        };
        if diff.provider.is_empty() {
            diff.provider = classifier.name();
        }
        let mut note = String::new();
        if let (Some(roles), Some(answer)) = (&roles, diff.role.clone()) {
            note = self.apply_role(o, &answer, roles, &diff)?;
        }
        let floor = o.tier_floor.clone();
        let profile = o.profile.clone();
        note = join_note(
            &note,
            &raise_to_floor(&mut diff.difficulty, &floor, &profile),
        );
        o.difficulty = Some(diff);
        Ok(note)
    }

    /// The roles the classifier chooses among for a request, None when it
    /// picks none: the request named one or `--role none`, the policy turns
    /// the pick off, or no role is pickable.
    fn pick_roles(&self, o: &Options) -> Option<BTreeMap<String, String>> {
        if !o.profile.is_empty() || o.no_profile || !self.policy.roles {
            return None;
        }
        let roles = pick_roles(&self.profiles);
        (!roles.is_empty()).then_some(roles)
    }

    /// Records the classifier's pick and applies the picked role; general
    /// keeps the plain route.
    fn apply_role(
        &self,
        o: &mut Options,
        answer: &RoleAnswer,
        roles: &BTreeMap<String, String>,
        diff: &Classification,
    ) -> Result<String, Error> {
        let role = pick_role(&answer.role, &answer.probs, roles);
        o.pick = Some(RolePick {
            auto: true,
            classifier: diff.provider.clone(),
            role: role.clone(),
            probs: answer.probs.clone(),
            confidence: answer.confidence,
        });
        if role == GENERAL_ROLE {
            return Ok(String::new());
        }
        let mut note = format!("role {role} picked");
        if let Some(p) = answer.probs.get(&role) {
            note.push_str(&format!(" (P={p:.2})"));
        }
        let profile = self.profiles.get(&role).cloned().unwrap_or_default();
        apply_picked(o, &role, &profile)?;
        Ok(note)
    }
}

/// What a classifier reads: the card the run names, then the request.
pub fn task_text(card: &str, prompt: &str) -> String {
    let card = card.trim();
    if card.is_empty() {
        return prompt.into();
    }
    format!("{card}\n\nRequest:\n{prompt}")
}

/// Writes the route a new request settled on: an Auto run records the band
/// of its difficulty as its tier.
pub(crate) fn record_difficulty(rec: &mut Run, o: &Options) {
    if o.tier == TIER_AUTO
        && let Some(diff) = &o.difficulty
    {
        rec.requested_tier = TIER_AUTO.into();
        rec.tier = difficulty_tier(diff.difficulty.d).into();
        rec.classifier.clone_from(&diff.provider);
        rec.difficulty = Some(diff.difficulty.d);
        if let Some(p) = diff.difficulty.probs {
            rec.difficulty_probs = p.to_vec();
        }
        return;
    }
    rec.tier.clone_from(&o.tier);
    if valid_tier(&rec.tier) {
        rec.requested_tier.clone_from(&rec.tier);
    }
}

/// The difficulty a recorded Auto run was routed on.
pub fn run_difficulty(rec: &Run) -> Option<Classification> {
    let d = rec.difficulty?;
    let mut diff = Classification::from(Difficulty {
        probs: None,
        tier: difficulty_tier(d).into(),
        d,
    });
    diff.provider.clone_from(&rec.classifier);
    if let [a, b, c] = rec.difficulty_probs[..] {
        diff.difficulty.probs = Some([a, b, c]);
    }
    Some(diff)
}

/// The note of a tier route whose first candidate the router model does not
/// expect to pass: its bound is under the priority's bar.
pub fn tier_warning(cs: &[Candidate]) -> String {
    let Some(c) = cs.first() else {
        return String::new();
    };
    let Some(a) = c.route.auto.as_ref().filter(|a| !a.qualified) else {
        return String::new();
    };
    format!(
        "auto: {} P(pass)={:.2} (bound {:.2}) below the bar {:.2} at d={:.2}",
        c.route.model, a.p_pass, a.p_pass_low, a.threshold, a.difficulty
    )
}
