package dispatch

import (
	"context"
	"fmt"
	"io"
	"net/http"
	"time"

	adapter "github.com/polterware/dispatch/go-core/providers/adapters"
)

// CatalogEntry is how the Auto route prices one model: θ and its source, the
// cost per task, and P(pass) with its lower bound at each band's typical d.
// Tier is the tier-table row it sits in; empty for a model only an agent CLI
// reports.
type CatalogEntry struct {
	Agent  string     `json:"agent"`
	Model  string     `json:"model"`
	Tier   string     `json:"tier,omitempty"`
	Effort string     `json:"effort,omitempty"`
	Bands  []BandPass `json:"bands"`
	ModelEstimate
}

// BandPass is σ(θ − d) and its bound at one band's typical difficulty.
type BandPass struct {
	Tier  string  `json:"tier"`
	D     float64 `json:"d"`
	PPass float64 `json:"pPass"`
	Low   float64 `json:"low"`
}

// CatalogReport is the catalog's provenance and its estimate of each model
// the router can route to.
type CatalogReport struct {
	Generated     string             `json:"generated"`
	PricesFetched string             `json:"pricesFetched,omitempty"`
	Sources       []CatalogSource    `json:"sources"`
	Models        int                `json:"models"`
	Scores        int                `json:"scores"`
	ECI           int                `json:"eci"`
	Bars          map[string]float64 `json:"bars"`
	// Priority is the policy's priority, whose bar Auto holds each model to.
	Priority string `json:"priority,omitempty"`
	// Discover says whether Auto routes to the reported models outside the tier table.
	Discover bool           `json:"discover"`
	Entries  []CatalogEntry `json:"entries"`
}

// ReportCatalog resolves every tier-table entry at the effort its tier runs
// with when the model names none, then each discovered model the table does
// not hold.
func ReportCatalog(cat Catalog, t Table, discovered []Entry) CatalogReport {
	r := CatalogReport{
		Generated: cat.Generated, PricesFetched: cat.PricesFetched, Sources: cat.Sources,
		Models: len(cat.Models), Scores: len(cat.Scores), ECI: len(cat.ECI), Entries: []CatalogEntry{},
		Bars: map[string]float64{PrioritySpeed: passThreshold(PrioritySpeed), PriorityBalanced: passThreshold(PriorityBalanced), PriorityQuality: passThreshold(PriorityQuality)},
	}
	seen := map[string]bool{}
	add := func(e Entry, tier adapter.Tier, effort string) {
		seen[e.Agent+"\x00"+e.Model] = true
		est := cat.Estimate(e.Model, effort, e.Strength)
		ce := CatalogEntry{Agent: e.Agent, Model: e.Model, Tier: string(tier), Effort: effort, ModelEstimate: est}
		for _, band := range []adapter.Tier{adapter.TierEasy, adapter.TierNormal, adapter.TierHard} {
			d := bandDifficulty(band)
			p, low := PassProbability(est.Theta, d, estimateUncertainty(est))
			ce.Bands = append(ce.Bands, BandPass{Tier: string(band), D: d, PPass: p, Low: low})
		}
		r.Entries = append(r.Entries, ce)
	}
	for _, tier := range []adapter.Tier{adapter.TierEasy, adapter.TierNormal, adapter.TierHard} {
		for _, e := range t.Tiers[tier] {
			effort := adapter.EffortOf(e.Model)
			if effort == "" {
				effort = e.Effort
			}
			if effort == "" && len(e.Efforts) > 0 {
				effort = tierEffort(tier)
			}
			add(e, tier, effort)
		}
	}
	for _, e := range discovered {
		if !seen[e.Agent+"\x00"+e.Model] {
			add(e, "", adapter.EffortOf(e.Model))
		}
	}
	return r
}

// tierEffort is the reasoning level a tier runs with by default.
func tierEffort(t adapter.Tier) string {
	switch t {
	case adapter.TierEasy:
		return "low"
	case adapter.TierHard:
		return "high"
	default:
		return "medium"
	}
}

// RefreshCatalog fetches models.dev and keeps its prices in stateDir.
func RefreshCatalog(ctx context.Context, client *http.Client, stateDir string, now time.Time) (int, error) {
	if stateDir == "" {
		return 0, fmt.Errorf("no dispatch state directory")
	}
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, ModelsDevURL, nil)
	if err != nil {
		return 0, err
	}
	res, err := client.Do(req)
	if err != nil {
		return 0, err
	}
	defer res.Body.Close()
	if res.StatusCode != http.StatusOK {
		return 0, fmt.Errorf("%s: %s", ModelsDevURL, res.Status)
	}
	raw, err := io.ReadAll(io.LimitReader(res.Body, 32<<20))
	if err != nil {
		return 0, err
	}
	return SaveRefreshedPrices(stateDir, raw, now)
}
