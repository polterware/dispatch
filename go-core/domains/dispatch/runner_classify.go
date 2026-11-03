package dispatch

import (
	"context"
	"errors"
	"fmt"
	"strings"
	"time"

	adapter "github.com/polterware/dispatch/go-core/providers/adapters"
)

// ErrAutoOff is an explicit Auto request with no classifier configured.
var ErrAutoOff = errors.New("auto needs a classifier: set one with `config --classifier jev|openai|cli`")

// classifyTimeout bounds one classification, the CLI classifier included.
const classifyTimeout = 90 * time.Second

// resolveAuto settles a new request's route before it is recorded. A request
// with no tier, model, or provider is Auto when a classifier is on, else the
// normal tier; an Auto one asks the classifier, and a failed classification
// falls back to the normal tier. The note says why the route changed.
func (d Deps) resolveAuto(ctx context.Context, o *Options) (string, error) {
	if o.Tier == "" && o.Filters.Model == "" && o.Filters.Provider == "" {
		if d.Classifier == nil {
			o.Tier = adapter.TierNormal
			if tierIndex(o.TierFloor) > tierIndex(o.Tier) {
				o.Tier = o.TierFloor
				return fmt.Sprintf("auto is off (no classifier): role %s's %s tier", o.Profile, o.Tier), nil
			}
			return "auto is off (no classifier): normal tier", nil
		}
		o.Tier = TierAuto
	}
	if o.Tier != TierAuto || o.difficulty != nil {
		return "", nil
	}
	if d.Classifier == nil {
		return "", ErrAutoOff
	}
	if err := validateOptions(*o); err != nil {
		return "", err
	}
	cctx, cancel := context.WithTimeout(ctx, classifyTimeout)
	defer cancel()
	roles := d.pickRoles(*o)
	diff, err := d.Classifier.Classify(cctx, Task{Text: TaskText(o.CardText, o.Prompt), Roles: roles})
	if err != nil {
		o.Tier = adapter.TierNormal
		return fmt.Sprintf("classifier %s failed (%v): normal tier", d.Classifier.Name(), err), nil
	}
	if diff.Provider == "" {
		diff.Provider = d.Classifier.Name()
	}
	note := ""
	if roles != nil && diff.Role != nil {
		note, err = d.applyRole(o, *diff.Role, roles, &diff)
		if err != nil {
			return "", err
		}
	}
	note = joinNote(note, raiseToFloor(&diff, o.TierFloor, o.Profile))
	o.difficulty = &diff
	return note, nil
}

// pickRoles is the roles the classifier chooses among for a request, nil when
// it picks none: the request named one or `--role none`, the policy turns the
// pick off, or no role is pickable.
func (d Deps) pickRoles(o Options) map[string]string {
	if o.Profile != "" || o.NoProfile || !d.Policy.Roles {
		return nil
	}
	return PickRoles(d.Profiles)
}

// applyRole records the classifier's pick and applies the picked role;
// general keeps the plain route.
func (d Deps) applyRole(o *Options, answer RoleAnswer, roles map[string]string, diff *Difficulty) (string, error) {
	role := PickRole(answer, roles)
	o.pick = &RolePick{Auto: true, Classifier: diff.Provider, Role: role, Probs: answer.Probs, Confidence: answer.Confidence}
	if role == GeneralRole {
		return "", nil
	}
	note := "role " + role + " picked"
	if p, ok := answer.Probs[role]; ok {
		note += fmt.Sprintf(" (P=%.2f)", p)
	}
	if err := applyPicked(o, role, d.Profiles[role]); err != nil {
		return "", err
	}
	return note, nil
}

// TaskText is what a classifier reads: the card the run names, then the request.
func TaskText(card, prompt string) string {
	card = strings.TrimSpace(card)
	if card == "" {
		return prompt
	}
	return card + "\n\nRequest:\n" + prompt
}

// recordDifficulty writes the route a new request settled on: an Auto run
// records the band of its difficulty as its tier.
func recordDifficulty(rec *Run, o Options) {
	if o.Tier == TierAuto && o.difficulty != nil {
		diff := o.difficulty
		rec.RequestedTier, rec.Tier = string(TierAuto), string(DifficultyTier(diff.D))
		rec.Classifier, rec.Difficulty = diff.Provider, F64(diff.D)
		if diff.HasProbs {
			rec.DifficultyProbs = append([]float64(nil), diff.Probs[:]...)
		}
		return
	}
	rec.Tier = string(o.Tier)
	if adapter.Tier(rec.Tier).Valid() {
		rec.RequestedTier = rec.Tier
	}
}

// runDifficulty is the difficulty a recorded Auto run was routed on.
func runDifficulty(rec Run) *Difficulty {
	if rec.Difficulty == nil {
		return nil
	}
	diff := &Difficulty{D: *rec.Difficulty, Tier: DifficultyTier(*rec.Difficulty), Provider: rec.Classifier}
	if len(rec.DifficultyProbs) == 3 {
		copy(diff.Probs[:], rec.DifficultyProbs)
		diff.HasProbs = true
	}
	return diff
}

// tierWarning is the note of a tier route whose first candidate the router
// model does not expect to pass: its bound is under the priority's bar.
func tierWarning(cs []Candidate) string {
	if len(cs) == 0 || cs[0].Auto == nil || cs[0].Auto.Qualified {
		return ""
	}
	a := cs[0].Auto
	return fmt.Sprintf("auto: %s P(pass)=%.2f (bound %.2f) below the bar %.2f at d=%.2f", cs[0].Model, a.PPass, a.PPassLow, a.Threshold, a.Difficulty)
}
