package dispatch

import (
	"strings"
	"testing"
)

// The router's scenarios on the embedded catalog: with a fixed difficulty and
// quota, Auto picks the model the catalog's numbers say it should. A catalog
// refresh that changes a winner fails here, so the change is seen.
func TestAutoScenariosOnTheCatalog(t *testing.T) {
	cat := LoadCatalog("")
	pool := func() []Candidate {
		return []Candidate{
			{Model: "gpt-5.6-luna", Effort: "medium"},
			{Model: "gpt-5.6-terra", Effort: "medium"},
			{Model: "claude-haiku-5-5", Effort: "high"},
			{Model: "claude-opus-5-5", Effort: "high"},
			{Model: "gemini-4-argon", Effort: "high"},
		}
	}
	gone := &Pace{WillLast: false}
	for _, tc := range []struct {
		name     string
		d        float64
		priority string
		quota    map[string]*Pace
		want     string
		quali    bool
	}{
		{"an easy task on speed takes the cheapest model over the bar", -2.8, PrioritySpeed, nil, "gpt-5.6-luna", true},
		{"an easy task on balanced needs a bound of 0.80", -2.8, PriorityBalanced, nil, "claude-haiku-5-5", true},
		{"an easy task on quality takes the strongest", -2.8, PriorityQuality, nil, "claude-opus-5-5", true},
		{"a normal task on balanced takes the cheapest that still qualifies", -1.8, PriorityBalanced, nil, "claude-opus-5-5", true},
		{"a hard task qualifies nobody, so the most likely to pass wins", -0.8, PriorityBalanced, nil, "claude-opus-5-5", false},
		{"a window that will not last makes its model dearer", -2.8, PriorityBalanced, map[string]*Pace{"claude-haiku-5-5": gone}, "gpt-5.6-terra", true},
	} {
		t.Run(tc.name, func(t *testing.T) {
			cs := pool()
			for i := range cs {
				cs[i].Pace = tc.quota[cs[i].Model]
			}
			got := rankAuto(cs, tc.d, tc.priority, cat)
			if got[0].Model != tc.want || got[0].Auto.Qualified != tc.quali {
				t.Fatalf("got %s qualified=%v, want %s qualified=%v", got[0].Model, got[0].Auto.Qualified, tc.want, tc.quali)
			}
		})
	}
}

// Every θ the scenarios use traces to a benchmark row, not to a guess.
func TestAutoThetaTracesToTheCatalog(t *testing.T) {
	cat := LoadCatalog("")
	for _, c := range rankAuto([]Candidate{{Model: "claude-opus-5-5", Effort: "high"}, {Model: "gpt-5.6-luna", Effort: "medium"}}, -1.8, PriorityBalanced, cat) {
		if !strings.HasPrefix(c.Auto.ThetaSource, "cursorbench") || c.Auto.Cost == nil {
			t.Fatalf("%s: θ from %q, cost %v", c.Model, c.Auto.ThetaSource, c.Auto.Cost)
		}
	}
}
