package dispatch

import (
	"context"
	"time"

	adapter "github.com/polterware/dispatch/go-core/providers/adapters"
)

// Policy versions name the routing policy that ordered a run's candidates.
// tier-v1 rows carry the retired per-tier classifier's estimate; tier-v2
// prices a tier route's candidates with the model catalog, as Auto does,
// without reordering them.
const (
	PolicyTierV1 = "tier-v1"
	PolicyTierV2 = "tier-v2"
)

// Route kinds say what the caller asked for.
const (
	RouteTier  = "tier"
	RouteModel = "model"
)

// DecisionCandidate is one candidate as the policy saw it when it ordered them.
// PPass is the catalog's probability that this candidate does the task; nil
// when the run had no difficulty to price it with.
type DecisionCandidate struct {
	Agent           string   `json:"agent"`
	Model           string   `json:"model"`
	Effort          string   `json:"effort,omitempty"`
	PPass           *float64 `json:"pPass,omitempty"`
	PPassLow        *float64 `json:"pPassLow,omitempty"`
	Price           *float64 `json:"price,omitempty"`
	QuotaMultiplier float64  `json:"quotaMultiplier,omitempty"`
	EffectiveCost   *float64 `json:"effectiveCost,omitempty"`
	Theta           *float64 `json:"theta,omitempty"`
	ThetaSource     string   `json:"thetaSource,omitempty"`
	Qualified       bool     `json:"qualified,omitempty"`
	Score           float64  `json:"score"`
}

// RouteDecision is the routing record of one new run: the classifier's
// answer, the ordered candidates, and the one tried first. It never holds the prompt.
type RouteDecision struct {
	RunID         string
	ProjectID     string
	PolicyVersion string
	Route         string
	Priority      string
	Classifier    string
	Probs         []float64
	Difficulty    *float64
	Area          Area
	Candidates    []DecisionCandidate
	Chosen        int
	CreatedAt     time.Time
}

// routeKind names what a run's request asked for: a model or provider filter
// pins the route, otherwise the tier table ordered it.
func routeKind(rec Run) string {
	if rec.RequestedTier == string(TierAuto) {
		return RouteAuto
	}
	if rec.Requested != nil && (rec.Requested.Model != "" || rec.Requested.Provider != "") {
		return RouteModel
	}
	if adapter.Tier(rec.Tier).Valid() {
		return RouteTier
	}
	return RouteModel
}

// decisionCandidates records the ordered candidates as the policy priced them.
func decisionCandidates(cs []Candidate) []DecisionCandidate {
	out := make([]DecisionCandidate, 0, len(cs))
	for _, c := range cs {
		dc := DecisionCandidate{Model: c.Model, Effort: c.Effort, Score: Score(c)}
		if c.Adapter != nil {
			dc.Agent = c.Adapter.Name()
		}
		if a := c.Auto; a != nil {
			dc.PPass, dc.PPassLow, dc.Price, dc.QuotaMultiplier, dc.EffectiveCost = F64(a.PPass), F64(a.PPassLow), a.Cost, a.Quota, a.EffectiveCost
			dc.Theta, dc.ThetaSource, dc.Qualified = F64(a.Theta), a.ThetaSource, a.Qualified
		}
		out = append(out, dc)
	}
	return out
}

// recordRouteDecision logs how a new run's candidates were ordered. Resumed
// runs reuse their session's candidate and are not routing decisions. A store
// without the log, or a failed write, never fails the run.
func (d Deps) recordRouteDecision(ctx context.Context, rec Run, cs []Candidate) {
	if d.Store == nil || rec.ResumedOf != "" || len(cs) == 0 {
		return
	}
	dec := RouteDecision{
		RunID: rec.ID, ProjectID: rec.ProjectID, Route: routeKind(rec), Area: rec.Area,
		Classifier: rec.Classifier, Probs: append([]float64(nil), rec.DifficultyProbs...), Difficulty: rec.Difficulty,
		PolicyVersion: PolicyTierV2, Candidates: decisionCandidates(cs), CreatedAt: d.now(),
	}
	if rec.Requested != nil {
		dec.Priority = rec.Requested.Priority
	}
	if dec.Priority == "" {
		dec.Priority = PriorityBalanced
	}
	if a := cs[0].Auto; a != nil {
		dec.Difficulty = F64(a.Difficulty)
		if dec.Route == RouteAuto {
			dec.PolicyVersion = PolicyAutoV1
		}
	}
	_ = d.Store.SaveRouteDecision(ctx, dec)
}

// priceTierRoute prices a tier or model route's candidates with the model
// catalog, in their routed order, when the run has a classified difficulty, so
// the decision log and the bar warning use the same estimate as Auto. With no
// difficulty the candidates stay unpriced.
func (d Deps) priceTierRoute(rec Run, cs []Candidate) []Candidate {
	if rec.Difficulty == nil {
		return cs
	}
	diff := *rec.Difficulty
	priority := PriorityBalanced
	if rec.Requested != nil && rec.Requested.Priority != "" {
		priority = rec.Requested.Priority
	}
	return priceAuto(cs, diff, priority, LoadCatalog(d.stateDir()))
}
