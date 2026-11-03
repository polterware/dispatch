package main

import (
	"bufio"
	"context"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"os"
	"os/signal"
	"path/filepath"
	"sort"
	"strings"
	"syscall"
	"time"

	"connectrpc.com/connect"

	"github.com/polterware/dispatch/go-core/domains/dispatch"
	dispatchv1 "github.com/polterware/dispatch/go-core/platform/gen/dispatch/v1"
	"github.com/polterware/dispatch/go-core/providers/adapters"
	"github.com/polterware/dispatch/go-core/providers/hook"
	"github.com/polterware/dispatch/go-core/providers/sandbox"
	"github.com/polterware/dispatch/go-core/providers/setup"
	"github.com/polterware/dispatch/go-core/providers/skill"
)

const dispatchOpsUsage = `       dispatch wait <id> [--timeout <dur>] [--json=false]
       dispatch route [--tier <auto|easy|normal|hard>] [--provider <p>] [--model <m>] [--effort <e>] [--agent <a>] [--priority speed|balanced|quality] [--relax] [--readonly] [--role <name|none>] [--card <slug>] [--dir <repo>] [--json] [<prompt>]
       dispatch models [--json] | refresh | bench [--json]
       dispatch classify [--card <slug>] [--dir <repo>] [--json] <prompt>
       dispatch config get [priority|discover|roles|classifier|classifier_model|classifier_keys|classifier_cli|allow|deny] [--project] [--dir <repo>]
       dispatch config set priority <speed|balanced|quality> | discover <true|false> | roles <true|false> | classifier <off|jev|openai|cli> | classifier_model <model> [--project]
       dispatch config allow|deny add|rm providers|models|agents <value>... [--project]
       dispatch config key set|clear jev|openai   (set reads the key from stdin into a Keychain item only dispatch reads; TYPESAFE_API_KEY or OPENAI_API_KEY wins over it)
       dispatch doctor [--json]
       dispatch merge <id>
       dispatch clean <id> [--force]
       dispatch recover <id>
       dispatch roles [--check] [--dir <repo>]
       dispatch roles save <name> [--when <text>] [--tier easy|normal|hard] [--easy|--normal|--hard agent/model,...] [--readonly] [--instructions <text>] [--manual] [--prefer a,b] [--avoid a,b] [--timeout <dur>] | remove <name>
       dispatch on|off|hook
       dispatch setup [--json] | link | skill [<agent>] | hook on|off | opencode [--remove | on | off]
       dispatch skill install [--agent <name>] [--force] | show | refresh
       dispatch update [--check]`

// dispatchOpResult is the JSON body of a core operation. OK is false when the
// operation ran but reports a failure.
type dispatchOpResult struct {
	OK   bool
	JSON string
}

// dispatchOpsClient is the part of the core's DispatchService used by the
// operational commands.
type dispatchOpsClient interface {
	Route(ctx context.Context, req *dispatchv1.RouteRequest) (dispatchOpResult, error)
	Classify(ctx context.Context, req *dispatchv1.ClassifyRequest) (dispatchOpResult, error)
	ListModelCatalog(ctx context.Context) (dispatchOpResult, error)
	RefreshModelCatalog(ctx context.Context) (dispatchOpResult, error)
	ListModelBenchmarks(ctx context.Context) (dispatchOpResult, error)
	BuildTiers(ctx context.Context, req *dispatchv1.BuildTiersRequest) (dispatchOpResult, error)
	MergeRun(ctx context.Context, req *dispatchv1.MergeRunRequest) (dispatchOpResult, error)
	CleanRun(ctx context.Context, req *dispatchv1.CleanRunRequest) (dispatchOpResult, error)
	RecoverRun(ctx context.Context, id string) (dispatchOpResult, error)
	Config(ctx context.Context, req *dispatchv1.ConfigRequest) (dispatchOpResult, error)
	Subagents(ctx context.Context, req *dispatchv1.SubagentsRequest) (dispatchOpResult, error)
}

func (c *connectDispatchClient) Route(ctx context.Context, req *dispatchv1.RouteRequest) (dispatchOpResult, error) {
	res, err := c.svc.Route(ctx, connect.NewRequest(req))
	if err != nil {
		return dispatchOpResult{}, wrapDispatchError(err)
	}
	return dispatchOpResult{OK: res.Msg.GetOk(), JSON: res.Msg.GetJson()}, nil
}

func (c *connectDispatchClient) Classify(ctx context.Context, req *dispatchv1.ClassifyRequest) (dispatchOpResult, error) {
	res, err := c.svc.Classify(ctx, connect.NewRequest(req))
	if err != nil {
		return dispatchOpResult{}, wrapDispatchError(err)
	}
	return dispatchOpResult{OK: res.Msg.GetOk(), JSON: res.Msg.GetJson()}, nil
}

func (c *connectDispatchClient) ListModelCatalog(ctx context.Context) (dispatchOpResult, error) {
	res, err := c.svc.ListModelCatalog(ctx, connect.NewRequest(&dispatchv1.ListModelCatalogRequest{}))
	if err != nil {
		return dispatchOpResult{}, wrapDispatchError(err)
	}
	return dispatchOpResult{OK: res.Msg.GetOk(), JSON: res.Msg.GetJson()}, nil
}

func (c *connectDispatchClient) RefreshModelCatalog(ctx context.Context) (dispatchOpResult, error) {
	res, err := c.svc.RefreshModelCatalog(ctx, connect.NewRequest(&dispatchv1.RefreshModelCatalogRequest{}))
	if err != nil {
		return dispatchOpResult{}, wrapDispatchError(err)
	}
	return dispatchOpResult{OK: res.Msg.GetOk(), JSON: res.Msg.GetJson()}, nil
}

func (c *connectDispatchClient) ListModelBenchmarks(ctx context.Context) (dispatchOpResult, error) {
	res, err := c.svc.ListModelBenchmarks(ctx, connect.NewRequest(&dispatchv1.ListModelBenchmarksRequest{}))
	if err != nil {
		return dispatchOpResult{}, wrapDispatchError(err)
	}
	return dispatchOpResult{OK: res.Msg.GetOk(), JSON: res.Msg.GetJson()}, nil
}

func (c *connectDispatchClient) BuildTiers(ctx context.Context, req *dispatchv1.BuildTiersRequest) (dispatchOpResult, error) {
	res, err := c.svc.BuildTiers(ctx, connect.NewRequest(req))
	if err != nil {
		return dispatchOpResult{}, wrapDispatchError(err)
	}
	return dispatchOpResult{OK: res.Msg.GetOk(), JSON: res.Msg.GetJson()}, nil
}

func (c *connectDispatchClient) MergeRun(ctx context.Context, req *dispatchv1.MergeRunRequest) (dispatchOpResult, error) {
	res, err := c.svc.MergeRun(ctx, connect.NewRequest(req))
	if err != nil {
		return dispatchOpResult{}, wrapDispatchError(err)
	}
	return dispatchOpResult{OK: res.Msg.GetOk(), JSON: res.Msg.GetJson()}, nil
}

func (c *connectDispatchClient) CleanRun(ctx context.Context, req *dispatchv1.CleanRunRequest) (dispatchOpResult, error) {
	res, err := c.svc.CleanRun(ctx, connect.NewRequest(req))
	if err != nil {
		return dispatchOpResult{}, wrapDispatchError(err)
	}
	return dispatchOpResult{OK: res.Msg.GetOk(), JSON: res.Msg.GetJson()}, nil
}

func (c *connectDispatchClient) RecoverRun(ctx context.Context, id string) (dispatchOpResult, error) {
	res, err := c.svc.RecoverRun(ctx, connect.NewRequest(&dispatchv1.RecoverRunRequest{Id: id}))
	if err != nil {
		return dispatchOpResult{}, wrapDispatchError(err)
	}
	return dispatchOpResult{OK: res.Msg.GetOk(), JSON: res.Msg.GetJson()}, nil
}

func (c *connectDispatchClient) Config(ctx context.Context, req *dispatchv1.ConfigRequest) (dispatchOpResult, error) {
	res, err := c.svc.Config(ctx, connect.NewRequest(req))
	if err != nil {
		return dispatchOpResult{}, wrapDispatchError(err)
	}
	return dispatchOpResult{OK: res.Msg.GetOk(), JSON: res.Msg.GetJson()}, nil
}

func (c *connectDispatchClient) Subagents(ctx context.Context, req *dispatchv1.SubagentsRequest) (dispatchOpResult, error) {
	res, err := c.svc.Subagents(ctx, connect.NewRequest(req))
	if err != nil {
		return dispatchOpResult{}, wrapDispatchError(err)
	}
	return dispatchOpResult{OK: res.Msg.GetOk(), JSON: res.Msg.GetJson()}, nil
}

// dialDispatchOps dials the core for the operational commands.
func dialDispatchOps(dbPath string) (dispatchOpsClient, error) {
	client, _, err := dialDispatch(dbPath, "")
	if err != nil {
		return nil, err
	}
	ops, ok := client.(dispatchOpsClient)
	if !ok {
		return nil, errors.New("dispatch client does not support operational commands")
	}
	return ops, nil
}

// printDispatchOp prints an operation body and turns a reported failure into
// exit status 1 after the body is shown.
func printDispatchOp(res dispatchOpResult) error {
	var v any
	if err := json.Unmarshal([]byte(res.JSON), &v); err != nil {
		return err
	}
	if err := writeJSON(v); err != nil {
		return err
	}
	if !res.OK {
		return exitError{code: 1}
	}
	return nil
}

func dispatchStateDir(dbPath string) string {
	if strings.TrimSpace(dbPath) == "" {
		dbPath = defaultDB()
	}
	return filepath.Join(filepath.Dir(dbPath), "dispatch")
}

func dispatchWD(dir string) (string, error) {
	if strings.TrimSpace(dir) != "" {
		return filepath.Abs(dir)
	}
	return os.Getwd()
}

func singleRunID(flags *flag.FlagSet, args []string, verb string) (string, error) {
	positional, err := parseInterspersed(flags, args)
	if err != nil {
		return "", err
	}
	if len(positional) != 1 || strings.TrimSpace(positional[0]) == "" {
		return "", fmt.Errorf("dispatch %s needs one run id\n%s", verb, dispatchOpsUsage)
	}
	return strings.TrimSpace(positional[0]), nil
}

func isTerminalDispatchRun(status string) bool {
	switch status {
	case dispatch.StatusCompleted, dispatch.StatusFailed, dispatch.StatusCancelled, dispatch.StatusTimedOut, dispatch.StatusOrphaned:
		return true
	}
	return false
}

// dispatchExitCode maps a terminal status to the exit status of wait.
func dispatchExitCode(status string) int {
	switch status {
	case dispatch.StatusCompleted:
		return 0
	case dispatch.StatusTimedOut:
		return 124
	default:
		return 1
	}
}

func runDispatchWait(dbPath string, args []string) error {
	flags := flag.NewFlagSet("dispatch wait", flag.ContinueOnError)
	flags.SetOutput(os.Stderr)
	timeout := flags.Duration("timeout", 0, "stop waiting after this long (0 = no limit)")
	asJSON := flags.Bool("json", true, "print compact summary JSON; --json=false prints text")
	projectID := flags.String("project", "", "workspace id (default: the workspace holding the cwd)")
	id, err := singleRunID(flags, args, "wait")
	if err != nil {
		return err
	}
	client, _, err := dialDispatch(dbPath, *projectID)
	if err != nil {
		return err
	}
	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer stop()
	if *timeout > 0 {
		var cancel context.CancelFunc
		ctx, cancel = context.WithTimeout(ctx, *timeout)
		defer cancel()
	}
	tick := time.NewTicker(500 * time.Millisecond)
	defer tick.Stop()
	for {
		run, err := client.GetRun(ctx, id)
		if err != nil && ctx.Err() == nil {
			return err
		}
		if err == nil && isTerminalDispatchRun(run.Status) {
			if err := writeDispatchRun(run, *asJSON, true); err != nil {
				return err
			}
			if code := dispatchExitCode(run.Status); code != 0 {
				return exitError{code: code}
			}
			return nil
		}
		select {
		case <-ctx.Done():
			// Still running when we stopped waiting.
			if latest, lerr := client.GetRun(context.Background(), id); lerr == nil {
				_ = writeDispatchRun(latest, *asJSON, true)
			}
			return exitError{code: 3}
		case <-tick.C:
		}
	}
}

func runDispatchRoute(dbPath string, args []string) error {
	flags := flag.NewFlagSet("dispatch route", flag.ContinueOnError)
	flags.SetOutput(os.Stderr)
	req := &dispatchv1.RouteRequest{}
	flags.StringVar(&req.Tier, "tier", "", "auto (the default with a classifier, else normal) or a tier: easy|normal|hard")
	flags.StringVar(&req.Provider, "provider", "", "restrict to a provider")
	flags.StringVar(&req.Model, "model", "", "restrict to a model")
	flags.StringVar(&req.Effort, "effort", "", "restrict to an effort: low|medium|high|xhigh")
	flags.StringVar(&req.Agent, "agent", "", "restrict to an agent")
	flags.StringVar(&req.Priority, "priority", "", "speed|balanced|quality")
	flags.BoolVar(&req.Relax, "relax", false, "fall back to tier routing when filters match nothing")
	flags.BoolVar(&req.ReadOnly, "readonly", false, "require read-only capability")
	flags.StringVar(&req.Profile, "role", "", "role to route as; none routes with no role and no pick")
	flags.StringVar(&req.Dir, "dir", "", "repository (default: current directory)")
	flags.StringVar(&req.CardSlug, "card", "", "Work card whose component prices the candidates")
	projectID := flags.String("project", "", "workspace id (default: the workspace holding the cwd)")
	asJSON := flags.Bool("json", false, "print the full report as JSON")
	positional, err := parseInterspersed(flags, args)
	if err != nil {
		return err
	}
	req.Prompt = strings.TrimSpace(strings.Join(positional, " "))
	if req.Dir, err = dispatchWD(req.Dir); err != nil {
		return err
	}
	dc, project, err := dialDispatch(dbPath, *projectID)
	if err != nil {
		return err
	}
	client, ok := dc.(dispatchOpsClient)
	if !ok {
		return errors.New("dispatch client does not support route")
	}
	req.ProjectId = project
	res, err := client.Route(context.Background(), req)
	if err != nil {
		return err
	}
	if *asJSON {
		return printDispatchOp(res)
	}
	var report dispatch.RouteReport
	if err := json.Unmarshal([]byte(res.JSON), &report); err != nil {
		return err
	}
	if report.Winner == nil {
		fmt.Println(report.Reason)
		for _, e := range report.Eliminated {
			fmt.Println("  -", e)
		}
		return exitError{code: 1}
	}
	fmt.Printf("winner: %s/%s effort=%s priority=%s\n", report.Winner.Agent, report.Winner.Model, dispatchOrUnknown(report.Winner.Effort), report.Winner.Priority)
	fmt.Println("reason:", report.Reason)
	if report.Difficulty != nil {
		fmt.Printf("difficulty: d=%.2f (%s) classifier=%s", *report.Difficulty, dispatch.DifficultyTier(*report.Difficulty), report.Classifier)
		if len(report.Probs) == 3 {
			fmt.Printf(" P(easy)=%.2f P(normal)=%.2f P(hard)=%.2f", report.Probs[0], report.Probs[1], report.Probs[2])
		}
		fmt.Println()
	}
	if line := roleLine(report.Role, report.RolePick); line != "" {
		fmt.Println(line)
	}
	for i, c := range report.Candidates {
		if a := c.Auto; a != nil {
			mark := " "
			if a.Qualified {
				mark = "✓"
			}
			fmt.Printf("  %d. %s %s/%s effort=%s P=%.2f bound=%.2f cost=$%.2f q=%.2f → $%.2f  θ=%.2f %s\n", i+1, mark, c.Filters.Agent, c.Filters.Model, dispatchOrUnknown(c.Filters.Effort), a.PPass, a.PPassLow, *a.EffectiveCost/a.Quota, a.Quota, *a.EffectiveCost, a.Theta, a.ThetaSource)
			continue
		}
		fmt.Printf("  %d. %s/%s score=%.2f  %s\n", i+1, c.Filters.Agent, c.Filters.Model, c.Score, c.Reason)
	}
	for _, e := range report.Eliminated {
		fmt.Println("  eliminated:", e)
	}
	if report.Note != "" {
		fmt.Println("note:", report.Note)
	}
	return nil
}

func runDispatchClassify(dbPath string, args []string) error {
	flags := flag.NewFlagSet("dispatch classify", flag.ContinueOnError)
	flags.SetOutput(os.Stderr)
	req := &dispatchv1.ClassifyRequest{}
	flags.StringVar(&req.Dir, "dir", "", "repository whose policy picks the classifier (default: current directory)")
	flags.StringVar(&req.CardSlug, "card", "", "Work card the task names: the classifier reads it before the prompt")
	projectID := flags.String("project", "", "workspace id for --card (default: the workspace holding the cwd)")
	asJSON := flags.Bool("json", false, "print JSON")
	positional, err := parseInterspersed(flags, args)
	if err != nil {
		return err
	}
	req.Prompt = strings.TrimSpace(strings.Join(positional, " "))
	if req.Prompt == "" {
		return errors.New("dispatch classify needs a prompt")
	}
	if req.Dir, err = dispatchWD(req.Dir); err != nil {
		return err
	}
	client, err := dialDispatchOps(dbPath)
	if err != nil {
		return err
	}
	if req.CardSlug != "" {
		_, project, err := dialDispatch(dbPath, *projectID)
		if err != nil {
			return err
		}
		req.ProjectId = project
	}
	res, err := client.Classify(context.Background(), req)
	if err != nil {
		return err
	}
	if *asJSON {
		return printDispatchOp(res)
	}
	var body struct {
		KeyStatus  string `json:"keyStatus"`
		Difficulty *struct {
			Provider   string    `json:"provider"`
			Model      string    `json:"model"`
			Tier       string    `json:"tier"`
			D          float64   `json:"d"`
			Probs      []float64 `json:"probs"`
			Confidence *float64  `json:"confidence"`
			LatencyMs  int64     `json:"latencyMs"`
		} `json:"difficulty"`
		Role *struct {
			Role        string             `json:"role"`
			Probs       map[string]float64 `json:"probs"`
			Unsupported bool               `json:"unsupported"`
		} `json:"role"`
	}
	if err := json.Unmarshal([]byte(res.JSON), &body); err != nil {
		return err
	}
	if d := body.Difficulty; d != nil {
		fmt.Printf("tier=%s d=%.2f classifier=%s", d.Tier, d.D, d.Provider)
		if d.Model != "" {
			fmt.Printf(" model=%s", d.Model)
		}
		fmt.Printf(" latency=%dms\n", d.LatencyMs)
		if len(d.Probs) == 3 {
			fmt.Printf("  P(easy)=%.2f P(normal)=%.2f P(hard)=%.2f\n", d.Probs[0], d.Probs[1], d.Probs[2])
		}
		if d.Confidence != nil {
			fmt.Printf("  confidence=%.2f\n", *d.Confidence)
		}
	}
	if s := body.Role; s != nil {
		if s.Unsupported {
			fmt.Println("role=general (this classifier does not pick one)")
		} else {
			fmt.Println(roleLine("", &dispatch.RolePick{Auto: true, Role: s.Role, Probs: s.Probs}))
		}
	}
	return nil
}

// roleLine names a route's role: the classifier's pick with its
// probabilities, highest first, or the one the request named.
func roleLine(profile string, pick *dispatch.RolePick) string {
	if pick == nil || !pick.Auto {
		if profile == "" {
			return ""
		}
		return "role=" + profile
	}
	line := "role=" + pick.Role + " (picked)"
	names := make([]string, 0, len(pick.Probs))
	for name := range pick.Probs {
		names = append(names, name)
	}
	sort.Slice(names, func(i, j int) bool { return pick.Probs[names[i]] > pick.Probs[names[j]] })
	for _, name := range names {
		line += fmt.Sprintf(" P(%s)=%.2f", name, pick.Probs[name])
	}
	return line
}

// runDispatchModels prints the catalog the auto route prices models with,
// refreshes its prices from models.dev, or prints the installed models' benchmarks.
func runDispatchModels(dbPath string, args []string) error {
	flags := flag.NewFlagSet("models", flag.ContinueOnError)
	flags.SetOutput(os.Stderr)
	asJSON := flags.Bool("json", false, "print JSON")
	positional, err := parseInterspersed(flags, args)
	if err != nil {
		return err
	}
	if len(positional) > 1 || len(positional) == 1 && positional[0] != "refresh" && positional[0] != "bench" {
		return errors.New("usage: dispatch models [--json] | refresh | bench [--json]")
	}
	client, err := dialDispatchOps(dbPath)
	if err != nil {
		return err
	}
	if len(positional) == 1 && positional[0] == "bench" {
		res, err := client.ListModelBenchmarks(context.Background())
		if err != nil {
			return err
		}
		if *asJSON || !res.OK {
			return printDispatchOp(res)
		}
		var report dispatch.BenchmarkReport
		if err := json.Unmarshal([]byte(res.JSON), &report); err != nil {
			return err
		}
		printDispatchBenchmarks(report)
		return nil
	}
	if len(positional) == 1 {
		res, err := client.RefreshModelCatalog(context.Background())
		if err != nil {
			return err
		}
		if *asJSON || !res.OK {
			return printDispatchOp(res)
		}
		var body struct {
			Models int    `json:"models"`
			Source string `json:"source"`
		}
		if err := json.Unmarshal([]byte(res.JSON), &body); err != nil {
			return err
		}
		fmt.Printf("refreshed %d model prices from %s\n", body.Models, body.Source)
		return nil
	}
	res, err := client.ListModelCatalog(context.Background())
	if err != nil {
		return err
	}
	if *asJSON {
		return printDispatchOp(res)
	}
	var report dispatch.CatalogReport
	if err := json.Unmarshal([]byte(res.JSON), &report); err != nil {
		return err
	}
	fmt.Printf("catalog %s: %d models, %d CursorBench scores, %d ECI scores", report.Generated, report.Models, report.Scores, report.ECI)
	if report.PricesFetched != "" {
		fmt.Printf("; prices refreshed %s", report.PricesFetched)
	}
	fmt.Println()
	for _, e := range report.Entries {
		cost := "price unknown"
		if e.CostPerTask != nil {
			cost = fmt.Sprintf("$%.2f/task (%s)", *e.CostPerTask, e.CostSource)
		}
		theta := fmt.Sprintf("θ=%.2f %s", e.Theta, e.ThetaSource)
		if e.Score != nil {
			theta += " " + catalogScore(e.ThetaSource, *e.Score)
		}
		pass := make([]string, 0, len(e.Bands))
		for _, b := range e.Bands {
			pass = append(pass, fmt.Sprintf("%s %.2f≥%.2f", b.Tier, b.PPass, b.Low))
		}
		tier := e.Tier
		if tier == "" {
			tier = "found"
		}
		fmt.Printf("  %-6s %s/%s effort=%s  %s  %s  P(pass): %s\n", tier, e.Agent, e.Model, dispatchOrUnknown(e.Effort), theta, cost, strings.Join(pass, " "))
	}
	for _, s := range report.Sources {
		fmt.Printf("source: %s (%s): %s\n", s.Name, s.License, s.Fields)
	}
	return nil
}

// catalogScore is the benchmark figure θ comes from, as the benchmark reports it.
func catalogScore(source string, score float64) string {
	if strings.HasPrefix(source, "eci") {
		return fmt.Sprintf("(ECI %.0f)", score)
	}
	return fmt.Sprintf("(CursorBench %.0f%%)", score*100)
}

func runDispatchConfig(dbPath string, args []string) error {
	usage := errors.New("usage: dispatch config get [key] | set priority|discover|classifier|classifier_model <value> | allow|deny add|rm providers|models|agents <value>... [--project] | key set|clear jev|openai")
	flags := flag.NewFlagSet("dispatch config", flag.ContinueOnError)
	flags.SetOutput(os.Stderr)
	project := flags.Bool("project", false, "read/write <repo>/.dispatch/config.json")
	dir := flags.String("dir", "", "repository (default: current directory)")
	positional, err := parseInterspersed(flags, args)
	if err != nil {
		return err
	}
	if len(positional) == 0 {
		return usage
	}
	req := &dispatchv1.ConfigRequest{Project: *project}
	if req.Dir, err = dispatchWD(*dir); err != nil {
		return err
	}
	switch positional[0] {
	case "get":
		req.Action = "get"
		if len(positional) > 1 {
			req.Key = positional[1]
		}
	case "set":
		if len(positional) != 3 {
			return usage
		}
		req.Action, req.Key, req.Values = "set", positional[1], positional[2:]
	case "allow", "deny":
		if len(positional) < 4 || (positional[1] != "add" && positional[1] != "rm") {
			return usage
		}
		req.Action, req.Key, req.Values = positional[0]+"-"+positional[1], positional[2], positional[3:]
	case "key":
		// The key never goes in the arguments, where the shell history and ps would keep it.
		if len(positional) != 3 || (positional[1] != "set" && positional[1] != "clear") {
			return usage
		}
		req.Action, req.Key = "key-"+positional[1], positional[2]
		if positional[1] == "set" {
			if info, err := os.Stdin.Stat(); err == nil && info.Mode()&os.ModeCharDevice != 0 {
				fmt.Fprint(os.Stderr, "Paste the key, then press Return: ")
			}
			line, err := bufio.NewReader(os.Stdin).ReadString('\n')
			if err != nil && line == "" {
				return errors.New("no key on stdin")
			}
			req.Values = []string{strings.TrimSpace(line)}
		}
	default:
		return usage
	}
	client, err := dialDispatchOps(dbPath)
	if err != nil {
		return err
	}
	res, err := client.Config(context.Background(), req)
	if err != nil {
		return err
	}
	return printDispatchOp(res)
}

func runDispatchDoctor(dbPath string, args []string) error {
	flags := flag.NewFlagSet("dispatch doctor", flag.ContinueOnError)
	flags.SetOutput(os.Stderr)
	asJSON := flags.Bool("json", false, "print JSON")
	positional, err := parseInterspersed(flags, args)
	if err != nil {
		return err
	}
	if len(positional) != 0 {
		return errors.New(dispatchOpsUsage)
	}
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()
	reports := adapters.Doctor(ctx)

	var tableProblems []string
	if client, _, err := dialDispatch(dbPath, ""); err == nil {
		if rows, err := client.ListTiers(ctx); err != nil {
			tableProblems = []string{err.Error()}
		} else {
			tableProblems = dispatch.TableFromRows(rows).CapabilityProblems(adapters.Known())
		}
	}
	for i := range reports {
		if len(tableProblems) > 0 {
			reports[i].Status = adapters.StatusDegraded
			reports[i].Notes = append(reports[i].Notes, tableProblems...)
		}
	}
	if *asJSON {
		return writeJSON(reports)
	}
	if len(reports) == 0 {
		fmt.Println("no installed agent CLIs")
		return nil
	}
	for _, r := range reports {
		fmt.Printf("%-12s %-14s %s\n", r.Name, dispatchOrUnknown(r.Version), r.Status)
		var have, miss []string
		for _, f := range r.Flags {
			if f.Present {
				have = append(have, f.Name)
			} else {
				miss = append(miss, f.Name)
			}
		}
		if len(have) > 0 {
			fmt.Printf("  flags: %s\n", strings.Join(have, " "))
		}
		if len(miss) > 0 {
			fmt.Printf("  missing: %s\n", strings.Join(miss, " "))
		}
		for _, n := range r.Notes {
			fmt.Printf("  note: %s\n", sandbox.Redact(n))
		}
	}
	return nil
}

func runDispatchMerge(dbPath string, args []string) error {
	flags := flag.NewFlagSet("dispatch merge", flag.ContinueOnError)
	flags.SetOutput(os.Stderr)
	id, err := singleRunID(flags, args, "merge")
	if err != nil {
		return err
	}
	client, err := dialDispatchOps(dbPath)
	if err != nil {
		return err
	}
	res, err := client.MergeRun(context.Background(), &dispatchv1.MergeRunRequest{Id: id})
	if err != nil {
		return err
	}
	return printDispatchOp(res)
}

func runDispatchClean(dbPath string, args []string) error {
	flags := flag.NewFlagSet("dispatch clean", flag.ContinueOnError)
	flags.SetOutput(os.Stderr)
	force := flags.Bool("force", false, "discard even if the branch is not merged")
	id, err := singleRunID(flags, args, "clean")
	if err != nil {
		return err
	}
	client, err := dialDispatchOps(dbPath)
	if err != nil {
		return err
	}
	res, err := client.CleanRun(context.Background(), &dispatchv1.CleanRunRequest{Id: id, Force: *force})
	if err != nil {
		return err
	}
	return printDispatchOp(res)
}

func runDispatchRecover(dbPath string, args []string) error {
	flags := flag.NewFlagSet("dispatch recover", flag.ContinueOnError)
	flags.SetOutput(os.Stderr)
	id, err := singleRunID(flags, args, "recover")
	if err != nil {
		return err
	}
	client, err := dialDispatchOps(dbPath)
	if err != nil {
		return err
	}
	res, err := client.RecoverRun(context.Background(), id)
	if err != nil {
		return err
	}
	return printDispatchOp(res)
}

func runDispatchRoles(dbPath string, args []string) error {
	flags := flag.NewFlagSet("dispatch roles", flag.ContinueOnError)
	flags.SetOutput(os.Stderr)
	check := flags.Bool("check", false, "validate roles against known and installed agents")
	dir := flags.String("dir", "", "repository (default: current directory)")
	tier := flags.String("tier", "", "save: the lowest tier it runs at, easy|normal|hard")
	readOnly := flags.Bool("readonly", false, "save: the run may not change files")
	instructions := flags.String("instructions", "", "save: the prompt put before the task")
	prefer := flags.String("prefer", "", "save: agents to prefer, comma separated")
	avoid := flags.String("avoid", "", "save: agents to avoid, comma separated")
	timeout := flags.String("timeout", "", "save: how long a run may take, e.g. 15m")
	when := flags.String("when", "", "save: the tasks it fits, read by the classifier that picks a role")
	manual := flags.Bool("manual", false, "save: only --role gives it; the classifier never picks it")
	board := map[string]*string{}
	for _, t := range []string{"easy", "normal", "hard"} {
		board[t] = flags.String(t, "", "save: its "+t+" models in order, agent/model, comma separated; empty uses the tier table")
	}
	positional, err := parseInterspersed(flags, args)
	if err != nil {
		return err
	}
	action := "list"
	req := &dispatchv1.SubagentsRequest{}
	switch {
	case len(positional) == 2 && (positional[0] == "save" || positional[0] == "remove") && !*check:
		action, req.Name = positional[0], positional[1]
		if action == "save" {
			var entries []dispatch.RoleEntry
			for _, t := range []string{"easy", "normal", "hard"} {
				got, err := parseRoleModels(t, *board[t])
				if err != nil {
					return err
				}
				entries = append(entries, got...)
			}
			b, err := json.Marshal(dispatch.Profile{
				Tier: *tier, ReadOnly: *readOnly, Instructions: *instructions,
				When: *when, Tiers: entries, Manual: *manual,
				Prefer: splitList(*prefer), Avoid: splitList(*avoid), Timeout: *timeout,
			})
			if err != nil {
				return err
			}
			req.Profile = string(b)
		}
	case len(positional) != 0:
		return errors.New(dispatchOpsUsage)
	case *check:
		action = "check"
	}
	wd, err := dispatchWD(*dir)
	if err != nil {
		return err
	}
	client, err := dialDispatchOps(dbPath)
	if err != nil {
		return err
	}
	req.Action, req.Dir = action, wd
	res, err := client.Subagents(context.Background(), req)
	if err != nil {
		return err
	}
	if action == "check" {
		var body struct {
			Roles    int      `json:"roles"`
			Problems []string `json:"problems"`
		}
		if err := json.Unmarshal([]byte(res.JSON), &body); err != nil {
			return err
		}
		if len(body.Problems) == 0 {
			fmt.Printf("ok: %d role(s)\n", body.Roles)
			return nil
		}
		for _, p := range body.Problems {
			fmt.Println("!", p)
		}
		return exitError{code: 1}
	}
	return printDispatchOp(res)
}

// parseRoleModels reads one tier's agent/model entries, comma separated, in order.
func parseRoleModels(tier, list string) ([]dispatch.RoleEntry, error) {
	var out []dispatch.RoleEntry
	for _, item := range splitList(list) {
		agent, model, ok := strings.Cut(item, "/")
		if !ok || agent == "" || model == "" {
			return nil, fmt.Errorf("--%s: %q is not agent/model", tier, item)
		}
		out = append(out, dispatch.RoleEntry{Tier: tier, Agent: agent, Model: model})
	}
	return out, nil
}

// --- on / off / hook / skill / update (local, no core) ---

func runDispatchOn(dbPath string) error {
	exe, err := os.Executable()
	if err != nil {
		return err
	}
	stateDir := dispatchStateDir(dbPath)
	if err := hook.SetEnabled(stateDir, true); err != nil {
		return err
	}
	settings := hook.SettingsPath("")
	if err := hook.Install(settings, exe); err != nil {
		return err
	}
	fmt.Println("dispatch on: SessionStart hook registered in", settings)
	return nil
}

func runDispatchOff(dbPath string) error {
	if err := hook.SetEnabled(dispatchStateDir(dbPath), false); err != nil {
		return err
	}
	settings := hook.SettingsPath("")
	if err := hook.Uninstall(settings); err != nil {
		return err
	}
	fmt.Println("dispatch off: SessionStart hook removed from", settings)
	return nil
}

// runDispatchHook is the SessionStart hook target. It prints one line of
// context while dispatch is on and nothing otherwise, so a stale hook is
// harmless.
func runDispatchHook(dbPath string) error {
	err := recordDispatchHookStdin()
	if hook.IsEnabled(dispatchStateDir(dbPath)) {
		fmt.Print(hook.ContextBlock())
	}
	return err
}

func runDispatchSkill(args []string) error {
	usage := errors.New("usage: dispatch skill install [--agent <name>] [--force] | show | refresh")
	if len(args) == 0 {
		return usage
	}
	home, err := os.UserHomeDir()
	if err != nil {
		return err
	}
	targets := skill.Targets(home)
	switch args[0] {
	case "show":
		fmt.Print(string(skill.Content))
		return nil
	case "refresh":
		paths, err := skill.Refresh(home)
		for _, p := range paths {
			fmt.Println("refreshed", p)
		}
		if err == nil && len(paths) == 0 {
			fmt.Println("no installed dispatch skill to refresh")
		}
		return err
	case "install":
		flags := flag.NewFlagSet("dispatch skill install", flag.ContinueOnError)
		flags.SetOutput(os.Stderr)
		agent := flags.String("agent", "", "install for one agent (claude|cursor|codex|grok|antigravity); default all")
		force := flags.Bool("force", false, "overwrite an existing skill")
		if err := flags.Parse(args[1:]); err != nil {
			return err
		}
		names := make([]string, 0, len(targets))
		for name := range targets {
			if *agent == "" || *agent == name {
				names = append(names, name)
			}
		}
		if len(names) == 0 {
			return fmt.Errorf("unknown agent %q", *agent)
		}
		sort.Strings(names)
		seen := map[string]bool{}
		for _, name := range names {
			dir := targets[name].Dir
			if seen[dir] {
				continue
			}
			seen[dir] = true
			path, err := skill.Install(dir, *force)
			if err != nil {
				fmt.Fprintf(os.Stderr, "skip %s: %v\n", name, err)
				continue
			}
			fmt.Println("installed", path)
		}
		return nil
	default:
		return usage
	}
}

// runDispatchSetup reports how the agent CLIs reach dispatch, or changes one
// part of it: link points ~/.local/bin/dispatch at this binary, skill installs
// the current skill (for one agent or all), hook switches the Claude Code
// SessionStart hook, and opencode installs or removes the Dispatch plugin in
// OpenCode or switches its delegation. The app's Settings › Agents does the same.
func runDispatchSetup(dbPath string, args []string) error {
	home, err := os.UserHomeDir()
	if err != nil {
		return err
	}
	binary, err := os.Executable()
	if err != nil {
		return err
	}
	if resolved, err := filepath.EvalSymlinks(binary); err == nil {
		binary = resolved
	}
	state := dispatchStateDir(dbPath)
	action := ""
	if len(args) > 0 {
		action = args[0]
	}
	switch action {
	case "", "--json":
	case "link":
		path, err := setup.LinkCLI(home, binary)
		if err != nil {
			return err
		}
		fmt.Println("linked", path, "->", binary)
	case "skill":
		agent := ""
		if len(args) > 1 {
			agent = args[1]
		}
		paths, err := setup.InstallSkill(home, agent)
		for _, p := range paths {
			fmt.Println("installed", p)
		}
		if err != nil {
			return err
		}
	case "hook":
		if len(args) < 2 || (args[1] != "on" && args[1] != "off") {
			return errors.New("usage: dispatch setup hook on|off")
		}
		if err := setup.SetHook(home, binary, state, args[1] == "on"); err != nil {
			return err
		}
		fmt.Println("hook", args[1])
	case "opencode":
		sub := ""
		if len(args) > 1 {
			sub = args[1]
		}
		switch sub {
		case "":
			path, err := setup.InstallOpenCodePlugin(home, setup.OpenCodeBinary(home, binary), state)
			if err != nil {
				return err
			}
			fmt.Println("installed", path, "and loaded it from", setup.OpenCodeConfigPath(home))
		case "--remove":
			if err := setup.RemoveOpenCodePlugin(home, state); err != nil {
				return err
			}
			fmt.Println("removed the Dispatch plugin from", setup.OpenCodeConfigPath(home))
		case "on", "off":
			if err := setup.SetOpenCodeDelegation(state, sub == "on"); err != nil {
				return err
			}
			fmt.Println("OpenCode delegation through dispatch", sub)
		default:
			return errors.New("usage: dispatch setup opencode [--remove | on | off]")
		}
	default:
		return errors.New("usage: dispatch setup [--json] | link | skill [<agent>] | hook on|off | opencode [--remove | on | off]")
	}
	report := setup.Read(home, binary, state)
	if action == "--json" {
		return json.NewEncoder(os.Stdout).Encode(report)
	}
	if action == "" {
		printDispatchSetup(report)
	}
	return nil
}

func printDispatchSetup(r setup.Report) {
	cli := map[string]string{setup.Linked: "linked to this binary", setup.Other: "another copy (" + r.CLI.Target + ")", setup.Missing: "not installed"}
	fmt.Printf("command  %s: %s\n", r.CLI.Path, cli[r.CLI.State])
	for _, s := range r.Skills {
		fmt.Printf("skill    %-12s %s  %s\n", s.Agent, s.State, s.Path)
	}
	hookState := "off"
	if r.Hook.On {
		hookState = "on"
		if !r.Hook.Current {
			hookState += " (runs " + r.Hook.Command + ")"
		}
	}
	fmt.Printf("hook     Claude Code SessionStart: %s\n", hookState)
	plugin := map[string]string{setup.Current: "current", setup.Outdated: "outdated", setup.Missing: "not installed"}[r.OpenCode.State]
	if r.OpenCode.State != setup.Missing {
		if r.OpenCode.On {
			plugin += ", delegation on"
		} else {
			plugin += ", delegation off"
		}
	}
	fmt.Printf("plugin   OpenCode %s  %s\n", plugin, r.OpenCode.Path)
}

// runDispatchUpdate explains where updates come from: dispatch ships inside
// the Polterware Dispatch app, so it has no updater of its own.
func runDispatchUpdate(args []string) error {
	flags := flag.NewFlagSet("dispatch update", flag.ContinueOnError)
	flags.SetOutput(os.Stderr)
	_ = flags.Bool("check", false, "report only")
	if err := flags.Parse(args); err != nil {
		return err
	}
	fmt.Println("dispatch ships inside Dispatch and has no updater of its own.")
	fmt.Println("Update Dispatch (Dispatch › Check for Updates…, or its daily check), then run `dispatch skill refresh`.")
	return nil
}

// printDispatchBenchmarks prints each installed model's figures as the
// benchmarks publish them, then where they come from.
func printDispatchBenchmarks(report dispatch.BenchmarkReport) {
	if len(report.Models) == 0 {
		fmt.Println("no installed agent CLI reported models")
		return
	}
	for _, m := range report.Models {
		var levels []string
		for _, l := range m.CursorBench {
			level := fmt.Sprintf("%.0f%%", l.Score*100)
			if l.Effort != "" {
				level = l.Effort + " " + level
			}
			levels = append(levels, level)
		}
		bench := "unmeasured"
		if len(levels) > 0 {
			bench = strings.Join(levels, ", ")
			if m.CursorBenchAs != "" {
				bench += " (as " + m.CursorBenchAs + ")"
			}
		}
		eci := "unmeasured"
		if m.ECI != nil {
			eci = fmt.Sprintf("%.0f", *m.ECI)
			if m.ECIAs != "" {
				eci += " (as " + m.ECIAs + ")"
			}
		}
		fmt.Printf("%-8s  %-28s  cursorbench=%s  eci=%s\n", m.Agent, m.Model, bench, eci)
	}
	fmt.Printf("catalog %s", report.Generated)
	for _, src := range report.Sources {
		fmt.Printf("; %s %s", src.Name, src.URL)
	}
	fmt.Println()
}

// splitList reads a comma-separated flag; empty is none.
func splitList(s string) []string {
	var out []string
	for _, part := range strings.Split(s, ",") {
		if part = strings.TrimSpace(part); part != "" {
			out = append(out, part)
		}
	}
	return out
}
