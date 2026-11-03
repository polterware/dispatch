package sqlite_test

import (
	"context"
	"testing"
	"time"

	"github.com/polterware/dispatch/go-core/domains/dispatch"
)

func TestRunUsageRoundTripAndAllWorkspaces(t *testing.T) {
	store := newStore(t)
	ctx := context.Background()
	now := time.Now().UTC().Truncate(time.Second)
	usage := &dispatch.Usage{InputTokens: 10, CacheReadTokens: 20, CacheWriteTokens: 1, OutputTokens: 5, ReasoningTokens: 2, CostUSD: dispatch.F64(0.25), CostSource: dispatch.CostAgent}
	for _, r := range []dispatch.Run{
		{ID: "a", ProjectID: "one", Status: dispatch.StatusCompleted, CreatedAt: now, Usage: usage},
		{ID: "b", ProjectID: "two", Status: dispatch.StatusCompleted, CreatedAt: now.Add(time.Second)},
	} {
		if err := store.SaveRun(ctx, r); err != nil {
			t.Fatal(err)
		}
	}
	got, err := store.GetRun(ctx, "a")
	if err != nil || got.Usage == nil || got.Usage.InputTokens != 10 || got.Usage.CacheWriteTokens != 1 || got.Usage.CostUSD == nil || *got.Usage.CostUSD != 0.25 || got.Usage.CostSource != dispatch.CostAgent {
		t.Fatalf("usage = %+v, %v", got.Usage, err)
	}
	if b, _ := store.GetRun(ctx, "b"); b.Usage != nil {
		t.Fatalf("a run with no reported usage must read nil: %+v", b.Usage)
	}
	if one, _ := store.ListRuns(ctx, "one", 0, 0); len(one) != 1 {
		t.Fatalf("one workspace = %d runs", len(one))
	}
	if all, _ := store.ListRuns(ctx, "", 0, 0); len(all) != 2 {
		t.Fatalf("an empty project must list every workspace's runs, got %d", len(all))
	}
}
