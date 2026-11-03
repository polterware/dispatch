package dispatch

import (
	_ "embed"
	"encoding/json"
	"fmt"
	"math"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"time"

	adapter "github.com/polterware/dispatch/go-core/providers/adapters"
)

// models_snapshot.json is written by tooling/models.mjs from
// models.dev (prices, MIT) and Epoch AI (CursorBench and ECI, CC-BY 4.0).
//
//go:embed models_snapshot.json
var modelsSnapshot []byte

// ModelsDevURL is the public, keyless price catalog `polterware models refresh` reads.
const ModelsDevURL = "https://models.dev/api.json"

// catalogProviders are the models.dev providers the dispatch CLIs serve;
// aggregators repeat their models and are skipped.
var catalogProviders = []string{"openai", "google", "xai", "anthropic", "deepseek", "zai", "moonshotai", "mistral", "alibaba", "meta"}

// effortRanks orders reasoning levels; adjacent ranks differ by one step.
var effortRanks = map[string]int{"minimal": 0, "low": 1, "medium": 2, "high": 3, "xhigh": 4, "max": 5}

// effortCost scales a per-task cost measured at medium to another level.
var effortCost = map[string]float64{"minimal": 0.4, "low": 0.6, "medium": 1, "high": 1.6, "xhigh": 2.5, "max": 3.5}

const (
	// effortThetaStep is the ability gained per reasoning level when a
	// benchmark lacks the requested level and the nearest one stands in.
	effortThetaStep = 0.15
	// defaultTokensPerTask turns a token price into a per-task cost when no
	// benchmark measured the model and the snapshot cannot calibrate one.
	defaultTokensPerTask = 40_000.0
	// unknownTheta is a mid-field ability for a model nothing describes.
	unknownTheta = -0.7
	// strengthThetaStep maps a tiers.json strength (1–5) around unknownTheta.
	strengthThetaStep = 0.35
)

type CatalogSource struct {
	Name    string `json:"name"`
	URL     string `json:"url"`
	License string `json:"license"`
	Fields  string `json:"fields"`
}

// CatalogModel is one model's list price in dollars per million tokens.
// CacheRead and CacheWrite are nil when models.dev lists no cache price.
// Tiers are the higher prices a call pays once its context passes Size.
type CatalogModel struct {
	ID         string      `json:"id"`
	Provider   string      `json:"provider"`
	Name       string      `json:"name"`
	Input      float64     `json:"input"`
	Output     float64     `json:"output"`
	CacheRead  *float64    `json:"cacheRead,omitempty"`
	CacheWrite *float64    `json:"cacheWrite,omitempty"`
	Tiers      []PriceTier `json:"tiers,omitempty"`
	Context    int         `json:"context"`
	MaxOutput  int         `json:"maxOutput"`
	Reasoning  bool        `json:"reasoning"`
}

// PriceTier is a context tier's price: models.dev `cost.tiers` with
// `tier.type` "context".
type PriceTier struct {
	Size       int      `json:"size"`
	Input      float64  `json:"input"`
	Output     float64  `json:"output"`
	CacheRead  *float64 `json:"cacheRead,omitempty"`
	CacheWrite *float64 `json:"cacheWrite,omitempty"`
}

// BenchScore is one CursorBench result: the share of tasks solved at one
// reasoning level, and what a task cost.
type BenchScore struct {
	Model         string   `json:"model"`
	Effort        string   `json:"effort"`
	Score         float64  `json:"score"`
	CostPerTask   *float64 `json:"costPerTask,omitempty"`
	TokensPerTask *float64 `json:"tokensPerTask,omitempty"`
	Date          string   `json:"date"`
}

type ECIScore struct {
	Model string  `json:"model"`
	ECI   float64 `json:"eci"`
}

// Catalog is what the router knows about models before any local run.
type Catalog struct {
	Generated string          `json:"generated"`
	Sources   []CatalogSource `json:"sources"`
	Models    []CatalogModel  `json:"models"`
	Scores    []BenchScore    `json:"scores"`
	ECI       []ECIScore      `json:"eci"`
	// PricesFetched is set when prices come from `polterware models refresh`.
	PricesFetched string `json:"pricesFetched,omitempty"`

	eciSlope, eciIntercept float64
	eciFit                 bool
	// tokensPerTask is the median of CursorBench cost over list price for the
	// models both describe, so a price-only model costs on the same scale.
	tokensPerTask float64
}

// refreshedPrices is the file `polterware models refresh` writes.
type refreshedPrices struct {
	Fetched string         `json:"fetched"`
	Models  []CatalogModel `json:"models"`
}

func pricesPath(stateDir string) string {
	if stateDir == "" {
		return ""
	}
	return filepath.Join(stateDir, "models.json")
}

// LoadCatalog reads the embedded snapshot, with prices from a refresh in
// stateDir when there is one. A missing or broken refresh keeps the snapshot.
func LoadCatalog(stateDir string) Catalog {
	var c Catalog
	if err := json.Unmarshal(modelsSnapshot, &c); err != nil {
		panic(fmt.Errorf("models snapshot: %w", err))
	}
	if p := pricesPath(stateDir); p != "" {
		if b, err := os.ReadFile(p); err == nil {
			var r refreshedPrices
			if json.Unmarshal(b, &r) == nil && len(r.Models) > 0 {
				c.Models, c.PricesFetched = r.Models, r.Fetched
			}
		}
	}
	c.fitECI()
	c.fitTokens()
	return c
}

// ParseModelsDev keeps the tool-calling text models of the catalog providers.
func ParseModelsDev(raw []byte) ([]CatalogModel, error) {
	var api map[string]struct {
		Models map[string]struct {
			Name       string `json:"name"`
			ToolCall   bool   `json:"tool_call"`
			Reasoning  bool   `json:"reasoning"`
			Modalities struct {
				Output []string `json:"output"`
			} `json:"modalities"`
			Cost *struct {
				Input      float64  `json:"input"`
				Output     float64  `json:"output"`
				CacheRead  *float64 `json:"cache_read"`
				CacheWrite *float64 `json:"cache_write"`
				Tiers      []struct {
					Input      float64  `json:"input"`
					Output     float64  `json:"output"`
					CacheRead  *float64 `json:"cache_read"`
					CacheWrite *float64 `json:"cache_write"`
					Tier       struct {
						Type string `json:"type"`
						Size int    `json:"size"`
					} `json:"tier"`
				} `json:"tiers"`
			} `json:"cost"`
			Limit struct {
				Context int `json:"context"`
				Output  int `json:"output"`
			} `json:"limit"`
		} `json:"models"`
	}
	if err := json.Unmarshal(raw, &api); err != nil {
		return nil, err
	}
	var out []CatalogModel
	for _, provider := range catalogProviders {
		for id, m := range api[provider].Models {
			text := false
			for _, o := range m.Modalities.Output {
				text = text || o == "text"
			}
			if !m.ToolCall || !text || m.Cost == nil {
				continue
			}
			name := m.Name
			if name == "" {
				name = id
			}
			var tiers []PriceTier
			for _, t := range m.Cost.Tiers {
				if t.Tier.Type == "context" && t.Tier.Size > 0 {
					tiers = append(tiers, PriceTier{Size: t.Tier.Size, Input: t.Input, Output: t.Output, CacheRead: t.CacheRead, CacheWrite: t.CacheWrite})
				}
			}
			sort.Slice(tiers, func(i, j int) bool { return tiers[i].Size < tiers[j].Size })
			out = append(out, CatalogModel{
				ID: id, Provider: provider, Name: name, Input: m.Cost.Input, Output: m.Cost.Output,
				CacheRead: m.Cost.CacheRead, CacheWrite: m.Cost.CacheWrite, Tiers: tiers,
				Context: m.Limit.Context, MaxOutput: m.Limit.Output, Reasoning: m.Reasoning,
			})
		}
	}
	if len(out) == 0 {
		return nil, fmt.Errorf("models.dev returned no usable model")
	}
	sort.Slice(out, func(i, j int) bool {
		return out[i].Provider < out[j].Provider || out[i].Provider == out[j].Provider && out[i].ID < out[j].ID
	})
	return out, nil
}

// SaveRefreshedPrices writes fetched models.dev prices over the snapshot's.
func SaveRefreshedPrices(stateDir string, raw []byte, now time.Time) (int, error) {
	p := pricesPath(stateDir)
	if p == "" {
		return 0, fmt.Errorf("no dispatch state directory")
	}
	models, err := ParseModelsDev(raw)
	if err != nil {
		return 0, err
	}
	b, err := json.MarshalIndent(refreshedPrices{Fetched: now.UTC().Format(time.RFC3339), Models: models}, "", " ")
	if err != nil {
		return 0, err
	}
	if err := os.MkdirAll(filepath.Dir(p), 0o755); err != nil {
		return 0, err
	}
	return len(models), os.WriteFile(p, b, 0o644)
}

func logit(p float64) float64 {
	p = clamp(p, 1e-4, 1-1e-4)
	return math.Log(p / (1 - p))
}

// catalogKey normalizes a CLI model name to a catalog id: no provider prefix
// (opencode's "openai/gpt-5.6"), no effort suffix, lower case, dashes for spaces.
func catalogKey(model string) string {
	m := adapter.BaseModel(strings.TrimSpace(model))
	if i := strings.LastIndex(m, "/"); i >= 0 {
		m = m[i+1:]
	}
	return strings.ReplaceAll(strings.ToLower(m), " ", "-")
}

// lookupKeys is the exact key, then shorter ones with trailing segments
// dropped ("grok-4.7-build-fast" reads as "grok-4.7" when only that is known).
func lookupKeys(key string) []string {
	keys := []string{key}
	for {
		i := strings.LastIndex(key, "-")
		if i <= 0 {
			return keys
		}
		key = key[:i]
		keys = append(keys, key)
	}
}

// scoreAt picks the benchmark row for a model at a reasoning level: the exact
// level, else the nearest one with θ shifted by the levels between them.
func (c Catalog) scoreAt(key, effort string) (BenchScore, float64, string, bool) {
	var rows []BenchScore
	for _, s := range c.Scores {
		if s.Model == key {
			rows = append(rows, s)
		}
	}
	if len(rows) == 0 {
		return BenchScore{}, 0, "", false
	}
	want, ok := effortRanks[effort]
	if !ok {
		want = effortRanks["medium"]
	}
	best, bestDist := rows[0], math.MaxInt
	for _, r := range rows {
		rank, ok := effortRanks[r.Effort]
		if !ok {
			rank = want // a model measured without levels stands for every level
		}
		dist := rank - want
		if dist < 0 {
			dist = -dist
		}
		if dist < bestDist {
			best, bestDist = r, dist
		}
	}
	shift := 0.0
	note := "cursorbench"
	if best.Effort != "" {
		note += " " + best.Effort
		if rank := effortRanks[best.Effort]; rank != want {
			shift = float64(want-rank) * effortThetaStep
			note += fmt.Sprintf(" (nearest to %s)", levelName(want))
		}
	}
	return best, shift, note, true
}

func levelName(rank int) string {
	for name, r := range effortRanks {
		if r == rank {
			return name
		}
	}
	return "medium"
}

// fitECI maps the Epoch Capabilities Index onto CursorBench's logit scale with
// a least-squares line over the models both measure.
func (c *Catalog) fitECI() {
	var xs, ys []float64
	for _, e := range c.ECI {
		if s, shift, _, ok := c.scoreAt(e.Model, "medium"); ok {
			xs, ys = append(xs, e.ECI), append(ys, logit(s.Score)+shift)
		}
	}
	if len(xs) < 5 {
		return
	}
	var mx, my float64
	for i := range xs {
		mx, my = mx+xs[i], my+ys[i]
	}
	mx, my = mx/float64(len(xs)), my/float64(len(ys))
	var sxy, sxx float64
	for i := range xs {
		sxy += (xs[i] - mx) * (ys[i] - my)
		sxx += (xs[i] - mx) * (xs[i] - mx)
	}
	if sxx == 0 {
		return
	}
	c.eciSlope = sxy / sxx
	c.eciIntercept = my - c.eciSlope*mx
	c.eciFit = true
}

// blendedPrice weights input three to one, as agent runs read far more than they write.
func blendedPrice(m CatalogModel) float64 { return (3*m.Input + m.Output) / 4 }

// fitTokens calibrates the per-task token count that turns a list price into
// CursorBench's per-task cost.
func (c *Catalog) fitTokens() {
	c.tokensPerTask = defaultTokensPerTask
	var ratios []float64
	for _, s := range c.Scores {
		if s.CostPerTask == nil || s.Effort != "medium" && s.Effort != "" {
			continue
		}
		if m, ok := c.price(s.Model); ok && blendedPrice(m) > 0 {
			ratios = append(ratios, *s.CostPerTask/blendedPrice(m)*1e6)
		}
	}
	if len(ratios) < 3 {
		return
	}
	sort.Float64s(ratios)
	c.tokensPerTask = ratios[len(ratios)/2]
}

// ModelEstimate is the catalog's prior for one candidate: its ability on the
// CursorBench logit scale and the dollars one task is expected to cost.
type ModelEstimate struct {
	Key         string   `json:"key"`
	Theta       float64  `json:"theta"`
	ThetaSource string   `json:"thetaSource"`
	CostPerTask *float64 `json:"costPerTask,omitempty"`
	CostSource  string   `json:"costSource,omitempty"`
	// Score is the benchmark figure θ comes from, as the benchmark reports
	// it: CursorBench's pass rate (0 to 1) or the ECI value; nil for a prior.
	Score *float64 `json:"score,omitempty"`
	// Match is the catalog key the benchmark score was read from, which
	// differs from Key's model when a shorter key matched; empty when none did.
	Match string `json:"-"`
	// ExtrapolatedLevels counts reasoning levels away from a measurement.
	ExtrapolatedLevels int `json:"extrapolatedLevels,omitempty"`
}

// Estimate resolves a candidate. strength is its tiers.json prior, used only
// when no benchmark describes the model.
func (c Catalog) Estimate(model, effort string, strength int) ModelEstimate {
	if effort == "" {
		effort = adapter.EffortOf(model)
	}
	key := catalogKey(model)
	est := ModelEstimate{Key: key + "@" + effort}
	for _, k := range lookupKeys(key) {
		if s, shift, note, ok := c.scoreAt(k, effort); ok {
			est.Theta, est.ThetaSource, est.Match, est.Score = logit(s.Score)+shift, note, k, F64(s.Score)
			est.ExtrapolatedLevels = int(math.Round(math.Abs(shift / effortThetaStep)))
			if k != key {
				est.ThetaSource += " (as " + k + ")"
			}
			if s.CostPerTask != nil {
				cost := *s.CostPerTask
				if s.Effort != "" && s.Effort != effort {
					if m, ok := effortCost[effort]; ok {
						cost *= m / effortCost[s.Effort]
					}
				}
				est.CostPerTask, est.CostSource = F64(cost), "cursorbench"
			}
			break
		}
	}
	if est.ThetaSource == "" && c.eciFit {
		for _, k := range lookupKeys(key) {
			for _, e := range c.ECI {
				if e.Model == k {
					est.Theta, est.ThetaSource, est.Match, est.Score = c.eciSlope*e.ECI+c.eciIntercept, "eci", k, F64(e.ECI)
					if k != key {
						est.ThetaSource += " (as " + k + ")"
					}
					break
				}
			}
			if est.ThetaSource != "" {
				break
			}
		}
	}
	if est.ThetaSource == "" {
		if strength > 0 {
			est.Theta, est.ThetaSource = unknownTheta+float64(strength-3)*strengthThetaStep, "tiers.json strength"
		} else {
			est.Theta, est.ThetaSource = unknownTheta, "unknown"
		}
	}
	if !strings.HasPrefix(est.ThetaSource, "cursorbench") {
		if rank, ok := effortRanks[effort]; ok {
			levels := rank - effortRanks["medium"]
			est.Theta += float64(levels) * effortThetaStep
			est.ExtrapolatedLevels = int(math.Abs(float64(levels)))
		}
	}
	if est.CostPerTask == nil {
		if m, ok := c.price(key); ok {
			mult := 1.0
			if v, ok := effortCost[effort]; ok {
				mult = v
			}
			tokens := c.tokensPerTask
			if tokens == 0 {
				tokens = defaultTokensPerTask
			}
			cost := blendedPrice(m) * tokens / 1e6 * mult
			est.CostPerTask, est.CostSource = F64(cost), "models.dev price"
		}
	}
	return est
}

func (c Catalog) price(key string) (CatalogModel, bool) {
	for _, k := range lookupKeys(key) {
		for _, m := range c.Models {
			if m.ID == k {
				return m, true
			}
		}
	}
	return CatalogModel{}, false
}
