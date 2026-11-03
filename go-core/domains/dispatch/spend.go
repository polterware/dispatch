package dispatch

import adapter "github.com/polterware/dispatch/go-core/providers/adapters"

// Usage is what a run spent, as the agent CLIs reported it, summed over its
// attempts (a failed fallback spent tokens too). Input excludes cache reads
// and writes, and Output includes reasoning. CostUSD is the API-equivalent
// price: what the CLI reported, else the tokens at the catalog's models.dev
// list prices. It is nil when an attempt could not be priced, because a
// partial sum would read as the whole.
type Usage struct {
	InputTokens      int64    `json:"inputTokens"`
	CacheReadTokens  int64    `json:"cacheReadTokens"`
	CacheWriteTokens int64    `json:"cacheWriteTokens"`
	OutputTokens     int64    `json:"outputTokens"`
	ReasoningTokens  int64    `json:"reasoningTokens"`
	CostUSD          *float64 `json:"costUsd,omitempty"`
	CostSource       string   `json:"costSource,omitempty"`
	// ContextTokens and ContextWindow are the last attempt's context: how
	// full its window was after its last model call, and the window's size
	// (the CLI's, else the catalog's). Zero when unknown.
	ContextTokens int64 `json:"contextTokens,omitempty"`
	ContextWindow int64 `json:"contextWindow,omitempty"`
}

// Where a run's cost comes from.
const (
	CostAgent   = "agent"
	CostCatalog = "models.dev price"
	CostMixed   = "agent and models.dev price"
)

// TotalTokens is every token the run sent or received, cache included.
func (u Usage) TotalTokens() int64 {
	return u.InputTokens + u.CacheReadTokens + u.CacheWriteTokens + u.OutputTokens
}

// UsageCost prices one attempt: the CLI's own price when it gave one, else
// its tokens at the model's list prices. A model with no cache price pays
// input price for cache tokens. Like OpenCode, an attempt whose context
// passed a price tier's size pays that tier; the last call's context stands
// for the attempt's, since the CLIs report the sum. ok is false when nothing
// prices it.
func (c Catalog) UsageCost(model string, u adapter.Usage) (cost float64, source string, ok bool) {
	if u.CostUSD != nil {
		return *u.CostUSD, CostAgent, true
	}
	m, found := c.price(catalogKey(model))
	if !found {
		return 0, "", false
	}
	p := PriceTier{Input: m.Input, Output: m.Output, CacheRead: m.CacheRead, CacheWrite: m.CacheWrite}
	for _, t := range m.Tiers {
		if u.ContextTokens > int64(t.Size) {
			p = t
		}
	}
	read, write := p.Input, p.Input
	if p.CacheRead != nil {
		read = *p.CacheRead
	}
	if p.CacheWrite != nil {
		write = *p.CacheWrite
	}
	cost = (float64(u.InputTokens)*p.Input + float64(u.CacheReadTokens)*read +
		float64(u.CacheWriteTokens)*write + float64(u.OutputTokens)*p.Output) / 1e6
	return cost, CostCatalog, true
}

// contextWindow is the window an attempt ran in: the CLI's, else the
// catalog's for its model, else zero.
func (c Catalog) contextWindow(model string, u adapter.Usage) int64 {
	if u.ContextWindow > 0 {
		return u.ContextWindow
	}
	if m, found := c.price(catalogKey(model)); found {
		return int64(m.Context)
	}
	return 0
}

// addUsage adds one attempt's reported usage, run on model, to the run.
func (r *Run) addUsage(model string, u *adapter.Usage, cat Catalog) {
	if u == nil {
		return
	}
	cost, source, priced := cat.UsageCost(model, *u)
	if r.Usage == nil {
		r.Usage = &Usage{}
		if priced {
			r.Usage.CostUSD, r.Usage.CostSource = F64(0), source
		}
	}
	s := r.Usage
	s.InputTokens += u.InputTokens
	s.CacheReadTokens += u.CacheReadTokens
	s.CacheWriteTokens += u.CacheWriteTokens
	s.OutputTokens += u.OutputTokens
	s.ReasoningTokens += u.ReasoningTokens
	if u.ContextTokens > 0 {
		s.ContextTokens, s.ContextWindow = u.ContextTokens, cat.contextWindow(model, *u)
	}
	switch {
	case !priced:
		s.CostUSD, s.CostSource = nil, ""
	case s.CostUSD != nil:
		*s.CostUSD += cost
		if s.CostSource != source {
			s.CostSource = CostMixed
		}
	}
}
