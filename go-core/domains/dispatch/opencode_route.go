package dispatch

import (
	"context"

	adapter "github.com/polterware/dispatch/go-core/providers/adapters"
)

// OpenCodeAgent serves the models of every provider OpenCode is logged into,
// so a route restricted to it reads the models `opencode models` lists beside
// the table's OpenCode entries. `dispatch run|route --agent opencode` routes this way.
const OpenCodeAgent = "opencode"

// openCodePickNote marks a candidate a tier route took from the models
// OpenCode lists because the tier has no OpenCode entry.
const openCodePickNote = "the tier has no OpenCode entry: OpenCode's models ranked for the tier"

// AgentPool is what Build from model ranks for one agent: its entries in the
// table, then the models it lists that the table does not hold. Claude
// models, which third-party CLIs may not run, are left out.
func AgentPool(agent string, table Table, listed []Entry) []Entry {
	var out []Entry
	for _, t := range []adapter.Tier{adapter.TierEasy, adapter.TierNormal, adapter.TierHard} {
		for _, e := range table.Tiers[t] {
			if e.Agent == agent {
				out = append(out, e)
			}
		}
	}
	for _, e := range listed {
		if e.Agent == agent {
			out = append(out, e)
		}
	}
	var pool []Entry
	for _, e := range uniqueEntries(out) {
		if adapter.AllowedModel(e.Agent, e.Model) {
			pool = append(pool, e)
		}
	}
	return pool
}

// MergeAgent replaces agent's entries in each tier of table with built's,
// after the other agents' entries, which keep their order.
func MergeAgent(table, built Table, agent string) Table {
	out := Table{Tiers: map[adapter.Tier][]Entry{}}
	for _, t := range []adapter.Tier{adapter.TierEasy, adapter.TierNormal, adapter.TierHard} {
		for _, e := range table.Tiers[t] {
			if e.Agent != agent {
				out.Tiers[t] = append(out.Tiers[t], e)
			}
		}
		for _, e := range built.Tiers[t] {
			if e.Agent == agent {
				out.Tiers[t] = append(out.Tiers[t], e)
			}
		}
	}
	return out
}

// openCodeEntries widens a route restricted to OpenCode with the models it
// lists. Auto weighs all of them. A tier route keeps the tier's OpenCode
// entries when the table has some; otherwise it takes the models BuildTiers
// would put in the tier, best first, in place of any discovered ones.
func (d Deps) openCodeEntries(ctx context.Context, o Options, entries []tieredEntry) []tieredEntry {
	a := d.adapter(OpenCodeAgent)
	if a == nil || !d.installed(a.Binary()) {
		return entries
	}
	if o.Tier != "" && o.Tier != TierAuto {
		for _, e := range d.Table.Tiers[o.Tier] {
			if e.Agent == OpenCodeAgent {
				return entries
			}
		}
	}
	ms, err := a.Models(ctx)
	if err != nil {
		return entries
	}
	listed := make([]Entry, 0, len(ms))
	for _, m := range ms {
		if m.ID != "" {
			listed = append(listed, Entry{Agent: OpenCodeAgent, Model: m.ID, Provider: adapter.ProviderOf(OpenCodeAgent, m.ID)})
		}
	}
	if o.Tier == "" || o.Tier == TierAuto {
		known := map[string]bool{}
		for _, e := range entries {
			known[e.Agent+"/"+e.Model] = true
		}
		for _, e := range AgentPool(OpenCodeAgent, Table{}, listed) {
			if !known[e.Agent+"/"+e.Model] {
				entries = append(entries, tieredEntry{Entry: e, tier: adapter.TierNormal})
			}
		}
		return entries
	}
	var pool []Entry
	for _, e := range AgentPool(OpenCodeAgent, d.Table, listed) {
		if policyReason(d.Policy, e.Provider, e.Model, e.Agent) == "" {
			pool = append(pool, e)
		}
	}
	kept := entries[:0]
	for _, e := range entries {
		if e.Agent != OpenCodeAgent {
			kept = append(kept, e)
		}
	}
	for _, b := range BuildTiers(pool, LoadCatalog(d.stateDir()), o.Filters.Priority).Tiers[o.Tier] {
		kept = append(kept, tieredEntry{Entry: b.Entry, tier: o.Tier, note: openCodePickNote})
	}
	return kept
}
