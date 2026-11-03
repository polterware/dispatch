package dispatch

import (
	"context"
	"sync"
	"time"
)

// fakeStore is an in-memory implementation of dispatch.Store for unit tests.
type fakeStore struct {
	mu        sync.Mutex
	stateDir  string
	runs      map[string]Run
	runOrder  []string
	events    []RunEvent
	worktrees map[string]WorktreeInfo
	wtOrder   []string
	quotas    []QuotaSnapshot
	tiers     []Tier
	agents    map[string]Agent
	decisions []RouteDecision
	writes    uint64
}

func newFakeStore() *fakeStore {
	return &fakeStore{
		runs:      map[string]Run{},
		worktrees: map[string]WorktreeInfo{},
		agents:    map[string]Agent{},
	}
}

func newFakeStoreWithDir(dir string) *fakeStore {
	s := newFakeStore()
	s.stateDir = dir
	return s
}

func (s *fakeStore) StateDir() string {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.stateDir
}

func (s *fakeStore) writeCount() uint64 {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.writes
}

func cloneRun(r Run) Run {
	out := r
	if r.Prefer != nil {
		out.Prefer = append([]string(nil), r.Prefer...)
	}
	if r.Avoid != nil {
		out.Avoid = append([]string(nil), r.Avoid...)
	}
	if r.Requested != nil {
		f := *r.Requested
		out.Requested = &f
	}
	if r.Resolved != nil {
		f := *r.Resolved
		out.Resolved = &f
	}
	if r.Usage != nil {
		u := *r.Usage
		if u.CostUSD != nil {
			u.CostUSD = F64(*u.CostUSD)
		}
		out.Usage = &u
	}
	if r.Attempts != nil {
		out.Attempts = make([]Attempt, len(r.Attempts))
		for i, a := range r.Attempts {
			out.Attempts[i] = a
			if a.DurationMs != nil {
				d := *a.DurationMs
				out.Attempts[i].DurationMs = &d
			}
		}
	}
	if r.DifficultyProbs != nil {
		out.DifficultyProbs = append([]float64(nil), r.DifficultyProbs...)
	}
	if r.Difficulty != nil {
		v := *r.Difficulty
		out.Difficulty = &v
	}
	if r.FilesChanged != nil {
		v := *r.FilesChanged
		out.FilesChanged = &v
	}
	if r.Insertions != nil {
		v := *r.Insertions
		out.Insertions = &v
	}
	if r.Deletions != nil {
		v := *r.Deletions
		out.Deletions = &v
	}
	if r.QueuedMs != nil {
		v := *r.QueuedMs
		out.QueuedMs = &v
	}
	if r.RunMs != nil {
		v := *r.RunMs
		out.RunMs = &v
	}
	if r.MergedAt != nil {
		v := *r.MergedAt
		out.MergedAt = &v
	}
	return out
}

func (s *fakeStore) SaveRun(_ context.Context, run Run) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	if _, exists := s.runs[run.ID]; !exists {
		s.runOrder = append(s.runOrder, run.ID)
	}
	s.runs[run.ID] = cloneRun(run)
	s.writes++
	return nil
}

func (s *fakeStore) GetRun(_ context.Context, id string) (Run, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	r, ok := s.runs[id]
	if !ok {
		return Run{}, ErrUnknownRun
	}
	return cloneRun(r), nil
}

func (s *fakeStore) ListRuns(_ context.Context, projectID string, limit, offset int, conversationID ...string) ([]Run, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	var out []Run
	for _, id := range s.runOrder {
		r := s.runs[id]
		if (projectID != "" && r.ProjectID != projectID) || (len(conversationID) > 0 && conversationID[0] != "" && r.ConversationID != conversationID[0]) {
			continue
		}
		out = append(out, cloneRun(r))
	}
	if offset > 0 {
		if offset >= len(out) {
			return nil, nil
		}
		out = out[offset:]
	}
	if limit > 0 && limit < len(out) {
		out = out[:limit]
	}
	return out, nil
}

func (s *fakeStore) AppendEvent(_ context.Context, event RunEvent) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	if event.Seq == 0 {
		event.Seq = int64(len(s.events) + 1)
	}
	s.events = append(s.events, event)
	s.writes++
	return nil
}

func (s *fakeStore) ListEvents(_ context.Context, runID string, afterSeq int64, limit int) ([]RunEvent, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	var out []RunEvent
	for _, ev := range s.events {
		if runID != "" && ev.RunID != runID {
			continue
		}
		if ev.Seq <= afterSeq {
			continue
		}
		out = append(out, ev)
		if limit > 0 && len(out) >= limit {
			break
		}
	}
	return out, nil
}

func (s *fakeStore) ListHistory(_ context.Context, projectID, agent, tier string, limit, offset int) ([]RunEvent, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	var out []RunEvent
	for _, ev := range s.events {
		if projectID != "" && ev.ProjectID != projectID {
			continue
		}
		if agent != "" && ev.Agent != agent {
			continue
		}
		if tier != "" && ev.Tier != tier {
			continue
		}
		out = append(out, ev)
	}
	if offset > 0 {
		if offset >= len(out) {
			return nil, nil
		}
		out = out[offset:]
	}
	if limit > 0 && limit < len(out) {
		out = out[:limit]
	}
	return out, nil
}

func (s *fakeStore) SaveWorktree(_ context.Context, info WorktreeInfo) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	if _, exists := s.worktrees[info.ID]; !exists {
		s.wtOrder = append(s.wtOrder, info.ID)
	}
	s.worktrees[info.ID] = info
	s.writes++
	return nil
}

func (s *fakeStore) ListWorktrees(_ context.Context, projectID string) ([]WorktreeInfo, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	var out []WorktreeInfo
	for _, id := range s.wtOrder {
		info := s.worktrees[id]
		if projectID != "" && info.ProjectID != projectID {
			continue
		}
		out = append(out, info)
	}
	return out, nil
}

func (s *fakeStore) SaveQuota(_ context.Context, snap QuotaSnapshot) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	cp := snap
	if snap.UsedPercent != nil {
		u := *snap.UsedPercent
		cp.UsedPercent = &u
	}
	s.quotas = append(s.quotas, cp)
	s.writes++
	return nil
}

func (s *fakeStore) ListQuota(_ context.Context, agent string, since time.Time) ([]QuotaSnapshot, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	var out []QuotaSnapshot
	for _, q := range s.quotas {
		if agent != "" && q.Agent != agent {
			continue
		}
		if !since.IsZero() && q.SampledAt.Before(since) && q.ResetsAt.Before(since) {
			continue
		}
		cp := q
		if q.UsedPercent != nil {
			u := *q.UsedPercent
			cp.UsedPercent = &u
		}
		out = append(out, cp)
	}
	return out, nil
}

func (s *fakeStore) ListTiers(_ context.Context) ([]Tier, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	return append([]Tier(nil), s.tiers...), nil
}

func (s *fakeStore) ReplaceTiers(_ context.Context, tiers []Tier) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.tiers = append([]Tier(nil), tiers...)
	s.writes++
	return nil
}

func (s *fakeStore) UpsertAgent(_ context.Context, agent Agent) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	cp := agent
	if agent.UsedPercent != nil {
		u := *agent.UsedPercent
		cp.UsedPercent = &u
	}
	if agent.CooldownUntil != nil {
		c := *agent.CooldownUntil
		cp.CooldownUntil = &c
	}
	s.agents[agent.Name] = cp
	s.writes++
	return nil
}

func (s *fakeStore) ListAgents(_ context.Context) ([]Agent, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	out := make([]Agent, 0, len(s.agents))
	for _, a := range s.agents {
		cp := a
		if a.UsedPercent != nil {
			u := *a.UsedPercent
			cp.UsedPercent = &u
		}
		if a.CooldownUntil != nil {
			c := *a.CooldownUntil
			cp.CooldownUntil = &c
		}
		out = append(out, cp)
	}
	return out, nil
}

func (s *fakeStore) SetCooldown(_ context.Context, agent string, until time.Time) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	a := s.agents[agent]
	a.Name = agent
	u := until
	a.CooldownUntil = &u
	s.agents[agent] = a
	s.writes++
	return nil
}

func (s *fakeStore) ClearCooldown(_ context.Context, agent string) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	if a, ok := s.agents[agent]; ok {
		a.CooldownUntil = nil
		s.agents[agent] = a
	}
	s.writes++
	return nil
}

func (s *fakeStore) SaveRouteDecision(_ context.Context, dec RouteDecision) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	for i, d := range s.decisions {
		if d.RunID == dec.RunID {
			s.decisions[i] = dec
			s.writes++
			return nil
		}
	}
	s.decisions = append(s.decisions, dec)
	s.writes++
	return nil
}

func (s *fakeStore) GetRouteDecision(_ context.Context, runID string) (RouteDecision, bool, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	for _, d := range s.decisions {
		if d.RunID == runID {
			return d, true, nil
		}
	}
	return RouteDecision{}, false, nil
}

func (s *fakeStore) ListRouteDecisions(_ context.Context, projectID string) ([]RouteDecision, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	var out []RouteDecision
	for i := len(s.decisions) - 1; i >= 0; i-- {
		if projectID == "" || s.decisions[i].ProjectID == projectID {
			out = append(out, s.decisions[i])
		}
	}
	return out, nil
}
