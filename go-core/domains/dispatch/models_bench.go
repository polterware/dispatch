package dispatch

import (
	"sort"
	"strings"
)

// BenchLevel is one CursorBench result for a model at one reasoning level,
// as the benchmark reports it.
type BenchLevel struct {
	Effort string  `json:"effort,omitempty"`
	Score  float64 `json:"score"`
	Date   string  `json:"date,omitempty"`
}

// ModelBenchmark is what the public benchmarks measured for one model an
// agent CLI on this machine reports, read straight from the catalog. The *As
// fields name the catalog model a figure was read from when a shorter key
// matched; they are empty when the model's own key did.
type ModelBenchmark struct {
	Agent         string       `json:"agent"`
	Model         string       `json:"model"`
	Provider      string       `json:"provider,omitempty"`
	CursorBench   []BenchLevel `json:"cursorBench"`
	CursorBenchAs string       `json:"cursorBenchAs,omitempty"`
	ECI           *float64     `json:"eci,omitempty"`
	ECIAs         string       `json:"eciAs,omitempty"`
}

// BenchmarkReport is each installed model's benchmarks and where they come from.
type BenchmarkReport struct {
	Generated string           `json:"generated"`
	Sources   []CatalogSource  `json:"sources"`
	Models    []ModelBenchmark `json:"models"`
}

// BenchmarkModels reads the catalog's CursorBench and ECI figures for each
// model, once per agent and model. Measured models come first, the strongest
// CursorBench result, then the highest ECI, leading.
func BenchmarkModels(cat Catalog, models []Entry) BenchmarkReport {
	r := BenchmarkReport{Generated: cat.Generated, Models: []ModelBenchmark{}}
	for _, src := range cat.Sources {
		if strings.Contains(src.Name, "CursorBench") || strings.Contains(src.Name, "Capabilities Index") {
			r.Sources = append(r.Sources, src)
		}
	}
	seen := map[string]bool{}
	for _, e := range models {
		id := e.Agent + "\x00" + e.Model
		if seen[id] {
			continue
		}
		seen[id] = true
		key := catalogKey(e.Model)
		b := ModelBenchmark{Agent: e.Agent, Model: e.Model, Provider: e.Provider, CursorBench: []BenchLevel{}}
		for _, k := range lookupKeys(key) {
			for _, s := range cat.Scores {
				if s.Model == k {
					b.CursorBench = append(b.CursorBench, BenchLevel{Effort: s.Effort, Score: s.Score, Date: s.Date})
				}
			}
			if len(b.CursorBench) > 0 {
				if k != key {
					b.CursorBenchAs = k
				}
				break
			}
		}
		sort.SliceStable(b.CursorBench, func(i, j int) bool {
			return effortRanks[b.CursorBench[i].Effort] < effortRanks[b.CursorBench[j].Effort]
		})
	eci:
		for _, k := range lookupKeys(key) {
			for _, s := range cat.ECI {
				if s.Model == k {
					b.ECI = F64(s.ECI)
					if k != key {
						b.ECIAs = k
					}
					break eci
				}
			}
		}
		r.Models = append(r.Models, b)
	}
	best := func(b ModelBenchmark) float64 {
		top := -1.0
		for _, l := range b.CursorBench {
			top = max(top, l.Score)
		}
		return top
	}
	eci := func(b ModelBenchmark) float64 {
		if b.ECI == nil {
			return -1
		}
		return *b.ECI
	}
	sort.SliceStable(r.Models, func(i, j int) bool {
		a, b := r.Models[i], r.Models[j]
		if best(a) != best(b) {
			return best(a) > best(b)
		}
		if eci(a) != eci(b) {
			return eci(a) > eci(b)
		}
		return a.Agent+a.Model < b.Agent+b.Model
	})
	return r
}
