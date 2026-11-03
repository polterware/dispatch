package dispatch

import "testing"

func TestBenchmarkModelsReadsTheFiguresAsTheyAre(t *testing.T) {
	cat := Catalog{
		Generated: "2026-10-31",
		Sources: []CatalogSource{
			{Name: "models.dev", URL: "https://models.dev/api.json"},
			{Name: "Epoch AI, CursorBench", URL: "https://epoch.ai/benchmarks"},
			{Name: "Epoch AI, Epoch Capabilities Index", URL: "https://epoch.ai/benchmarks"},
		},
		Scores: []BenchScore{
			{Model: "gpt-5.6", Effort: "high", Score: 0.64},
			{Model: "gpt-5.6", Effort: "low", Score: 0.5},
		},
		ECI: []ECIScore{{Model: "gpt-5.6", ECI: 150}, {Model: "grok-4.7", ECI: 138}},
	}
	r := BenchmarkModels(cat, []Entry{
		{Agent: "grok", Model: "grok-4.7-build-fast"},
		{Agent: "codex", Model: "gpt-5.6", Provider: "openai"},
		{Agent: "codex", Model: "gpt-5.6"},
		{Agent: "cursor", Model: "mystery"},
	})
	if len(r.Sources) != 2 || r.Generated != "2026-10-31" {
		t.Fatalf("sources = %+v", r.Sources)
	}
	if len(r.Models) != 3 {
		t.Fatalf("one row per agent and model: %+v", r.Models)
	}
	gpt, grok, unknown := r.Models[0], r.Models[1], r.Models[2]
	if gpt.Model != "gpt-5.6" || len(gpt.CursorBench) != 2 || gpt.CursorBench[0].Effort != "low" || gpt.ECI == nil || *gpt.ECI != 150 || gpt.ECIAs != "" {
		t.Fatalf("gpt = %+v", gpt)
	}
	if grok.Model != "grok-4.7-build-fast" || len(grok.CursorBench) != 0 || grok.ECI == nil || grok.ECIAs != "grok-4.7" {
		t.Fatalf("grok = %+v", grok)
	}
	if unknown.Model != "mystery" || unknown.ECI != nil || len(unknown.CursorBench) != 0 {
		t.Fatalf("unknown = %+v", unknown)
	}
}
