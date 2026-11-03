package dispatch

import (
	"fmt"
	"sort"

	adapter "github.com/polterware/dispatch/go-core/providers/adapters"
)

// TierAuto routes across every tier: the cheapest candidate likely to pass
// this task wins. It is a request, never a recorded run tier: an
// Auto run records the band of its difficulty.
const TierAuto adapter.Tier = "auto"

// PolicyAutoV1 is the Auto policy's version in the route decision log.
const PolicyAutoV1 = "auto-v1"

// RouteAuto is the route kind of an Auto request.
const RouteAuto = "auto"

// AutoInfo is how the Auto policy saw one candidate.
type AutoInfo struct {
	Difficulty    float64  `json:"difficulty"`
	Theta         float64  `json:"theta"`
	ThetaSource   string   `json:"thetaSource"`
	PPass         float64  `json:"pPass"`
	PPassLow      float64  `json:"pPassLow"`
	Cost          *float64 `json:"cost,omitempty"`
	CostSource    string   `json:"costSource,omitempty"`
	Quota         float64  `json:"quotaMultiplier"`
	EffectiveCost *float64 `json:"effectiveCost,omitempty"`
	Threshold     float64  `json:"threshold"`
	Qualified     bool     `json:"qualified"`
}

// passThreshold is the pass bound a candidate must reach, by priority.
func passThreshold(priority string) float64 {
	switch priority {
	case PrioritySpeed:
		return 0.70
	case PriorityQuality:
		return 0.90
	default:
		return 0.80
	}
}

// quotaMultiplier scales a candidate's cost by how its account is pacing:
// spare quota makes it cheaper, a deficit or a window that will not last much
// dearer. Prefer and avoid from a role scale it too.
func quotaMultiplier(c Candidate) float64 {
	q := 1.0
	switch {
	case c.Pace == nil:
	case !c.Pace.WillLast:
		q = 5
	case c.Pace.Deficit:
		q = 3
	default:
		q = clamp(1+c.Pace.Delta/100, 0.5, 1)
	}
	if c.Bias < 0 {
		q *= 0.9
	}
	if c.Bias >= AvoidBias {
		q *= 2
	}
	return q
}

// priceAuto sets each candidate's Auto info for a task of difficulty d,
// keeping their order. A candidate with no known cost is priced at the median
// of the others.
func priceAuto(cs []Candidate, d float64, priority string, cat Catalog) []Candidate {
	out := append([]Candidate(nil), cs...)
	tau := passThreshold(priority)
	var known []float64
	for i := range out {
		c := &out[i]
		est := cat.Estimate(c.Model, c.Effort, c.Strength)
		p, low := PassProbability(est.Theta, d, estimateUncertainty(est))
		c.Auto = &AutoInfo{
			Difficulty: d, Theta: est.Theta, ThetaSource: est.ThetaSource,
			PPass: p, PPassLow: low, Cost: est.CostPerTask, CostSource: est.CostSource,
			Quota: quotaMultiplier(*c), Threshold: tau, Qualified: low >= tau,
		}
		if c.CooledDown {
			c.Auto.Qualified = false
		}
		if est.CostPerTask != nil {
			known = append(known, *est.CostPerTask)
		}
	}
	median := 1.0
	if len(known) > 0 {
		sort.Float64s(known)
		median = known[len(known)/2]
	}
	for i := range out {
		a := out[i].Auto
		cost := median
		if a.Cost != nil {
			cost = *a.Cost
		} else {
			a.CostSource = "price unknown: median"
		}
		a.EffectiveCost = F64(cost * a.Quota)
	}
	return out
}

// rankAuto orders candidates for a task of difficulty d: the ones whose pass
// bound reaches the priority's threshold by effective cost, then the rest by
// pass bound. Ties keep the table order. The first is the greedy pick. Each
// agent's model keeps only its best effort, so a fallback tries another model
// instead of the same one at a level that ranked lower.
func rankAuto(cs []Candidate, d float64, priority string, cat Catalog) []Candidate {
	out := priceAuto(cs, d, priority, cat)
	sort.SliceStable(out, func(i, j int) bool { return autoLess(out[i].Auto, out[j].Auto) })
	seen := map[string]bool{}
	kept := out[:0]
	for _, c := range out {
		k := adapter.BaseModel(c.Model)
		if c.Adapter != nil {
			k = c.Adapter.Name() + "/" + k
		}
		if seen[k] {
			continue
		}
		seen[k] = true
		kept = append(kept, c)
	}
	return kept
}

// autoLess is Auto's order: qualifiers by effective cost, then by bound.
func autoLess(a, b *AutoInfo) bool {
	if a.Qualified != b.Qualified {
		return a.Qualified
	}
	if a.Qualified && *a.EffectiveCost != *b.EffectiveCost {
		return *a.EffectiveCost < *b.EffectiveCost
	}
	return a.PPassLow > b.PPassLow
}

// autoReason explains an Auto candidate in one line.
func autoReason(c Candidate) string {
	a := c.Auto
	verdict := "below"
	if a.Qualified {
		verdict = "meets"
	}
	return fmt.Sprintf("P(pass)=%.2f (bound %.2f %s %.2f), θ=%.2f %s, cost $%.2f × quota %.2f = $%.2f (%s)",
		a.PPass, a.PPassLow, verdict, a.Threshold, a.Theta, a.ThetaSource, *a.EffectiveCost/a.Quota, a.Quota, *a.EffectiveCost, a.CostSource)
}

// autoNote is the run note of an Auto choice.
func autoNote(cs []Candidate) string {
	if len(cs) == 0 || cs[0].Auto == nil {
		return ""
	}
	c, a := cs[0], cs[0].Auto
	pick := c.Model
	if c.Adapter != nil {
		pick = c.Adapter.Name() + "/" + c.Model
	}
	if c.Effort != "" {
		pick += " " + c.Effort
	}
	why := "cheapest likely to pass"
	if !a.Qualified {
		why = "none reaches the bound; most likely to pass"
	}
	return fmt.Sprintf("auto: d=%.2f (%s), picked %s (P=%.2f, $%.2f): %s", a.Difficulty, DifficultyTier(a.Difficulty), pick, a.PPass, *a.EffectiveCost, why)
}
