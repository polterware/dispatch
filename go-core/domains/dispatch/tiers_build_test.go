package dispatch

import (
	"encoding/json"
	"testing"

	adapter "github.com/polterware/dispatch/go-core/providers/adapters"
)

func TestBuildTiersPutsTheCheapModelOnEasyAndTheStrongOneOnHard(t *testing.T) {
	entries := []Entry{
		{Agent: "a", Model: "cheap"}, {Agent: "b", Model: "strong"}, {Agent: "a", Model: "cheap"}, {Agent: "c", Model: "cheap"}, {Agent: "d", Model: "cheap-fast"},
	}
	got := BuildTiers(entries, autoCatalog(), PriorityBalanced)
	easy, hard := got.Tiers[adapter.TierEasy], got.Tiers[adapter.TierHard]
	if len(easy) != 2 || easy[0].Model != "cheap" || !easy[0].Auto.Qualified {
		t.Fatalf("easy %+v", easy)
	}
	if hard[0].Model != "strong" || !hard[0].Auto.Qualified || hard[1].Auto.Qualified {
		t.Fatalf("hard %+v", hard)
	}
	if len(got.Table.Tiers[adapter.TierNormal]) != 2 {
		t.Fatalf("one entry per base model: %+v", got.Table.Tiers[adapter.TierNormal])
	}
	if _, err := got.Table.Validated(); err != nil {
		t.Fatalf("built table is invalid: %v", err)
	}
}

func TestBuildTiersSelectsAndStoresEachTiersBestEffort(t *testing.T) {
	got := BuildTiers([]Entry{{Agent: "codex", Model: "elastic", Efforts: []string{"low", "medium", "high"}}}, effortCatalog(), PriorityBalanced)
	for tier, want := range map[adapter.Tier]string{adapter.TierEasy: "low", adapter.TierNormal: "medium", adapter.TierHard: "high"} {
		built := got.Tiers[tier]
		if len(built) != 1 || built[0].Effort != want || got.Table.Tiers[tier][0].Effort != want {
			t.Fatalf("%s: %+v, want %s", tier, built, want)
		}
	}
	data, err := json.Marshal(got.Table)
	if err != nil {
		t.Fatal(err)
	}
	table, err := parseTiers(data)
	if err != nil {
		t.Fatal(err)
	}
	// The store/API carries the choice as a model suffix. Restoring a config
	// agent splits it back out so Auto can explore its supported levels again.
	table = TableFromRows(table.Rows())
	d, _, _ := routingDeps(t)
	a := d.Adapters[0].(routingFake)
	a.fakeAdapter.name = "codex"
	d.Adapters = []adapter.Adapter{a}
	d.Table = table
	for tier, want := range map[adapter.Tier]string{adapter.TierEasy: "low", adapter.TierNormal: "medium", adapter.TierHard: "high"} {
		cs, rejected := d.selectCandidates(Options{Tier: tier})
		if len(cs) != 1 || cs[0].Model != "elastic" || cs[0].Effort != want {
			t.Fatalf("%s restored: %+v, rejected %v", tier, cs, rejected)
		}
	}
	// A stored choice overrides the tier default, including after a board move.
	d.Table.Tiers[adapter.TierEasy] = table.Tiers[adapter.TierHard]
	cs, _ := d.selectCandidates(Options{Tier: adapter.TierEasy})
	if len(cs) != 1 || cs[0].Effort != "high" {
		t.Fatalf("stored effort lost: %+v", cs)
	}
}

func TestBuildTiersPreservesSuffixAndUsesAgentEfforts(t *testing.T) {
	got := BuildTiers([]Entry{{Agent: "codex", Model: "elastic-high", Efforts: []string{"low", "high"}}}, effortCatalog(), PriorityBalanced)
	for tier, entries := range got.Tiers {
		if len(entries) != 1 || entries[0].Model != "elastic-high" || entries[0].Effort != "high" {
			t.Fatalf("%s: %+v", tier, entries)
		}
	}
	got = BuildTiers([]Entry{{Agent: "codex", Model: "elastic"}}, effortCatalog(), PriorityBalanced)
	if got.Tiers[adapter.TierEasy][0].Effort != "low" || got.Tiers[adapter.TierHard][0].Effort != "high" {
		t.Fatalf("agent effort defaults: %+v", got.Tiers)
	}
}

func TestTierEffortValidation(t *testing.T) {
	for _, body := range []string{
		`{"tiers":{"easy":[{"agent":"codex","model":"elastic","effort":"wild"}]}}`,
		`{"tiers":{"easy":[{"agent":"codex","model":"elastic","effort":"high","efforts":["low"]}]}}`,
		`{"tiers":{"easy":[{"agent":"codex","model":"elastic-low","effort":"high"}]}}`,
	} {
		if _, err := parseTiers([]byte(body)); err == nil {
			t.Fatalf("accepted %s", body)
		}
	}
}
