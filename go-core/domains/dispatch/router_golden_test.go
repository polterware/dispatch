package dispatch

import (
	"context"
	"encoding/json"
	"os"
	"testing"
	"time"

	adapter "github.com/polterware/dispatch/go-core/providers/adapters"
)

type goldenQuota struct {
	Used    float64 `json:"used"`    // percent of the window used
	Elapsed float64 `json:"elapsed"` // percent of the window gone by
}

// goldenFile is testdata/router_golden.json. Its labeled prompts are eval
// data for tooling/classify-eval.mjs, which scores a live classifier
// on them outside go test; the tests here read the table and the scenarios.
type goldenFile struct {
	Table     map[adapter.Tier][]Entry `json:"table"`
	Scenarios []struct {
		Name     string                 `json:"name"`
		Tier     adapter.Tier           `json:"tier"`
		Quota    map[string]goldenQuota `json:"quota"`
		Cooldown []string               `json:"cooldown"`
		Want     string                 `json:"want"`
	} `json:"scenarios"`
}

func loadGolden(t *testing.T) goldenFile {
	t.Helper()
	raw, err := os.ReadFile("testdata/router_golden.json")
	if err != nil {
		t.Fatal(err)
	}
	var g goldenFile
	if err := json.Unmarshal(raw, &g); err != nil {
		t.Fatal(err)
	}
	return g
}

// TestGoldenScenariosFollowTheTierOrder checks a tier route against the rule
// a person expects: the tier's first model, unless its account's quota is low
// or rate limited, then the next one, and so on.
func TestGoldenScenariosFollowTheTierOrder(t *testing.T) {
	g := loadGolden(t)
	now := time.Date(2026, 10, 15, 12, 0, 0, 0, time.UTC)
	const week = 10080
	caps := adapter.Capabilities{ReadOnly: true, FileEditing: true}
	for _, sc := range g.Scenarios {
		t.Run(sc.Name, func(t *testing.T) {
			ran := []string{}
			var fakes []adapter.Adapter
			for _, name := range []string{"a", "b", "c"} {
				fakes = append(fakes, routingFake{fakeAdapter: fakeAdapter{name: name, ran: &ran}, caps: caps})
			}
			d := testDeps(t, &ran, fakes...)
			d.Now = func() time.Time { return now }
			d.Table = Table{Tiers: g.Table}
			// These tier-order goldens are independent of discovered models.
			d.Policy = DefaultPolicy()
			ctx := context.Background()
			for agent, q := range sc.Quota {
				used := q.Used
				left := time.Duration(float64(week)*(1-q.Elapsed/100)) * time.Minute
				if err := d.Store.SaveQuota(ctx, QuotaSnapshot{Agent: agent, UsedPercent: &used, WindowMinutes: week, ResetsAt: now.Add(left), SampledAt: now}); err != nil {
					t.Fatal(err)
				}
			}
			for _, agent := range sc.Cooldown {
				if err := d.Store.SetCooldown(ctx, agent, now.Add(time.Hour)); err != nil {
					t.Fatal(err)
				}
			}
			r, err := d.Route(ctx, Options{Tier: sc.Tier})
			if err != nil || r.Winner == nil {
				t.Fatalf("route: %+v %v", r, err)
			}
			got := r.Winner.Agent + "/" + r.Winner.Model
			if got != sc.Want {
				var order []string
				for _, c := range r.Candidates {
					order = append(order, c.Filters.Agent+"/"+c.Filters.Model)
				}
				t.Errorf("tried %s first, want %s; order %v", got, sc.Want, order)
			}
		})
	}
}
