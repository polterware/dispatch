package dispatch

import (
	"context"
	_ "embed"
	"encoding/json"
	"fmt"
	"os"
	"slices"
	"sort"
	"strings"

	adapter "github.com/polterware/dispatch/go-core/providers/adapters"
)

//go:embed default.json
var defaultJSON []byte

type Entry struct {
	Provider string   `json:"provider,omitempty"`
	Efforts  []string `json:"efforts,omitempty"`
	Effort   string   `json:"effort,omitempty"`
	Agent    string   `json:"agent"`
	Model    string   `json:"model"`
	// Speed and Strength are optional 1–5 priors. Zero means absent (neutral).
	Speed    int `json:"speed,omitempty"`
	Strength int `json:"strength,omitempty"`
}

type Table struct {
	Tiers map[adapter.Tier][]Entry `json:"tiers"`
}

// Default returns the embedded table (placeholders; expected to be edited).
func Default() Table {
	t, err := parseTiers(defaultJSON)
	if err != nil {
		panic(err)
	}
	return t
}

// DefaultTiers is an alias for Default.
func DefaultTiers() Table { return Default() }

// LoadTiers reads path, or the embedded default when the file does not exist.
func LoadTiers(path string) (Table, error) {
	if path == "" {
		return Default(), nil
	}
	b, err := os.ReadFile(path)
	if os.IsNotExist(err) {
		return Default(), nil
	}
	if err != nil {
		return Table{}, err
	}
	t, err := parseTiers(b)
	if err != nil {
		return Table{}, fmt.Errorf("%s: %w", path, err)
	}
	return t, nil
}

// LoadTiersFromStore reads the tier table from the store, falling back to
// Default when no rows are stored.
func LoadTiersFromStore(ctx context.Context, s Store) (Table, error) {
	if s == nil {
		return Default(), nil
	}
	rows, err := s.ListTiers(ctx)
	if err != nil {
		return Table{}, err
	}
	if len(rows) == 0 {
		return Default(), nil
	}
	return TableFromRows(rows), nil
}

// storedModel carries a selected effort through the store and the tier API,
// whose model field already supports effort suffixes.
func storedModel(e Entry) string {
	if e.Effort != "" && adapter.EffortOf(e.Model) == "" {
		return e.Model + "-" + e.Effort
	}
	return e.Model
}

// Rows flattens Table into ordered Tier rows for Store.ReplaceTiers.
func (t Table) Rows() []Tier {
	var out []Tier
	for _, tier := range []adapter.Tier{adapter.TierEasy, adapter.TierNormal, adapter.TierHard} {
		for i, e := range t.Tiers[tier] {
			p := e.Provider
			if p == "" {
				p = adapter.ProviderOf(e.Agent, e.Model)
			}
			out = append(out, Tier{
				Name:     string(tier),
				Provider: p,
				Agent:    e.Agent,
				Model:    storedModel(e),
				Speed:    e.Speed,
				Strength: e.Strength,
				Position: i,
			})
		}
	}
	return out
}

// TableFromRows rebuilds a Table from stored Tier rows, sorted by Position.
func TableFromRows(rows []Tier) Table {
	cp := append([]Tier(nil), rows...)
	sort.SliceStable(cp, func(i, j int) bool { return cp[i].Position < cp[j].Position })
	t := Table{Tiers: map[adapter.Tier][]Entry{}}
	for _, r := range cp {
		tier := adapter.Tier(r.Name)
		p := r.Provider
		if p == "" {
			p = adapter.ProviderOf(r.Agent, r.Model)
		}
		model, effort := r.Model, ""
		for _, a := range adapter.Known() {
			if a.Name() == r.Agent && a.Capabilities().EffortMode == "config" {
				effort = adapter.EffortOf(model)
				model = adapter.BaseModel(model)
				break
			}
		}
		t.Tiers[tier] = append(t.Tiers[tier], Entry{
			Provider: p,
			Agent:    r.Agent,
			Model:    model,
			Effort:   effort,
			Speed:    r.Speed,
			Strength: r.Strength,
		})
	}
	return t
}

// DefaultJSON returns the embedded table text, for editors that start from it.
func DefaultJSON() []byte { return append([]byte(nil), defaultJSON...) }

// WriteDefault writes the embedded table to path unless it already exists.
func WriteDefault(path string) error {
	if _, err := os.Stat(path); err == nil {
		return fmt.Errorf("%s already exists", path)
	}
	return os.WriteFile(path, defaultJSON, 0o644)
}

func parseTiers(b []byte) (Table, error) {
	var t Table
	if err := json.Unmarshal(b, &t); err != nil {
		return Table{}, err
	}
	return t.Validated()
}

// Validated fills missing providers and rejects unknown tiers, invalid efforts, provider/model
// disagreements, empty agent or model, and speed or strength outside 1–5.
func (t Table) Validated() (Table, error) {
	for tier, es := range t.Tiers {
		if !tier.Valid() {
			return Table{}, fmt.Errorf("unknown tier %q", tier)
		}
		for i, e := range es {
			if e.Provider == "" {
				e.Provider = adapter.ProviderOf(e.Agent, e.Model)
				es[i].Provider = e.Provider
			}
			for _, effort := range entryEfforts(e) {
				if !adapter.ValidEffort(effort) {
					return Table{}, fmt.Errorf("tier %s: invalid effort %q", tier, effort)
				}
			}
			if e.Effort != "" && adapter.EffortOf(e.Model) != "" && e.Effort != adapter.EffortOf(e.Model) {
				return Table{}, fmt.Errorf("tier %s: effort %q disagrees with model %s", tier, e.Effort, e.Model)
			}
			if e.Effort != "" && len(e.Efforts) > 0 && !slices.Contains(e.Efforts, e.Effort) {
				return Table{}, fmt.Errorf("tier %s: effort %q not in supported efforts", tier, e.Effort)
			}
			if derived := adapter.ProviderOf("", e.Model); e.Provider != "" && derived != "" && e.Provider != derived {
				return Table{}, fmt.Errorf("tier %s: provider %q disagrees with model %s", tier, e.Provider, e.Model)
			}
			if e.Agent == "" || e.Model == "" {
				return Table{}, fmt.Errorf("tier %q: entries need agent and model", tier)
			}
		}
	}
	if ps := t.FieldProblems(); len(ps) > 0 {
		return Table{}, fmt.Errorf("%s", strings.Join(ps, "; "))
	}
	return t, nil
}

// FieldProblems reports speed and strength values outside 1–5.
// Zero is absent and valid. tiers --check includes this list.
func (t Table) FieldProblems() []string {
	var out []string
	for tier, es := range t.Tiers {
		for _, e := range es {
			if e.Speed != 0 && (e.Speed < 1 || e.Speed > 5) {
				out = append(out, fmt.Sprintf("%s: speed %d out of range 1-5", tier, e.Speed))
			}
			if e.Strength != 0 && (e.Strength < 1 || e.Strength > 5) {
				out = append(out, fmt.Sprintf("%s: strength %d out of range 1-5", tier, e.Strength))
			}
		}
	}
	sort.Strings(out)
	return out
}

// CapabilityProblems validates declared effort controls without invoking CLIs.
func (t Table) CapabilityProblems(adapters []adapter.Adapter) []string {
	var problems []string
	for tier, entries := range t.Tiers {
		for _, e := range entries {
			for _, a := range adapters {
				if a.Name() != e.Agent {
					continue
				}
				caps := a.Capabilities()
				for _, effort := range entryEfforts(e) {
					supported := true
					if !adapter.ValidEffort(effort) {
						supported = false
					}
					switch caps.EffortMode {
					case "config":
						found := false
						for _, v := range caps.Efforts {
							if v == effort {
								found = true
							}
						}
						supported = supported && found
					case "model-suffix":
					default:
						supported = supported && adapter.EffortOf(e.Model) == effort
					}
					if !supported {
						problems = append(problems, fmt.Sprintf("%s: %s/%s: effort %s unsupported by agent", tier, e.Agent, e.Model, effort))
					}
				}
			}
		}
	}
	return problems
}

// entryEfforts includes the selected effort when validating table metadata.
func entryEfforts(e Entry) []string {
	efforts := append([]string(nil), e.Efforts...)
	if e.Effort != "" {
		efforts = append(efforts, e.Effort)
	}
	return efforts
}
