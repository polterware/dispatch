package adapters

import (
	"context"
	"encoding/json"
	"strings"
)

// Pi drives the pi coding agent in print mode with its JSON event stream.
// A fresh run is given a caller-chosen session id (--session-id creates it)
// so it can be resumed the same way. A read-only run keeps only the tools
// that read; Pi has no permission prompt to refuse.
type Pi struct{}

var piRun = cliAdapter{
	name: "pi", binary: "pi",
	caps: Capabilities{EffortMode: "config", Efforts: []string{"low", "medium", "high", "xhigh"},
		SessionResume: true, StructuredOutput: true, ReadOnly: true, FileEditing: true},
	flags:       piProbeFlags,
	listArgs:    []string{"--list-models"},
	parseModels: parsePiModels,
	newSession:  func(context.Context) (string, error) { return newUUID(), nil },
	setDir:      true,
	args: func(r Request, model, session string, _ bool) []string {
		a := []string{"--mode", "json", "-p"}
		if session != "" {
			a = append(a, "--session-id", session)
		}
		if model != "" {
			a = append(a, "--model", model)
		}
		if r.Effort != "" {
			a = append(a, "--thinking", r.Effort)
		}
		if r.ReadOnly {
			a = append(a, "--tools", "read,grep,find,ls")
		}
		return append(a, "--", r.Prompt)
	},
	parse: parsePiStream,
	usage: usagePi,
	live:  piLive,
}

func (Pi) Name() string                                { return "pi" }
func (Pi) Binary() string                              { return Bin("pi") }
func (Pi) Capabilities() Capabilities                  { return piRun.caps }
func (Pi) Probe(ctx context.Context) ProbeReport       { return piRun.Probe(ctx) }
func (Pi) Quota(context.Context) (Quota, error)        { return Quota{}, ErrNoQuota }
func (Pi) Models(ctx context.Context) ([]Model, error) { return piRun.Models(ctx) }
func (Pi) Run(ctx context.Context, req Request, model string) (Result, error) {
	return piRun.Run(ctx, req, model)
}
func (Pi) Resume(ctx context.Context, req Request, model string) (Result, error) {
	return piRun.Resume(ctx, req, model)
}

var piProbeFlags = []probeFlag{
	{Name: "--mode", Capability: "structuredOutput"},
	{Name: "--print"},
	{Name: "--session-id", Capability: "sessionResume"},
	{Name: "--model"},
	{Name: "--thinking"},
	{Name: "--tools", Capability: "readOnly"},
}

// parsePiModels reads `pi --list-models`, a table whose first two columns
// are the provider and the model, as provider/model.
func parsePiModels(out string) []string {
	var ids []string
	for _, l := range lines(out) {
		f := strings.Fields(l)
		if len(f) < 2 || f[0] == "provider" {
			continue
		}
		ids = append(ids, f[0]+"/"+f[1])
	}
	return ids
}

// piEvent is the part of a Pi JSON line dispatch reads.
type piEvent struct {
	Type    string `json:"type"`
	ID      string `json:"id"`
	Message struct {
		Role         string            `json:"role"`
		Content      []json.RawMessage `json:"content"`
		StopReason   string            `json:"stopReason"`
		ErrorMessage string            `json:"errorMessage"`
		Usage        *piUsage          `json:"usage"`
	} `json:"message"`
	Delta struct {
		Type    string `json:"type"`
		Delta   string `json:"delta"`
		Content string `json:"content"`
	} `json:"assistantMessageEvent"`
}

// piUsage is one model call's usage: input apart from cache, output with
// reasoning, and the price Pi computed for it.
type piUsage struct {
	Input      int64 `json:"input"`
	Output     int64 `json:"output"`
	CacheRead  int64 `json:"cacheRead"`
	CacheWrite int64 `json:"cacheWrite"`
	Cost       *struct {
		Total float64 `json:"total"`
	} `json:"cost"`
}

// parsePiStream takes the text of the last assistant message and the
// session id from the stream's header.
func parsePiStream(stdout string) (string, string) {
	var last, session string
	found := false
	jsonEvents(stdout, func(ev piEvent) {
		if ev.Type == "" {
			return
		}
		found = true
		switch {
		case ev.Type == "session":
			session = ev.ID
		case ev.Type == "message_end" && ev.Message.Role == "assistant":
			if t := claudeText(ev.Message.Content); strings.TrimSpace(t) != "" {
				last = t
			}
		}
	})
	if !found {
		return strings.TrimSpace(stdout), ""
	}
	return strings.TrimSpace(last), session
}

// piTally sums each assistant message's usage and keeps the last one's
// context.
type piTally struct{ u *Usage }

func (t *piTally) read(ev piEvent) bool {
	if ev.Type != "message_end" || ev.Message.Role != "assistant" || ev.Message.Usage == nil {
		return false
	}
	x := ev.Message.Usage
	if t.u == nil {
		t.u = &Usage{}
	}
	t.u.InputTokens += x.Input
	t.u.CacheReadTokens += x.CacheRead
	t.u.CacheWriteTokens += x.CacheWrite
	t.u.OutputTokens += x.Output
	t.u.ContextTokens = x.Input + x.CacheRead + x.CacheWrite + x.Output
	if x.Cost != nil {
		total := x.Cost.Total
		if t.u.CostUSD != nil {
			total += *t.u.CostUSD
		}
		t.u.CostUSD = &total
	}
	return true
}

func usagePi(stdout string) *Usage {
	t := piTally{}
	jsonEvents(stdout, func(ev piEvent) { t.read(ev) })
	return t.u
}

// piLive keeps what the run page draws from Pi's stream. A message_update
// repeats the whole message with each delta, so only the delta is kept;
// partial tool output is dropped, and each finished assistant message adds
// a usage line.
func piLive() func([]byte) []byte {
	tally := piTally{}
	return func(line []byte) []byte {
		var ev piEvent
		if json.Unmarshal(line, &ev) != nil {
			return withNewline(line)
		}
		switch ev.Type {
		case "message_update":
			switch ev.Delta.Type {
			case "text_delta", "thinking_delta":
				return logAs(map[string]any{"type": ev.Type, "assistantMessageEvent": map[string]any{"type": ev.Delta.Type, "delta": ev.Delta.Delta}})
			case "text_end", "thinking_end":
				return logAs(map[string]any{"type": ev.Type, "assistantMessageEvent": map[string]any{"type": ev.Delta.Type, "content": ev.Delta.Content}})
			}
			return nil
		case "message_end":
			out := logAs(map[string]any{"type": ev.Type, "message": map[string]any{
				"role": ev.Message.Role, "stopReason": ev.Message.StopReason, "errorMessage": ev.Message.ErrorMessage}})
			if tally.read(ev) {
				out = append(out, usageLine(tally.u)...)
			}
			return out
		case "tool_execution_start", "tool_execution_end":
			return withNewline(line)
		}
		return nil
	}
}
