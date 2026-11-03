package dispatch

import (
	"context"
	"fmt"
	"math"
	"time"

	adapter "github.com/polterware/dispatch/go-core/providers/adapters"
)

// Classifier names in the routing policy. Off, the default, leaves Auto
// unavailable: routes follow the tier table.
const (
	ClassifierOff    = ""
	ClassifierJev    = "jev"
	ClassifierOpenAI = "openai"
	ClassifierCLI    = "cli"
)

// ValidClassifier reports a known classifier name, off included.
func ValidClassifier(name string) bool {
	switch name {
	case ClassifierOff, ClassifierJev, ClassifierOpenAI, ClassifierCLI:
		return true
	}
	return false
}

// DifficultyQuestion and DifficultyLevels are the one rubric every classifier
// asks, written in general terms and measured on the router's golden prompts.
const DifficultyQuestion = "How hard is this coding task for an AI coding agent working in the project's repository?"

var DifficultyLevels = [3]string{
	"Easy: trivial and local. A change of a few lines in one place, or a question about existing code. Nothing to design.",
	"Normal: a contained feature or bug fix. Several files in one part of the system, some tests or investigation, and a clear approach.",
	"Hard: broad or deep. Spans many parts of the system or needs design decisions, touches security, money, data migration, or concurrency, or the cause of a bug is unknown and hard to reproduce.",
}

// Task is what a classifier judges: the request, after the card it names.
// Roles, when set, also asks which role fits it: each role's name and when
// it fits, general included.
type Task struct {
	Text  string
	Roles map[string]string
}

// RoleQuestion is what every classifier asks to pick a role.
const RoleQuestion = "Which role fits this coding task best, judged by the kind of work it asks for?"

// RoleAnswer is a classifier's pick among a task's roles: Probs per role when
// it gives them, else only Role.
type RoleAnswer struct {
	Role       string
	Probs      map[string]float64
	Confidence *float64
}

// Difficulty is a classifier's answer. Probs are P(easy), P(normal), P(hard)
// when the classifier gives them; a classifier that only names a tier leaves
// them zero.
type Difficulty struct {
	Probs      [3]float64
	HasProbs   bool
	Tier       adapter.Tier
	D          float64
	Provider   string
	Model      string
	Confidence *float64
	Latency    time.Duration
	// Role answers the task's roles; nil when none were asked or the
	// classifier cannot pick one.
	Role *RoleAnswer
}

// Classifier judges how hard a task is before routing.
type Classifier interface {
	Name() string
	Classify(ctx context.Context, t Task) (Difficulty, error)
}

// difficultyTiers orders the tiers the way Probs do.
var difficultyTiers = [3]adapter.Tier{adapter.TierEasy, adapter.TierNormal, adapter.TierHard}

// bandMid is the typical d of each tier's band, the midpoints of the Auto
// bands (easyBelow, normalBelow).
var bandMid = [3]float64{-2.8, -1.8, -0.8}

// TierFromProbs is the measured rule: a hard tail of 0.2 already makes a task
// hard, because under-routing one costs more than over-routing an easy one.
func TierFromProbs(p [3]float64) adapter.Tier {
	switch {
	case p[2] >= 0.2:
		return adapter.TierHard
	case p[1]+p[2] >= 0.35:
		return adapter.TierNormal
	default:
		return adapter.TierEasy
	}
}

// NewDifficulty completes a classifier's answer: the tier from the
// probabilities, and d as their expected band midpoint kept inside the tier's
// band. Without probabilities, d is the named tier's midpoint.
func NewDifficulty(probs *[3]float64, tier adapter.Tier) (Difficulty, error) {
	if probs == nil {
		i := tierIndex(tier)
		if i < 0 {
			return Difficulty{}, fmt.Errorf("classifier: unknown tier %q", tier)
		}
		return Difficulty{Tier: tier, D: bandMid[i]}, nil
	}
	p, err := normalProbs(*probs)
	if err != nil {
		return Difficulty{}, err
	}
	t := TierFromProbs(p)
	d := 0.0
	for i := range p {
		d += p[i] * bandMid[i]
	}
	lo, hi := bandOf(t)
	return Difficulty{Probs: p, HasProbs: true, Tier: t, D: clamp(d, lo, hi)}, nil
}

func normalProbs(p [3]float64) ([3]float64, error) {
	sum := 0.0
	for _, v := range p {
		if math.IsNaN(v) || math.IsInf(v, 0) || v < 0 {
			return p, fmt.Errorf("classifier: bad probability %v", v)
		}
		sum += v
	}
	if sum <= 0 {
		return p, fmt.Errorf("classifier: probabilities sum to 0")
	}
	for i := range p {
		p[i] /= sum
	}
	return p, nil
}

func tierIndex(t adapter.Tier) int {
	for i, x := range difficultyTiers {
		if x == t {
			return i
		}
	}
	return -1
}

// bandOf is the d interval DifficultyTier maps to t, kept finite and a hair
// inside its edges so the band of d is t again.
func bandOf(t adapter.Tier) (lo, hi float64) {
	const eps = 1e-6
	switch t {
	case adapter.TierEasy:
		return -4, easyBelow - eps
	case adapter.TierNormal:
		return easyBelow, normalBelow - eps
	default:
		return normalBelow, 1
	}
}
