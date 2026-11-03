package dispatch

import (
	"context"
	"strings"
	"testing"
	"time"
)

func statsMs(n int64) *int64 { return &n }

func TestComputeRatesPercentilesAndRanks(t *testing.T) {
	now := time.Date(2026, 10, 15, 12, 0, 0, 0, time.UTC)
	mk := func(id, model, provider, status string, run *int64, attempts []Attempt, class string) Run {
		return Run{
			ID: id, Model: model, Provider: provider, Agent: "codex", Tier: "normal",
			Status: status, CreatedAt: now.Add(-time.Hour), RunMs: run, Attempts: attempts, ExitClass: class,
		}
	}
	var recs []Run
	for i, d := range []int64{10, 20, 30, 40, 50, 60} {
		recs = append(recs, mk("a"+string(rune('0'+i)), "gpt-5.6-terra", "openai", StatusCompleted, statsMs(d), nil, "ok"))
	}
	rl := []Attempt{{Agent: "grok", Model: "grok-4.7", Error: "429 too many requests", ExitClass: "rate_limit"}}
	recs = append(recs,
		mk("a6", "gpt-5.6-terra", "openai", StatusCompleted, statsMs(70), rl, "ok"),
		mk("a7", "gpt-5.6-terra", "openai", StatusCompleted, statsMs(80), rl, "ok"),
		mk("a8", "gpt-5.6-terra", "openai", StatusFailed, statsMs(90), rl, "rate_limit"),
		mk("a9", "gpt-5.6-terra", "openai", StatusOrphaned, nil, nil, ""),
	)
	for i := 0; i < 5; i++ {
		recs = append(recs, mk("b"+string(rune('0'+i)), "gemini-3.8-flash-low", "google", StatusFailed, statsMs(200), nil, "failed"))
	}
	recs = append(recs, Run{ID: "fly", Status: StatusRunning, Model: "gpt-5.6-terra", CreatedAt: now})

	rep, err := Compute(recs, StatsOptions{By: "model", Since: now.Add(-7 * 24 * time.Hour), Now: now})
	if err != nil {
		t.Fatal(err)
	}
	if rep.Schema != StatsSchema || rep.InFlight != 1 || rep.MinSamples != MinSamples {
		t.Fatalf("%+v", rep)
	}
	gpt := findRow(t, rep.Rows, "gpt-5.6-terra")
	if gpt.Runs != 10 || gpt.SuccessPct != 80 || gpt.FirstTryPct != 60 || gpt.Fallbacks != 3 || gpt.RateLimits != 3 || gpt.Orphans != 1 {
		t.Fatalf("%+v", gpt)
	}
	if gpt.P50Ms == nil || *gpt.P50Ms != 50 || gpt.P90Ms == nil || *gpt.P90Ms != 90 || gpt.DurationN != 9 {
		t.Fatalf("percentiles %+v", gpt)
	}
	gem := findRow(t, rep.Rows, "gemini-3.8-flash-low")
	if gem.Runs != 5 || gem.SuccessPct != 0 || gem.P50Ms == nil || *gem.P50Ms != 200 {
		t.Fatalf("%+v", gem)
	}
	if rep.Rankings.MostUsedModel.Key != "gpt-5.6-terra" || rep.Rankings.MostUsedModel.Runs != 10 {
		t.Fatalf("used %+v", rep.Rankings.MostUsedModel)
	}
	if rep.Rankings.MostUsedProvider.Key != "openai" || rep.Rankings.MostUsedProvider.Runs != 10 {
		t.Fatalf("provider %+v", rep.Rankings.MostUsedProvider)
	}
	if rep.Rankings.SlowestModel.Key != "gemini-3.8-flash-low" || rep.Rankings.SlowestModel.P50Ms == nil || *rep.Rankings.SlowestModel.P50Ms != 200 {
		t.Fatalf("slow %+v", rep.Rankings.SlowestModel)
	}
	if rep.Rankings.MostFailedModel.Key != "gemini-3.8-flash-low" || rep.Rankings.MostFailedModel.SuccessPct == nil || *rep.Rankings.MostFailedModel.SuccessPct != 0 {
		t.Fatalf("fail %+v", rep.Rankings.MostFailedModel)
	}
	text := rep.Text()
	if !strings.Contains(text, "gpt-5.6-terra") || !strings.Contains(text, "slowest model: gemini-3.8-flash-low") {
		t.Fatal(text)
	}

	// Too few samples to call a model slow or failure-prone.
	small, err := Compute([]Run{
		mk("s1", "fast", "openai", StatusCompleted, statsMs(1), nil, "ok"),
		mk("s2", "slow", "openai", StatusFailed, statsMs(999), nil, "failed"),
	}, StatsOptions{By: "model", Now: now})
	if err != nil {
		t.Fatal(err)
	}
	if small.Rankings.SlowestModel.Key != "unknown" || !strings.Contains(small.Rankings.SlowestModel.Reason, "minimum") {
		t.Fatalf("%+v", small.Rankings.SlowestModel)
	}
	if small.Rankings.MostFailedModel.Reason == "" || small.Rankings.MostUsedModel.Key == "" {
		t.Fatalf("%+v", small.Rankings)
	}
	if !strings.Contains(small.Text(), "unknown") {
		t.Fatal(small.Text())
	}
}

func TestUnknownGroupSinceAndZeroDuration(t *testing.T) {
	now := time.Date(2026, 10, 15, 12, 0, 0, 0, time.UTC)
	old := now.Add(-30 * 24 * time.Hour)
	recs := []Run{
		{ID: "e1", Status: StatusCompleted, Effort: "high", Model: "gemini-3.8-flash-high", CreatedAt: now, RunMs: statsMs(0)},
		{ID: "e2", Status: StatusFailed, Model: "grok-4.7", CreatedAt: now},
		{ID: "old", Status: StatusCompleted, Model: "gpt-5.6-terra", CreatedAt: old, RunMs: statsMs(5)},
		{ID: "d1", Status: StatusCompleted, Model: "m", CreatedAt: time.Date(2026, 10, 14, 8, 0, 0, 0, time.UTC)},
		{ID: "d2", Status: StatusCompleted, Model: "m", CreatedAt: time.Date(2026, 10, 15, 8, 0, 0, 0, time.UTC)},
	}
	rep, err := Compute(recs, StatsOptions{By: "effort", Since: now.Add(-7 * 24 * time.Hour), Now: now})
	if err != nil {
		t.Fatal(err)
	}
	high := findRow(t, rep.Rows, "high")
	if high.Runs != 1 || high.DurationN != 1 || high.P50Ms == nil || *high.P50Ms != 0 || high.Duration != "" {
		t.Fatalf("known zero %+v", high)
	}
	unk := findRow(t, rep.Rows, "unknown")
	if unk.Runs != 3 || unk.Duration != "unknown" || unk.P50Ms != nil {
		t.Fatalf("unknown %+v", unk)
	}
	// The 30-day-old run is outside the window. d1 and d2 and e2 have no effort.
	if len(rep.Rows) != 2 {
		t.Fatalf("rows %+v", rep.Rows)
	}

	day, err := Compute(recs[3:], StatsOptions{By: "day", Now: now})
	if err != nil {
		t.Fatal(err)
	}
	if len(day.Rows) != 2 || day.Rows[0].Key != "2026-10-14" || day.Rows[1].Key != "2026-10-15" {
		t.Fatalf("%+v", day.Rows)
	}
	week, err := Compute(recs[3:], StatsOptions{By: "week", Now: now})
	if err != nil || len(week.Rows) != 1 || !strings.HasPrefix(week.Rows[0].Key, "2026-W") {
		t.Fatalf("%+v %v", week.Rows, err)
	}
	if _, err := Compute(recs, StatsOptions{By: "nope"}); err == nil {
		t.Fatal("bad --by")
	}
	start, err := ParseSince("7d", now)
	if err != nil || !start.Equal(now.Add(-7*24*time.Hour)) {
		t.Fatal(start, err)
	}
	if start, err = ParseSince("all", now); err != nil || !start.IsZero() {
		t.Fatal(start, err)
	}
	if _, err := ParseSince("soon", now); err == nil {
		t.Fatal("expected error")
	}
}

func TestStatsFromStore(t *testing.T) {
	s := newFakeStore()
	ctx := context.Background()
	_ = s.SaveRun(ctx, Run{ID: "old", Status: StatusCompleted, Model: "gpt-5.6-terra", Provider: "openai", CreatedAt: time.Date(2026, 10, 10, 0, 0, 0, 0, time.UTC), RunMs: statsMs(10)})
	_ = s.SaveRun(ctx, Run{ID: "new", Status: StatusFailed, Model: "gpt-5.6-terra", Provider: "openai", CreatedAt: time.Date(2026, 10, 14, 0, 0, 0, 0, time.UTC), RunMs: statsMs(20)})
	rep, err := FromStore(s, StatsOptions{By: "model", Now: time.Date(2026, 10, 15, 0, 0, 0, 0, time.UTC)})
	if err != nil {
		t.Fatal(err)
	}
	row := findRow(t, rep.Rows, "gpt-5.6-terra")
	if row.Runs != 2 || row.SuccessPct != 50 || row.DurationN != 2 {
		t.Fatalf("%+v", row)
	}
}

func TestStatsByAreaReadsTheRoutingDecisions(t *testing.T) {
	s := newFakeStore()
	ctx := context.Background()
	now := time.Date(2026, 10, 15, 0, 0, 0, 0, time.UTC)
	for _, id := range []string{"a", "b", "c"} {
		_ = s.SaveRun(ctx, Run{ID: id, Status: StatusCompleted, CreatedAt: now.Add(-time.Hour)})
	}
	_ = s.SaveRouteDecision(ctx, RouteDecision{RunID: "a", Area: NewArea("service", "go"), CreatedAt: now})
	_ = s.SaveRouteDecision(ctx, RouteDecision{RunID: "b", Area: NewArea("service", "go"), CreatedAt: now})
	rep, err := FromStore(s, StatsOptions{By: "area", Now: now})
	if err != nil {
		t.Fatal(err)
	}
	if row := findRow(t, rep.Rows, "service, go"); row.Runs != 2 {
		t.Fatalf("service, go: %+v", row)
	}
	if row := findRow(t, rep.Rows, "unknown"); row.Runs != 1 {
		t.Fatalf("a run with no decision has no area: %+v", row)
	}
}

func TestCapacityWindows(t *testing.T) {
	now := time.Date(2026, 10, 15, 0, 0, 0, 0, time.UTC)
	week := 7 * 24 * time.Hour
	snap := func(key string, used float64, reset, at time.Time) QuotaSnapshot {
		u := used
		return QuotaSnapshot{
			Agent:         key,
			UsedPercent:   &u,
			ResetsAt:      reset,
			WindowMinutes: int(week / time.Minute),
			SampledAt:     at,
		}
	}
	reset1 := now.Add(-48 * time.Hour)
	reset2 := now.Add(-14 * 24 * time.Hour)
	reset3 := now.Add(72 * time.Hour)
	rep := Capacity([]QuotaSnapshot{
		snap("codex", 40, reset1, reset1.Add(-time.Hour)),
		snap("codex", 100, reset2, reset2.Add(-30*time.Minute)),
		snap("codex", 80, reset3, now.Add(-time.Hour)),
	}, time.Time{}, now)
	if len(rep.Keys) != 1 {
		t.Fatalf("%+v", rep.Keys)
	}
	k := rep.Keys[0]
	if k.Windows != 3 || k.HitFull != 1 || k.LeftoverKnown != 2 || k.LeftoverUnknown != 1 {
		t.Fatalf("%+v", k)
	}
	if k.ConsumedPct == nil || *k.ConsumedPct != 73.3 || k.LeftoverPct == nil || *k.LeftoverPct != 30 {
		t.Fatalf("means consumed %v leftover %v", k.ConsumedPct, k.LeftoverPct)
	}
	if k.Pace != "ahead" {
		t.Fatal(k.Pace)
	}
	var closed, open, full int
	for _, w := range k.Detail {
		if w.Open {
			open++
			if w.Leftover != "unknown" || w.LeftoverPct != nil || w.Pace != "ahead" {
				t.Fatalf("open %+v", w)
			}
		} else {
			closed++
		}
		if w.HitFull {
			full++
			if w.LeftoverPct == nil || *w.LeftoverPct != 0 || w.Pace != "on pace" {
				t.Fatalf("full %+v", w)
			}
		}
		if !w.Open && !w.HitFull {
			if w.LeftoverPct == nil || *w.LeftoverPct != 60 || w.Pace != "behind" {
				t.Fatalf("partial %+v", w)
			}
		}
	}
	if closed != 2 || open != 1 || full != 1 {
		t.Fatalf("closed %d open %d full %d", closed, open, full)
	}
	if !strings.Contains(rep.Text(), "unknown") || !strings.Contains(rep.Note, "unknown") {
		t.Fatal(rep.Text())
	}

	empty := Capacity(nil, time.Time{}, now)
	if len(empty.Keys) != 0 || !strings.Contains(empty.Note, "unknown") {
		t.Fatalf("%+v", empty)
	}

	// 99.5 counts as touching 100; a sample far from the reset leaves leftover unknown.
	barely := now.Add(-time.Hour)
	early := barely.Add(-7 * 24 * time.Hour).Add(time.Hour)
	edge := Capacity([]QuotaSnapshot{
		snap("codex", 99.5, barely, barely.Add(-time.Hour)),
		snap("grok", 10, barely, early),
	}, time.Time{}, now)
	if len(edge.Keys) != 2 || edge.Keys[0].Key != "codex" || edge.Keys[0].HitFull != 1 {
		t.Fatalf("%+v", edge.Keys)
	}
	grok := edge.Keys[1]
	if grok.Key != "grok" || grok.Leftover != "unknown" || grok.HitFull != 0 {
		t.Fatalf("%+v", grok)
	}
}

func findRow(t *testing.T, rows []Row, key string) Row {
	t.Helper()
	for _, r := range rows {
		if r.Key == key {
			return r
		}
	}
	t.Fatalf("missing %s in %+v", key, rows)
	return Row{}
}

func TestEvidenceSegmentsByTier(t *testing.T) {
	ms := func(n int64) *int64 { return &n }
	recs := []Run{
		{ID: "1", Status: StatusCompleted, Tier: "easy", Model: "m", RunMs: ms(1000)},
		{ID: "2", Status: StatusCompleted, Tier: "easy", Model: "m", RunMs: ms(3000)},
		{ID: "2b", Status: StatusCompleted, Tier: "easy", Model: "m", RunMs: ms(5000)},
		{ID: "3", Status: StatusCompleted, Tier: "hard", Model: "m", RunMs: ms(9000)},
		{ID: "4", Status: StatusRunning, Tier: "easy", Model: "m", RunMs: ms(1)},
		{ID: "5", Status: StatusCompleted, Tier: "easy", Model: "", RunMs: ms(1)},
	}
	ev := EvidenceByTierModel(recs)
	easy := ev[EvidenceKey("easy", "m")]
	hard := ev[EvidenceKey("hard", "m")]
	if easy.Finished != 3 || easy.DurationN != 3 || hard.Finished != 1 {
		t.Fatalf("easy %+v hard %+v", easy, hard)
	}
	if easy.P50Ms == nil || *easy.P50Ms != 3000 || hard.P50Ms == nil || *hard.P50Ms != 9000 {
		t.Fatalf("easy %+v hard %+v", easy, hard)
	}
	if _, ok := ev[EvidenceKey("easy", "")]; ok {
		t.Fatal("empty model counted")
	}
}

func TestRoutingFilterAggregations(t *testing.T) {
	r := Run{ID: "filters", Status: StatusCompleted, Requested: &Filters{Provider: "google"}, Resolved: &Filters{Provider: "google", Model: "gemini-fast", Agent: "antigravity", Effort: "high"}}
	for _, by := range []string{"requested", "resolved"} {
		report, err := Compute([]Run{r}, StatsOptions{By: by})
		if err != nil || len(report.Rows) != 1 || !strings.Contains(report.Rows[0].Key, "google") {
			t.Fatalf("%+v %v", report, err)
		}
	}
}

func TestEvidenceDurationsOnlyFromCompletedRuns(t *testing.T) {
	ms := func(n int64) *int64 { return &n }
	recs := []Run{
		{ID: "1", Status: StatusCompleted, Tier: "easy", Model: "m", RunMs: ms(1000)},
		{ID: "2", Status: StatusTimedOut, Tier: "easy", Model: "m", RunMs: ms(600000)},
		{ID: "3", Status: StatusCancelled, Tier: "easy", Model: "m", RunMs: ms(500000)},
	}
	e := EvidenceByTierModel(recs)[EvidenceKey("easy", "m")]
	if e.Finished != 3 || e.DurationN != 1 || e.P50Ms == nil || *e.P50Ms != 1000 {
		t.Fatalf("%+v", e)
	}
}
