package dispatch

import (
	"context"
	"reflect"
	"slices"
	"strings"
	"testing"
	"time"

	adapter "github.com/polterware/dispatch/go-core/providers/adapters"
)

// openCodeDeps has a Codex-like agent "a" in the table and an OpenCode that
// lists a cheap model, a strong one, and a Claude model it may not run.
func openCodeDeps(t *testing.T) Deps {
	t.Helper()
	ran := []string{}
	caps := adapter.Capabilities{ReadOnly: true, FileEditing: true, EffortMode: "config", Efforts: []string{"low", "medium", "high", "xhigh"}}
	a := routingFake{fakeAdapter: fakeAdapter{name: "a", ran: &ran}, caps: caps}
	oc := routingFake{fakeAdapter: fakeAdapter{name: OpenCodeAgent, ran: &ran}, caps: caps, models: []adapter.Model{
		{ID: "zai/glm-5.3-flash"}, {ID: "openai/gpt-5.6-sol"}, {ID: "anthropic/claude-sonnet-5-5"}, {ID: "opencode/glm-5.3"},
	}}
	d := testDeps(t, &ran, a, oc)
	d.Now = func() time.Time { return time.Date(2026, 10, 15, 12, 0, 0, 0, time.UTC) }
	d.Table = Table{Tiers: map[adapter.Tier][]Entry{
		adapter.TierEasy:   {{Agent: "a", Model: "gpt-5.6-luna"}},
		adapter.TierNormal: {{Agent: "a", Model: "gpt-5.6-terra"}},
		adapter.TierHard:   {{Agent: "a", Model: "gpt-5.6-sol"}},
	}}
	return d
}

func candidateModels(r RouteReport) []string {
	var out []string
	for _, c := range r.Candidates {
		out = append(out, c.Agent+"/"+c.Model)
	}
	return out
}

func TestOpenCodeAutoWeighsTheModelsItLists(t *testing.T) {
	d := openCodeDeps(t)
	d.Classifier = easyClassifier()
	r, err := d.Route(context.Background(), Options{Prompt: "fix the footer link", Filters: Filters{Agent: OpenCodeAgent}})
	if err != nil || r.Winner == nil || r.Winner.Agent != OpenCodeAgent || r.Difficulty == nil {
		t.Fatalf("%+v %v", r, err)
	}
	got := candidateModels(r)
	for _, want := range []string{"opencode/zai/glm-5.3-flash", "opencode/openai/gpt-5.6-sol", "opencode/opencode/glm-5.3"} {
		if !slices.Contains(got, want) {
			t.Fatalf("auto lacks %s: %v", want, got)
		}
	}
	if slices.Contains(got, "opencode/anthropic/claude-sonnet-5-5") || slices.Contains(got, "a/gpt-5.6-luna") {
		t.Fatalf("auto weighs a refused model or another agent: %v", got)
	}
}

func TestOpenCodeTierWithoutEntryRanksItsModelsForTheTier(t *testing.T) {
	d := openCodeDeps(t)
	d.Policy.Deny.Models = []string{"opencode/glm-5.3"}
	pool := []Entry{
		{Agent: OpenCodeAgent, Model: "zai/glm-5.3-flash", Provider: "zai"},
		{Agent: OpenCodeAgent, Model: "openai/gpt-5.6-sol", Provider: "openai"},
	}
	for _, tier := range []adapter.Tier{adapter.TierEasy, adapter.TierHard} {
		r, err := d.Route(context.Background(), Options{Tier: tier, Filters: Filters{Agent: OpenCodeAgent}})
		if err != nil || r.Winner == nil {
			t.Fatalf("%s: %+v %v", tier, r, err)
		}
		var want []string
		for _, b := range BuildTiers(pool, LoadCatalog(d.StateDir), PriorityBalanced).Tiers[tier] {
			want = append(want, b.Agent+"/"+b.Model)
		}
		got := candidateModels(r)
		if len(got) == 0 || got[0] != want[0] {
			t.Fatalf("%s: got %v, BuildTiers ranks %v", tier, got, want)
		}
		if slices.Contains(got, "opencode/opencode/glm-5.3") || slices.Contains(got, "opencode/anthropic/claude-sonnet-5-5") {
			t.Fatalf("%s: a denied or refused model was ranked: %v", tier, got)
		}
		if !strings.Contains(r.Note, openCodePickNote) {
			t.Fatalf("%s: note %q", tier, r.Note)
		}
	}
}

func TestOpenCodeTierKeepsTheTablesOpenCodeEntries(t *testing.T) {
	d := openCodeDeps(t)
	d.Table.Tiers[adapter.TierNormal] = append(d.Table.Tiers[adapter.TierNormal], Entry{Agent: OpenCodeAgent, Model: "opencode/glm-5.3"})
	r, err := d.Route(context.Background(), Options{Tier: adapter.TierNormal, Filters: Filters{Agent: OpenCodeAgent}})
	if err != nil || r.Winner == nil {
		t.Fatalf("%+v %v", r, err)
	}
	if got := candidateModels(r); !slices.Equal(got, []string{"opencode/opencode/glm-5.3"}) || strings.Contains(r.Note, openCodePickNote) {
		t.Fatalf("got %v note %q", got, r.Note)
	}
}

func TestOpenCodeNotInstalledAddsNothing(t *testing.T) {
	d := openCodeDeps(t)
	d.Installed = func(b string) bool { return b != OpenCodeAgent }
	r, err := d.Route(context.Background(), Options{Tier: adapter.TierHard, Filters: Filters{Agent: OpenCodeAgent}})
	if err != nil || r.Winner != nil {
		t.Fatalf("%+v %v", r, err)
	}
}

func TestAgentPoolAndMergeAgent(t *testing.T) {
	table := Table{Tiers: map[adapter.Tier][]Entry{
		adapter.TierEasy: {{Agent: "a", Model: "m1"}, {Agent: OpenCodeAgent, Model: "zai/glm-5.3-flash"}},
		adapter.TierHard: {{Agent: OpenCodeAgent, Model: "openai/gpt-5.6-sol"}, {Agent: "a", Model: "m2"}},
	}}
	listed := []Entry{
		{Agent: OpenCodeAgent, Model: "zai/glm-5.3-flash"}, {Agent: OpenCodeAgent, Model: "anthropic/claude-opus-5-5"},
		{Agent: OpenCodeAgent, Model: "opencode/glm-5.3"}, {Agent: "a", Model: "m3"},
	}
	var pool []string
	for _, e := range AgentPool(OpenCodeAgent, table, listed) {
		pool = append(pool, e.Model)
	}
	if !slices.Equal(pool, []string{"zai/glm-5.3-flash", "openai/gpt-5.6-sol", "opencode/glm-5.3"}) {
		t.Fatalf("pool %v", pool)
	}

	built := Table{Tiers: map[adapter.Tier][]Entry{
		adapter.TierNormal: {{Agent: OpenCodeAgent, Model: "opencode/glm-5.3"}, {Agent: "a", Model: "ignored"}},
		adapter.TierHard:   {{Agent: OpenCodeAgent, Model: "zai/glm-5.3-flash"}},
	}}
	merged := MergeAgent(table, built, OpenCodeAgent)
	want := map[adapter.Tier][]Entry{
		adapter.TierEasy:   {{Agent: "a", Model: "m1"}},
		adapter.TierNormal: {{Agent: OpenCodeAgent, Model: "opencode/glm-5.3"}},
		adapter.TierHard:   {{Agent: "a", Model: "m2"}, {Agent: OpenCodeAgent, Model: "zai/glm-5.3-flash"}},
	}
	for tier, es := range want {
		if !reflect.DeepEqual(merged.Tiers[tier], es) {
			t.Fatalf("%s: %+v", tier, merged.Tiers[tier])
		}
	}
}
