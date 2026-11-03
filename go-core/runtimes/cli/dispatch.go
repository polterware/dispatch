package main

import (
	"context"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"io"
	"os"
	"strings"
	"time"

	"golang.org/x/term"

	"github.com/polterware/dispatch/go-core/domains/dispatch"
	dispatchv1 "github.com/polterware/dispatch/go-core/platform/gen/dispatch/v1"
)

const dispatchUsage = `usage: dispatch run [--tier <auto|easy|normal|hard>] [--role <name|none>] [--card <slug>] [--agent <name>] [--provider <p>] [--model <m>] [--effort <e>] [--priority <p>] [--relax] [--readonly] [--timeout <dur>] [--dir <repo>] [--worktree <dir>] [--json] [--compact] <prompt|->
       dispatch ls [--conversation <id>] [--json] [--compact]
       dispatch status <id> [--json] [--compact]
       dispatch result <id> [--section summary|agent|files|attempts|meta|output] [--offset <n>] [--limit <n>] [--json] [--compact]
       dispatch cancel <id> [--json] [--compact]
       dispatch tiers [--check] [--json] [--compact]
       dispatch tiers build [--agent <a>] [--apply] [--json]
       dispatch stats [--json] [--compact]
       dispatch agents [--json] [--compact]
       dispatch agents add <name> -- <command that starts its ACP server...>
       dispatch agents remove <name>
` + dispatchOpsUsage

type dispatchRunSummary struct {
	ConversationID string            `json:"conversation_id,omitempty"`
	CallerAgent    string            `json:"caller_agent,omitempty"`
	ID             string            `json:"id"`
	ProjectID      string            `json:"project_id,omitempty"`
	Status         string            `json:"status"`
	Tier           string            `json:"tier,omitempty"`
	Agent          string            `json:"agent,omitempty"`
	Model          string            `json:"model,omitempty"`
	Provider       string            `json:"provider,omitempty"`
	Effort         string            `json:"effort,omitempty"`
	CardSlug       string            `json:"card_slug,omitempty"`
	AgentRunID     string            `json:"agent_run_id,omitempty"`
	Branch         string            `json:"branch,omitempty"`
	Worktree       string            `json:"worktree,omitempty"`
	Attempts       int               `json:"attempts"`
	ExitClass      string            `json:"exit_class,omitempty"`
	QueuedMs       *int64            `json:"queued_ms,omitempty"`
	RunMs          *int64            `json:"run_ms,omitempty"`
	FilesChanged   *int              `json:"files_changed,omitempty"`
	Insertions     *int              `json:"insertions,omitempty"`
	Deletions      *int              `json:"deletions,omitempty"`
	Usage          *dispatchRunUsage `json:"usage,omitempty"`
	Error          string            `json:"error,omitempty"`
	Note           string            `json:"note,omitempty"`
	Details        map[string]string `json:"details"`
}

// dispatchRunUsage is a run's reported tokens and API-equivalent cost.
type dispatchRunUsage struct {
	InputTokens      int64    `json:"input_tokens"`
	CacheReadTokens  int64    `json:"cache_read_tokens"`
	CacheWriteTokens int64    `json:"cache_write_tokens"`
	OutputTokens     int64    `json:"output_tokens"`
	ReasoningTokens  int64    `json:"reasoning_tokens"`
	CostUSD          *float64 `json:"cost_usd,omitempty"`
	CostSource       string   `json:"cost_source,omitempty"`
	ContextTokens    int64    `json:"context_tokens,omitempty"`
	ContextWindow    int64    `json:"context_window,omitempty"`
}

func runDispatch(dbPath string, args []string) error {
	if len(args) == 0 {
		return errors.New(dispatchUsage)
	}
	sub, rest := args[0], args[1:]
	switch sub {
	case "run":
		return runDispatchRun(dbPath, rest)
	case "ls", "runs":
		return runDispatchList(dbPath, rest)
	case "status":
		return runDispatchStatus(dbPath, rest)
	case "result":
		return runDispatchResult(dbPath, rest)
	case "cancel":
		return runDispatchCancel(dbPath, rest)
	case "tiers":
		return runDispatchTiers(dbPath, rest)
	case "stats":
		return runDispatchStats(dbPath, rest)
	case "agents":
		return runDispatchAgents(dbPath, rest)
	case "wait":
		return runDispatchWait(dbPath, rest)
	case "route":
		return runDispatchRoute(dbPath, rest)
	case "classify":
		return runDispatchClassify(dbPath, rest)
	case "models":
		return runDispatchModels(dbPath, rest)
	case "config":
		return runDispatchConfig(dbPath, rest)
	case "doctor":
		return runDispatchDoctor(dbPath, rest)
	case "merge":
		return runDispatchMerge(dbPath, rest)
	case "clean":
		return runDispatchClean(dbPath, rest)
	case "recover":
		return runDispatchRecover(dbPath, rest)
	case "roles":
		return runDispatchRoles(dbPath, rest)
	case "on":
		return runDispatchOn(dbPath)
	case "off":
		return runDispatchOff(dbPath)
	case "hook":
		return runDispatchHook(dbPath)
	case "skill":
		return runDispatchSkill(rest)
	case "setup":
		return runDispatchSetup(dbPath, rest)
	case "update":
		return runDispatchUpdate(rest)
	default:
		return errors.New(dispatchUsage)
	}
}

func runDispatchRun(dbPath string, args []string) error {
	flags := flag.NewFlagSet("dispatch run", flag.ContinueOnError)
	flags.SetOutput(os.Stderr)
	tier := flags.String("tier", "", "auto (the default with a classifier, else normal) or a tier: easy|normal|hard")
	card := flags.String("card", "", "optional Work card slug")
	agent := flags.String("agent", "", "optional agent pin")
	worktree := flags.String("worktree", "", "optional worktree path")
	provider := flags.String("provider", "", "restrict to a provider")
	model := flags.String("model", "", "restrict to a model")
	effort := flags.String("effort", "", "restrict to an effort: low|medium|high|xhigh")
	priority := flags.String("priority", "", "speed|balanced|quality")
	relax := flags.Bool("relax", false, "fall back to tier routing when filters match nothing")
	readOnly := flags.Bool("readonly", false, "run read-only")
	timeout := flags.Duration("timeout", 0, "per-attempt timeout (0 = default)")
	profile := flags.String("role", "", "role to run as; none runs with no role and no pick")
	dir := flags.String("dir", "", "repository (default: current directory)")
	projectID := flags.String("project", "", "workspace id (default: the workspace holding the cwd)")
	asJSON := flags.Bool("json", false, "print one JSON object")
	compact := flags.Bool("compact", false, "compact output")
	positional, err := parseInterspersed(flags, args)
	if err != nil {
		return err
	}
	if *tier != "" && *tier != string(dispatch.TierAuto) && !validDispatchTier(*tier) {
		return errors.New(dispatchUsage)
	}
	prompt, err := readDispatchPrompt(positional)
	if err != nil {
		return err
	}
	wd, err := dispatchWD(*dir)
	if err != nil {
		return err
	}
	client, project, err := dialDispatch(dbPath, *projectID)
	if err != nil {
		return err
	}
	run, err := client.StartRun(context.Background(), dispatchStartRequest{
		ProjectID:  project,
		Tier:       *tier,
		CardSlug:   strings.TrimSpace(*card),
		Agent:      strings.TrimSpace(*agent),
		Worktree:   strings.TrimSpace(*worktree),
		Dir:        wd,
		ReadOnly:   *readOnly,
		Provider:   strings.TrimSpace(*provider),
		Model:      strings.TrimSpace(*model),
		Effort:     strings.TrimSpace(*effort),
		Priority:   strings.TrimSpace(*priority),
		Relax:      *relax,
		TimeoutSec: int(timeout.Seconds()),
		Profile:    strings.TrimSpace(*profile),
		Prompt:     prompt,
		Caller:     dispatchCaller(dbPath),
	})
	if err != nil {
		return err
	}
	return writeDispatchRun(run, *asJSON, *compact)
}

func runDispatchList(dbPath string, args []string) error {
	flags := flag.NewFlagSet("dispatch runs", flag.ContinueOnError)
	flags.SetOutput(os.Stderr)
	limit := flags.Int("limit", 20, "max runs to list")
	conversation := flags.String("conversation", "", "filter by calling conversation id")
	projectID := flags.String("project", "", "workspace id (default: the workspace holding the cwd)")
	asJSON := flags.Bool("json", false, "print one JSON object")
	compact := flags.Bool("compact", false, "compact output")
	positional, err := parseInterspersed(flags, args)
	if err != nil {
		return err
	}
	if len(positional) != 0 {
		return errors.New(dispatchUsage)
	}
	client, project, err := dialDispatch(dbPath, *projectID)
	if err != nil {
		return err
	}
	conversationID := strings.TrimSpace(*conversation)
	// A conversation lists its runs in every workspace unless --project narrows it.
	if conversationID != "" && strings.TrimSpace(*projectID) == "" {
		project = ""
	}
	runs, err := client.ListRuns(context.Background(), dispatchListRunsRequest{
		ProjectID:      project,
		Limit:          *limit,
		ConversationID: conversationID,
	})
	if err != nil {
		return err
	}
	if *asJSON {
		return writeDispatchJSON(map[string]any{"runs": runs}, *compact)
	}
	if len(runs) == 0 {
		fmt.Println("no dispatch runs")
		return nil
	}
	for _, r := range runs {
		fmt.Printf("%s  %-10s  %-6s  %s/%s  %s%s\n",
			shortID(r.ID), r.Status, dispatchOrUnknown(r.Tier),
			dispatchOrUnknown(r.Agent), dispatchOrUnknown(r.Model),
			oneLine(r.Prompt), conversationSuffix(r.ConversationID))
	}
	return nil
}

func runDispatchStatus(dbPath string, args []string) error {
	flags := flag.NewFlagSet("dispatch status", flag.ContinueOnError)
	flags.SetOutput(os.Stderr)
	projectID := flags.String("project", "", "workspace id (default: the workspace holding the cwd)")
	asJSON := flags.Bool("json", false, "print one JSON object")
	compact := flags.Bool("compact", false, "compact output")
	positional, err := parseInterspersed(flags, args)
	if err != nil {
		return err
	}
	if len(positional) != 1 || strings.TrimSpace(positional[0]) == "" {
		return errors.New(dispatchUsage)
	}
	client, _, err := dialDispatch(dbPath, *projectID)
	if err != nil {
		return err
	}
	run, err := client.GetRun(context.Background(), strings.TrimSpace(positional[0]))
	if err != nil {
		return err
	}
	return writeDispatchRun(run, *asJSON, *compact)
}

func runDispatchResult(dbPath string, args []string) error {
	flags := flag.NewFlagSet("dispatch result", flag.ContinueOnError)
	flags.SetOutput(os.Stderr)
	section := flags.String("section", "summary", "summary|agent|files|attempts|meta|output")
	offset := flags.Int("offset", 0, "first line offset")
	limit := flags.Int("limit", 200, "max lines (0 = all)")
	projectID := flags.String("project", "", "workspace id (default: the workspace holding the cwd)")
	asJSON := flags.Bool("json", false, "print one JSON object")
	compact := flags.Bool("compact", false, "compact output")
	positional, err := parseInterspersed(flags, args)
	if err != nil {
		return err
	}
	if len(positional) != 1 || strings.TrimSpace(positional[0]) == "" || !validDispatchSection(*section) {
		return errors.New(dispatchUsage)
	}
	client, _, err := dialDispatch(dbPath, *projectID)
	if err != nil {
		return err
	}
	res, err := client.Result(context.Background(), dispatchResultRequest{
		RunID:   strings.TrimSpace(positional[0]),
		Section: *section,
		Offset:  *offset,
		Limit:   *limit,
	})
	if err != nil {
		return err
	}
	return writeDispatchResult(res, *asJSON, *compact)
}

func runDispatchCancel(dbPath string, args []string) error {
	flags := flag.NewFlagSet("dispatch cancel", flag.ContinueOnError)
	flags.SetOutput(os.Stderr)
	projectID := flags.String("project", "", "workspace id (default: the workspace holding the cwd)")
	asJSON := flags.Bool("json", false, "print one JSON object")
	compact := flags.Bool("compact", false, "compact output")
	positional, err := parseInterspersed(flags, args)
	if err != nil {
		return err
	}
	if len(positional) != 1 || strings.TrimSpace(positional[0]) == "" {
		return errors.New(dispatchUsage)
	}
	client, _, err := dialDispatch(dbPath, *projectID)
	if err != nil {
		return err
	}
	run, err := client.CancelRun(context.Background(), strings.TrimSpace(positional[0]))
	if err != nil {
		return err
	}
	return writeDispatchRun(run, *asJSON, *compact)
}

func runDispatchTiers(dbPath string, args []string) error {
	flags := flag.NewFlagSet("dispatch tiers", flag.ContinueOnError)
	flags.SetOutput(os.Stderr)
	check := flags.Bool("check", false, "validate tiers against installed agents and models")
	apply := flags.Bool("apply", false, "with build: replace the stored tiers with the proposal")
	agent := flags.String("agent", "", "with build: only this agent's entries and models; --apply replaces only its entries")
	projectID := flags.String("project", "", "workspace id (default: the workspace holding the cwd)")
	asJSON := flags.Bool("json", false, "print one JSON object")
	compact := flags.Bool("compact", false, "compact output")
	positional, err := parseInterspersed(flags, args)
	if err != nil {
		return err
	}
	if len(positional) == 1 && positional[0] == "build" {
		return runDispatchTiersBuild(dbPath, *projectID, &dispatchv1.BuildTiersRequest{Apply: *apply, Agent: *agent}, *asJSON)
	}
	if len(positional) != 0 {
		return errors.New(dispatchUsage)
	}
	client, _, err := dialDispatch(dbPath, *projectID)
	if err != nil {
		return err
	}
	ctx := context.Background()
	if *check {
		res, err := client.CheckTiers(ctx)
		if err != nil {
			return err
		}
		if *asJSON {
			return writeDispatchJSON(res, *compact)
		}
		if len(res.Problems) == 0 {
			fmt.Println("ok: every tier entry resolves to an installed agent and model")
			return nil
		}
		for _, p := range res.Problems {
			fmt.Println("!", p)
		}
		return exitError{code: 1}
	}
	tiers, err := client.ListTiers(ctx)
	if err != nil {
		return err
	}
	if *asJSON {
		return writeDispatchJSON(map[string]any{"tiers": tiers}, *compact)
	}
	if len(tiers) == 0 {
		fmt.Println("no dispatch tiers")
		return nil
	}
	for _, t := range tiers {
		fmt.Printf("%-6s  %-12s  %-24s  provider=%s  speed=%d  strength=%d\n",
			t.Name, t.Agent, t.Model, dispatchOrUnknown(t.Provider), t.Speed, t.Strength)
	}
	return nil
}

func runDispatchStats(dbPath string, args []string) error {
	flags := flag.NewFlagSet("dispatch stats", flag.ContinueOnError)
	flags.SetOutput(os.Stderr)
	by := flags.String("by", "model", "group by: model|agent|provider|tier|effort")
	since := flags.String("since", "7d", "window such as 7d, 24h, or all")
	projectID := flags.String("project", "", "workspace id (default: the workspace holding the cwd)")
	asJSON := flags.Bool("json", false, "print one JSON object")
	compact := flags.Bool("compact", false, "compact output")
	positional, err := parseInterspersed(flags, args)
	if err != nil {
		return err
	}
	if len(positional) != 0 {
		return errors.New(dispatchUsage)
	}
	client, project, err := dialDispatch(dbPath, *projectID)
	if err != nil {
		return err
	}
	res, err := client.Stats(context.Background(), dispatchStatsRequest{
		ProjectID: project,
		By:        *by,
		Since:     *since,
	})
	if err != nil {
		return err
	}
	if *asJSON {
		return writeDispatchJSON(res, *compact)
	}
	if len(res.Rows) == 0 {
		fmt.Println("no dispatch runs in window")
		return nil
	}
	for _, row := range res.Rows {
		fmt.Printf("%-24s  runs=%d  ok=%.0f%%  p50=%s  fallbacks=%d  %s\n",
			row.Key, row.Runs, row.SuccessPct, formatDispatchMs(row.P50Ms), row.Fallbacks, formatDispatchSpend(row))
	}
	return nil
}

// runDispatchTiersBuild prints the tier table the model catalog proposes
// and, with apply, stores it.
func runDispatchTiersBuild(dbPath, projectID string, req *dispatchv1.BuildTiersRequest, asJSON bool) error {
	client, _, err := dialDispatch(dbPath, projectID)
	if err != nil {
		return err
	}
	ops, ok := client.(dispatchOpsClient)
	if !ok {
		return errors.New("dispatch client does not support tiers build")
	}
	res, err := ops.BuildTiers(context.Background(), req)
	if err != nil {
		return err
	}
	if asJSON {
		return printDispatchOp(res)
	}
	var body struct {
		Priority string                           `json:"priority"`
		Applied  bool                             `json:"applied"`
		Problems []string                         `json:"problems"`
		Tiers    map[string][]dispatch.BuiltEntry `json:"tiers"`
	}
	if err := json.Unmarshal([]byte(res.JSON), &body); err != nil {
		return err
	}
	for _, tier := range []string{dispatch.TierEasy, dispatch.TierNormal, dispatch.TierHard} {
		fmt.Printf("%s (d=%.2f):\n", tier, firstDifficulty(body.Tiers[tier]))
		for i, e := range body.Tiers[tier] {
			mark := " "
			if e.Auto.Qualified {
				mark = "✓"
			}
			model := e.Agent + "/" + e.Model
			if e.Effort != "" {
				model += "@" + e.Effort
			}
			fmt.Printf("  %d. %s %s P=%.2f bound=%.2f cost=$%.2f  θ=%.2f %s\n", i+1, mark, model, e.Auto.PPass, e.Auto.PPassLow, *e.Auto.EffectiveCost, e.Auto.Theta, e.Auto.ThetaSource)
		}
	}
	for _, p := range body.Problems {
		fmt.Fprintln(os.Stderr, "models:", p)
	}
	switch {
	case body.Applied && req.Agent != "":
		fmt.Printf("stored as %s's entries, after the other agents' in each tier\n", req.Agent)
	case body.Applied:
		fmt.Println("stored as the tier table")
	case req.Agent != "":
		fmt.Printf("priority %s; run `dispatch tiers build --agent %s --apply` to store it as %s's entries\n", body.Priority, req.Agent, req.Agent)
	default:
		fmt.Printf("priority %s; run `dispatch tiers build --apply` to store it\n", body.Priority)
	}
	return nil
}

func firstDifficulty(es []dispatch.BuiltEntry) float64 {
	if len(es) == 0 {
		return 0
	}
	return es[0].Auto.Difficulty
}

func runDispatchAgents(dbPath string, args []string) error {
	if len(args) > 0 && (args[0] == "add" || args[0] == "remove") {
		return runDispatchAgentsEdit(dbPath, args)
	}
	flags := flag.NewFlagSet("dispatch agents", flag.ContinueOnError)
	flags.SetOutput(os.Stderr)
	projectID := flags.String("project", "", "workspace id (default: the workspace holding the cwd)")
	asJSON := flags.Bool("json", false, "print one JSON object")
	compact := flags.Bool("compact", false, "compact output")
	positional, err := parseInterspersed(flags, args)
	if err != nil {
		return err
	}
	if len(positional) != 0 {
		return errors.New(dispatchUsage)
	}
	client, _, err := dialDispatch(dbPath, *projectID)
	if err != nil {
		return err
	}
	agents, err := client.ListAgents(context.Background())
	if err != nil {
		return err
	}
	if *asJSON {
		return writeDispatchJSON(map[string]any{"agents": agents}, *compact)
	}
	if len(agents) == 0 {
		fmt.Println("no dispatch agents")
		return nil
	}
	for _, a := range agents {
		used := "unknown"
		if a.UsedPercent != nil {
			used = fmt.Sprintf("%.0f%%", *a.UsedPercent)
		}
		fmt.Printf("%-12s  %-10s  used=%s\n", a.Name, dispatchOrUnknown(a.Provider), used)
	}
	return nil
}

// dialDispatch dials the core and resolves the workspace: the one with
// workspaceID, else the one holding the cwd (created on first use).
func dialDispatch(dbPath, workspaceID string) (dispatchClient, string, error) {
	c, err := dial(dbPath)
	if err != nil {
		return nil, "", err
	}
	ws, err := resolveWorkspace(c, workspaceID, "")
	if err != nil {
		if workspaceID != "" {
			return nil, "", err
		}
		return newDispatchClient(c), "", nil
	}
	return newDispatchClient(c), ws.GetId(), nil
}

func validDispatchTier(tier string) bool {
	return tier == dispatch.TierEasy || tier == dispatch.TierNormal || tier == dispatch.TierHard
}

func validDispatchSection(section string) bool {
	switch section {
	case "summary", "agent", "files", "attempts", "meta", "output":
		return true
	default:
		return false
	}
}

func readDispatchPrompt(positional []string) (string, error) {
	prompt := strings.TrimSpace(strings.Join(positional, " "))
	if prompt == "" || prompt == "-" {
		if len(positional) == 0 && term.IsTerminal(int(os.Stdin.Fd())) {
			return "", errors.New(dispatchUsage)
		}
		body, err := io.ReadAll(os.Stdin)
		if err != nil {
			return "", err
		}
		prompt = strings.TrimSpace(string(body))
	}
	if prompt == "" {
		return "", errors.New(dispatchUsage)
	}
	return prompt, nil
}

func summarizeDispatchRun(run dispatch.Run) dispatchRunSummary {
	attempts := len(run.Attempts)
	if run.Agent != "" && run.Status != dispatch.StatusFailed && run.Status != dispatch.StatusPending {
		attempts++
	}
	details := map[string]string{
		"summary":  fmt.Sprintf("dispatch result %s --section summary", run.ID),
		"files":    fmt.Sprintf("dispatch result %s --section files", run.ID),
		"agent":    fmt.Sprintf("dispatch result %s --section agent", run.ID),
		"attempts": fmt.Sprintf("dispatch result %s --section attempts", run.ID),
		"meta":     fmt.Sprintf("dispatch result %s --section meta", run.ID),
	}
	return dispatchRunSummary{
		ConversationID: run.ConversationID, CallerAgent: run.CallerAgent,
		ID:           run.ID,
		ProjectID:    run.ProjectID,
		Status:       run.Status,
		Tier:         run.Tier,
		Agent:        run.Agent,
		Model:        run.Model,
		Provider:     run.Provider,
		Effort:       run.Effort,
		CardSlug:     run.CardSlug,
		AgentRunID:   run.AgentRunID,
		Branch:       run.Branch,
		Worktree:     run.Worktree,
		Attempts:     attempts,
		ExitClass:    run.ExitClass,
		QueuedMs:     run.QueuedMs,
		RunMs:        run.RunMs,
		FilesChanged: run.FilesChanged,
		Insertions:   run.Insertions,
		Deletions:    run.Deletions,
		Usage:        summarizeDispatchUsage(run.Usage),
		Error:        run.Error,
		Note:         run.Note,
		Details:      details,
	}
}

func summarizeDispatchUsage(u *dispatch.Usage) *dispatchRunUsage {
	if u == nil {
		return nil
	}
	return &dispatchRunUsage{
		InputTokens: u.InputTokens, CacheReadTokens: u.CacheReadTokens, CacheWriteTokens: u.CacheWriteTokens,
		OutputTokens: u.OutputTokens, ReasoningTokens: u.ReasoningTokens, CostUSD: u.CostUSD, CostSource: u.CostSource,
		ContextTokens: u.ContextTokens, ContextWindow: u.ContextWindow,
	}
}

func (s dispatchRunSummary) Text() string {
	line1 := fmt.Sprintf("%s %s/%s tier=%s %s %s %s attempts=%d",
		s.Status, dispatchOrUnknown(s.Agent), dispatchOrUnknown(s.Model), dispatchOrUnknown(s.Tier),
		formatDispatchMs(s.RunMs), formatDispatchFiles(s.FilesChanged, s.Insertions, s.Deletions), formatDispatchUsage(s.Usage), s.Attempts)
	line2 := fmt.Sprintf("branch=%s worktree=%s", dispatchOrUnknown(s.Branch), dispatchOrUnknown(s.Worktree)) + conversationSuffix(s.ConversationID)
	if s.CardSlug != "" {
		line2 += " card=" + s.CardSlug
	}
	if s.Error != "" {
		line2 += " error=" + s.Error
	}
	if s.Note != "" {
		line2 += " note=" + s.Note
	}
	line3 := fmt.Sprintf("details: dispatch result %s --section summary|files|agent|attempts|meta", s.ID)
	return line1 + "\n" + line2 + "\n" + line3 + "\n"
}

func writeDispatchRun(run dispatch.Run, asJSON, compact bool) error {
	sum := summarizeDispatchRun(run)
	if asJSON {
		return writeDispatchJSON(sum, compact)
	}
	fmt.Print(sum.Text())
	return nil
}

func writeDispatchResult(res dispatchResultResponse, asJSON, compact bool) error {
	switch res.Section {
	case "summary":
		return writeDispatchRun(res.Run, asJSON, compact)
	case "files":
		payload := map[string]any{
			"id":            res.Run.ID,
			"status":        res.Run.Status,
			"section":       "files",
			"files_changed": res.Run.FilesChanged,
			"insertions":    res.Run.Insertions,
			"deletions":     res.Run.Deletions,
		}
		if asJSON {
			return writeDispatchJSON(payload, compact)
		}
		fmt.Printf("%s %s\n", res.Run.ID, formatDispatchFiles(res.Run.FilesChanged, res.Run.Insertions, res.Run.Deletions))
		return nil
	case "attempts":
		payload := map[string]any{
			"id":       res.Run.ID,
			"status":   res.Run.Status,
			"section":  "attempts",
			"attempts": res.Run.Attempts,
		}
		if asJSON {
			return writeDispatchJSON(payload, compact)
		}
		if len(res.Run.Attempts) == 0 {
			fmt.Println("no fallback attempts")
			return nil
		}
		for _, a := range res.Run.Attempts {
			fmt.Printf("%s/%s class=%s duration=%s %s\n",
				a.Agent, a.Model, dispatchOrUnknown(a.ExitClass), formatDispatchMs(a.DurationMs), a.Error)
		}
		return nil
	case "meta":
		r := res.Run
		r.Output = ""
		if asJSON {
			return writeDispatchJSON(r, compact)
		}
		fmt.Print(summarizeDispatchRun(r).Text())
		return nil
	default:
		payload := map[string]any{
			"id":          res.Run.ID,
			"status":      res.Run.Status,
			"section":     res.Section,
			"offset":      res.Offset,
			"limit":       res.Limit,
			"total_lines": res.TotalLines,
			"has_more":    res.HasMore,
			"content":     res.Content,
		}
		if asJSON {
			return writeDispatchJSON(payload, compact)
		}
		if res.Content != "" {
			fmt.Print(res.Content)
			if !strings.HasSuffix(res.Content, "\n") {
				fmt.Println()
			}
		}
		return nil
	}
}

func writeDispatchJSON(v any, compact bool) error {
	if !compact {
		return writeJSON(v)
	}
	return json.NewEncoder(os.Stdout).Encode(v)
}

func dispatchOrUnknown(s string) string {
	if strings.TrimSpace(s) == "" {
		return "unknown"
	}
	return s
}

func formatDispatchMs(ms *int64) string {
	if ms == nil {
		return "unknown"
	}
	if *ms < 1000 {
		return fmt.Sprintf("%dms", *ms)
	}
	return (time.Duration(*ms) * time.Millisecond).Round(100 * time.Millisecond).String()
}

// formatDispatchTokens writes a token count as 950, 12k, or 6.5M.
func formatDispatchTokens(n int64) string {
	switch {
	case n >= 1_000_000:
		return fmt.Sprintf("%.1fM", float64(n)/1e6)
	case n >= 10_000:
		return fmt.Sprintf("%dk", n/1000)
	case n >= 1000:
		return fmt.Sprintf("%.1fk", float64(n)/1e3)
	default:
		return fmt.Sprintf("%d", n)
	}
}

func formatDispatchUsage(u *dispatchRunUsage) string {
	if u == nil {
		return "tokens=unknown"
	}
	total := u.InputTokens + u.CacheReadTokens + u.CacheWriteTokens + u.OutputTokens
	cost := "unknown"
	if u.CostUSD != nil {
		cost = fmt.Sprintf("$%.2f", *u.CostUSD)
	}
	out := fmt.Sprintf("tokens=%s cost=%s", formatDispatchTokens(total), cost)
	if c := formatDispatchContext(u.ContextTokens, u.ContextWindow); c != "" {
		out += " context=" + c
	}
	return out
}

// formatDispatchContext is what the last model call held, out of its window: 45k/272k(17%).
func formatDispatchContext(tokens, window int64) string {
	switch {
	case tokens <= 0:
		return ""
	case window <= 0:
		return formatDispatchTokens(tokens)
	}
	return fmt.Sprintf("%s/%s(%d%%)", formatDispatchTokens(tokens), formatDispatchTokens(window), min(100, tokens*100/window))
}

// formatDispatchSpend is a stats row's tokens and the cost of its priced runs.
func formatDispatchSpend(row dispatchStatsRow) string {
	if row.UsageRuns == 0 {
		return "tokens=unknown"
	}
	total := row.InputTokens + row.CacheReadTokens + row.CacheWriteTokens + row.OutputTokens
	out := fmt.Sprintf("tokens=%s", formatDispatchTokens(total))
	if row.CostRuns > 0 {
		out += fmt.Sprintf(" cost=$%.2f", row.CostUSD)
		if row.CostRuns < row.Runs {
			out += fmt.Sprintf(" (%d/%d runs priced)", row.CostRuns, row.Runs)
		}
	}
	return out
}

func formatDispatchFiles(files, ins, del *int) string {
	if files == nil {
		return "files=unknown"
	}
	if ins == nil || del == nil {
		return fmt.Sprintf("files=%d", *files)
	}
	return fmt.Sprintf("files=%d +%d -%d", *files, *ins, *del)
}

// runDispatchAgentsEdit adds an ACP agent by name and the command that
// starts its server, or removes one. Everything after "--" is the command,
// flags included, so it is read before any flag parsing.
func runDispatchAgentsEdit(dbPath string, args []string) error {
	usage := errors.New("usage: dispatch agents add <name> -- <command...> | dispatch agents remove <name>")
	req := &dispatchv1.ConfigRequest{}
	switch {
	case args[0] == "add" && len(args) >= 4 && args[2] == "--":
		req.Action, req.Key, req.Values = "agent-add", args[1], args[3:]
	case args[0] == "remove" && len(args) == 2:
		req.Action, req.Key = "agent-remove", args[1]
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

func conversationSuffix(id string) string {
	if id == "" {
		return ""
	}
	agent, session, found := strings.Cut(id, ":")
	if found {
		id = agent + ":" + shortID(session)
	} else {
		id = shortID(id)
	}
	return " conversation=" + id
}
