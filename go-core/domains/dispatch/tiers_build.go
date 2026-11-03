package dispatch

import (
	"sort"

	adapter "github.com/polterware/dispatch/go-core/providers/adapters"
)

// buildPerTier is how many entries Build from model puts in each tier.
const buildPerTier = 3

// bandDifficulty is the typical d of a tier: the middle of its band.
func bandDifficulty(t adapter.Tier) float64 {
	switch t {
	case adapter.TierEasy:
		return easyBelow - 0.5
	case adapter.TierHard:
		return normalBelow + 0.5
	default:
		return (easyBelow + normalBelow) / 2
	}
}

// BuiltEntry is one proposed tier entry with how the catalog priced it.
type BuiltEntry struct {
	Entry
	Auto AutoInfo `json:"auto"`
}

// TierBuild is the tier table the catalog proposes, and each row's price.
type TierBuild struct {
	Table Table                         `json:"-"`
	Tiers map[adapter.Tier][]BuiltEntry `json:"tiers"`
}

// BuildTiers proposes a tier table from the model catalog: for each tier, the
// candidates at every supported effort priced at the tier's typical difficulty,
// the cheapest ones over the priority's bar first, filled by the most likely to
// pass, at most three,
// one per benchmark entry, so a variant priced as its base model is not
// proposed beside it. Quota is not read: a table outlives today's pace.
func BuildTiers(entries []Entry, cat Catalog, priority string) TierBuild {
	var expanded []Entry
	for _, e := range uniqueEntries(entries) {
		efforts := e.Efforts
		if adapter.EffortOf(e.Model) != "" {
			efforts = []string{adapter.EffortOf(e.Model)}
		} else if len(efforts) == 0 {
			for _, a := range adapter.Known() {
				if a.Name() == e.Agent {
					efforts = a.Capabilities().Efforts
					break
				}
			}
		}
		if len(efforts) == 0 {
			efforts = []string{e.Effort}
		}
		for _, effort := range efforts {
			e.Effort = effort
			expanded = append(expanded, e)
		}
	}
	entries = expanded
	out := TierBuild{Table: Table{Tiers: map[adapter.Tier][]Entry{}}, Tiers: map[adapter.Tier][]BuiltEntry{}}
	for _, t := range []adapter.Tier{adapter.TierEasy, adapter.TierNormal, adapter.TierHard} {
		cs := make([]Candidate, len(entries))
		for i, e := range entries {
			cs[i] = Candidate{Model: e.Model, Effort: e.Effort, Strength: e.Strength}
		}
		priced := priceAuto(cs, bandDifficulty(t), priority, cat)
		order := make([]int, len(priced))
		for i := range order {
			order[i] = i
		}
		sort.SliceStable(order, func(i, j int) bool { return autoLess(priced[order[i]].Auto, priced[order[j]].Auto) })
		seen := map[string]bool{}
		for _, i := range order {
			base := cat.Estimate(entries[i].Model, priced[i].Effort, entries[i].Strength).Match
			if base == "" {
				base = catalogKey(entries[i].Model)
			}
			if seen[base] {
				continue
			}
			seen[base] = true
			out.Table.Tiers[t] = append(out.Table.Tiers[t], entries[i])
			out.Tiers[t] = append(out.Tiers[t], BuiltEntry{Entry: entries[i], Auto: *priced[i].Auto})
			if len(out.Tiers[t]) == buildPerTier {
				break
			}
		}
	}
	return out
}

// uniqueEntries keeps the first entry of each agent and model, with its
// provider filled in.
func uniqueEntries(es []Entry) []Entry {
	seen := map[string]bool{}
	var out []Entry
	for _, e := range es {
		k := e.Agent + "\x00" + e.Model
		if e.Agent == "" || e.Model == "" || seen[k] {
			continue
		}
		seen[k] = true
		if e.Provider == "" {
			e.Provider = adapter.ProviderOf(e.Agent, e.Model)
		}
		out = append(out, e)
	}
	return out
}
