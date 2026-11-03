package core

import (
	"context"
	"encoding/json"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"connectrpc.com/connect"

	"github.com/polterware/dispatch/go-core/domains/dispatch"
	dispatchv1 "github.com/polterware/dispatch/go-core/platform/gen/dispatch/v1"
	"github.com/polterware/dispatch/go-core/platform/gen/dispatch/v1/dispatchv1connect"
	"github.com/polterware/dispatch/go-core/providers/adapters"
	storepkg "github.com/polterware/dispatch/go-core/providers/sqlite"
)

func TestReplaceTiersPreservesBuiltEffort(t *testing.T) {
	store, err := storepkg.New(filepath.Join(t.TempDir(), "dispatch.db"))
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = store.Close() })
	core := &Core{Store: store}
	ctx := context.Background()
	cat := dispatch.Catalog{Scores: []dispatch.BenchScore{
		{Model: "gpt-test", Effort: "low", Score: 0.1, CostPerTask: dispatch.F64(0.1)},
		{Model: "gpt-test", Effort: "medium", Score: 0.5, CostPerTask: dispatch.F64(0.5)},
		{Model: "gpt-test", Effort: "high", Score: 0.8, CostPerTask: dispatch.F64(1)},
	}}
	built := dispatch.BuildTiers([]dispatch.Entry{{Agent: "codex", Model: "gpt-test", Efforts: []string{"low", "medium", "high"}}}, cat, dispatch.PriorityBalanced)
	if built.Table.Tiers[adapters.TierEasy][0].Effort != "medium" {
		t.Fatalf("easy effort: %+v", built.Table)
	}
	if err := store.ReplaceTiers(ctx, built.Table.Rows()); err != nil {
		t.Fatal(err)
	}
	listed, err := core.ListTiers(ctx, connect.NewRequest(&dispatchv1.ListTiersRequest{}))
	if err != nil {
		t.Fatal(err)
	}
	// A board edit replaces the table through the existing model-suffix API.
	for _, row := range listed.Msg.Tiers {
		if row.Name == "easy" && row.Model != "gpt-test-medium" {
			t.Fatalf("effort not exposed to board: %+v", row)
		}
	}
	_, err = core.ReplaceTiers(ctx, connect.NewRequest(&dispatchv1.ReplaceTiersRequest{Tiers: listed.Msg.Tiers}))
	if err != nil {
		t.Fatal(err)
	}
	table, err := dispatch.LoadTiersFromStore(ctx, store)
	if err != nil || table.Tiers[adapters.TierEasy][0].Model != "gpt-test" || table.Tiers[adapters.TierEasy][0].Effort != "medium" {
		t.Fatalf("restored: %+v, %v", table, err)
	}
}

func TestDispatchServiceRequiresBearerToken(t *testing.T) {
	_, url := startCore(t)
	client := dispatchv1connect.NewDispatchServiceClient(http.DefaultClient, url)
	_, err := client.ListTiers(context.Background(), connect.NewRequest(&dispatchv1.ListTiersRequest{}))
	if connect.CodeOf(err) != connect.CodeUnauthenticated {
		t.Fatalf("code = %v, want unauthenticated", connect.CodeOf(err))
	}
}

func TestDispatchTiersStatsResultAndWatch(t *testing.T) {
	core, url := startCore(t)
	core.DBDir = t.TempDir()
	ctx := context.Background()
	client := dispatchv1connect.NewDispatchServiceClient(http.DefaultClient, url)

	tiersRes, err := client.ListTiers(ctx, withToken(connect.NewRequest(&dispatchv1.ListTiersRequest{}), testToken))
	if err != nil {
		t.Fatal(err)
	}
	if len(tiersRes.Msg.Tiers) == 0 {
		t.Fatal("expected seeded default tiers")
	}

	checkRes, err := client.CheckTiers(ctx, withToken(connect.NewRequest(&dispatchv1.CheckTiersRequest{}), testToken))
	if err != nil {
		t.Fatal(err)
	}
	_ = checkRes.Msg.Problems

	projRoot := t.TempDir()
	proj := storepkg.Workspace{ID: "proj-dispatch", Name: "Dispatch Test"}
	if err := core.Store.SaveWorkspace(proj); err != nil {
		t.Fatal(err)
	}

	now := time.Now().UTC()
	filesChanged := 2
	run := dispatch.Run{
		ID:           "run-1",
		ProjectID:    proj.ID,
		Status:       dispatch.StatusCompleted,
		Tier:         dispatch.TierEasy,
		Agent:        "codex",
		Model:        "gpt-5.4",
		Provider:     "openai",
		Prompt:       "fix bug",
		Dir:          projRoot,
		Output:       "line1\nline2\nline3\n",
		FilesChanged: &filesChanged,
		CreatedAt:    now.Add(-2 * time.Second),
		StartedAt:    now.Add(-2 * time.Second),
		FinishedAt:   now,
	}
	if err := core.Store.SaveRun(ctx, run); err != nil {
		t.Fatal(err)
	}
	if err := core.Store.AppendEvent(ctx, dispatch.RunEvent{
		RunID:     run.ID,
		ProjectID: proj.ID,
		Kind:      dispatch.EventStarted,
		Agent:     run.Agent,
		Tier:      run.Tier,
		Text:      run.Prompt,
		At:        now,
	}); err != nil {
		t.Fatal(err)
	}

	logPath := filepath.Join(core.dispatchStateDir(), "logs", run.ID+".agent.log")
	if err := os.MkdirAll(filepath.Dir(logPath), 0o700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(logPath, []byte("streamed line\n"), 0o600); err != nil {
		t.Fatal(err)
	}

	stream, err := client.WatchRun(ctx, withToken(connect.NewRequest(&dispatchv1.WatchRunRequest{Id: run.ID}), testToken))
	if err != nil {
		t.Fatal(err)
	}
	var kinds []string
	for stream.Receive() {
		kinds = append(kinds, stream.Msg().Kind)
	}
	if err := stream.Err(); err != nil {
		t.Fatal(err)
	}
	if len(kinds) < 2 || kinds[0] != dispatch.EventStarted || kinds[1] != dispatch.EventOutput {
		t.Fatalf("watch kinds = %v", kinds)
	}

	for _, sec := range []string{"summary", "files", "attempts", "meta", "output"} {
		res, err := client.Result(ctx, withToken(connect.NewRequest(&dispatchv1.ResultRequest{
			Id:      run.ID,
			Section: sec,
			Offset:  0,
			Limit:   10,
		}), testToken))
		if err != nil {
			t.Fatalf("section %s: %v", sec, err)
		}
		if sec == "meta" && strings.Contains(res.Msg.Content, "line1") {
			t.Fatalf("meta leaked output body: %s", res.Msg.Content)
		}
	}

	if _, err := client.Result(ctx, withToken(connect.NewRequest(&dispatchv1.ResultRequest{
		Id:      run.ID,
		Section: "unknown",
	}), testToken)); connect.CodeOf(err) != connect.CodeInvalidArgument {
		t.Fatalf("unknown section code = %v, want invalid_argument", connect.CodeOf(err))
	}

	statsRes, err := client.Stats(ctx, withToken(connect.NewRequest(&dispatchv1.StatsRequest{
		ProjectId: proj.ID,
		By:        "model",
		Since:     "7d",
	}), testToken))
	if err != nil {
		t.Fatal(err)
	}
	if len(statsRes.Msg.Rows) != 1 || statsRes.Msg.Rows[0].Key != "gpt-5.4" || statsRes.Msg.Rows[0].P50Ms == nil {
		t.Fatalf("stats = %+v", statsRes.Msg)
	}
}

func TestDispatchReplaceTiersStoresOrderAndRejectsInvalidRows(t *testing.T) {
	_, url := startCore(t)
	ctx := context.Background()
	client := dispatchv1connect.NewDispatchServiceClient(http.DefaultClient, url)

	res, err := client.ReplaceTiers(ctx, withToken(connect.NewRequest(&dispatchv1.ReplaceTiersRequest{Tiers: []*dispatchv1.DispatchTier{
		{Name: "easy", Agent: "grok", Model: "grok-4.7", Speed: 4, Strength: 2, Position: 9},
		{Name: "easy", Agent: "codex", Model: "gpt-5.6-luna"},
		{Name: "hard", Agent: "codex", Model: "gpt-6.1-sol", Strength: 5},
	}}), testToken))
	if err != nil {
		t.Fatal(err)
	}
	var easy []string
	for _, tier := range res.Msg.Tiers {
		if tier.Name == "easy" {
			easy = append(easy, tier.Agent)
			if tier.Agent == "codex" && tier.Position != 1 {
				t.Fatalf("codex position = %d, want 1", tier.Position)
			}
			if tier.Agent == "grok" && tier.Provider != "xai" {
				t.Fatalf("grok provider = %q, want derived xai", tier.Provider)
			}
		}
	}
	if strings.Join(easy, ",") != "grok,codex" {
		t.Fatalf("easy order = %v", easy)
	}

	listed, err := client.ListTiers(ctx, withToken(connect.NewRequest(&dispatchv1.ListTiersRequest{}), testToken))
	if err != nil {
		t.Fatal(err)
	}
	if len(listed.Msg.Tiers) != 3 {
		t.Fatalf("stored %d tiers, want 3", len(listed.Msg.Tiers))
	}

	for _, bad := range []*dispatchv1.DispatchTier{
		{Name: "extreme", Agent: "codex", Model: "gpt-5.6-luna"},
		{Name: "easy", Agent: "", Model: "gpt-5.6-luna"},
		{Name: "easy", Agent: "codex", Model: "gpt-5.6-luna", Speed: 9},
	} {
		_, err := client.ReplaceTiers(ctx, withToken(connect.NewRequest(&dispatchv1.ReplaceTiersRequest{Tiers: []*dispatchv1.DispatchTier{bad}}), testToken))
		if connect.CodeOf(err) != connect.CodeInvalidArgument {
			t.Fatalf("row %+v: code = %v, want invalid argument", bad, connect.CodeOf(err))
		}
	}
}

func TestDispatchModelCatalogPricesTheTierTable(t *testing.T) {
	_, url := startCore(t)
	client := dispatchv1connect.NewDispatchServiceClient(http.DefaultClient, url)
	res, err := client.ListModelCatalog(context.Background(), withToken(connect.NewRequest(&dispatchv1.ListModelCatalogRequest{}), testToken))
	if err != nil || !res.Msg.GetOk() {
		t.Fatalf("catalog = %+v, %v", res, err)
	}
	var report dispatch.CatalogReport
	if err := json.Unmarshal([]byte(res.Msg.GetJson()), &report); err != nil || report.Generated == "" || len(report.Entries) == 0 {
		t.Fatalf("report = %+v, %v", report, err)
	}
	for _, e := range report.Entries {
		if e.ThetaSource == "" || e.Key == "" || len(e.Bands) != 3 {
			t.Fatalf("unresolved entry %+v", e)
		}
	}
}

func TestDispatchRouterReadsExplainAndBuild(t *testing.T) {
	core, url := startCore(t)
	core.DBDir = t.TempDir()
	ctx := context.Background()
	client := dispatchv1connect.NewDispatchServiceClient(http.DefaultClient, url)

	proj := storepkg.Workspace{ID: "proj-route", Name: "Route"}
	if err := core.Store.SaveWorkspace(proj); err != nil {
		t.Fatal(err)
	}
	now := time.Now().UTC()
	if err := core.Store.SaveRun(ctx, dispatch.Run{ID: "route-1", ProjectID: proj.ID, Status: dispatch.StatusCompleted, Tier: "normal", Agent: "codex", Model: "m", CreatedAt: now}); err != nil {
		t.Fatal(err)
	}
	missing, err := client.GetRouteDecision(ctx, withToken(connect.NewRequest(&dispatchv1.GetRouteDecisionRequest{RunId: "route-1"}), testToken))
	if err != nil || missing.Msg.GetOk() {
		t.Fatalf("missing decision = %+v, %v", missing, err)
	}
	d := 0.4
	if err := core.Store.SaveRouteDecision(ctx, dispatch.RouteDecision{
		RunID: "route-1", ProjectID: proj.ID, PolicyVersion: dispatch.PolicyAutoV1, Route: dispatch.RouteAuto, Difficulty: &d,
		Area:       dispatch.NewArea("service", "Go"),
		Candidates: []dispatch.DecisionCandidate{{Agent: "codex", Model: "m", PPass: dispatch.F64(0.9)}}, CreatedAt: now,
	}); err != nil {
		t.Fatal(err)
	}
	dec, err := client.GetRouteDecision(ctx, withToken(connect.NewRequest(&dispatchv1.GetRouteDecisionRequest{RunId: "route-1"}), testToken))
	if err != nil || !dec.Msg.GetOk() {
		t.Fatalf("decision = %+v, %v", dec, err)
	}
	for _, want := range []string{`"policy":"auto-v1"`, `"tier":"hard"`, `"tech":["go"]`} {
		if !strings.Contains(dec.Msg.GetJson(), want) {
			t.Fatalf("decision json %s lacks %s", dec.Msg.GetJson(), want)
		}
	}

	built, err := client.BuildTiers(ctx, withToken(connect.NewRequest(&dispatchv1.BuildTiersRequest{}), testToken))
	if err != nil || !built.Msg.GetOk() {
		t.Fatalf("build = %+v, %v", built, err)
	}
	var body struct {
		Applied bool                             `json:"applied"`
		Tiers   map[string][]dispatch.BuiltEntry `json:"tiers"`
	}
	if err := json.Unmarshal([]byte(built.Msg.GetJson()), &body); err != nil || body.Applied || len(body.Tiers["easy"]) == 0 {
		t.Fatalf("build body = %+v, %v", body, err)
	}

	if _, err := client.BuildTiers(ctx, withToken(connect.NewRequest(&dispatchv1.BuildTiersRequest{Agent: "nope"}), testToken)); err == nil {
		t.Fatal("an unknown agent must be refused")
	}
	one, err := client.BuildTiers(ctx, withToken(connect.NewRequest(&dispatchv1.BuildTiersRequest{Agent: "codex"}), testToken))
	if err != nil || !one.Msg.GetOk() {
		t.Fatalf("build codex = %+v, %v", one, err)
	}
	body.Tiers = nil
	if err := json.Unmarshal([]byte(one.Msg.GetJson()), &body); err != nil || len(body.Tiers["hard"]) == 0 {
		t.Fatalf("build codex body = %+v, %v", body, err)
	}
	for tier, es := range body.Tiers {
		for _, e := range es {
			if e.Agent != "codex" {
				t.Fatalf("%s holds %+v", tier, e)
			}
		}
	}
}

func TestDispatchClassifyNeedsAClassifier(t *testing.T) {
	core, url := startCore(t)
	core.DBDir = t.TempDir()
	ctx := context.Background()
	client := dispatchv1connect.NewDispatchServiceClient(http.DefaultClient, url)
	if _, err := client.Classify(ctx, withToken(connect.NewRequest(&dispatchv1.ClassifyRequest{Prompt: "fix the footer"}), testToken)); err == nil || !strings.Contains(err.Error(), "auto needs a classifier") {
		t.Fatalf("classify without a classifier: %v", err)
	}
	set := func(key, value string) error {
		_, err := client.Config(ctx, withToken(connect.NewRequest(&dispatchv1.ConfigRequest{Action: "set", Key: key, Values: []string{value}}), testToken))
		return err
	}
	if err := set("classifier", "nope"); err == nil {
		t.Fatal("an unknown classifier must be refused")
	}
	if err := set("classifier", "jev"); err != nil {
		t.Fatal(err)
	}
	got, err := client.Config(ctx, withToken(connect.NewRequest(&dispatchv1.ConfigRequest{Action: "get", Key: "classifier"}), testToken))
	if err != nil || got.Msg.GetJson() != `"jev"` {
		t.Fatalf("get classifier = %+v, %v", got, err)
	}
	// The environment wins, so this never reads or moves an item in the person's Keychain.
	t.Setenv("TYPESAFE_API_KEY", "sk-test-value")
	t.Setenv("OPENAI_API_KEY", "sk-test-value")
	keys, err := client.Config(ctx, withToken(connect.NewRequest(&dispatchv1.ConfigRequest{Action: "get", Key: "classifier_keys"}), testToken))
	if err != nil || keys.Msg.GetJson() != `{"jev":"env","openai":"env"}` {
		t.Fatalf("keys = %+v, %v", keys, err)
	}
	cli, err := client.Config(ctx, withToken(connect.NewRequest(&dispatchv1.ConfigRequest{Action: "get", Key: "classifier_cli"}), testToken))
	if err != nil || !strings.Contains(cli.Msg.GetJson(), `"agent":`) || !strings.Contains(cli.Msg.GetJson(), `"model":`) {
		t.Fatalf("cli = %+v, %v", cli, err)
	}
	if err := set("classifier", "off"); err != nil {
		t.Fatal(err)
	}
	if _, err := client.Config(ctx, withToken(connect.NewRequest(&dispatchv1.ConfigRequest{Action: "key-set", Key: "cli", Values: []string{"x"}}), testToken)); err == nil {
		t.Fatal("a classifier with no key must be refused")
	}
	if _, err := client.Config(ctx, withToken(connect.NewRequest(&dispatchv1.ConfigRequest{Action: "key-set", Key: "jev"}), testToken)); err == nil {
		t.Fatal("key-set with no value must be refused")
	}

	sub := func(action, name, profile string) (string, error) {
		res, err := client.Subagents(ctx, withToken(connect.NewRequest(&dispatchv1.SubagentsRequest{Action: action, Name: name, Profile: profile}), testToken))
		if err != nil {
			return "", err
		}
		return res.Msg.GetJson(), nil
	}
	if _, err := sub("save", "Bad Name", `{"tier":"hard"}`); err == nil {
		t.Fatal("an invalid name must be refused")
	}
	if _, err := sub("save", "reviewer", `{"tier":"impossible"}`); err == nil {
		t.Fatal("an invalid profile must be refused")
	}
	if _, err := sub("save", "reviewer", `{"tier":"hard","readonly":true,"instructions":"Review."}`); err != nil {
		t.Fatal(err)
	}
	if _, err := sub("save", "general", `{"tier":"hard"}`); err == nil {
		t.Fatal("general is reserved")
	}
	list, err := sub("list", "", "")
	if err != nil || !strings.Contains(list, `"instructions":"Review."`) || !strings.Contains(list, `"user":["reviewer"]`) || !strings.Contains(list, `"reviewer":"user"`) || !strings.Contains(list, `"planner":"builtin"`) {
		t.Fatalf("list = %s, %v", list, err)
	}
	if _, err := sub("remove", "reviewer", ""); err != nil {
		t.Fatal(err)
	}
	// Removing the user's copy of a built-in restores the built-in one.
	if list, _ := sub("list", "", ""); strings.Contains(list, `"instructions":"Review."`) || !strings.Contains(list, `"reviewer":"builtin"`) {
		t.Fatalf("after remove = %s", list)
	}
}

func TestClaudeQuotaKeepsWhenClaudeReportedIt(t *testing.T) {
	core, _ := startCore(t)
	ctx := context.Background()
	bin := filepath.Join(t.TempDir(), "claude")
	if err := os.WriteFile(bin, []byte("#!/bin/sh\n"), 0o755); err != nil {
		t.Fatal(err)
	}
	adapters.SetBinaries(map[string]string{"claude": bin})
	t.Cleanup(func() { adapters.SetBinaries(nil) })

	now := time.Now().UTC().Truncate(time.Second)
	reported := now.Add(-2 * time.Hour)
	used := 51.0
	raw, err := json.Marshal([]adapters.QuotaSample{{
		Window: "seven_day", Used: &used, ResetAt: now.Add(48 * time.Hour), Duration: adapters.Span(7 * 24 * time.Hour),
		Source: adapters.SourceClaudeStream, Confidence: adapters.ConfidenceOfficial, SampledAt: reported,
	}})
	if err != nil {
		t.Fatal(err)
	}
	state := t.TempDir()
	if err := os.WriteFile(filepath.Join(state, "claude-rate-limits.json"), raw, 0o600); err != nil {
		t.Fatal(err)
	}
	adapters.SetStateDir(state)
	t.Cleanup(func() { adapters.SetStateDir("") })

	core.probeAgentsAndQuota(ctx, "claude", now)
	snaps, err := core.Store.ListQuota(ctx, "claude", time.Time{})
	if err != nil {
		t.Fatal(err)
	}
	if len(snaps) != 1 || snaps[0].UsedPercent == nil || *snaps[0].UsedPercent != 51 || snaps[0].Window != "seven_day" ||
		snaps[0].Source != adapters.SourceClaudeStream || !snaps[0].SampledAt.Equal(reported) {
		t.Fatalf("the last run's rate limits are Claude's quota, sampled when Claude reported them: %+v", snaps)
	}
}
