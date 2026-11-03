package sqlite_test

import (
	"context"
	"path/filepath"
	"testing"
	"time"

	"github.com/polterware/dispatch/go-core/providers/sqlite"

	"github.com/polterware/dispatch/go-core/domains/dispatch"
)

func TestRouteDecisionRoundTrip(t *testing.T) {
	store := newStore(t)
	if err := store.SaveWorkspace(sqlite.Workspace{ID: "proj", Name: "proj"}); err != nil {
		t.Fatal(err)
	}
	ctx := context.Background()
	now := time.Now().UTC().Truncate(time.Second)
	if err := store.SaveRun(ctx, dispatch.Run{ID: "run-1", ProjectID: "proj", Status: dispatch.StatusPending, Tier: "normal", CreatedAt: now}); err != nil {
		t.Fatal(err)
	}
	dec := dispatch.RouteDecision{
		RunID: "run-1", ProjectID: "proj", PolicyVersion: dispatch.PolicyTierV1, Route: dispatch.RouteTier, Priority: "balanced",
		Classifier: "jev", Probs: []float64{0.2, 0.5, 0.3}, Difficulty: dispatch.F64(0.25),
		Candidates: []dispatch.DecisionCandidate{{Agent: "codex", Model: "m", PPass: dispatch.F64(0.8), Score: 12}},
		CreatedAt:  now,
	}
	if err := store.SaveRouteDecision(ctx, dec); err != nil {
		t.Fatal(err)
	}
	dec.Chosen = 0
	if err := store.SaveRouteDecision(ctx, dec); err != nil {
		t.Fatal("a second save of the same run must update, not fail:", err)
	}
	got, err := store.ListRouteDecisions(ctx, "proj")
	if err != nil || len(got) != 1 {
		t.Fatalf("decisions = %+v, %v", got, err)
	}
	g := got[0]
	if g.PolicyVersion != dispatch.PolicyTierV1 || len(g.Probs) != 3 || g.Classifier != "jev" || g.Difficulty == nil || *g.Difficulty != 0.25 ||
		len(g.Candidates) != 1 || *g.Candidates[0].PPass != 0.8 || !g.CreatedAt.Equal(now) {
		t.Fatalf("decision = %+v", g)
	}
}

func newStore(t *testing.T) *sqlite.Store {
	t.Helper()
	store, err := sqlite.New(filepath.Join(t.TempDir(), "dispatch.db"))
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = store.Close() })
	return store
}

func TestWorkspaceAtPicksTheDeepestFolder(t *testing.T) {
	store := newStore(t)
	for _, w := range []sqlite.Workspace{{ID: "a", Path: "/code"}, {ID: "b", Path: "/code/app"}, {ID: "c", PolterwareProjectID: "p"}} {
		if err := store.SaveWorkspace(w); err != nil {
			t.Fatal(err)
		}
	}
	if w, err := store.WorkspaceAt("/code/app/src"); err != nil || w.ID != "b" {
		t.Fatalf("%+v %v", w, err)
	}
	if _, err := store.WorkspaceAt("/elsewhere"); err != sqlite.ErrNoWorkspace {
		t.Fatalf("outside: %v", err)
	}
}
