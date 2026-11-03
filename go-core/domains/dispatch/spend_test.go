package dispatch

import (
	"testing"

	adapter "github.com/polterware/dispatch/go-core/providers/adapters"
)

func TestUsageCostAndRunTotals(t *testing.T) {
	cat := Catalog{Models: []CatalogModel{
		{ID: "gpt-x", Input: 2, Output: 10, CacheRead: F64(0.2)},
		{ID: "nocache", Input: 1, Output: 4},
	}}
	// 1M fresh input, 1M cache reads, 1M output: 2 + 0.2 + 10.
	cost, source, ok := cat.UsageCost("openai/gpt-x-high", adapter.Usage{InputTokens: 1e6, CacheReadTokens: 1e6, OutputTokens: 1e6})
	if !ok || source != CostCatalog || !near(cost, 12.2) {
		t.Fatalf("catalog cost = %v %q %v", cost, source, ok)
	}
	// No cache price: cache tokens pay the input price.
	if cost, _, _ := cat.UsageCost("nocache", adapter.Usage{CacheReadTokens: 1e6, CacheWriteTokens: 1e6}); !near(cost, 2) {
		t.Fatalf("no cache price = %v", cost)
	}
	if cost, source, _ := cat.UsageCost("unknown-model", adapter.Usage{InputTokens: 5, CostUSD: F64(0.5)}); source != CostAgent || cost != 0.5 {
		t.Fatalf("the CLI's own price wins: %v %q", cost, source)
	}
	if _, _, ok := cat.UsageCost("unknown-model", adapter.Usage{InputTokens: 5}); ok {
		t.Fatal("an unknown model with no reported price must stay unpriced")
	}

	var r Run
	r.addUsage("gpt-x", nil, cat)
	if r.Usage != nil {
		t.Fatal("no reported usage leaves the run without usage")
	}
	r.addUsage("gpt-x", &adapter.Usage{InputTokens: 1e6, OutputTokens: 1e5}, cat)
	r.addUsage("other", &adapter.Usage{InputTokens: 10, CostUSD: F64(1)}, cat)
	if r.Usage.InputTokens != 1e6+10 || r.Usage.CostUSD == nil || !near(*r.Usage.CostUSD, 4) || r.Usage.CostSource != CostMixed {
		t.Fatalf("two priced attempts = %+v", r.Usage)
	}
	r.addUsage("unknown-model", &adapter.Usage{OutputTokens: 7}, cat)
	if r.Usage.CostUSD != nil || r.Usage.OutputTokens != 1e5+7 || r.Usage.TotalTokens() != 1e6+10+1e5+7 {
		t.Fatalf("an unpriced attempt must leave the run's cost unknown: %+v", r.Usage)
	}
}

func TestStatsSumSpend(t *testing.T) {
	recs := []Run{
		{Model: "m", Status: StatusCompleted, Usage: &Usage{InputTokens: 10, OutputTokens: 2, CostUSD: F64(0.5)}},
		{Model: "m", Status: StatusFailed, Usage: &Usage{InputTokens: 5, CacheReadTokens: 100}},
		{Model: "m", Status: StatusCompleted},
	}
	rep, err := Compute(recs, StatsOptions{By: "model"})
	if err != nil || len(rep.Rows) != 1 {
		t.Fatal(rep, err)
	}
	s := rep.Rows[0].Spend
	if s.UsageRuns != 2 || s.InputTokens != 15 || s.CacheReadTokens != 100 || s.OutputTokens != 2 || s.CostRuns != 1 || s.CostUSD != 0.5 {
		t.Fatalf("spend = %+v", s)
	}
}

func TestUsageCostTiersAndContext(t *testing.T) {
	cat := Catalog{Models: []CatalogModel{{
		ID: "gpt-y", Input: 2, Output: 10, CacheRead: F64(0.2), Context: 400_000,
		Tiers: []PriceTier{{Size: 272_000, Input: 4, Output: 15, CacheRead: F64(0.4)}},
	}}}
	u := adapter.Usage{InputTokens: 1e6, CacheReadTokens: 1e6, OutputTokens: 1e6}
	if cost, _, _ := cat.UsageCost("gpt-y", u); !near(cost, 12.2) {
		t.Fatalf("under the tier = %v", cost)
	}
	u.ContextTokens = 300_000
	if cost, _, _ := cat.UsageCost("gpt-y", u); !near(cost, 19.4) {
		t.Fatalf("past the tier's size the attempt pays the tier = %v", cost)
	}

	var r Run
	r.addUsage("gpt-y", &adapter.Usage{InputTokens: 10, ContextTokens: 50_000}, cat)
	if r.Usage.ContextTokens != 50_000 || r.Usage.ContextWindow != 400_000 {
		t.Fatalf("the catalog fills a window the CLI did not report: %+v", r.Usage)
	}
	r.addUsage("other", &adapter.Usage{InputTokens: 10, ContextTokens: 9_000, ContextWindow: 128_000}, cat)
	r.addUsage("gpt-y", &adapter.Usage{InputTokens: 10}, cat)
	if r.Usage.ContextTokens != 9_000 || r.Usage.ContextWindow != 128_000 {
		t.Fatalf("the context is the last attempt that reported one: %+v", r.Usage)
	}
}
