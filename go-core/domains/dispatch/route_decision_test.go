package dispatch

import (
	"context"
	"encoding/json"
	"errors"
	"strings"
	"testing"

	adapter "github.com/polterware/dispatch/go-core/providers/adapters"
)

func TestRunRecordsOneRouteDecisionWithoutThePrompt(t *testing.T) {
	var ran []string
	d := testDeps(t, &ran,
		fakeAdapter{name: "a", err: errors.New("rate limited"), ran: &ran},
		fakeAdapter{name: "b", out: "hello\n", ran: &ran})
	prompt := "secret-prompt-text fix the footer link"
	rec, err := d.Run(context.Background(), Options{Tier: adapter.TierEasy, Prompt: prompt})
	if err != nil || rec.Status != StatusCompleted {
		t.Fatalf("%+v %v", rec, err)
	}
	decs, err := d.Store.ListRouteDecisions(context.Background(), "")
	if err != nil || len(decs) != 1 {
		t.Fatalf("decisions = %+v, %v", decs, err)
	}
	dec := decs[0]
	if dec.RunID != rec.ID || dec.PolicyVersion != PolicyTierV2 || dec.Route != RouteTier || dec.Priority != PriorityBalanced {
		t.Fatalf("decision = %+v", dec)
	}
	if len(dec.Candidates) != 2 || dec.Candidates[0].Agent != "a" || dec.Candidates[1].Agent != "b" {
		t.Fatalf("candidates = %+v", dec.Candidates)
	}
	if p := dec.Candidates[0].PPass; p != nil || dec.Difficulty != nil || dec.Classifier != "" {
		t.Fatalf("a tier run with no classifier has no estimate: %+v", dec)
	}
	b, _ := json.Marshal(dec)
	if strings.Contains(string(b), "secret-prompt-text") {
		t.Fatal("the decision log holds the prompt")
	}
}

func TestRouteNeverRecordsADecision(t *testing.T) {
	d := priorityDeps(t)
	if _, err := d.Route(context.Background(), Options{Tier: adapter.TierNormal}); err != nil {
		t.Fatal(err)
	}
	if decs, _ := d.Store.ListRouteDecisions(context.Background(), ""); len(decs) != 0 {
		t.Fatalf("Route logged %+v", decs)
	}
}
