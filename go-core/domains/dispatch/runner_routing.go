package dispatch

import (
	"context"
	"fmt"
	"slices"
	"strings"
	"time"

	adapter "github.com/polterware/dispatch/go-core/providers/adapters"
)

func validateOptions(o Options) error {
	if o.Tier != "" && o.Tier != TierAuto && !o.Tier.Valid() {
		return fmt.Errorf("invalid tier %q (want auto|easy|normal|hard)", o.Tier)
	}
	if o.Tier == TierAuto && (o.Filters.Model != "" || o.Filters.Provider != "") {
		return fmt.Errorf("tier auto picks the model; drop --model and --provider")
	}
	if o.Filters.Effort != "" && !adapter.ValidEffort(o.Filters.Effort) {
		return fmt.Errorf("invalid effort %q (want low|medium|high|xhigh)", o.Filters.Effort)
	}
	if o.Filters.Effort != "" && adapter.EffortOf(o.Filters.Model) != "" && adapter.EffortOf(o.Filters.Model) != o.Filters.Effort {
		return fmt.Errorf("model suffix conflicts with requested effort")
	}
	if o.Filters.Priority != "" && !ValidPriority(o.Filters.Priority) {
		return fmt.Errorf("invalid priority %q (want speed|balanced|quality)", o.Filters.Priority)
	}
	return nil
}

func normalizePriority(o *Options) error {
	if o.Filters.Priority == "" {
		o.Filters.Priority = PriorityBalanced
		return nil
	}
	if !ValidPriority(o.Filters.Priority) {
		return fmt.Errorf("invalid priority %q (want speed|balanced|quality)", o.Filters.Priority)
	}
	return nil
}

// tieredEntry keeps the source tier so measured evidence is not pooled across
// difficulties, and a note on why the route took it.
type tieredEntry struct {
	Entry
	tier adapter.Tier
	note string
}

// selectCandidates applies hard constraints before quota/model ranking. In
// particular a fixed model never competes with a different model in Rank.
// Priority is copied onto candidates and never rejects one.
func (d Deps) selectCandidates(o Options) ([]Candidate, []string) {
	return d.selectCandidatesWith(context.Background(), o)
}

func (d Deps) selectCandidatesWith(ctx context.Context, o Options) ([]Candidate, []string) {
	return d.selectCandidatesCool(ctx, o, storeCooldowns(ctx, d.Store, d.now()))
}

func (d Deps) selectCandidatesCool(ctx context.Context, o Options, cool map[string]time.Time) ([]Candidate, []string) {
	f := o.Filters
	var entries []tieredEntry
	var rejected []string
	var all []tieredEntry
	for _, t := range []adapter.Tier{adapter.TierEasy, adapter.TierNormal, adapter.TierHard} {
		for _, e := range d.Table.Tiers[t] {
			te := tieredEntry{Entry: e, tier: t}
			all = append(all, te)
			if o.Tier == "" || o.Tier == TierAuto || o.Tier == t {
				entries = append(entries, te)
			} else {
				rejected = append(rejected, e.Agent+"/"+e.Model+": tier filter requires "+string(o.Tier))
			}
		}
	}
	// Discovery deliberately only augments normal routing and only asks the
	// model-list endpoint already used by `tiers --check`; it never invokes a model.
	if d.Policy.Discover && (o.Tier == "" || o.Tier == TierAuto || o.Tier == adapter.TierNormal) {
		listed := map[string]bool{}
		for _, e := range all {
			listed[e.Agent+"/"+e.Model] = true
		}
		for _, a := range d.Adapters {
			if !d.installed(a.Binary()) {
				continue
			}
			ms, err := a.Models(ctx)
			if err != nil {
				continue
			}
			for _, m := range ms {
				if m.ID == "" || listed[a.Name()+"/"+m.ID] {
					continue
				}
				te := tieredEntry{Entry: Entry{Agent: a.Name(), Model: m.ID, Provider: adapter.ProviderOf(a.Name(), m.ID)}, tier: adapter.TierNormal}
				all = append(all, te)
				entries = append(entries, te)
				listed[a.Name()+"/"+m.ID] = true
			}
		}
	}
	if f.Agent == OpenCodeAgent && f.Model == "" {
		entries = d.openCodeEntries(ctx, o, entries)
	}
	found := false
	for _, e := range all {
		if matchesModel(e.Model, f.Model) {
			found = true
		}
	}
	outside := f.Model != "" && !found
	if outside {
		// An unlisted model can use an explicitly named agent, or agents with
		// a declared model of the same provider. Never guess cross-provider access.
		entries = nil
		add := func(e Entry) {
			entries = append(entries, tieredEntry{Entry: e, tier: o.Tier})
		}
		for _, a := range d.Adapters {
			desiredProvider := adapter.ProviderOf("", f.Model)
			if desiredProvider == "" {
				desiredProvider = f.Provider
			}
			if desiredProvider == "" {
				add(Entry{Agent: a.Name(), Model: f.Model})
				continue
			}
			if f.Agent != "" && a.Name() == f.Agent {
				add(Entry{Agent: a.Name(), Model: f.Model})
				continue
			}
			if adapter.ProviderOf(a.Name(), "") == desiredProvider {
				add(Entry{Agent: a.Name(), Model: f.Model, Provider: desiredProvider})
				continue
			}
			for _, e := range all {
				provider := e.Provider
				if provider == "" {
					provider = adapter.ProviderOf(e.Agent, e.Model)
				}
				if e.Agent == a.Name() && provider == desiredProvider && provider != "" {
					add(Entry{Agent: a.Name(), Model: f.Model, Provider: provider})
					break
				}
			}
		}
	}
	var cs []Candidate
	seen := map[string]bool{}
	for _, e := range entries {
		label := e.Agent + "/" + e.Model
		reject := func(reason string) { rejected = append(rejected, label+": "+reason) }
		provider := e.Provider
		if provider == "" {
			provider = adapter.ProviderOf(e.Agent, e.Model)
		}
		if reason := policyReason(d.Policy, provider, e.Model, e.Agent); reason != "" {
			reject(reason)
			continue
		}
		switch {
		case f.Provider != "" && f.Provider != provider:
			reject("provider filter requires " + f.Provider)
			continue
		case f.Model != "" && !matchesModel(e.Model, f.Model):
			reject("model filter requires " + f.Model)
			continue
		case f.Agent != "" && f.Agent != e.Agent:
			reject("agent filter requires " + f.Agent)
			continue
		}
		a := d.adapter(e.Agent)
		if a == nil || !d.installed(a.Binary()) {
			reject("agent unavailable")
			continue
		}
		if !adapter.AllowedModel(e.Agent, e.Model) {
			reject("Claude models refused in third-party CLIs")
			continue
		}
		if key, ok := cooling(cool, e.Agent, creditScope(e.Agent, provider, e.Model)); ok {
			reject(key + " in cooldown")
			continue
		}
		caps := a.Capabilities()
		if o.ReadOnly && !caps.ReadOnly {
			reject("does not support readOnly")
			continue
		}
		if !o.ReadOnly && !caps.FileEditing {
			reject("does not support fileEditing")
			continue
		}
		model, effort := e.Model, f.Effort
		if f.Model != "" && adapter.EffortOf(f.Model) != "" {
			model = f.Model
			if effort == "" {
				effort = adapter.EffortOf(f.Model)
			}
		}
		if effort == "" && o.Tier != TierAuto {
			effort = e.Effort
		}
		explicitEffort := effort != ""
		if effort == "" {
			effort = adapter.EffortOf(model)
		}
		efforts := []string{effort}
		if effort == "" && caps.EffortMode != "" {
			if o.Tier == TierAuto {
				// Auto weighs every level the model runs at: each is its own
				// point of ability and cost, and rankAuto keeps the best one.
				efforts = e.Efforts
				if len(efforts) == 0 {
					efforts = caps.Efforts
				}
				if len(efforts) == 0 {
					efforts = []string{tierEffort(e.tier)}
				}
			} else {
				efforts = []string{tierEffort(o.Tier)}
			}
		}
		for _, effort := range efforts {
			model := model
			if effort != "" {
				supported := e.Efforts
				if len(supported) == 0 {
					supported = caps.Efforts
				}
				if len(supported) == 0 && adapter.EffortOf(e.Model) != "" {
					supported = []string{adapter.EffortOf(e.Model)}
				}
				if explicitEffort && caps.EffortMode == "" {
					// A listed immutable model suffix is the only supported control for
					// CLIs without an independent reasoning setting.
					if adapter.EffortOf(model) != effort {
						reject("effort unsupported by agent (no effort control)")
						continue
					}
				}
				if len(supported) == 0 || !slices.Contains(supported, effort) || (len(caps.Efforts) > 0 && !slices.Contains(caps.Efforts, effort)) {
					reject("effort " + effort + " unsupported by model/agent")
					continue
				}
				switch caps.EffortMode {
				case "model-suffix":
					model = adapter.BaseModel(model) + "-" + effort
				case "config":
					model = adapter.BaseModel(model)
				default:
					if adapter.EffortOf(model) != effort {
						reject("effort unsupported by agent")
						continue
					}
				}
			}
			key := e.Agent + "/" + model + "/" + effort
			if seen[key] {
				continue
			}
			seen[key] = true
			c := Candidate{
				Adapter: a, Model: model, Provider: provider, Effort: effort,
				Priority: f.Priority, Tier: string(e.tier), Speed: e.Speed, Strength: e.Strength,
			}
			switch {
			case outside:
				c.Note = "explicit model outside tier table"
			case e.note != "":
				c.Note = e.note
			case d.Policy.Discover && !tableHas(d.Table, e.Agent, e.Model):
				c.Note = "discovered"
			}
			if slices.Contains(o.Prefer, e.Agent) {
				c.Bias = PreferBias
			}
			if slices.Contains(o.Avoid, e.Agent) {
				c.Bias += AvoidBias
			}
			cs = append(cs, c)
		}
	}
	if len(cs) == 0 && len(rejected) == 0 {
		rejected = append(rejected, "no candidate matches tier/model intersection")
	}
	return cs, rejected
}

func tableHas(t Table, agent, model string) bool {
	for _, es := range t.Tiers {
		for _, e := range es {
			if e.Agent == agent && e.Model == model {
				return true
			}
		}
	}
	return false
}

// policyReason returns the exact persistent rule that excludes a candidate.
func policyReason(p Policy, provider, model, agent string) string {
	// A suffix variant is covered by a policy written for its base model.
	if base := adapter.BaseModel(model); base != model {
		if slices.Contains(p.Allow.Models, base) {
			p.Allow.Models = append(p.Allow.Models, model)
		}
		if slices.Contains(p.Deny.Models, base) {
			p.Deny.Models = append(p.Deny.Models, model)
		}
	}
	return PolicyReason(p, provider, model, agent)
}

// policyRequestReason catches a requested filter that is itself outside the
// persistent policy. In that case --relax must not silently substitute a
// different provider/model/agent.
func policyRequestReason(p Policy, f Filters) string {
	if f.Provider != "" {
		if r := policyReason(p, f.Provider, "", ""); r != "" && !strings.Contains(r, "models") && !strings.Contains(r, "agents") {
			return r
		}
	}
	if f.Model != "" {
		q := p
		q.Allow.Providers = nil
		q.Allow.Agents = nil
		q.Deny.Providers = nil
		q.Deny.Agents = nil
		if r := policyReason(q, "", f.Model, ""); r != "" {
			return r
		}
	}
	if f.Agent != "" {
		q := p
		q.Allow.Providers = nil
		q.Allow.Models = nil
		q.Deny.Providers = nil
		q.Deny.Models = nil
		if r := policyReason(q, "", "", f.Agent); r != "" {
			return r
		}
	}
	return ""
}

func matchesModel(model, requested string) bool {
	if requested == "" {
		return false
	}
	if model == requested {
		return true
	}
	return adapter.BaseModel(model) == adapter.BaseModel(requested)
}

type RouteCandidate struct {
	Filters
	Score  float64   `json:"score"`
	Reason string    `json:"reason"`
	Terms  []Term    `json:"terms,omitempty"`
	Auto   *AutoInfo `json:"auto,omitempty"`
}

// RouteReport contains only structured routing decisions, never agent output.
type RouteReport struct {
	Requested      Filters          `json:"requested"`
	Priority       string           `json:"priority,omitempty"`
	Candidates     []RouteCandidate `json:"candidates"`
	Eliminated     []string         `json:"eliminated"`
	Winner         *Filters         `json:"winner,omitempty"`
	BalancedWinner *Filters         `json:"balancedWinner,omitempty"`
	Reason         string           `json:"reason"`
	PriorityNote   string           `json:"priorityNote,omitempty"`
	Note           string           `json:"note,omitempty"`
	// Difficulty is d of the prompt on an Auto route, and Classifier and
	// Probs the answer it came from.
	Difficulty *float64  `json:"difficulty,omitempty"`
	Classifier string    `json:"classifier,omitempty"`
	Probs      []float64 `json:"probs,omitempty"`
	// Role is the role the route runs as, and RolePick how the classifier
	// picked it when it did.
	Role     string    `json:"role,omitempty"`
	RolePick *RolePick `json:"rolePick,omitempty"`
}

// Route reads cached quota and cooldown state. It never records a
// snapshot/run, reconciles processes, or creates a worktree; on Auto it asks
// the classifier, which with the CLI classifier runs an agent CLI.
func (d Deps) Route(ctx context.Context, o Options) (RouteReport, error) {
	if err := normalizePriority(&o); err != nil {
		return RouteReport{}, err
	}
	r := RouteReport{Requested: o.Filters, Priority: o.Filters.Priority, Candidates: []RouteCandidate{}}
	note, err := d.resolveAuto(ctx, &o)
	if err != nil {
		return r, err
	}
	r.Note = note
	r.Role, r.RolePick = o.Profile, o.pick
	if err := validateOptions(o); err != nil {
		return r, err
	}
	if diff := o.difficulty; diff != nil {
		r.Classifier = diff.Provider
		if diff.HasProbs {
			r.Probs = append([]float64(nil), diff.Probs[:]...)
		}
	}
	ranked, balanced, rejected, relaxed := d.plan(ctx, o, false)
	r.Eliminated = rejected
	if relaxed {
		r.Note = joinNote(r.Note, "--relax: filters relaxed to tier routing")
	}
	for _, c := range ranked {
		rc := RouteCandidate{Filters: routeFilters(c), Score: Score(c), Reason: Reason(c), Terms: Terms(c)}
		if c.Auto != nil {
			rc.Auto, rc.Reason, rc.Terms = c.Auto, autoReason(c), nil
			r.Difficulty = F64(c.Auto.Difficulty)
		}
		r.Candidates = append(r.Candidates, rc)
		if c.Note != "" {
			r.Note = joinNote(r.Note, c.Note)
		}
	}
	if len(r.Candidates) == 0 {
		r.Reason = "no eligible candidate: " + strings.Join(r.Eliminated, "; ")
		return r, nil
	}
	r.Winner = &r.Candidates[0].Filters
	if len(balanced) > 0 {
		bw := routeFilters(balanced[0])
		bw.Priority = o.Filters.Priority
		r.BalancedWinner = &bw
	}
	r.PriorityNote = PriorityEffect(o.Filters.Priority, ranked, balanced)
	r.Reason = "lowest score wins; stable table order breaks ties; cached quota only: " + r.Candidates[0].Reason
	if tierRoute(o) {
		r.Reason = fmt.Sprintf("the tier's table order wins, passing over an account with low quota (%d%% used or out before reset) or a rate limit; cached quota only: %s", LowQuotaPercent, r.Candidates[0].Reason)
	}
	if ranked[0].Auto != nil {
		r.Note = joinNote(r.Note, autoNote(ranked))
		r.Reason = "auto: the cheapest candidate whose pass bound meets the threshold wins, else the most likely to pass; cached quota only: " + r.Candidates[0].Reason
	}
	if r.PriorityNote != "" {
		r.Reason += "; " + r.PriorityNote
	}
	return r, nil
}

func routeFilters(c Candidate) Filters {
	agent := ""
	if c.Adapter != nil {
		agent = c.Adapter.Name()
	}
	prio := c.Priority
	if prio == "" {
		prio = PriorityBalanced
	}
	return Filters{Provider: c.Provider, Model: c.Model, Effort: c.Effort, Agent: agent, Priority: prio}
}

func shiftPriority(cs []Candidate, priority string) []Candidate {
	out := append([]Candidate(nil), cs...)
	for i := range out {
		out[i].Priority = priority
	}
	return out
}

func (d Deps) now() time.Time {
	if d.Now != nil {
		return d.Now()
	}
	return time.Now()
}
