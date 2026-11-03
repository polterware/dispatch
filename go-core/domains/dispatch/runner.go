package dispatch

import (
	"context"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"sync"
	"time"
	"unicode/utf8"

	adapter "github.com/polterware/dispatch/go-core/providers/adapters"
	"github.com/polterware/dispatch/go-core/providers/sandbox"
	"github.com/polterware/dispatch/go-core/providers/worktree"
)

const (
	depthEnv       = "DISPATCH_DEPTH"
	noDelegateEnv  = "DISPATCH_NO_DELEGATE"
	runIDEnv       = "DISPATCH_RUN_ID"
	parentRunIDEnv = "DISPATCH_PARENT_RUN_ID"
)

var ErrRecursive = errors.New("refusing recursive delegation (DISPATCH_DEPTH set)")

// rateLimited matches provider errors that mean "stop sending work here".
var rateLimited = RateLimitRe

const cooldownFor = time.Hour

// creditCooldownFor is longer: a model outside the plan's allowance stays
// out until the user buys credits or the allowance resets.
const creditCooldownFor = 12 * time.Hour

// creditKey scopes a credit cooldown to one provider inside one CLI (e.g.
// "cursor:openai"): Cursor stops serving third-party models while its own
// and Grok's keep working, and every effort variant of the model is out.
func creditKey(agent, provider string) string { return agent + ":" + provider }

// cooling reports whether the agent, or this provider inside it, is cooling
// down.
func cooling(cool map[string]time.Time, agent, provider string) (string, bool) {
	if _, ok := cool[agent]; ok {
		return agent, true
	}
	if provider != "" {
		if _, ok := cool[creditKey(agent, provider)]; ok {
			return creditKey(agent, provider), true
		}
	}
	return "", false
}

// creditScope is the provider, or the model when the provider is unknown.
func creditScope(agent, provider, model string) string {
	if provider == "" {
		provider = ProviderOf(agent, model)
	}
	if provider == "" {
		return model
	}
	return provider
}

func candidateProvider(c Candidate) string {
	return creditScope(c.Adapter.Name(), c.Provider, c.Model)
}

type stateDirProvider interface {
	StateDir() string
}

type Deps struct {
	Adapters []adapter.Adapter
	Table    Table
	Store    Store
	Now      func() time.Time
	// Installed reports whether a binary is available; defaults to exec.LookPath.
	Installed func(binary string) bool
	// WorktreeDir overrides where write-run worktrees are created. When
	// empty they go in <repo>/.dispatch/worktrees, which keeps them under
	// the repo path that agent permission rules usually already cover.
	WorktreeDir string
	// StateDir is the directory for file-backed sidecars (the catalog prices and live logs).
	StateDir string
	// Policy is the effective global/project routing policy.
	Policy Policy
	// Classifier judges a task's difficulty for Auto; nil leaves Auto off.
	Classifier Classifier
	// Profiles are the roles for the run's directory: built-in, user, and
	// project. The classifier picks among them, and Execute reads a run's
	// role's board here.
	Profiles map[string]Profile
}

// Options describes a request. For resumes, Resume names the earlier run.
type Options struct {
	Caller     Caller
	ProjectID  string
	AgentRunID string
	CardSlug   string
	Filters    Filters
	Tier       adapter.Tier
	Prompt     string
	Dir        string
	ReadOnly   bool
	Timeout    time.Duration
	NoWorktree bool
	Resume     string
	// Role: its name for the record, soft agent preferences, its own board,
	// and its floor tier. NoProfile (`--role none`) keeps the classifier from
	// picking one.
	Profile       string
	Prefer, Avoid []string
	RoleBoard     Table
	TierFloor     adapter.Tier
	NoProfile     bool
	// Area is the part of the project the task touches, resolved by the caller.
	Area Area
	// CardText is the text of the card the run names, read by the caller,
	// which the classifier judges before the prompt.
	CardText string
	// difficulty is an Auto run's classification, so Execute prices the
	// candidates on the same difficulty Begin recorded.
	difficulty *Difficulty
	// pick is how the classifier chose the run's role, nil when it did not.
	pick *RolePick
}

func (d Deps) stateDir() string {
	if d.StateDir != "" {
		return d.StateDir
	}
	if p, ok := d.Store.(stateDirProvider); ok {
		if dir := p.StateDir(); dir != "" {
			return dir
		}
	}
	return os.Getenv("DISPATCH_HOME")
}

func storeCooldowns(ctx context.Context, st Store, now time.Time) map[string]time.Time {
	out := map[string]time.Time{}
	if st == nil {
		return out
	}
	agents, err := st.ListAgents(ctx)
	if err != nil {
		return out
	}
	for _, a := range agents {
		if a.Name != "" && a.CooldownUntil != nil && a.CooldownUntil.After(now) {
			out[a.Name] = *a.CooldownUntil
		}
	}
	return out
}

func storeSnapshots(ctx context.Context, st Store, key string) []Snapshot {
	if st == nil || key == "" {
		return nil
	}
	snaps, err := st.ListQuota(ctx, key, time.Time{})
	if err != nil {
		return nil
	}
	return SnapshotsFromQuota(snaps)
}

// IsInstalled reports whether bin is available to run.
func (d Deps) IsInstalled(bin string) bool { return d.installed(bin) }

func (d Deps) installed(bin string) bool {
	if d.Installed != nil {
		return d.Installed(bin)
	}
	_, err := exec.LookPath(bin)
	return err == nil
}

func (d Deps) adapter(name string) adapter.Adapter {
	for _, a := range d.Adapters {
		if a.Name() == name {
			return a
		}
	}
	return nil
}

// Candidates resolves the tier table against installed adapters, attaches
// quota, pace, history and cooldowns, and ranks them. Table order is kept
// among equals.
func (d Deps) Candidates(ctx context.Context, tier adapter.Tier, needReadOnly bool, prefer, avoid []string) ([]Candidate, []string) {
	cs, rejected := d.selectCandidates(Options{Tier: tier, ReadOnly: needReadOnly, Prefer: prefer, Avoid: avoid})
	return d.evaluate(ctx, cs, true), rejected
}

func (d Deps) evaluate(ctx context.Context, cs []Candidate, live bool) []Candidate {
	return Rank(d.observe(ctx, cs, live))
}

// plan is the one candidate list behind both Route (live=false) and Execute
// (live=true): filtered candidates first, then, with --relax and no policy
// objection, tier-routed ones that are not already listed. Quota is observed
// once for the union; each group is ranked on its own so a relaxed candidate
// never outranks a filtered one.
func (d Deps) plan(ctx context.Context, o Options, live bool) (ordered, balanced []Candidate, rejected []string, relaxed bool) {
	return d.planWithCooldowns(ctx, o, live, storeCooldowns(ctx, d.Store, d.now()))
}

func (d Deps) planWithCooldowns(ctx context.Context, o Options, live bool, cool map[string]time.Time) (ordered, balanced []Candidate, rejected []string, relaxed bool) {
	if o.Tier == TierAuto && o.difficulty == nil {
		o.Tier = adapter.TierNormal // an Auto run with no recorded difficulty
	}
	d, o = d.withRoleBoard(o)
	primary, rejected := d.selectCandidatesCool(ctx, o, cool)
	var extra []Candidate
	if o.Filters.Relax && policyRequestReason(d.Policy, o.Filters) == "" {
		fb := o
		fb.Filters = Filters{Priority: o.Filters.Priority}
		if fb.Tier == "" {
			fb.Tier = adapter.TierNormal
		}
		found, why := d.selectCandidatesCool(ctx, fb, cool)
		for _, c := range found {
			dup := false
			for _, p := range primary {
				if p.Adapter.Name() == c.Adapter.Name() && p.Model == c.Model && p.Effort == c.Effort {
					dup = true
				}
			}
			if !dup {
				c.Note = joinNote(c.Note, "--relax: filters relaxed to tier routing")
				extra = append(extra, c)
			}
		}
		if len(primary) == 0 {
			rejected = append(rejected, why...)
		}
	}
	all := d.observeWithCooldowns(ctx, append(append([]Candidate{}, primary...), extra...), live, cool)
	if o.Tier == TierAuto {
		diff := o.difficulty.D
		cat := LoadCatalog(d.stateDir())
		return rankAuto(all, diff, o.Filters.Priority, cat), rankAuto(all, diff, PriorityBalanced, cat), rejected, false
	}
	order := Rank
	if tierRoute(o) {
		order = OrderTier
	}
	n := len(primary)
	ordered = append(order(all[:n]), order(all[n:])...)
	sh := shiftPriority(all, PriorityBalanced)
	balanced = append(order(sh[:n]), order(sh[n:])...)
	return ordered, balanced, rejected, len(extra) > 0
}

// withRoleBoard routes a run whose role has models in the run's band over
// those alone, in board order, as a tier route of that band: they replace the
// tier table and discovery, and the band still prices them. A band the role
// leaves empty, and a model or provider filter, keep the general route.
func (d Deps) withRoleBoard(o Options) (Deps, Options) {
	if len(o.RoleBoard.Tiers) == 0 || o.Filters.Model != "" || o.Filters.Provider != "" {
		return d, o
	}
	band := o.Tier
	if band == TierAuto {
		band = DifficultyTier(o.difficulty.D)
	}
	if !band.Valid() {
		band = adapter.TierNormal
	}
	entries := o.RoleBoard.Tiers[band]
	if len(entries) == 0 {
		return d, o
	}
	d.Table = Table{Tiers: map[adapter.Tier][]Entry{band: append([]Entry(nil), entries...)}}
	d.Policy.Discover = false
	o.Tier = band
	return d, o
}

// tierRoute is a route that names only a tier: no model, agent, or provider
// filter. It follows the table order instead of ranking by reserve.
func tierRoute(o Options) bool {
	return o.Tier != "" && o.Tier != TierAuto && o.Filters.Model == "" && o.Filters.Agent == "" && o.Filters.Provider == ""
}

func (d Deps) observe(ctx context.Context, cs []Candidate, live bool) []Candidate {
	return d.observeWithCooldowns(ctx, cs, live, storeCooldowns(ctx, d.Store, d.now()))
}

func (d Deps) observeWithCooldowns(ctx context.Context, cs []Candidate, live bool, cool map[string]time.Time) []Candidate {
	if len(cs) == 0 {
		return cs
	}
	// One quota fetch per agent, in parallel; ModelQuoter adapters resolve
	// per model, others share one quota across their models.
	byAgent := map[string][]int{}
	for i, c := range cs {
		byAgent[c.Adapter.Name()] = append(byAgent[c.Adapter.Name()], i)
	}
	var wg sync.WaitGroup
	for _, idxs := range byAgent {
		if !live {
			continue
		}
		wg.Add(1)
		go func(idxs []int) {
			defer wg.Done()
			qctx, cancel := context.WithTimeout(ctx, 100*time.Second)
			defer cancel()
			a := cs[idxs[0]].Adapter
			if mq, ok := a.(adapter.ModelQuoter); ok {
				models := make([]string, len(idxs))
				for k, i := range idxs {
					models[k] = cs[i].Model
				}
				if qs, err := mq.ModelQuotas(qctx, models); err == nil {
					for _, i := range idxs {
						if q, ok := qs[cs[i].Model]; ok {
							q := q
							cs[i].Quota = &q
						}
					}
				}
				return
			}
			if q, err := a.Quota(qctx); err == nil {
				if q.Key == "" {
					q.Key = a.Name()
				}
				for _, i := range idxs {
					q := q
					cs[i].Quota = &q
				}
			}
		}(idxs)
	}
	wg.Wait()

	now := d.now()
	recorded := map[string]bool{}
	snaps := map[string][]Snapshot{}
	snapshots := func(key string) []Snapshot {
		if s, ok := snaps[key]; ok {
			return s
		}
		s := storeSnapshots(ctx, d.Store, key)
		snaps[key] = s
		return s
	}
	for i := range cs {
		c := &cs[i]
		_, c.CooledDown = cooling(cool, c.Adapter.Name(), candidateProvider(*c))
		if !live {
			key := c.Adapter.Name()
			if key == "antigravity" {
				key += ":Claude and GPT models"
				if strings.Contains(strings.ToLower(c.Model), "gemini") {
					key = "antigravity:Gemini Models"
				}
			}
			for _, snap := range snapshots(key) {
				if snap.ResetsAt.After(now) {
					c.Quota = &adapter.Quota{Key: key, UsedPercent: snap.UsedPercent, ResetsAt: snap.ResetsAt, WindowMinutes: snap.WindowMinutes}
				}
			}
		}
		if c.Quota == nil {
			continue
		}
		Evaluate(c, now)
		key := c.Quota.Key
		if live && !recorded[key] && d.Store != nil {
			recorded[key] = true
			used := c.Quota.UsedPercent
			src := ""
			switch c.Adapter.Name() {
			case "codex":
				src = adapter.SourceCodexAppServer
			case "antigravity":
				src = adapter.SourceAgyUsage
			case "claude":
				src = adapter.SourceClaudeStream
			}
			at := now
			if !c.Quota.SampledAt.IsZero() {
				at = c.Quota.SampledAt
			}
			_ = d.Store.SaveQuota(ctx, QuotaSnapshot{
				Agent:         key,
				Provider:      c.Provider,
				Source:        src,
				Confidence:    string(adapter.ConfidenceOfficial),
				UsedPercent:   &used,
				WindowMinutes: c.Quota.WindowMinutes,
				ResetsAt:      c.Quota.ResetsAt,
				SampledAt:     at,
			})
		}
		if c.Quota.WindowMinutes == 7*24*60 {
			h := Historical(snapshots(key), c.Quota.UsedPercent, c.Quota.ResetsAt, now)
			if h.Valid {
				c.Hist = &h
			}
		}
	}
	d.attachEvidence(ctx, cs)
	return cs
}

// attachEvidence copies tier-segmented measurements onto candidates once for
// the whole candidate slice. A read error leaves the static prior in place.
func (d Deps) attachEvidence(ctx context.Context, cs []Candidate) {
	if d.Store == nil || len(cs) == 0 {
		return
	}
	recs, err := d.Store.ListRuns(ctx, "", 0, 0)
	if err != nil || len(recs) == 0 {
		return
	}
	ev := EvidenceByTierModel(recs)
	for i := range cs {
		if e, ok := ev[EvidenceKey(cs[i].Tier, cs[i].Model)]; ok {
			cs[i].Evidence = e
		}
	}
}

// block probes the CLI when it can. DEGRADED still runs and returns a note
// via the record. A missing flag for a capability this run requires is refused.
func (d Deps) block(ctx context.Context, rec *Run, c Candidate, resume bool) string {
	p, ok := c.Adapter.(adapter.Prober)
	if !ok {
		return ""
	}
	report := p.Probe(ctx)
	if report.Status != adapter.StatusDegraded {
		return ""
	}
	if rec.ReadOnly {
		if miss := missingFlags(report, "readOnly"); len(miss) > 0 {
			return fmt.Sprintf("%s does not support readOnly (missing %s)", c.Adapter.Name(), strings.Join(miss, ", "))
		}
	} else if miss := missingFlags(report, "fileEditing"); len(miss) > 0 {
		return fmt.Sprintf("%s does not support fileEditing (missing %s)", c.Adapter.Name(), strings.Join(miss, ", "))
	}
	if c.Model != "" {
		for _, f := range report.Flags {
			if !f.Present && (f.Name == "--model" || f.Name == "-m") {
				return "model selection unavailable (missing " + f.Name + ")"
			}
		}
	}
	if c.Effort != "" && c.Adapter.Capabilities().EffortMode == "config" {
		for _, f := range report.Flags {
			if !f.Present && f.Name == "-c" {
				return "effort configuration unavailable (missing -c)"
			}
		}
	}
	if resume {
		if miss := missingFlags(report, "sessionResume"); len(miss) > 0 {
			return fmt.Sprintf("%s does not support session resume (missing %s)", c.Adapter.Name(), strings.Join(miss, ", "))
		}
	}
	detail := strings.Join(report.Notes, "; ")
	if detail == "" {
		detail = "capability flags missing"
	}
	rec.Note = joinNote(rec.Note, c.Adapter.Name()+" probe DEGRADED: "+detail)
	return ""
}

func missingFlags(r adapter.ProbeReport, capability string) []string {
	var out []string
	for _, f := range r.Flags {
		if !f.Present && f.Capability == capability {
			out = append(out, f.Name)
		}
	}
	return out
}

// Begin validates the request and persists a PENDING record.
func (d Deps) Begin(o Options) (Run, error) {
	if os.Getenv(depthEnv) != "" || os.Getenv(noDelegateEnv) != "" {
		return Run{}, ErrRecursive
	}
	if err := normalizePriority(&o); err != nil {
		return Run{}, err
	}
	routeNote := ""
	if o.Resume == "" {
		note, err := d.resolveAuto(context.Background(), &o)
		if err != nil {
			return Run{}, err
		}
		routeNote = note
	}
	id, err := NewID()
	if err != nil {
		return Run{}, err
	}
	reqFilters := o.Filters
	rec := Run{
		ConversationID: o.Caller.ConversationID, CallerAgent: o.Caller.Agent,
		PID: os.Getpid(), ProcStart: processStart(context.Background(), os.Getpid()),
		ID: id, ProjectID: o.ProjectID, AgentRunID: o.AgentRunID, CardSlug: o.CardSlug, Area: o.Area,
		Status: StatusPending, Tier: string(o.Tier),
		Prompt: o.Prompt, Dir: o.Dir, ReadOnly: o.ReadOnly, CreatedAt: d.now(),
		TimeoutSec: int(o.Timeout / time.Second), NoWorktree: o.NoWorktree,
		Profile: o.Profile, Prefer: o.Prefer, Avoid: o.Avoid, RolePick: o.pick,
		Requested:   &reqFilters,
		ParentRunID: os.Getenv(runIDEnv),
		Note:        routeNote,
	}
	if o.Resume != "" {
		prev, err := d.Store.GetRun(context.Background(), o.Resume)
		if err != nil {
			return rec, fmt.Errorf("resume %s: %w", o.Resume, err)
		}
		if prev.SessionID == "" || prev.Agent == "" {
			return rec, fmt.Errorf("run %s has no resumable session", prev.ID)
		}
		rec.ResumedOf, rec.Tier = prev.ID, prev.Tier
		if rec.ProjectID == "" {
			rec.ProjectID = prev.ProjectID
		}
		rec.Agent, rec.Model, rec.SessionID = prev.Agent, prev.Model, prev.SessionID
		rec.Repo, rec.Worktree, rec.Branch = prev.Repo, prev.Worktree, prev.Branch
		rec.BaseSHA = prev.BaseSHA
		rec.Provider, rec.Effort, rec.Resolved = prev.Provider, prev.Effort, prev.Resolved
		if o.Filters.Agent != "" && o.Filters.Agent != prev.Agent || o.Filters.Model != "" && !matchesModel(prev.Model, o.Filters.Model) || o.Filters.Provider != "" && o.Filters.Provider != ProviderOf(prev.Agent, prev.Model) || o.Filters.Effort != "" && o.Filters.Effort != prev.Effort {
			return rec, fmt.Errorf("resume filters conflict with existing session")
		}
		if rec.Dir == "" {
			rec.Dir = prev.Dir
		}
	} else if !adapter.Tier(rec.Tier).Valid() && rec.Tier != string(TierAuto) && !(rec.Tier == "" && (o.Filters.Model != "" || o.Filters.Provider != "")) {
		return rec, fmt.Errorf("invalid tier %q (want auto|easy|normal|hard)", rec.Tier)
	}
	if o.Resume != "" {
		o.Tier = adapter.Tier(rec.Tier)
	}
	if rec.Dir == "" {
		// An absolute dir lets the live viewer match runs to their repository.
		if wd, err := os.Getwd(); err == nil {
			rec.Dir = wd
		}
	} else if abs, err := filepath.Abs(rec.Dir); err == nil {
		rec.Dir = abs
	}
	if o.Resume == "" {
		recordDifficulty(&rec, o)
	}
	o.Tier = adapter.Tier(rec.Tier)
	if err := validateOptions(o); err != nil {
		return rec, err
	}
	if d.Store == nil {
		return rec, nil
	}
	return rec, d.Store.SaveRun(context.Background(), rec)
}

// Run is Begin + Execute.
func (d Deps) Run(ctx context.Context, o Options) (Run, error) {
	if err := d.Reconcile(ctx); err != nil {
		return Run{}, err
	}
	rec, err := d.Begin(o)
	if err != nil {
		return rec, err
	}
	return d.Execute(ctx, rec)
}

// Execute runs a record created by Begin and returns its final state.
func (d Deps) Execute(ctx context.Context, rec Run) (Run, error) {
	if os.Getenv(depthEnv) != "" || os.Getenv(noDelegateEnv) != "" {
		return rec, ErrRecursive
	}
	if terminalStatus(rec.Status) {
		return rec, fmt.Errorf("run %s already %s", rec.ID, rec.Status)
	}
	rec.PID, rec.ProcStart, rec.StartedAt = os.Getpid(), processStart(ctx, os.Getpid()), d.now()
	if !rec.CreatedAt.IsZero() {
		rec.QueuedMs = I64(nonNegMs(rec.StartedAt.Sub(rec.CreatedAt)))
	}
	if d.Store != nil {
		if err := d.Store.SaveRun(ctx, rec); err != nil {
			return rec, err
		}
	}
	if err := d.Reconcile(ctx); err != nil {
		return rec, err
	}

	cool := storeCooldowns(ctx, d.Store, d.now())
	cands, resumeSession, failMsg := d.planCandidates(ctx, rec, cool)
	if failMsg != "" {
		return d.finishRun(ctx, rec, time.Time{}, StatusFailed, failMsg)
	}
	if note := autoNote(cands); note != "" {
		rec.Note = joinNote(rec.Note, note)
	}
	priced := cands
	if cands[0].Auto == nil && rec.ResumedOf == "" {
		priced = d.priceTierRoute(rec, cands)
		if note := tierWarning(priced); note != "" {
			rec.Note = joinNote(rec.Note, note)
		}
	}
	d.recordRouteDecision(ctx, rec, priced)
	dir, wtErr := d.prepareWorktree(ctx, &rec)
	if wtErr != "" {
		return d.finishRun(ctx, rec, time.Time{}, StatusFailed, wtErr)
	}

	timeout := time.Duration(rec.TimeoutSec) * time.Second
	childEnv := childEnviron(rec.ID)
	var agentStart time.Time // start of the attempt that ends the run
	var skipped []string

	for _, c := range cands {
		if key, ok := cooling(cool, c.Adapter.Name(), candidateProvider(c)); ok {
			skipped = append(skipped, key+" in cooldown")
			continue
		}
		if msg := d.block(ctx, &rec, c, resumeSession != ""); msg != "" {
			skipped = append(skipped, msg)
			continue
		}
		attemptStart := d.now()
		agentStart = attemptStart
		res, runErr, saveErr := d.runAttempt(ctx, &rec, c, dir, resumeSession, timeout, childEnv)
		if saveErr != nil {
			return rec, saveErr
		}
		switch {
		case errors.Is(runErr, adapter.ErrProcessesAlive):
			return d.finishRun(ctx, rec, agentStart, StatusFailed, runErr.Error())
		case runErr == nil:
			d.postRun(ctx, &rec, res.Output, agentStart)
			return d.finishRun(ctx, rec, agentStart, StatusCompleted, "")
		case ctx.Err() != nil:
			return d.finishRun(ctx, rec, agentStart, StatusCancelled, ctx.Err().Error())
		case errors.Is(runErr, context.DeadlineExceeded):
			return d.finishRun(ctx, rec, agentStart, StatusTimedOut, "timeout after "+timeout.String())
		}
		if d.recordAttemptFailure(ctx, &rec, c, runErr, attemptStart) {
			cool = storeCooldowns(ctx, d.Store, d.now())
		}
	}
	if len(rec.Attempts) == 0 {
		msg := strings.Join(skipped, "; ")
		if msg == "" {
			msg = "no candidate could run"
		}
		return d.finishRun(ctx, rec, agentStart, StatusFailed, msg)
	}
	last := rec.Attempts[len(rec.Attempts)-1]
	return d.finishRun(ctx, rec, agentStart, StatusFailed, fmt.Sprintf("all %d candidates failed; last: %s/%s: %s", len(rec.Attempts), last.Agent, last.Model, last.Error))
}

// childEnviron is the agent environment: recursion guards plus run ids.
func childEnviron(runID string) []string {
	extra := []string{
		depthEnv + "=1",
		runIDEnv + "=" + runID,
		noDelegateEnv + "=1",
	}
	if parent := os.Getenv(runIDEnv); parent != "" {
		extra = append(extra, parentRunIDEnv+"="+parent)
	}
	return sandbox.Environ(extra...)
}

// planCandidates returns the ordered candidates, the session to resume, and a
// failure message when nothing can run. cool is the Execute cooldown snapshot.
func (d Deps) planCandidates(ctx context.Context, rec Run, cool map[string]time.Time) ([]Candidate, string, string) {
	if rec.ResumedOf != "" {
		a := d.adapter(rec.Agent)
		if a == nil || !d.installed(a.Binary()) {
			return nil, "", "agent " + rec.Agent + " is not available to resume"
		}
		if !a.Capabilities().SessionResume {
			return nil, "", a.Name() + " does not support session resume"
		}
		cands := []Candidate{{Adapter: a, Model: rec.Model, Provider: rec.Provider, Effort: rec.Effort}}
		return cands, rec.SessionID, ""
	}
	o := Options{Tier: adapter.Tier(rec.Tier), ReadOnly: rec.ReadOnly, Prefer: rec.Prefer, Avoid: rec.Avoid, Profile: rec.Profile}
	if p, ok := d.Profiles[rec.Profile]; ok && rec.Profile != "" {
		o.RoleBoard = p.Board()
	}
	if rec.Requested != nil {
		o.Filters = *rec.Requested
	}
	o.Area = rec.Area
	if rec.RequestedTier == string(TierAuto) {
		o.Tier, o.Prompt, o.Dir, o.difficulty = TierAuto, rec.Prompt, rec.Dir, runDifficulty(rec)
	}
	if o.Filters.Priority == "" {
		o.Filters.Priority = PriorityBalanced
	}
	cands, _, rejected, _ := d.planWithCooldowns(ctx, o, true, cool)
	if len(cands) == 0 {
		if len(rejected) > 0 {
			return nil, "", strings.Join(rejected, "; ")
		}
		return nil, "", "no installed agent serves tier " + rec.Tier
	}
	return cands, "", ""
}

// prepareWorktree resolves the agent directory and, for writable runs in a git
// checkout, cuts the run worktree. A non-empty message fails the run.
func (d Deps) prepareWorktree(ctx context.Context, rec *Run) (string, string) {
	dir := rec.Dir
	if dir == "" {
		dir, _ = os.Getwd()
	}
	dir, _ = filepath.Abs(dir)
	if rec.ResumedOf != "" {
		if rec.Worktree != "" {
			if _, err := os.Stat(rec.Worktree); err == nil {
				if root := worktree.Root(rec.Worktree); root != "" {
					dir = rec.Worktree
				}
			} else {
				rec.Note = "previous worktree is gone; resumed in " + dir
			}
		}
		return dir, ""
	}
	if rec.ReadOnly || rec.NoWorktree {
		return dir, ""
	}
	root := worktree.Root(dir)
	if root == "" || !worktree.HasCommits(root) {
		return dir, ""
	}
	base := d.WorktreeDir
	if base == "" {
		base = filepath.Join(root, ".dispatch", "worktrees")
		worktree.Exclude(root, ".dispatch/")
	}
	path := filepath.Join(base, rec.ID)
	branch, err := worktree.Create(root, path, rec.ID)
	if err != nil {
		return dir, "worktree: " + err.Error()
	}
	rec.Repo, rec.Worktree, rec.Branch = root, path, branch
	rec.BaseSHA, err = worktree.Head(path)
	if err != nil {
		return dir, err.Error()
	}
	if d.Store != nil {
		_ = d.Store.SaveWorktree(ctx, WorktreeInfo{
			ID:        rec.ID,
			ProjectID: rec.ProjectID,
			RunID:     rec.ID,
			CardSlug:  rec.CardSlug,
			Path:      path,
			Branch:    branch,
			BaseSHA:   rec.BaseSHA,
			Status:    WorktreeActive,
		})
	}
	dir = filepath.Join(path, worktree.Rel(root, dir))
	if worktree.Dirty(root) {
		rec.Note = joinNote(rec.Note, "checkout has uncommitted changes; the worktree was cut from HEAD and does not include them")
	}
	return dir, ""
}

// runAttempt records the chosen candidate on rec and runs it. The second error
// is a store failure that aborts Execute without finishing the run.
func (d Deps) runAttempt(ctx context.Context, rec *Run, c Candidate, dir, resumeSession string, timeout time.Duration, childEnv []string) (adapter.Result, error, error) {
	rec.AgentPID = 0
	rec.Status, rec.Agent, rec.Model = StatusRunning, c.Adapter.Name(), c.Model
	rec.ExecutionTier = c.Tier
	if rec.ExecutionTier == "" && adapter.Tier(rec.Tier).Valid() {
		rec.ExecutionTier = rec.Tier
	}
	rec.Provider = c.Provider
	if rec.Provider == "" {
		rec.Provider = ProviderOf(rec.Agent, rec.Model)
	}
	rec.Effort = c.Effort
	if rec.Effort == "" {
		rec.Effort = EffortOf(rec.Model)
	}
	prio := c.Priority
	if prio == "" && rec.Requested != nil {
		prio = rec.Requested.Priority
	}
	if prio == "" {
		prio = PriorityBalanced
	}
	rec.Resolved = &Filters{Provider: rec.Provider, Model: rec.Model, Agent: rec.Agent, Effort: rec.Effort, Priority: prio}
	rec.Note = joinNote(rec.Note, c.Note)
	if d.Store != nil {
		if err := d.Store.SaveRun(ctx, *rec); err != nil {
			return adapter.Result{}, nil, err
		}
	}
	req := adapter.Request{
		Effort: rec.Effort, Prompt: rec.Prompt, Tier: adapter.Tier(rec.Tier), Dir: dir, ReadOnly: rec.ReadOnly,
		Started: func(pid int) error {
			rec.AgentPID = pid
			if d.Store == nil {
				return nil
			}
			return d.Store.SaveRun(ctx, *rec)
		},
		Timeout: timeout, ResumeSession: resumeSession, Env: childEnv,
		LogPath: d.agentLogPath(rec.ID),
	}
	markAttempt(req.LogPath, rec.Attempts, c)
	var res adapter.Result
	var err error
	if resumeSession != "" {
		if r, ok := c.Adapter.(adapter.Resumer); ok {
			res, err = r.Resume(ctx, req, c.Model)
		} else {
			res, err = c.Adapter.Run(ctx, req, c.Model)
		}
	} else {
		res, err = c.Adapter.Run(ctx, req, c.Model)
	}
	rec.ExitCode, rec.SessionID = res.ExitCode, res.SessionID
	rec.addUsage(c.Model, res.Usage, LoadCatalog(d.stateDir()))
	return res, err, nil
}

// postRun stores a successful attempt's output and commits the worktree.
func (d Deps) postRun(ctx context.Context, rec *Run, output string, agentStart time.Time) {
	rec.Output = output
	rec.RunMs = I64(nonNegMs(d.now().Sub(agentStart)))
	if rec.Worktree == "" || rec.ReadOnly {
		return
	}
	if ok, cerr := worktree.Commit(rec.Worktree, rec.ID); cerr != nil {
		rec.Note = joinNote(rec.Note, "commit failed: "+cerr.Error())
	} else if ok {
		rec.Note = joinNote(rec.Note, "changes committed on "+rec.Branch)
	} else {
		rec.Note = joinNote(rec.Note, "no file changes")
	}
}

// recordAttemptFailure sets cooldowns for quota errors and appends the failed
// attempt. It reports whether a cooldown was written.
func (d Deps) recordAttemptFailure(ctx context.Context, rec *Run, c Candidate, err error, attemptStart time.Time) bool {
	now := d.now()
	cls := classOf(err.Error())
	cooled := false
	if d.Store != nil {
		switch cls {
		case "credits":
			_ = d.Store.SetCooldown(ctx, creditKey(c.Adapter.Name(), candidateProvider(c)), now.Add(creditCooldownFor))
			cooled = true
		case "rate_limit":
			_ = d.Store.SetCooldown(ctx, c.Adapter.Name(), now.Add(cooldownFor))
			cooled = true
		}
	}
	rec.Attempts = append(rec.Attempts, Attempt{
		Agent: c.Adapter.Name(), Model: c.Model, Error: err.Error(),
		DurationMs: I64(nonNegMs(now.Sub(attemptStart))),
		ExitClass:  cls,
	})
	return cooled
}

// finishRun redacts and persists a terminal run.
func (d Deps) finishRun(ctx context.Context, rec Run, agentStart time.Time, status, msg string) (Run, error) {
	now := d.now()
	rec.Status, rec.Error, rec.FinishedAt = status, msg, now
	if rec.RunMs == nil && !agentStart.IsZero() {
		rec.RunMs = I64(nonNegMs(now.Sub(agentStart)))
	}
	completeTelemetry(&rec, rec.FinishedAt)
	rec.Output = sandbox.Redact(rec.Output)
	rec.Error = sandbox.Redact(rec.Error)
	rec.Note = sandbox.Redact(rec.Note)
	for i := range rec.Attempts {
		rec.Attempts[i].Error = sandbox.Redact(rec.Attempts[i].Error)
	}
	var err error
	if d.Store != nil {
		err = d.Store.SaveRun(ctx, rec)
	}
	return rec, err
}

// markAttempt writes a separator into the live log when a run falls back,
// so a viewer does not read the previous agent's error as the current one.
func markAttempt(path string, prev []Attempt, c Candidate) {
	if path == "" || len(prev) == 0 {
		return
	}
	last := prev[len(prev)-1]
	class := last.ExitClass
	if class == "" {
		class = "failed"
	}
	line := fmt.Sprintf("\n── dispatch: %s/%s failed (%s); trying %s/%s ──\n",
		last.Agent, last.Model, class, c.Adapter.Name(), c.Model)
	if os.MkdirAll(filepath.Dir(path), 0o700) != nil {
		return
	}
	f, err := os.OpenFile(path, os.O_CREATE|os.O_WRONLY|os.O_APPEND, 0o600)
	if err != nil {
		return
	}
	_, _ = f.WriteString(line)
	_ = f.Close()
}

func (d Deps) agentLogPath(id string) string {
	if dir := d.stateDir(); dir != "" {
		return filepath.Join(dir, "logs", id+".agent.log")
	}
	return AgentLogPath(d.Store, id)
}

// AgentLogPath is where the live, redacted copy of an agent's output goes.
func AgentLogPath(st Store, id string) string {
	dir := ""
	if p, ok := st.(stateDirProvider); ok {
		dir = p.StateDir()
	}
	if dir == "" {
		dir = os.Getenv("DISPATCH_HOME")
	}
	if dir == "" {
		return ""
	}
	return filepath.Join(dir, "logs", id+".agent.log")
}

// ExitClass maps a terminal status.
func ExitClass(status string) string {
	switch status {
	case StatusCompleted:
		return "ok"
	case StatusTimedOut:
		return "timeout"
	case StatusCancelled:
		return "cancelled"
	case StatusFailed:
		return "failed"
	default:
		return ""
	}
}

func nonNegMs(d time.Duration) int64 {
	ms := d.Milliseconds()
	if ms < 0 {
		return 0
	}
	return ms
}

// completeTelemetry fills provider, effort, diff stats and exit class.
// Nil numeric fields stay nil when the measurement is unknown.
func completeTelemetry(rec *Run, now time.Time) {
	if rec.RunMs == nil && !rec.StartedAt.IsZero() && !now.IsZero() && terminalStatus(rec.Status) {
		rec.RunMs = I64(nonNegMs(now.Sub(rec.StartedAt)))
	}
	if rec.Provider == "" {
		rec.Provider = ProviderOf(rec.Agent, rec.Model)
	}
	if rec.Effort == "" {
		rec.Effort = EffortOf(rec.Model)
	}
	if rec.FilesChanged == nil && rec.Worktree != "" && rec.BaseSHA != "" {
		fillDiff(rec)
	}
	if terminalStatus(rec.Status) {
		if cls := Classify(*rec); cls != "" {
			rec.ExitClass = cls
		}
	}
}

func fillDiff(rec *Run) {
	files, ins, del, linesKnown, err := worktree.NumStat(rec.Worktree, rec.BaseSHA)
	if err != nil {
		return
	}
	rec.FilesChanged = I(files)
	if linesKnown {
		rec.Insertions = I(ins)
		rec.Deletions = I(del)
	}
}

func joinNote(a, b string) string {
	if a == "" {
		return b
	}
	if b == "" || strings.Contains(a, b) {
		return a
	}
	return a + "; " + b
}

func terminalStatus(s string) bool {
	return s == StatusOrphaned || s == StatusCompleted || s == StatusFailed || s == StatusCancelled || s == StatusTimedOut
}

// Wait polls until the run reaches a terminal status or ctx ends.
func (d Deps) Wait(ctx context.Context, id string, poll time.Duration) (Run, error) {
	for {
		if err := d.Reconcile(ctx); err != nil {
			rec, _ := d.Store.GetRun(ctx, id)
			return rec, err
		}
		rec, err := d.Store.GetRun(ctx, id)
		if err != nil {
			return rec, err
		}
		if terminalStatus(rec.Status) {
			return rec, nil
		}
		select {
		case <-ctx.Done():
			return rec, ctx.Err()
		case <-time.After(poll):
		}
	}
}

// Summary returns a compact head of output: at most maxLines lines and
// maxBytes bytes (cut on a rune boundary), and whether it was truncated.
func Summary(output string, maxLines, maxBytes int) (string, bool) {
	end, lines := 0, 0
	for end < len(output) && lines < maxLines && end < maxBytes {
		if output[end] == '\n' {
			lines++
		}
		end++
	}
	truncated := end < len(output)
	for end > 0 && end < len(output) && !utf8.RuneStart(output[end]) {
		end--
	}
	return output[:end], truncated
}

// Page returns lines [offset, offset+limit) of text, the total line count
// and whether more lines follow.
func Page(text string, offset, limit int) (content string, total int, more bool) {
	var lines []string
	start := 0
	for i := 0; i < len(text); i++ {
		if text[i] == '\n' {
			lines = append(lines, text[start:i])
			start = i + 1
		}
	}
	if start < len(text) {
		lines = append(lines, text[start:])
	}
	total = len(lines)
	if offset < 0 {
		offset = 0
	}
	if offset > total {
		offset = total
	}
	end := total
	if limit > 0 && offset+limit < total {
		end = offset + limit
	}
	out := ""
	for i := offset; i < end; i++ {
		out += lines[i] + "\n"
	}
	return out, total, end < total
}
