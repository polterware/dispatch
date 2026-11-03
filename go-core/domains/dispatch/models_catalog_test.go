package dispatch

import (
	"encoding/json"
	"math"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func TestEmbeddedCatalogResolvesTheDefaultTable(t *testing.T) {
	c := LoadCatalog("")
	if len(c.Models) == 0 || len(c.Scores) == 0 || !c.eciFit {
		t.Fatalf("snapshot: %d models, %d scores, eci fit %v", len(c.Models), len(c.Scores), c.eciFit)
	}
	for _, es := range Default().Tiers {
		for _, e := range es {
			est := c.Estimate(e.Model, "", e.Strength)
			if est.ThetaSource == "unknown" || est.CostPerTask == nil {
				t.Errorf("%s/%s: %+v", e.Agent, e.Model, est)
			}
		}
	}
}

func TestEstimateUsesTheLevelThenTheNearestOne(t *testing.T) {
	c := Catalog{Scores: []BenchScore{
		{Model: "m", Effort: "medium", Score: 0.5, CostPerTask: F64(2)},
		{Model: "m", Effort: "high", Score: 0.6, CostPerTask: F64(3)},
		{Model: "flat", Score: 0.3},
	}, Models: []CatalogModel{{ID: "priced", Input: 4, Output: 8}}}
	high := c.Estimate("m-high", "", 0)
	if high.ExtrapolatedLevels != 0 || high.ThetaSource != "cursorbench high" || *high.CostPerTask != 3 || !near(high.Theta, logit(0.6)) {
		t.Fatalf("exact level: %+v", high)
	}
	xhigh := c.Estimate("m", "xhigh", 0)
	if xhigh.ExtrapolatedLevels != 1 || !strings.Contains(xhigh.ThetaSource, "nearest to xhigh") || !near(xhigh.Theta, logit(0.6)+effortThetaStep) || !near(*xhigh.CostPerTask, 3*2.5/1.6) {
		t.Fatalf("nearest level: %+v", xhigh)
	}
	if f := c.Estimate("openai/flat", "low", 0); f.ThetaSource != "cursorbench" || !near(f.Theta, logit(0.3)) {
		t.Fatalf("unlevelled model with a provider prefix: %+v", f)
	}
	if s := c.Estimate("m-build-fast", "medium", 0); s.ThetaSource != "cursorbench medium (as m)" {
		t.Fatalf("shorter key: %+v", s)
	}
	if s := c.Estimate("priced", "medium", 5); s.ThetaSource != "tiers.json strength" || !near(s.Theta, unknownTheta+2*strengthThetaStep) ||
		s.CostSource != "models.dev price" || !near(*s.CostPerTask, (3*4+8)/4.0*defaultTokensPerTask/1e6) {
		t.Fatalf("fallbacks: %+v", s)
	}
	if u := c.Estimate("nothing", "", 0); u.ThetaSource != "unknown" || u.CostPerTask != nil {
		t.Fatalf("unknown: %+v", u)
	}
}

func TestRefreshedPricesOverrideTheSnapshot(t *testing.T) {
	dir := t.TempDir()
	raw := []byte(`{"openai":{"models":{"gpt-x":{"name":"GPT X","tool_call":true,"modalities":{"output":["text"]},"cost":{"input":1,"output":2}},
		"img":{"tool_call":true,"modalities":{"output":["image"]},"cost":{"input":1,"output":2}}}},
		"someaggregator":{"models":{"gpt-y":{"tool_call":true,"modalities":{"output":["text"]},"cost":{"input":1,"output":2}}}}}`)
	n, err := SaveRefreshedPrices(dir, raw, time.Date(2026, 10, 31, 0, 0, 0, 0, time.UTC))
	if err != nil || n != 1 {
		t.Fatal(n, err)
	}
	c := LoadCatalog(dir)
	if len(c.Models) != 1 || c.Models[0].ID != "gpt-x" || c.PricesFetched == "" {
		t.Fatalf("%+v", c.Models)
	}
	if err := os.WriteFile(filepath.Join(dir, "models.json"), []byte("{broken"), 0o644); err != nil {
		t.Fatal(err)
	}
	if c := LoadCatalog(dir); len(c.Models) < 10 || c.PricesFetched != "" {
		t.Fatal("a broken refresh must keep the snapshot")
	}
}

func near(a, b float64) bool { d := a - b; return d < 1e-9 && d > -1e-9 }

func TestEstimateShiftsUnmeasuredEffortsFromMedium(t *testing.T) {
	cat := Catalog{ECI: []ECIScore{{Model: "eci-only", ECI: 100}}, eciFit: true, eciSlope: 0.01}
	for _, tc := range []struct {
		model    string
		strength int
		source   string
	}{
		{"eci-only", 0, "eci"}, {"prior", 5, "tiers.json strength"}, {"unknown", 0, "unknown"},
	} {
		t.Run(tc.source, func(t *testing.T) {
			medium := cat.Estimate(tc.model, "medium", tc.strength)
			for effort, levels := range map[string]int{"low": -1, "medium": 0, "high": 1, "xhigh": 2, "max": 3} {
				est := cat.Estimate(tc.model, effort, tc.strength)
				if est.ThetaSource != tc.source || !near(est.Theta, medium.Theta+float64(levels)*effortThetaStep) || est.ExtrapolatedLevels != int(math.Abs(float64(levels))) {
					t.Fatalf("%s: %+v, medium %+v", effort, est, medium)
				}
			}
		})
	}
}

func TestCatalogReportKeepsDiscoveryOptOut(t *testing.T) {
	body, err := json.Marshal(CatalogReport{Discover: false})
	if err != nil || !strings.Contains(string(body), `"discover":false`) {
		t.Fatalf("discovery opt-out: %s, %v", body, err)
	}
}
