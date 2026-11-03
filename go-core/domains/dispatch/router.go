package dispatch

import (
	"fmt"
	"math"
	"sort"
	"time"

	adapter "github.com/polterware/dispatch/go-core/providers/adapters"
)

const (
	PrioritySpeed    = "speed"
	PriorityBalanced = "balanced"
	PriorityQuality  = "quality"

	// priorK is the shrinkage count in the blend n/(n+k).
	priorK = 5.0
	// attrUnit maps a 1–5 static attribute onto the score. 5 → -200, 1 → +200, 3 → 0.
	attrUnit = 100.0
	// neutralP50Ms is the duration that scores as a neutral measurement.
	neutralP50Ms = 60_000.0
	// durationScale is wide enough that, at n == MinSamples and a 50/50
	// blend, a reversed measurement overrides a 1-vs-5 prior.
	durationScale = 200.0
	termClamp     = 800.0
)

// ValidPriority reports whether p is speed, balanced, or quality.
// Empty is not valid; callers treat empty as the balanced default.
func ValidPriority(p string) bool {
	switch p {
	case PrioritySpeed, PriorityBalanced, PriorityQuality:
		return true
	default:
		return false
	}
}

// Candidate is one adapter/model able to serve the requested tier.
type Candidate struct {
	Adapter  adapter.Adapter
	Provider string
	Effort   string
	Note     string
	Model    string
	Quota    *adapter.Quota // nil when unknown
	Pace     *Pace
	Hist     *History // nil unless >=5 complete weeks are on record
	// CooledDown marks an agent that recently hit a rate limit.
	CooledDown bool
	// Bias is a soft preference added to the score (negative = preferred).
	Bias float64
	// Priority selects score weights. Empty and balanced add nothing.
	Priority string
	// Tier segments measured evidence. Empty means the run had no tier.
	Tier string
	// Speed and Strength are static 1–5 priors from tiers.json. Zero is neutral.
	Speed, Strength int
	Evidence        ModelEvidence
	// Auto is how the Auto policy priced this candidate; nil on tier routes.
	Auto *AutoInfo
}

// Term is one addend of Score. The sum of Terms equals Score.
type Term struct {
	Name  string  `json:"name"`
	Value float64 `json:"value"`
	Note  string  `json:"note,omitempty"`
}

// Rank orders candidates: known reserve first (most reserve on top), then
// unknown quota, then deficit that still lasts, then deficit that runs out.
// Priority only changes weights; it does not add or drop candidates.
func Rank(cs []Candidate) []Candidate {
	out := append([]Candidate(nil), cs...)
	sort.SliceStable(out, func(i, j int) bool { return routerScore(out[i]) < routerScore(out[j]) })
	return out
}

// LowQuotaPercent is the used share at which a tier route passes over an
// account for the next entry of the table.
const LowQuotaPercent = 90

// LowQuota reports whether the account is nearly spent or runs out before its
// window resets. Unknown quota is not low.
func LowQuota(c Candidate) bool {
	return c.Pace != nil && (c.Pace.Actual >= LowQuotaPercent || !c.Pace.WillLast)
}

// OrderTier orders a tier route the way the table reads: entries in table
// order, an account with low quota after every usable one, and a rate-limited
// one last. Prefer and Avoid still move an entry within its group; low
// accounts go by the most left over.
func OrderTier(cs []Candidate) []Candidate {
	group := func(c Candidate) int {
		switch {
		case c.CooledDown:
			return 2
		case LowQuota(c):
			return 1
		}
		return 0
	}
	out := append([]Candidate(nil), cs...)
	sort.SliceStable(out, func(i, j int) bool {
		gi, gj := group(out[i]), group(out[j])
		if gi != gj {
			return gi < gj
		}
		if out[i].Bias != out[j].Bias {
			return out[i].Bias < out[j].Bias
		}
		if gi == 1 {
			return out[i].Pace.Actual < out[j].Pace.Actual
		}
		return false
	})
	return out
}

func routerScore(c Candidate) float64 {
	base := paceBase(c)
	if c.CooledDown {
		base += 10000
	}
	return base + c.Bias + priorityExtra(c)
}

// paceParts splits the legacy pace score into the quota band and history.
// Their sum is the pre-priority base, cooldown and bias excluded.
func paceParts(c Candidate) (base, hist float64) {
	switch {
	case c.Pace == nil:
		base = 1000 // unknown: after reserve, before deficit
	case !c.Pace.WillLast:
		base = 3000 + c.Pace.Delta
	case c.Pace.Deficit:
		base = 2000 + c.Pace.Delta
	default:
		base = c.Pace.Delta // negative = reserve, more negative first
	}
	if c.Hist != nil && c.Hist.Valid {
		hist = c.Hist.Risk * 40
	}
	return base, hist
}

func paceBase(c Candidate) float64 {
	base, hist := paceParts(c)
	return base + hist
}

// priorityExtra is zero for balanced and empty, so Score stays identical
// to the router from before priorities existed.
func priorityExtra(c Candidate) float64 {
	switch c.Priority {
	case PrioritySpeed:
		// One extra copy of the pace component, plus duration.
		return paceBase(c) + durationTerm(c)
	case PriorityQuality:
		return qualityTerm(c)
	default:
		return 0
	}
}

func attrTerm(v int) float64 {
	if v <= 0 {
		return 0
	}
	return float64(3-v) * attrUnit
}

func blendPrior(prior, measured float64, n int) float64 {
	if n < MinSamples {
		return prior
	}
	nf := float64(n)
	return (priorK*prior + nf*measured) / (priorK + nf)
}

func durationTerm(c Candidate) float64 {
	prior := attrTerm(c.Speed)
	ev := c.Evidence
	if ev.DurationN < MinSamples || ev.P50Ms == nil || *ev.P50Ms <= 0 {
		return prior
	}
	measured := durationScale * math.Log2(float64(*ev.P50Ms)/neutralP50Ms)
	return blendPrior(prior, clamp(measured, -termClamp, termClamp), ev.DurationN)
}

// qualityTerm is the tiers.json strength prior; no run outcome changes it.
func qualityTerm(c Candidate) float64 { return attrTerm(c.Strength) }

func durationNote(c Candidate) string {
	ev := c.Evidence
	if ev.DurationN < MinSamples || ev.P50Ms == nil || *ev.P50Ms <= 0 {
		if c.Speed == 0 {
			return "neutral"
		}
		return "static prior"
	}
	return fmt.Sprintf("blended n/(n+%g) samples=%d", priorK, ev.DurationN)
}

func qualityNote(c Candidate) string {
	if c.Strength == 0 {
		return "neutral"
	}
	return "static prior"
}

// Evaluate fills Pace from Quota at the given time.
func Evaluate(c *Candidate, now time.Time) {
	if c.Quota == nil {
		return
	}
	w := time.Duration(c.Quota.WindowMinutes) * time.Minute
	p := Linear(c.Quota.UsedPercent, w, c.Quota.ResetsAt, now)
	c.Pace = &p
}

// Bias values for roles. Preferring breaks ties inside a pace
// band; avoiding pushes an agent past unknown quota but not past deficit.
const (
	PreferBias = -50
	AvoidBias  = 1500
)

// Score is the same ordering value used by Rank; lower wins.
func Score(c Candidate) float64 { return routerScore(c) }

// Terms lists the addends of Score. Their sum equals Score.
func Terms(c Candidate) []Term {
	base, hist := paceParts(c)
	out := []Term{{Name: "pace", Value: base}}
	if hist != 0 {
		out = append(out, Term{Name: "history", Value: hist})
	}
	if c.CooledDown {
		out = append(out, Term{Name: "cooldown", Value: 10000})
	}
	if c.Bias != 0 {
		out = append(out, Term{Name: "bias", Value: c.Bias})
	}
	switch c.Priority {
	case PrioritySpeed:
		out = append(out,
			Term{Name: "paceSlack", Value: paceBase(c)},
			Term{Name: "duration", Value: durationTerm(c), Note: durationNote(c)},
		)
	case PriorityQuality:
		out = append(out, Term{Name: "quality", Value: qualityTerm(c), Note: qualityNote(c)})
	}
	return out
}

// Reason exposes the observations that contributed to a routing score.
func Reason(c Candidate) string {
	basis := "quota unknown"
	if c.Pace != nil {
		basis = fmt.Sprintf("pace delta=%.2f deficit=%t willLast=%t", c.Pace.Delta, c.Pace.Deficit, c.Pace.WillLast)
	}
	if c.Hist != nil && c.Hist.Valid {
		basis += fmt.Sprintf(" historyRisk=%.2f", c.Hist.Risk)
	}
	msg := fmt.Sprintf("score=%.2f (%s; bias=%.2f)", routerScore(c), basis, c.Bias)
	switch c.Priority {
	case PrioritySpeed:
		msg += fmt.Sprintf("; speed paceSlack=%.2f duration=%.2f (%s)", paceBase(c), durationTerm(c), durationNote(c))
	case PriorityQuality:
		msg += fmt.Sprintf("; quality=%.2f (%s)", qualityTerm(c), qualityNote(c))
	}
	return msg
}

func candidateLabel(c Candidate) string {
	name := "?"
	if c.Adapter != nil {
		name = c.Adapter.Name()
	}
	if c.Model == "" {
		return name
	}
	return name + "/" + c.Model
}

func sameChoice(a, b Candidate) bool {
	an, bn := "", ""
	if a.Adapter != nil {
		an = a.Adapter.Name()
	}
	if b.Adapter != nil {
		bn = b.Adapter.Name()
	}
	return an == bn && a.Model == b.Model && a.Effort == b.Effort
}

// PriorityEffect says whether priority changed the winner versus balanced.
// chosen and balanced are already ranked; the first element is the winner.
func PriorityEffect(priority string, chosen, balanced []Candidate) string {
	if len(chosen) == 0 {
		return ""
	}
	if priority == "" || priority == PriorityBalanced {
		return "balanced: score weights unchanged from the current router"
	}
	if len(balanced) == 0 {
		return priority + ": no balanced candidate"
	}
	w, b := chosen[0], balanced[0]
	if sameChoice(w, b) {
		return fmt.Sprintf("%s: winner stays %s; extra weights did not change the balanced order", priority, candidateLabel(w))
	}
	var why string
	switch priority {
	case PrioritySpeed:
		why = fmt.Sprintf("duration %.2f vs %.2f, paceSlack %.2f vs %.2f (lower wins)", durationTerm(w), durationTerm(b), paceBase(w), paceBase(b))
	default:
		why = fmt.Sprintf("quality %.2f vs %.2f from strength (lower wins)", qualityTerm(w), qualityTerm(b))
	}
	return fmt.Sprintf("%s: winner moved from %s to %s; %s", priority, candidateLabel(b), candidateLabel(w), why)
}
