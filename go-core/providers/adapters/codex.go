package adapters

import (
	"bufio"
	"context"
	"encoding/json"
	"fmt"
	"os/exec"
	"time"

	"github.com/polterware/dispatch/go-core/providers/sandbox"
)

// Codex drives the OpenAI Codex CLI over its official `codex app-server`
// JSON-RPC protocol: runs (codex_run.go), quota, and model discovery.
type Codex struct{}

func (Codex) Name() string   { return "codex" }
func (Codex) Binary() string { return Bin("codex") }
func (Codex) Capabilities() Capabilities {
	return Capabilities{EffortMode: "config", Efforts: []string{"low", "medium", "high", "xhigh"}, SessionResume: true, StructuredOutput: true, ReadOnly: true, FileEditing: true, Usage: true}
}
func (Codex) UsageSource() string { return SourceCodexAppServer }

func (Codex) Probe(ctx context.Context) ProbeReport {
	return probeBinary(ctx, "codex", Bin("codex"), Codex{}.Capabilities(), codexProbeFlags)
}

func (Codex) Resume(ctx context.Context, req Request, model string) (Result, error) {
	if req.ResumeSession == "" {
		return Result{}, fmt.Errorf("codex: resume requires a session")
	}
	return Codex{}.Run(ctx, req, model)
}

// Flags codex exec puts on the command line. -c carries the read-only sandbox.
var codexProbeFlags = []probeFlag{
	{Name: "exec"},
	{Name: "resume", Capability: "sessionResume", Sub: "exec"},
	{Name: "app-server", Capability: "usage"},
	{Name: "--json", Capability: "structuredOutput", Sub: "exec"},
	{Name: "--skip-git-repo-check", Sub: "exec"},
	{Name: "-o", Sub: "exec"},
	{Name: "-m", Sub: "exec"},
	{Name: "-c", Capability: "readOnly", Sub: "exec"},
}

type rpcMsg struct {
	ID     *int            `json:"id,omitempty"`
	Result json.RawMessage `json:"result,omitempty"`
	Error  *struct {
		Message string `json:"message"`
	} `json:"error,omitempty"`
}

// appServerCall performs initialize + one request against a short-lived
// `codex app-server` over stdio and returns the raw result.
func appServerCall(ctx context.Context, method string, params any) (json.RawMessage, error) {
	ctx, cancel := context.WithTimeout(ctx, 20*time.Second)
	defer cancel()
	cmd := exec.CommandContext(ctx, Bin("codex"), "-s", "read-only", "-a", "never", "app-server")
	cmd.Env = sandbox.Environ()
	stdin, err := cmd.StdinPipe()
	if err != nil {
		return nil, err
	}
	stdout, err := cmd.StdoutPipe()
	if err != nil {
		return nil, err
	}
	if err := cmd.Start(); err != nil {
		return nil, err
	}
	defer func() { stdin.Close(); cmd.Process.Kill(); cmd.Wait() }()

	enc := json.NewEncoder(stdin)
	enc.Encode(map[string]any{"jsonrpc": "2.0", "id": 1, "method": "initialize",
		"params": map[string]any{"clientInfo": map[string]string{"name": "dispatch", "version": "0.1.0"}}})
	enc.Encode(map[string]any{"jsonrpc": "2.0", "method": "initialized"})
	req := map[string]any{"jsonrpc": "2.0", "id": 2, "method": method}
	if params != nil {
		req["params"] = params
	}
	enc.Encode(req)

	sc := bufio.NewScanner(stdout)
	sc.Buffer(make([]byte, 1<<20), 8<<20)
	for sc.Scan() {
		var m rpcMsg
		if json.Unmarshal(sc.Bytes(), &m) != nil || m.ID == nil || *m.ID != 2 {
			continue
		}
		if m.Error != nil {
			return nil, fmt.Errorf("codex app-server: %s", m.Error.Message)
		}
		return m.Result, nil
	}
	if err := ctx.Err(); err != nil {
		return nil, err
	}
	return nil, fmt.Errorf("codex app-server closed without a response")
}

type codexWindow struct {
	UsedPercent        float64 `json:"usedPercent"`
	WindowDurationMins *int    `json:"windowDurationMins"`
	ResetsAt           *int64  `json:"resetsAt"`
}

func readCodexLimits(ctx context.Context) (primary, secondary *codexWindow, err error) {
	raw, err := appServerCall(ctx, "account/rateLimits/read", nil)
	if err != nil {
		return nil, nil, err
	}
	var res struct {
		RateLimits struct {
			Primary   *codexWindow `json:"primary"`
			Secondary *codexWindow `json:"secondary"`
		} `json:"rateLimits"`
	}
	if err := json.Unmarshal(raw, &res); err != nil {
		return nil, nil, err
	}
	return res.RateLimits.Primary, res.RateLimits.Secondary, nil
}

// Quota prefers the secondary (weekly) window and falls back to primary.
// An incomplete secondary window is an error, not a silent fallback.
func (Codex) Quota(ctx context.Context) (Quota, error) {
	p, s, err := readCodexLimits(ctx)
	if err != nil {
		return Quota{}, fmt.Errorf("%w: %v", ErrNoQuota, err)
	}
	return codexQuota(p, s)
}

func codexQuota(primary, secondary *codexWindow) (Quota, error) {
	w := secondary
	if w == nil {
		w = primary
	}
	if w == nil || w.WindowDurationMins == nil || w.ResetsAt == nil {
		return Quota{}, ErrNoQuota
	}
	return Quota{
		Key:           "codex",
		UsedPercent:   w.UsedPercent,
		WindowMinutes: *w.WindowDurationMins,
		ResetsAt:      time.Unix(*w.ResetsAt, 0),
	}, nil
}

// Usage returns every complete official window. It does not invent a percent.
func (Codex) Usage(ctx context.Context) ([]QuotaSample, error) {
	p, s, err := readCodexLimits(ctx)
	if err != nil {
		return nil, fmt.Errorf("%w: %v", ErrNoQuota, err)
	}
	return codexSamples(p, s)
}

func codexSamples(primary, secondary *codexWindow) ([]QuotaSample, error) {
	var out []QuotaSample
	if q, ok := codexSample("primary", primary); ok {
		out = append(out, q)
	}
	if q, ok := codexSample("secondary", secondary); ok {
		out = append(out, q)
	}
	if len(out) == 0 {
		return nil, ErrNoQuota
	}
	return out, nil
}

func codexSample(window string, w *codexWindow) (QuotaSample, bool) {
	if w == nil || w.WindowDurationMins == nil || w.ResetsAt == nil {
		return QuotaSample{}, false
	}
	used := w.UsedPercent
	return QuotaSample{
		Window:     window,
		Used:       &used,
		ResetAt:    time.Unix(*w.ResetsAt, 0).UTC(),
		Duration:   Span(time.Duration(*w.WindowDurationMins) * time.Minute),
		Source:     SourceCodexAppServer,
		Confidence: ConfidenceOfficial,
	}, true
}

func (Codex) Models(ctx context.Context) ([]Model, error) {
	raw, err := appServerCall(ctx, "model/list", map[string]any{})
	if err != nil {
		return nil, err
	}
	var res struct {
		Data []struct {
			ID    string `json:"id"`
			Model string `json:"model"`
		} `json:"data"`
	}
	if err := json.Unmarshal(raw, &res); err != nil {
		return nil, err
	}
	var out []Model
	for _, m := range res.Data {
		id := m.Model
		if id == "" {
			id = m.ID
		}
		// Tier assignment lives in the declarative tier table, not here.
		out = append(out, Model{ID: id})
	}
	return out, nil
}
