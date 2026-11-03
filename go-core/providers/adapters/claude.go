package adapters

import (
	"context"
	"encoding/json"
	"strconv"
	"strings"
	"time"
)

// Claude drives Claude Code in print mode with its stream-json output, the
// prompt on stdin. A read-only run is its plan permission mode; a write run
// accepts edits. Its own price wins: the result event carries the cost the
// CLI computed, with each model's context window. Its quota is the
// rate_limit_event a run prints, the only place Claude Code reports it.
type Claude struct{}

var claudeRun = cliAdapter{
	name: "claude", binary: "claude",
	caps: Capabilities{EffortMode: "config", Efforts: []string{"low", "medium", "high", "xhigh"},
		SessionResume: true, StructuredOutput: true, ReadOnly: true, FileEditing: true, Usage: true},
	flags:       claudeProbeFlags,
	setDir:      true,
	stdinPrompt: true,
	args: func(r Request, model, session string, resume bool) []string {
		a := []string{"-p", "--output-format", "stream-json", "--verbose", "--include-partial-messages"}
		if r.ReadOnly {
			a = append(a, "--permission-mode", "plan")
		} else {
			a = append(a, "--permission-mode", "acceptEdits")
		}
		if resume {
			a = append(a, "--resume", session)
		}
		if model != "" {
			a = append(a, "--model", model)
		}
		if r.Effort != "" {
			a = append(a, "--effort", r.Effort)
		}
		return a
	},
	parse: parseClaudeStream,
	usage: usageClaude,
	live:  claudeLive,
}

// claudeModels are the models Claude Code takes by id; it has no command
// that lists them.
var claudeModels = []string{"claude-fable-5-1", "claude-opus-5-5", "claude-sonnet-5-5", "claude-haiku-5-5"}

func (Claude) Name() string                          { return "claude" }
func (Claude) Binary() string                        { return Bin("claude") }
func (Claude) Capabilities() Capabilities            { return claudeRun.caps }
func (Claude) Probe(ctx context.Context) ProbeReport { return claudeRun.Probe(ctx) }
func (Claude) UsageSource() string                   { return SourceClaudeStream }

// Usage is the windows of the last rate_limit_event a run printed that have
// not reset since; Claude Code has no command that reports them.
func (Claude) Usage(context.Context) ([]QuotaSample, error) {
	s := claudeObserved(time.Now())
	if len(s) == 0 {
		return nil, ErrNoQuota
	}
	return s, nil
}

func (Claude) Quota(context.Context) (Quota, error) { return claudeQuota(claudeObserved(time.Now())) }

func (Claude) Models(context.Context) ([]Model, error) {
	models := make([]Model, len(claudeModels))
	for i, id := range claudeModels {
		models[i] = Model{ID: id}
	}
	return models, nil
}

func (Claude) Run(ctx context.Context, req Request, model string) (Result, error) {
	return claudeRun.Run(ctx, req, model)
}

func (Claude) Resume(ctx context.Context, req Request, model string) (Result, error) {
	return claudeRun.Resume(ctx, req, model)
}

var claudeProbeFlags = []probeFlag{
	{Name: "--print"},
	{Name: "--output-format", Capability: "structuredOutput"},
	{Name: "--include-partial-messages"},
	{Name: "--verbose"},
	{Name: "--permission-mode", Capability: "readOnly"},
	{Name: "--resume", Capability: "sessionResume"},
	{Name: "--model"},
	{Name: "--effort"},
}

// claudeEvent is the part of a stream-json line dispatch reads.
type claudeEvent struct {
	Type      string       `json:"type"`
	Subtype   string       `json:"subtype"`
	SessionID string       `json:"session_id"`
	Parent    string       `json:"parent_tool_use_id"`
	APIID     string       `json:"api_message_id"`
	Result    string       `json:"result"`
	IsError   bool         `json:"is_error"`
	TotalCost *float64     `json:"total_cost_usd"`
	Usage     *claudeUsage `json:"usage"`
	// Models is every model the run called, its subagents' included.
	Models map[string]struct {
		Input         int64 `json:"inputTokens"`
		Output        int64 `json:"outputTokens"`
		CacheRead     int64 `json:"cacheReadInputTokens"`
		CacheWrite    int64 `json:"cacheCreationInputTokens"`
		Thinking      int64 `json:"thinkingTokens"`
		ContextWindow int64 `json:"contextWindow"`
	} `json:"modelUsage"`
	Limits  *claudeLimits `json:"rate_limit_info"`
	Message struct {
		ID      string            `json:"id"`
		Role    string            `json:"role"`
		Content []json.RawMessage `json:"content"`
	} `json:"message"`
	Event struct {
		Type  string       `json:"type"`
		Usage *claudeUsage `json:"usage"`
		Delta struct {
			Type     string `json:"type"`
			Text     string `json:"text"`
			Thinking string `json:"thinking"`
		} `json:"delta"`
	} `json:"event"`
}

// claudeUsage counts like the Anthropic API: input apart from cache reads
// and writes, and output including thinking.
type claudeUsage struct {
	Input      int64 `json:"input_tokens"`
	CacheWrite int64 `json:"cache_creation_input_tokens"`
	CacheRead  int64 `json:"cache_read_input_tokens"`
	Output     int64 `json:"output_tokens"`
	Details    struct {
		Thinking int64 `json:"thinking_tokens"`
	} `json:"output_tokens_details"`
	Iterations []claudeUsage `json:"iterations"`
}

func (u claudeUsage) context() int64 { return u.Input + u.CacheRead + u.CacheWrite + u.Output }

func (u claudeUsage) add(to *Usage) {
	to.InputTokens += u.Input
	to.CacheReadTokens += u.CacheRead
	to.CacheWriteTokens += u.CacheWrite
	to.OutputTokens += u.Output
	to.ReasoningTokens += u.Details.Thinking
}

// parseClaudeStream takes the reply from the result event, else the last
// assistant text (a run that died mid-turn), and the session id.
func parseClaudeStream(stdout string) (string, string) {
	var result, last, session string
	found := false
	jsonEvents(stdout, func(ev claudeEvent) {
		if ev.Type == "" {
			return
		}
		found = true
		if ev.SessionID != "" {
			session = ev.SessionID
		}
		switch ev.Type {
		case "result":
			result = ev.Result
		case "assistant":
			if ev.Parent != "" {
				return
			}
			if t := claudeText(ev.Message.Content); strings.TrimSpace(t) != "" {
				last = t
			}
		}
	})
	if !found {
		return strings.TrimSpace(stdout), ""
	}
	if strings.TrimSpace(result) == "" {
		result = last
	}
	return strings.TrimSpace(result), session
}

func claudeText(content []json.RawMessage) string {
	var b strings.Builder
	for _, raw := range content {
		var part struct{ Type, Text string }
		if json.Unmarshal(raw, &part) == nil && part.Type == "text" {
			b.WriteString(part.Text)
		}
	}
	return b.String()
}

// usageClaude reads the result event: its price, the tokens of every model
// it called (its own usage, when it lists none), the context of its last
// model call, and the window of the model that wrote the most. A run that
// ended before its result sums each message's final usage.
func usageClaude(stdout string) *Usage {
	var final *Usage
	sum := claudeTally{}
	jsonEvents(stdout, func(ev claudeEvent) {
		sum.read(ev)
		if ev.Type != "result" || ev.Usage == nil {
			return
		}
		u := &Usage{CostUSD: ev.TotalCost}
		if len(ev.Models) == 0 {
			ev.Usage.add(u)
		}
		for _, m := range ev.Models {
			u.InputTokens += m.Input
			u.CacheReadTokens += m.CacheRead
			u.CacheWriteTokens += m.CacheWrite
			u.OutputTokens += m.Output
			u.ReasoningTokens += m.Thinking
		}
		if n := len(ev.Usage.Iterations); n > 0 {
			u.ContextTokens = ev.Usage.Iterations[n-1].context()
		} else {
			u.ContextTokens = sum.context
		}
		var most int64 = -1
		for _, m := range ev.Models {
			if m.Output > most {
				most, u.ContextWindow = m.Output, m.ContextWindow
			}
		}
		final = u
	})
	if final != nil {
		return final
	}
	return sum.usage()
}

// claudeTally sums the final usage of each model call (a message_delta
// carries the whole call's usage), so the live log and a cut-off run see
// what was spent so far.
type claudeTally struct {
	calls   map[string]claudeUsage
	order   []string
	context int64
}

// read reports whether ev changed the tally.
func (t *claudeTally) read(ev claudeEvent) bool {
	if ev.Type != "stream_event" || ev.Event.Type != "message_delta" || ev.Event.Usage == nil {
		return false
	}
	if t.calls == nil {
		t.calls = map[string]claudeUsage{}
	}
	id := ev.APIID
	if id == "" {
		id = strconv.Itoa(len(t.order))
	}
	if _, seen := t.calls[id]; !seen {
		t.order = append(t.order, id)
	}
	t.calls[id] = *ev.Event.Usage
	if ev.Parent == "" {
		t.context = ev.Event.Usage.context()
	}
	return true
}

func (t *claudeTally) usage() *Usage {
	if len(t.order) == 0 {
		return nil
	}
	u := &Usage{ContextTokens: t.context}
	for _, id := range t.order {
		t.calls[id].add(u)
	}
	return u
}

// claudeLive keeps what the run page draws from Claude Code's stream: text
// and thinking deltas, each assistant message's blocks (without thinking
// signatures), tool results, and the result; it drops system, hook, and
// rate-limit lines, remembering the rate limits as Claude's quota, and adds
// a usage line after each model call.
func claudeLive() func([]byte) []byte {
	tally := claudeTally{}
	return func(line []byte) []byte {
		var ev claudeEvent
		if json.Unmarshal(line, &ev) != nil {
			return withNewline(line)
		}
		switch ev.Type {
		case "stream_event":
			if tally.read(ev) {
				return usageLine(tally.usage())
			}
			if ev.Event.Type != "content_block_delta" {
				return nil
			}
			switch ev.Event.Delta.Type {
			case "text_delta":
				return logAs(map[string]any{"type": "stream_event", "event": map[string]any{
					"type": "content_block_delta", "delta": map[string]any{"type": "text_delta", "text": ev.Event.Delta.Text}}})
			case "thinking_delta":
				return logAs(map[string]any{"type": "stream_event", "event": map[string]any{
					"type": "content_block_delta", "delta": map[string]any{"type": "thinking_delta", "thinking": ev.Event.Delta.Thinking}}})
			}
			return nil
		case "assistant", "user":
			if ev.Parent != "" {
				return nil // a subagent's own conversation
			}
			parts := make([]map[string]any, 0, len(ev.Message.Content))
			for _, raw := range ev.Message.Content {
				var p map[string]any
				if json.Unmarshal(raw, &p) != nil {
					continue
				}
				delete(p, "signature")
				delete(p, "caller")
				parts = append(parts, p)
			}
			return logAs(map[string]any{"type": ev.Type, "message": map[string]any{"role": ev.Message.Role, "content": parts}})
		case "result":
			return logAs(map[string]any{"type": "result", "subtype": ev.Subtype, "is_error": ev.IsError, "result": ev.Result})
		case "rate_limit_event":
			if ev.Limits != nil {
				rememberClaudeLimits(ev.Limits.samples(time.Now().UTC()))
			}
			return nil
		case "system":
			return nil
		}
		return withNewline(line)
	}
}

// withNewline is a copy of line ending in a newline.
func withNewline(line []byte) []byte {
	return append(append(make([]byte, 0, len(line)+1), line...), '\n')
}
