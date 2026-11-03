package dispatch

import (
	"context"
	"errors"
	"strings"
	"testing"

	adapter "github.com/polterware/dispatch/go-core/providers/adapters"
)

// fakeClassifier answers every task with probs, or fails with err, and keeps
// the texts it was asked about.
type fakeClassifier struct {
	probs [3]float64
	err   error
	asked *[]string
}

func (fakeClassifier) Name() string { return "fake" }

func (f fakeClassifier) Classify(_ context.Context, t Task) (Difficulty, error) {
	if f.asked != nil {
		*f.asked = append(*f.asked, t.Text)
	}
	if f.err != nil {
		return Difficulty{}, f.err
	}
	return NewDifficulty(&f.probs, "")
}

func easyClassifier() fakeClassifier { return fakeClassifier{probs: [3]float64{0.9, 0.08, 0.02}} }

func TestTierFromProbsKeepsAHardTail(t *testing.T) {
	for _, tc := range []struct {
		p    [3]float64
		want adapter.Tier
	}{
		{[3]float64{0.95, 0.04, 0.01}, adapter.TierEasy},
		{[3]float64{0.6, 0.3, 0.1}, adapter.TierNormal},
		{[3]float64{0.7, 0.1, 0.2}, adapter.TierHard},
		{[3]float64{0.1, 0.2, 0.7}, adapter.TierHard},
	} {
		if got := TierFromProbs(tc.p); got != tc.want {
			t.Errorf("%v: got %s want %s", tc.p, got, tc.want)
		}
		diff, err := NewDifficulty(&tc.p, "")
		if err != nil || diff.Tier != tc.want || DifficultyTier(diff.D) != tc.want {
			t.Errorf("%v: difficulty %+v (band %s) %v", tc.p, diff, DifficultyTier(diff.D), err)
		}
	}
	if _, err := NewDifficulty(&[3]float64{0, 0, 0}, ""); err == nil {
		t.Fatal("zero probabilities must be refused")
	}
	diff, err := NewDifficulty(nil, adapter.TierHard)
	if err != nil || diff.HasProbs || DifficultyTier(diff.D) != adapter.TierHard {
		t.Fatalf("a named tier: %+v %v", diff, err)
	}
}

func TestNoClassifierMeansNoAuto(t *testing.T) {
	var ran []string
	d := testDeps(t, &ran, fakeAdapter{name: "a", out: "ok", ran: &ran})
	d.Table.Tiers[adapter.TierNormal] = []Entry{{Agent: "a", Model: "normal"}}
	r, err := d.Run(context.Background(), Options{Prompt: "fix the footer link", ReadOnly: true})
	if err != nil || r.Status != StatusCompleted || r.Tier != "normal" || r.RequestedTier != "normal" || r.Model != "normal" || !strings.Contains(r.Note, "auto is off") {
		t.Fatalf("default route: %+v %v", r, err)
	}
	if _, err := d.Run(context.Background(), Options{Tier: TierAuto, Prompt: "x", ReadOnly: true}); !errors.Is(err, ErrAutoOff) {
		t.Fatalf("explicit auto without a classifier: %v", err)
	}
	route, err := d.Route(context.Background(), Options{Prompt: "fix"})
	if err != nil || route.Difficulty != nil || route.Winner == nil || route.Winner.Model != "normal" {
		t.Fatalf("route: %+v %v", route, err)
	}
}

func TestClassifierFailureFallsBackToTheNormalTier(t *testing.T) {
	var ran []string
	d := testDeps(t, &ran, fakeAdapter{name: "a", out: "ok", ran: &ran})
	d.Table.Tiers[adapter.TierNormal] = []Entry{{Agent: "a", Model: "normal"}}
	d.Classifier = fakeClassifier{err: errors.New("HTTP 503")}
	r, err := d.Run(context.Background(), Options{Prompt: "fix", ReadOnly: true})
	if err != nil || r.Status != StatusCompleted || r.Tier != "normal" || r.Difficulty != nil || !strings.Contains(r.Note, "classifier fake failed (HTTP 503)") {
		t.Fatalf("%+v %v", r, err)
	}
}

func TestAutoRecordsTheClassificationAndResumeKeepsIt(t *testing.T) {
	var ran, asked []string
	d := testDeps(t, &ran, fakeAdapter{name: "a", out: "ok", ran: &ran})
	d.Classifier = fakeClassifier{probs: [3]float64{0.1, 0.2, 0.7}, asked: &asked}
	r, err := d.Run(context.Background(), Options{Prompt: "redesign the store", CardText: "## card: Store\nintent", ReadOnly: true})
	if err != nil || r.RequestedTier != string(TierAuto) || r.Tier != "hard" || r.Classifier != "fake" || r.Difficulty == nil || len(r.DifficultyProbs) != 3 {
		t.Fatalf("%+v %v", r, err)
	}
	if len(asked) != 1 || asked[0] != "## card: Store\nintent\n\nRequest:\nredesign the store" {
		t.Fatalf("asked %q", asked)
	}
	dec, ok, _ := d.Store.GetRouteDecision(context.Background(), r.ID)
	if !ok || dec.Classifier != "fake" || len(dec.Probs) != 3 || dec.Difficulty == nil || dec.PolicyVersion != PolicyAutoV1 {
		t.Fatalf("decision %+v", dec)
	}
	resume, err := d.Run(context.Background(), Options{Resume: r.ID, Prompt: "again", ReadOnly: true})
	if err != nil || resume.Tier != r.Tier || len(asked) != 1 {
		t.Fatalf("resume: %+v %v asked=%d", resume, err, len(asked))
	}
}

func TestTierRouteWarnsWhenTheCatalogExpectsAFailure(t *testing.T) {
	var ran []string
	d := testDeps(t, &ran, fakeAdapter{name: "a", out: "ok", ran: &ran})
	// m1 is in no catalog: θ −0.7 with a 0.8 margin, so its bound at d −2.5
	// is σ(1.0) ≈ 0.73, under the balanced bar.
	rec := Run{Difficulty: F64(-2.5)}
	cs := d.priceTierRoute(rec, []Candidate{{Adapter: fakeAdapter{name: "a"}, Model: "m1"}})
	if note := tierWarning(cs); !strings.Contains(note, "auto: m1 P(pass)=") || !strings.Contains(note, "below the bar 0.80") {
		t.Fatalf("advisory %q", note)
	}
	if got := d.priceTierRoute(Run{}, []Candidate{{Model: "m1"}}); got[0].Auto != nil {
		t.Fatal("a run with no difficulty must stay unpriced")
	}
}
