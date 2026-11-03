package adapters

import (
	"bytes"
	"encoding/json"
	"io"
	"strings"
)

// Usage is what one agent run reported spending, read from the CLI's own
// output. Input excludes cache reads and writes, Output includes reasoning
// (Reasoning is the part of it that was), and CostUSD is set only when the
// CLI reports a price. Each CLI counts these differently; the parsers below
// bring them to this one shape.
//
// ContextTokens is how full the context window was after the last model
// call (its input, cache, and output, as OpenCode counts it), and
// ContextWindow the window's size when the CLI reports one; both are zero
// when unknown.
type Usage struct {
	InputTokens      int64
	CacheReadTokens  int64
	CacheWriteTokens int64
	OutputTokens     int64
	ReasoningTokens  int64
	CostUSD          *float64
	ContextTokens    int64
	ContextWindow    int64
}

// jsonEvents decodes each JSON line of stdout into a fresh T and calls fn.
func jsonEvents[T any](stdout string, fn func(T)) {
	for _, l := range lines(stdout) {
		l = strings.TrimSpace(l)
		if l == "" || l[0] != '{' {
			continue
		}
		var ev T
		if json.Unmarshal([]byte(l), &ev) == nil {
			fn(ev)
		}
	}
}

// writeUsageLine adds the run's usage so far to the live log as one
// normalized line, which the run page reads for its context meter and does
// not draw.
func writeUsageLine(w io.Writer, u *Usage) {
	if u != nil {
		w.Write(usageLine(u))
	}
}

func usageLine(u *Usage) []byte {
	b, _ := json.Marshal(map[string]any{
		"dispatch": "usage", "context": u.ContextTokens, "window": u.ContextWindow,
		"input": u.InputTokens, "cacheRead": u.CacheReadTokens, "cacheWrite": u.CacheWriteTokens,
		"output": u.OutputTokens, "reasoning": u.ReasoningTokens, "costUsd": u.CostUSD,
	})
	return append(b, '\n')
}

// lineFilter passes a CLI's stdout to the live log one whole line at a
// time, as fn rewrites it: fn returns what to log for the line (nil drops
// it), so a compact form and a usage line stay whole between the CLI's own.
type lineFilter struct {
	w   io.Writer
	fn  func(line []byte) []byte
	buf []byte
}

func (f *lineFilter) Write(p []byte) (int, error) {
	f.buf = append(f.buf, p...)
	for {
		i := bytes.IndexByte(f.buf, '\n')
		if i < 0 {
			break
		}
		f.emit(f.buf[:i])
		f.buf = f.buf[i+1:]
	}
	if len(f.buf) > 32<<20 { // an unterminated giant line
		f.flush()
	}
	return len(p), nil
}

func (f *lineFilter) emit(line []byte) {
	if out := f.fn(line); len(out) > 0 {
		f.w.Write(out)
	}
}

func (f *lineFilter) flush() {
	if len(f.buf) > 0 {
		f.emit(f.buf)
		f.buf = nil
	}
}

// logAs is a line as the live log keeps it: v re-encoded, with a newline.
func logAs(v any) []byte {
	b, err := json.Marshal(v)
	if err != nil {
		return nil
	}
	return append(b, '\n')
}

func clampTokens(n int64) int64 {
	if n < 0 {
		return 0
	}
	return n
}

// usageCursor reads the usage of cursor-agent's "result" event, which
// counts cache reads and writes apart from input. Cursor reports no price.
func usageCursor(stdout string) *Usage {
	var u *Usage
	jsonEvents(stdout, func(ev struct {
		Type  string `json:"type"`
		Usage *struct {
			Input      int64 `json:"inputTokens"`
			Output     int64 `json:"outputTokens"`
			CacheRead  int64 `json:"cacheReadTokens"`
			CacheWrite int64 `json:"cacheWriteTokens"`
		} `json:"usage"`
	}) {
		if ev.Type != "result" || ev.Usage == nil {
			return
		}
		x := ev.Usage
		u = &Usage{InputTokens: x.Input, OutputTokens: x.Output, CacheReadTokens: x.CacheRead, CacheWriteTokens: x.CacheWrite}
	})
	return u
}

// usageGrok reads Grok's "end" event: the run's totals and its price. A run
// that died before "end" sums the per-call "usage" events instead, unpriced.
// Grok counts reasoning inside output_tokens.
func usageGrok(stdout string) *Usage {
	type grokTokens struct {
		Input      int64 `json:"input_tokens"`
		CacheRead  int64 `json:"cache_read_input_tokens"`
		CacheWrite int64 `json:"cache_creation_input_tokens"`
		Output     int64 `json:"output_tokens"`
		Reasoning  int64 `json:"reasoning_tokens"`
	}
	var end, calls *Usage
	var last int64 // the context after the last model call
	add := func(u *Usage, x grokTokens) *Usage {
		if u == nil {
			u = &Usage{}
		}
		u.InputTokens += x.Input
		u.CacheReadTokens += x.CacheRead
		u.CacheWriteTokens += x.CacheWrite
		u.OutputTokens += x.Output
		u.ReasoningTokens += x.Reasoning
		return u
	}
	jsonEvents(stdout, func(ev struct {
		Type  string      `json:"type"`
		Usage *grokTokens `json:"usage"`
		Cost  *float64    `json:"total_cost_usd"`
	}) {
		if ev.Usage == nil {
			return
		}
		switch ev.Type {
		case "usage":
			calls = add(calls, *ev.Usage)
			x := ev.Usage
			last = x.Input + x.CacheRead + x.CacheWrite + x.Output
		case "end":
			end = add(nil, *ev.Usage)
			end.CostUSD = ev.Cost
		}
	})
	u := calls
	if end != nil {
		u = end
	}
	if u != nil {
		u.ContextTokens = last
	}
	return u
}
