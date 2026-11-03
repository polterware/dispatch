package dispatch

import (
	"context"
	"errors"
	"strings"
	"testing"

	adapter "github.com/polterware/dispatch/go-core/providers/adapters"
)

// A cheap model with θ 0 and a strong one with θ ≈ 1.39, both from CursorBench.
func autoCatalog() Catalog {
	return Catalog{Scores: []BenchScore{
		{Model: "cheap", Score: 0.5, CostPerTask: F64(0.5)},
		{Model: "strong", Score: 0.8, CostPerTask: F64(1.0)},
	}}
}

func TestRankAuto(t *testing.T) {
	deficit := &Pace{Deficit: true, WillLast: true, Delta: 20}
	for _, tc := range []struct {
		name     string
		d        float64
		priority string
		cheap    *Pace
		want     string
		quali    bool
	}{
		{"the cheap model qualifies and wins", -2.3, PriorityBalanced, nil, "cheap", true},
		{"a quota deficit flips the winner", -2.3, PriorityBalanced, deficit, "strong", true},
		{"nobody qualifies, so the most likely to pass wins", 0, PriorityBalanced, nil, "strong", false},
		{"balanced takes the cheap model", -2, PriorityBalanced, nil, "cheap", true},
		{"quality asks for the stronger one", -2, PriorityQuality, nil, "strong", true},
	} {
		t.Run(tc.name, func(t *testing.T) {
			cs := []Candidate{{Model: "cheap", Pace: tc.cheap}, {Model: "strong"}}
			got := rankAuto(cs, tc.d, tc.priority, autoCatalog())
			if got[0].Model != tc.want || got[0].Auto.Qualified != tc.quali {
				t.Fatalf("got %s qualified=%v: %+v / %+v", got[0].Model, got[0].Auto.Qualified, *got[0].Auto, *got[1].Auto)
			}
		})
	}
}

func TestRankAutoPricesUnknownModelsAtTheMedian(t *testing.T) {
	got := rankAuto([]Candidate{{Model: "mystery"}, {Model: "cheap"}}, -2.3, PriorityBalanced, autoCatalog())
	for _, c := range got {
		if c.Model == "mystery" && (c.Auto.EffectiveCost == nil || *c.Auto.EffectiveCost != 0.5 || c.Auto.ThetaSource != "unknown") {
			t.Fatalf("mystery %+v", *c.Auto)
		}
	}
}

func TestRunWithoutATierIsAutoWithAClassifier(t *testing.T) {
	var ran []string
	d := testDeps(t, &ran,
		fakeAdapter{name: "a", err: errors.New("rate limited"), ran: &ran},
		fakeAdapter{name: "b", out: "hello\n", ran: &ran})
	d.Classifier = easyClassifier()
	rec, err := d.Run(context.Background(), Options{Prompt: "fix the footer link"})
	if err != nil || rec.Status != StatusCompleted {
		t.Fatalf("%+v %v", rec, err)
	}
	if rec.RequestedTier != string(TierAuto) || rec.Tier != string(adapter.TierEasy) || !strings.Contains(rec.Note, "auto: d=") {
		t.Fatalf("requested %q tier %q note %q", rec.RequestedTier, rec.Tier, rec.Note)
	}
	decs, _ := d.Store.ListRouteDecisions(context.Background(), "")
	if len(decs) != 1 || decs[0].PolicyVersion != PolicyAutoV1 || decs[0].Route != RouteAuto || decs[0].Difficulty == nil {
		t.Fatalf("decisions %+v", decs)
	}
	if c := decs[0].Candidates[0]; c.PPass == nil || c.EffectiveCost == nil || c.Theta == nil {
		t.Fatalf("candidate %+v", c)
	}
}

func TestRouteAutoReportsDifficultyAndPrices(t *testing.T) {
	var ran []string
	d := testDeps(t, &ran, fakeAdapter{name: "a", ran: &ran}, fakeAdapter{name: "b", ran: &ran})
	d.Classifier = easyClassifier()
	r, err := d.Route(context.Background(), Options{Prompt: "fix the footer link"})
	if err != nil || r.Difficulty == nil || r.Classifier != "fake" || len(r.Probs) != 3 || len(r.Candidates) != 2 || r.Candidates[0].Auto == nil {
		t.Fatalf("%+v %v", r, err)
	}
	if !strings.Contains(r.Candidates[0].Reason, "P(pass)=") {
		t.Fatalf("reason %q", r.Candidates[0].Reason)
	}
	if _, err := d.Route(context.Background(), Options{Tier: TierAuto, Filters: Filters{Model: "m1"}}); err == nil {
		t.Fatal("auto with a model filter must be refused")
	}
}

func effortCatalog() Catalog {
	return Catalog{Scores: []BenchScore{
		{Model: "elastic", Effort: "low", Score: sigmoid(-1), CostPerTask: F64(0.3)},
		{Model: "elastic", Effort: "medium", Score: 0.5, CostPerTask: F64(0.5)},
		{Model: "elastic", Effort: "high", Score: sigmoid(1), CostPerTask: F64(0.8)},
		{Model: "dearer", Effort: "medium", Score: sigmoid(2), CostPerTask: F64(2)},
	}}
}

func TestAutoPicksMediumOverADearerModelAndKeepsOneEffortPerAgentModel(t *testing.T) {
	d, _, _ := routingDeps(t)
	d.Table = Table{Tiers: map[adapter.Tier][]Entry{adapter.TierEasy: {
		{Agent: "a", Model: "elastic", Efforts: []string{"low", "medium", "high"}},
		{Agent: "a", Model: "dearer"},
		{Agent: "b", Model: "elastic", Efforts: []string{"low", "medium", "high"}},
	}}}
	cs, rejected := d.selectCandidates(Options{Tier: TierAuto})
	if len(cs) != 10 {
		t.Fatalf("expanded candidates: %d, rejected %v", len(cs), rejected)
	}
	got := rankAuto(cs, -1.8, PriorityBalanced, effortCatalog())
	if len(got) != 3 || got[0].Model != "elastic" || got[0].Effort != "medium" || !got[0].Auto.Qualified {
		t.Fatalf("best efforts: %+v", got)
	}
	seen := map[string]bool{}
	for _, c := range got {
		key := c.Adapter.Name() + "/" + adapter.BaseModel(c.Model)
		if seen[key] {
			t.Fatalf("duplicate model: %s", key)
		}
		seen[key] = true
	}
	// A model-suffix adapter must collapse suffix variants too.
	got = rankAuto([]Candidate{{Model: "elastic-low"}, {Model: "elastic-medium"}, {Model: "elastic-high"}}, -1.8, PriorityBalanced, effortCatalog())
	if len(got) != 1 || got[0].Model != "elastic-medium" {
		t.Fatalf("suffix variants: %+v", got)
	}
}

func TestExtrapolatedLevelsWidenThePassBound(t *testing.T) {
	cat := Catalog{Scores: []BenchScore{
		{Model: "measured", Effort: "xhigh", Score: sigmoid(0.3), CostPerTask: F64(1)},
		{Model: "extrapolated", Effort: "medium", Score: 0.5, CostPerTask: F64(1)},
	}}
	cs := priceAuto([]Candidate{{Model: "measured", Effort: "xhigh"}, {Model: "extrapolated", Effort: "xhigh"}}, -1.5, PriorityBalanced, cat)
	if !near(cs[0].Auto.Theta, cs[1].Auto.Theta) || cs[1].Auto.PPassLow >= cs[0].Auto.PPassLow {
		t.Fatalf("bounds: %+v / %+v", cs[0].Auto, cs[1].Auto)
	}
	_, want := PassProbability(cs[1].Auto.Theta, -1.5, 0.5)
	if !near(cs[1].Auto.PPassLow, want) {
		t.Fatalf("bound %v, want %v", cs[1].Auto.PPassLow, want)
	}
}
