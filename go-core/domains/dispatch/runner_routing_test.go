package dispatch

import (
	"context"
	"errors"
	"fmt"
	"os"
	"strings"
	"testing"
	"time"

	adapter "github.com/polterware/dispatch/go-core/providers/adapters"
)

type routingFake struct {
	fakeAdapter
	caps     adapter.Capabilities
	quota    adapter.Quota
	requests *[]adapter.Request
	calls    *int
	models   []adapter.Model
}

func (f routingFake) Capabilities() adapter.Capabilities { return f.caps }
func (f routingFake) Quota(context.Context) (adapter.Quota, error) {
	if f.calls != nil {
		*f.calls++
	}
	if f.quota.WindowMinutes == 0 {
		return adapter.Quota{}, adapter.ErrNoQuota
	}
	return f.quota, nil
}
func (f routingFake) Models(context.Context) ([]adapter.Model, error) { return f.models, nil }
func (f routingFake) Run(ctx context.Context, r adapter.Request, m string) (adapter.Result, error) {
	if f.requests != nil {
		*f.requests = append(*f.requests, r)
	}
	return f.fakeAdapter.Run(ctx, r, m)
}

func routingDeps(t *testing.T) (Deps, *[]string, *[]adapter.Request) {
	t.Helper()
	ran := []string{}
	requests := []adapter.Request{}
	caps := adapter.Capabilities{ReadOnly: true, FileEditing: true, EffortMode: "config", Efforts: []string{"low", "medium", "high", "xhigh"}}
	now := time.Date(2026, 10, 15, 12, 0, 0, 0, time.UTC)
	a := routingFake{fakeAdapter: fakeAdapter{name: "a", ran: &ran}, caps: caps, quota: adapter.Quota{UsedPercent: 40, WindowMinutes: 10080, ResetsAt: now.Add(84 * time.Hour)}, requests: &requests}
	b := a
	b.fakeAdapter.name = "b"
	b.quota.UsedPercent = 10
	d := testDeps(t, &ran, a, b)
	d.Now = func() time.Time { return now }
	d.Table = Table{Tiers: map[adapter.Tier][]Entry{
		adapter.TierEasy: {{Agent: "a", Model: "gpt-small"}, {Agent: "b", Model: "gpt-small"}, {Agent: "a", Model: "gemini-fast"}},
		adapter.TierHard: {{Agent: "a", Model: "gpt-large"}, {Agent: "b", Model: "gemini-pro"}},
	}}
	return d, &ran, &requests
}

func TestExplicitModelChoosesAgentByPace(t *testing.T) {
	d, ran, reqs := routingDeps(t)
	rec, err := d.Run(context.Background(), Options{Filters: Filters{Model: "gpt-small", Effort: "high"}, Prompt: "p", NoWorktree: true})
	if err != nil || rec.Status != StatusCompleted || rec.Agent != "b" || rec.Effort != "high" {
		t.Fatalf("%+v %v", rec, err)
	}
	if len(*ran) != 1 || (*reqs)[0].Effort != "high" || rec.Requested.Model != "gpt-small" || rec.Resolved.Provider != "openai" {
		t.Fatalf("%+v %v", rec, *reqs)
	}
	persisted, _ := d.Store.GetRun(context.Background(), rec.ID)
	if persisted.Resolved == nil || Summarize(persisted).Requested.Model != "gpt-small" {
		t.Fatal("routing not persisted in summary")
	}
}

func TestProviderAndTierIntersection(t *testing.T) {
	d, _, _ := routingDeps(t)
	for _, tt := range []struct {
		tier  adapter.Tier
		model string
	}{{adapter.TierEasy, "gpt-small"}, {adapter.TierHard, "gpt-large"}} {
		cs, rejected := d.selectCandidates(Options{Tier: tt.tier, Filters: Filters{Provider: "openai"}})
		if len(cs) == 0 || cs[0].Model != tt.model {
			t.Fatalf("%v %v", cs, rejected)
		}
	}
	cs, _ := d.selectCandidates(Options{Tier: adapter.TierHard, Filters: Filters{Model: "gpt-small"}})
	if len(cs) != 0 {
		t.Fatal("escaped tier intersection")
	}
}

func TestFixedModelEffortOnlyEvaluatesServingAgents(t *testing.T) {
	d, _, _ := routingDeps(t)
	cs, _ := d.selectCandidates(Options{Filters: Filters{Model: "gpt-small", Effort: "xhigh"}})
	for _, c := range d.evaluate(context.Background(), cs, true) {
		if c.Model != "gpt-small" || c.Effort != "xhigh" {
			t.Fatalf("ranked a different model: %+v", c)
		}
	}
	// Filtering occurs before quota/ranking: an unrelated model is never evaluated.
	calls := 0
	a := d.Adapters[0].(routingFake)
	a.fakeAdapter.name = "unrelated"
	a.calls = &calls
	d.Adapters = append(d.Adapters, a)
	d.Table.Tiers[adapter.TierHard] = append(d.Table.Tiers[adapter.TierHard], Entry{Agent: "unrelated", Model: "grok-other"})
	cs, _ = d.selectCandidates(Options{Filters: Filters{Model: "gpt-small", Effort: "high"}})
	d.evaluate(context.Background(), cs, true)
	if calls != 0 {
		t.Fatal("unrelated model entered ranking")
	}
}

func TestFiltersFailAndRelax(t *testing.T) {
	for _, relax := range []bool{false, true} {
		t.Run(map[bool]string{false: "strict", true: "relax"}[relax], func(t *testing.T) {
			d, ran, _ := routingDeps(t)
			rec, err := d.Run(context.Background(), Options{Tier: adapter.TierEasy, Filters: Filters{Provider: "missing", Relax: relax}, Prompt: "p", NoWorktree: true})
			if err != nil {
				t.Fatal(err)
			}
			if !relax && (rec.Status != StatusFailed || len(*ran) != 0 || !strings.Contains(rec.Error, "provider filter")) {
				t.Fatalf("%+v %v", rec, *ran)
			}
			if relax && (rec.Status != StatusCompleted || !strings.Contains(rec.Note, "--relax")) {
				t.Fatalf("%+v", rec)
			}
		})
	}
}

func TestFallbackNeverChangesFixedModel(t *testing.T) {
	d, ran, _ := routingDeps(t)
	for i, a := range d.Adapters {
		f := a.(routingFake)
		f.fakeAdapter.err = errors.New("failed")
		d.Adapters[i] = f
	}
	rec, _ := d.Run(context.Background(), Options{Tier: adapter.TierEasy, Filters: Filters{Model: "gpt-small"}, Prompt: "p", NoWorktree: true})
	if rec.Status != StatusFailed || len(*ran) != 2 {
		t.Fatalf("%+v %v", rec, *ran)
	}
	for _, run := range *ran {
		if !strings.HasSuffix(run, "/gpt-small") {
			t.Fatal(run)
		}
	}
}

func TestUnsupportedEffortRefused(t *testing.T) {
	d, ran, _ := routingDeps(t)
	a := d.Adapters[0].(routingFake)
	a.caps.EffortMode = ""
	a.caps.Efforts = nil
	d.Adapters = []adapter.Adapter{a}
	rec, _ := d.Run(context.Background(), Options{Tier: adapter.TierEasy, Filters: Filters{Agent: "a", Model: "gpt-small", Effort: "high"}, Prompt: "p", NoWorktree: true})
	if rec.Status != StatusFailed || len(*ran) != 0 || !strings.Contains(rec.Error, "effort unsupported") {
		t.Fatalf("%+v", rec)
	}
	a.caps.EffortMode = "config"
	a.caps.Efforts = []string{"low"}
	d.Adapters = []adapter.Adapter{a}
	cs, why := d.selectCandidates(Options{Tier: adapter.TierEasy, Filters: Filters{Agent: "a", Model: "gpt-small", Effort: "high"}})
	if len(cs) != 0 || !strings.Contains(strings.Join(why, ";"), "unsupported") {
		t.Fatal(cs, why)
	}
}

func TestRouteNoSideEffects(t *testing.T) {
	d, ran, _ := routingDeps(t)
	fs := d.Store.(*fakeStore)
	calls := 0
	for i, a := range d.Adapters {
		f := a.(routingFake)
		f.calls = &calls
		d.Adapters[i] = f
	}
	used := 5.0
	if err := d.Store.SaveQuota(context.Background(), QuotaSnapshot{
		Agent:         "b",
		UsedPercent:   &used,
		WindowMinutes: 10080,
		ResetsAt:      d.Now().Add(84 * time.Hour),
		SampledAt:     d.Now(),
	}); err != nil {
		t.Fatal(err)
	}
	beforeWrites := fs.writeCount()
	r, err := d.Route(context.Background(), Options{Filters: Filters{Model: "gpt-small", Effort: "high"}})
	afterWrites := fs.writeCount()
	if err != nil || r.Winner == nil || r.Winner.Agent != "b" || calls != 0 || len(*ran) != 0 || beforeWrites != afterWrites {
		t.Fatalf("%+v %v calls=%d", r, err, calls)
	}
	runs, _ := d.Store.ListRuns(context.Background(), "", 0, 0)
	if len(runs) != 0 {
		t.Fatal("created runs")
	}
	if _, err := os.Stat(d.WorktreeDir); err == nil {
		entries, _ := os.ReadDir(d.WorktreeDir)
		if len(entries) > 0 {
			t.Fatal("created worktree")
		}
	}
}

func TestOutsideTableAndSafetyConstraints(t *testing.T) {
	d, _, _ := routingDeps(t)
	cs, _ := d.selectCandidates(Options{Tier: adapter.TierHard, Filters: Filters{Model: "gpt-new"}})
	if len(cs) != 2 || cs[0].Note == "" {
		t.Fatal(cs)
	}
	cs, why := d.selectCandidates(Options{Filters: Filters{Model: "claude-opus", Agent: "a"}})
	if len(cs) != 0 || !strings.Contains(strings.Join(why, ";"), "Claude") {
		t.Fatal(cs, why)
	}
	_ = d.Store.SetCooldown(context.Background(), "b", d.Now().Add(time.Hour))
	cs, _ = d.selectCandidates(Options{Filters: Filters{Model: "gpt-small"}})
	if len(cs) != 1 || cs[0].Adapter.Name() != "a" {
		t.Fatal(cs)
	}
}

func TestSuffixEffortAndReadonly(t *testing.T) {
	d, _, _ := routingDeps(t)
	a := d.Adapters[0].(routingFake)
	a.caps = adapter.Capabilities{ReadOnly: true, FileEditing: true, EffortMode: "model-suffix"}
	d.Adapters = []adapter.Adapter{a}
	d.Table = Table{Tiers: map[adapter.Tier][]Entry{adapter.TierEasy: {{Agent: "a", Model: "gemini-fast-low", Efforts: []string{"low", "high"}}}}}
	cs, why := d.selectCandidates(Options{Tier: adapter.TierEasy, Filters: Filters{Model: "gemini-fast", Effort: "high"}})
	if len(cs) != 1 || cs[0].Model != "gemini-fast-high" || cs[0].Effort != "high" {
		t.Fatal(cs, why)
	}
	cs, why = d.selectCandidates(Options{Tier: adapter.TierEasy, Filters: Filters{Model: "gemini-fast", Effort: "xhigh"}})
	if len(cs) != 0 || !strings.Contains(strings.Join(why, ";"), "unsupported") {
		t.Fatal(cs, why)
	}
	a.caps.ReadOnly = false
	d.Adapters = []adapter.Adapter{a}
	cs, why = d.selectCandidates(Options{Tier: adapter.TierEasy, ReadOnly: true})
	if len(cs) != 0 || !strings.Contains(strings.Join(why, ";"), "readOnly") {
		t.Fatal(cs, why)
	}
}

func priorityDeps(t *testing.T) Deps {
	t.Helper()
	d, _, _ := routingDeps(t)
	for i, a := range d.Adapters {
		f := a.(routingFake)
		f.quota = adapter.Quota{}
		f.caps = adapter.Capabilities{ReadOnly: true, FileEditing: true}
		d.Adapters[i] = f
	}
	d.Table = Table{Tiers: map[adapter.Tier][]Entry{
		adapter.TierNormal: {
			{Agent: "a", Model: "fast", Provider: "openai", Speed: 5, Strength: 1},
			{Agent: "b", Model: "strong", Provider: "google", Speed: 1, Strength: 5},
		},
	}}
	return d
}

// rankedForPriority ranks the normal tier by score, as a filtered route does,
// so the priority weights stay measurable apart from the tier's table order.
func rankedForPriority(t *testing.T, d Deps, priority string) []Candidate {
	t.Helper()
	cs, _ := d.selectCandidates(Options{Tier: adapter.TierNormal, Filters: Filters{Priority: priority}})
	out := Rank(d.observe(context.Background(), cs, false))
	if len(out) == 0 {
		t.Fatal("no candidates")
	}
	return out
}

func TestPriorityRoutesDifferentModels(t *testing.T) {
	d := priorityDeps(t)
	route := func(priority string) RouteReport {
		t.Helper()
		r, err := d.Route(context.Background(), Options{Tier: adapter.TierNormal, Filters: Filters{Priority: priority}})
		if err != nil || r.Winner == nil || len(r.Candidates) != 2 {
			t.Fatalf("%s: %+v %v", priority, r, err)
		}
		return r
	}
	// A tier route follows the table whatever the priority.
	for _, priority := range []string{PrioritySpeed, PriorityQuality, PriorityBalanced} {
		if r := route(priority); r.Winner.Agent != "a" || r.Winner.Model != "fast" {
			t.Fatalf("%s: tier route left the table order: %+v", priority, r.Winner)
		}
	}
	speed := rankedForPriority(t, d, PrioritySpeed)
	if speed[0].Adapter.Name() != "a" || speed[0].Model != "fast" {
		t.Fatalf("speed %s/%s", speed[0].Adapter.Name(), speed[0].Model)
	}
	if !strings.Contains(Reason(speed[0]), "paceSlack=") || !strings.Contains(Reason(speed[0]), "duration=") {
		t.Fatal(Reason(speed[0]))
	}
	if quality := rankedForPriority(t, d, PriorityQuality); quality[0].Adapter.Name() != "b" || quality[0].Model != "strong" {
		t.Fatalf("quality %s/%s", quality[0].Adapter.Name(), quality[0].Model)
	}
	_, rejS := d.selectCandidates(Options{Tier: adapter.TierNormal, Filters: Filters{Priority: PrioritySpeed}})
	_, rejQ := d.selectCandidates(Options{Tier: adapter.TierNormal, Filters: Filters{Priority: PriorityQuality}})
	if strings.Join(rejS, "\n") != strings.Join(rejQ, "\n") {
		t.Fatalf("priority filtered\n%v\n%v", rejS, rejQ)
	}
	rec, err := d.Run(context.Background(), Options{Tier: adapter.TierNormal, Filters: Filters{Priority: PriorityQuality}, Prompt: "p", NoWorktree: true})
	if err != nil || rec.Agent != "a" || rec.Model != "fast" {
		t.Fatal(rec, err)
	}
	if rec.Requested == nil || rec.Requested.Priority != PriorityQuality || rec.Resolved == nil || rec.Resolved.Priority != PriorityQuality {
		t.Fatalf("record %+v %+v", rec.Requested, rec.Resolved)
	}
	sum := Summarize(rec)
	if sum.Requested.Priority != PriorityQuality || sum.Resolved.Priority != PriorityQuality {
		t.Fatalf("summary %+v %+v", sum.Requested, sum.Resolved)
	}
}

func TestMeasuredEvidenceOverridesPriorAfterMinSamples(t *testing.T) {
	d := priorityDeps(t)
	entries := d.Table.Tiers[adapter.TierNormal]
	entries[0].Speed = 1
	entries[1].Speed = 5
	d.Table.Tiers[adapter.TierNormal] = entries
	n := 0
	seed := func(tier, model string, count int, ms int64) {
		t.Helper()
		for i := 0; i < count; i++ {
			n++
			err := d.Store.SaveRun(context.Background(), Run{
				ID: fmt.Sprintf("e-%d", n), Status: StatusCompleted, Tier: tier, Model: model,
				RunMs: I64(ms), CreatedAt: d.Now(), FinishedAt: d.Now(),
			})
			if err != nil {
				t.Fatal(err)
			}
		}
	}
	seed("hard", "fast", 8, 1000)
	winner := func() string {
		t.Helper()
		return rankedForPriority(t, d, PrioritySpeed)[0].Model
	}
	if winner() != "strong" {
		t.Fatal("hard-tier samples changed the easy/normal prior")
	}
	seed("normal", "fast", 4, 1000)
	seed("normal", "strong", 4, 600000)
	if winner() != "strong" {
		t.Fatal("prior overridden before min samples")
	}
	seed("normal", "fast", 1, 1000)
	seed("normal", "strong", 1, 600000)
	if top := rankedForPriority(t, d, PrioritySpeed)[0]; top.Model != "fast" || !strings.Contains(Reason(top), "blended") {
		t.Fatalf("%s %s", top.Model, Reason(top))
	}
}

func TestPriorityDoesNotBypassConstraints(t *testing.T) {
	d := priorityDeps(t)
	if err := d.Store.SetCooldown(context.Background(), "a", d.Now().Add(time.Hour)); err != nil {
		t.Fatal(err)
	}
	cs, why := d.selectCandidates(Options{Tier: adapter.TierNormal, Filters: Filters{Priority: PrioritySpeed}})
	if len(cs) != 1 || cs[0].Adapter.Name() != "b" || !strings.Contains(strings.Join(why, ";"), "cooldown") {
		t.Fatal(cs, why)
	}
	d = priorityDeps(t)
	cs, why = d.selectCandidates(Options{Tier: adapter.TierNormal, Filters: Filters{Priority: PrioritySpeed, Provider: "google"}})
	if len(cs) != 1 || cs[0].Model != "strong" || !strings.Contains(strings.Join(why, ";"), "provider filter") {
		t.Fatal(cs, why)
	}
	a := d.Adapters[0].(routingFake)
	a.caps.ReadOnly = false
	d.Adapters[0] = a
	row := d.Table.Tiers[adapter.TierNormal]
	row[0].Strength = 5
	row[1].Strength = 1
	d.Table.Tiers[adapter.TierNormal] = row
	cs, why = d.selectCandidates(Options{Tier: adapter.TierNormal, ReadOnly: true, Filters: Filters{Priority: PriorityQuality}})
	if len(cs) != 1 || cs[0].Adapter.Name() != "b" || !strings.Contains(strings.Join(why, ";"), "readOnly") {
		t.Fatal(cs, why)
	}
}

func TestRelaxAfterFilteredCandidatesFail(t *testing.T) {
	d, ran, _ := routingDeps(t)
	for i, a := range d.Adapters {
		f := a.(routingFake)
		if f.fakeAdapter.name == "a" {
			f.fakeAdapter.err = errors.New("crash")
		}
		d.Adapters[i] = f
	}
	rec, err := d.Run(context.Background(), Options{Tier: adapter.TierEasy, Filters: Filters{Agent: "a", Relax: true}, Prompt: "p", NoWorktree: true})
	if err != nil || rec.Status != StatusCompleted || rec.Agent != "b" || !strings.Contains(rec.Note, "--relax") || len(*ran) != 3 {
		t.Fatal(rec, err, *ran)
	}
}

func TestPersistentPolicyAppliesToFallbackRelaxAndFlags(t *testing.T) {
	d, ran, _ := routingDeps(t)
	d.Policy.Allow.Providers = []string{"openai"}
	d.Policy.Deny.Models = []string{"gpt-small"}
	rec, _ := d.Run(context.Background(), Options{Tier: adapter.TierEasy, Filters: Filters{Relax: true}, Prompt: "p", NoWorktree: true})
	if rec.Status != StatusFailed || len(*ran) != 0 || !strings.Contains(rec.Error, "policy deny.models") {
		t.Fatalf("%+v %v", rec, *ran)
	}
	d.Policy.Deny = Rules{}
	cs, why := d.selectCandidates(Options{Tier: adapter.TierEasy, Filters: Filters{Provider: "google", Relax: true}})
	if len(cs) != 0 || !strings.Contains(strings.Join(why, ";"), "policy allow.providers") {
		t.Fatalf("%v %v", cs, why)
	}
	rec, _ = d.Run(context.Background(), Options{Tier: adapter.TierEasy, Filters: Filters{Provider: "google", Relax: true}, Prompt: "p", NoWorktree: true})
	if rec.Status != StatusFailed || !strings.Contains(rec.Error, "policy allow.providers") {
		t.Fatalf("%+v", rec)
	}
}

func TestDiscoverAddsNormalCandidateOnlyWhenEnabled(t *testing.T) {
	d, _, _ := routingDeps(t)
	a := d.Adapters[0].(routingFake)
	a.models = []adapter.Model{{ID: "gpt-discovered"}}
	d.Adapters[0] = a
	cs, _ := d.selectCandidatesWith(context.Background(), Options{Tier: adapter.TierNormal})
	if len(cs) != 0 {
		t.Fatal("discover default must be off", cs)
	}
	d.Policy.Discover = true
	cs, why := d.selectCandidatesWith(context.Background(), Options{Tier: adapter.TierNormal})
	if len(cs) != 1 || cs[0].Model != "gpt-discovered" || cs[0].Note != "discovered" {
		t.Fatal(cs, why)
	}
}
