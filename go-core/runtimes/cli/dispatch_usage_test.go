package main

import "testing"

func TestFormatDispatchUsageContext(t *testing.T) {
	cost := 0.42
	u := &dispatchRunUsage{InputTokens: 9000, CacheReadTokens: 36000, OutputTokens: 1200, CostUSD: &cost, ContextTokens: 46200, ContextWindow: 272000}
	if got, want := formatDispatchUsage(u), "tokens=46k cost=$0.42 context=46k/272k(16%)"; got != want {
		t.Fatalf("usage = %q, want %q", got, want)
	}
	u.ContextWindow = 0
	if got, want := formatDispatchUsage(u), "tokens=46k cost=$0.42 context=46k"; got != want {
		t.Fatalf("usage without a window = %q, want %q", got, want)
	}
	u.ContextTokens = 0
	if got, want := formatDispatchUsage(u), "tokens=46k cost=$0.42"; got != want {
		t.Fatalf("usage without context = %q, want %q", got, want)
	}
}
