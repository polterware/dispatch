package dispatch

import (
	"math"
	"strings"

	adapter "github.com/polterware/dispatch/go-core/providers/adapters"
)

// The Auto router's pass model: P(pass | m, x) = σ(θ_m − d(x)), with the
// task's difficulty d(x) from the classifier and θ_m the skill of one model at
// one effort, read from the model catalog (CursorBench, else the Epoch
// Capabilities Index, else tiers.json strength). Nothing is learned from runs:
// every number traces to a catalog row.
const (
	// Difficulty bands give an Auto run the tier its stats use.
	easyBelow   = -2.3
	normalBelow = -1.3
)

// Area is the part of the project a task touches: the kinds and technologies
// of its Structure component. A route decision records it; routing does not
// use it.
type Area struct {
	Kinds []string `json:"kinds,omitempty"`
	Tech  []string `json:"tech,omitempty"`
}

// NewArea reads a component's comma-separated kind and tech fields.
func NewArea(kind, tech string) Area {
	return Area{Kinds: areaTokens(kind), Tech: areaTokens(tech)}
}

func areaTokens(s string) []string {
	var out []string
	seen := map[string]bool{}
	for _, part := range strings.Split(s, ",") {
		t := strings.ToLower(strings.TrimSpace(part))
		if t != "" && !seen[t] {
			seen[t] = true
			out = append(out, t)
		}
	}
	return out
}

// Empty reports an unknown area.
func (a Area) Empty() bool { return len(a.Kinds) == 0 && len(a.Tech) == 0 }

// PassProbability is σ(θ − d) and its lower bound, which asks for a margin u
// that grows the less a benchmark measured the model.
func PassProbability(theta, d, u float64) (p, lcb float64) {
	return sigmoid(theta - d), sigmoid(theta - d - u)
}

// thetaUncertainty is the logit margin a θ needs, by its source.
func thetaUncertainty(source string) float64 {
	switch {
	case source == "unknown" || source == "tiers.json strength":
		return 0.8
	case strings.HasPrefix(source, "eci"):
		return 0.5
	default:
		return 0.3
	}
}

// DifficultyTier is the band of d, the tier an Auto run is recorded under.
func DifficultyTier(d float64) adapter.Tier {
	switch {
	case d < easyBelow:
		return adapter.TierEasy
	case d < normalBelow:
		return adapter.TierNormal
	default:
		return adapter.TierHard
	}
}

func sigmoid(z float64) float64 { return 1 / (1 + math.Exp(-clamp(z, -30, 30))) }

// estimateUncertainty adds a margin for each unmeasured reasoning level.
func estimateUncertainty(est ModelEstimate) float64 {
	return thetaUncertainty(est.ThetaSource) + 0.1*float64(est.ExtrapolatedLevels)
}
