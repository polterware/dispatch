package adapters

import (
	"os"
	"strings"
	"testing"
)

func TestUsageFromStreams(t *testing.T) {
	read := func(path string) string {
		b, err := os.ReadFile(path)
		if err != nil {
			t.Fatal(err)
		}
		return string(b)
	}
	cursor := usageCursor(read("testdata/cursor/stream.ndjson"))
	if cursor == nil || *cursor != (Usage{InputTokens: 14276, OutputTokens: 190, CacheReadTokens: 21917}) {
		t.Fatalf("cursor: %+v", cursor)
	}

	grokStream := read("testdata/grok/stream.ndjson")
	grok := usageGrok(grokStream)
	if grok == nil || grok.InputTokens != 15418 || grok.CacheReadTokens != 29056 || grok.OutputTokens != 134 || grok.ReasoningTokens != 64 {
		t.Fatalf("grok: %+v", grok)
	}
	if grok.CostUSD == nil || *grok.CostUSD != 0.092336 {
		t.Fatalf("grok cost: %v", grok.CostUSD)
	}
	// Without "end" the per-call events add up to the same tokens, unpriced.
	cut := usageGrok(grokStream[:strings.LastIndex(grokStream, `{"type":"end"`)])
	if cut == nil || cut.InputTokens != 15418 || cut.OutputTokens != 134 || cut.CostUSD != nil {
		t.Fatalf("grok without end: %+v", cut)
	}

	agy := acpUsage{Input: 16060, Output: 210, CacheRead: 9000, Total: 16270}.normalize()
	if *agy != (Usage{InputTokens: 7060, CacheReadTokens: 9000, OutputTokens: 210}) {
		t.Fatalf("an input that counts its cache: %+v", agy)
	}
	open := acpUsage{Input: 18996, Output: 62, CacheRead: 36992, Thought: 8, Total: 56058}.normalize()
	if *open != (Usage{InputTokens: 18996, CacheReadTokens: 36992, OutputTokens: 70, ReasoningTokens: 8}) {
		t.Fatalf("thinking counted apart from output: %+v", open)
	}

	for name, u := range map[string]*Usage{
		"cursor": usageCursor("plain text\n"),
		"grok":   usageGrok("plain text\n"), "claude": usageClaude("plain text\n"), "pi": usagePi("plain text\n"),
	} {
		if u != nil {
			t.Fatalf("%s: no usage must be nil, got %+v", name, u)
		}
	}
}
