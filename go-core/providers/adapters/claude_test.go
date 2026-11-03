package adapters

import (
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

// fakePrintCLI records its arguments and stdin, then prints a recorded stream.
const fakePrintCLI = `#!/bin/sh
printf '%s\n' "$@" > "$SENT"
cat > "$SENT.stdin"
cat "$STREAM"
`

func installFakePrintCLI(t *testing.T, agent, stream string) (sent string) {
	t.Helper()
	dir := t.TempDir()
	bin := filepath.Join(dir, agent)
	if err := os.WriteFile(bin, []byte(fakePrintCLI), 0o755); err != nil {
		t.Fatal(err)
	}
	SetBinaries(map[string]string{agent: bin})
	t.Cleanup(func() { SetBinaries(nil) })
	sent = filepath.Join(dir, "sent.txt")
	t.Setenv("SENT", sent)
	abs, err := filepath.Abs(stream)
	if err != nil {
		t.Fatal(err)
	}
	t.Setenv("STREAM", abs)
	return sent
}

func TestClaudeRunReadsItsStream(t *testing.T) {
	sent := installFakePrintCLI(t, "claude", "testdata/claude/stream.ndjson")
	state := t.TempDir()
	SetStateDir(state)
	t.Cleanup(func() { SetStateDir("") })
	dir := t.TempDir()
	logPath := filepath.Join(t.TempDir(), "run.agent.log")
	res, err := Claude{}.Run(context.Background(), Request{
		Prompt: "Read README.md", Effort: "high", ReadOnly: true, Dir: dir, LogPath: logPath, Env: os.Environ(),
	}, "claude-opus-5-5")
	if err != nil {
		t.Fatal(err)
	}
	if !strings.HasPrefix(res.Output, `The README has a "Probe" heading`) || res.SessionID != "928305c0-0399-49d0-aa60-e86bf07821e2" {
		t.Fatalf("%+v", res)
	}
	u := res.Usage
	if u == nil || u.CostUSD == nil || *u.CostUSD < 0.3007 || *u.CostUSD > 0.3008 {
		t.Fatalf("Claude Code's own price wins: %+v", u)
	}
	got := *u
	got.CostUSD = nil
	if got != (Usage{InputTokens: 4, CacheReadTokens: 63139, CacheWriteTokens: 35541, OutputTokens: 190, ReasoningTokens: 23, ContextTokens: 56842, ContextWindow: 1000000}) {
		t.Fatalf("usage %+v", got)
	}

	args, _ := os.ReadFile(sent)
	for _, want := range []string{"-p\n", "--output-format\nstream-json\n", "--include-partial-messages\n", "--permission-mode\nplan\n", "--model\nclaude-opus-5-5\n", "--effort\nhigh\n"} {
		if !strings.Contains(string(args), want) {
			t.Fatalf("args lack %q:\n%s", want, args)
		}
	}
	if stdin, _ := os.ReadFile(sent + ".stdin"); string(stdin) != "Read README.md" {
		t.Fatalf("the prompt goes on stdin: %q", stdin)
	}

	log, _ := os.ReadFile(logPath)
	for _, want := range []string{`"type":"text_delta"`, `"type":"tool_use"`, `"type":"tool_result"`, `"type":"result"`, `"dispatch":"usage"`, `"context":56842`} {
		if !strings.Contains(string(log), want) {
			t.Fatalf("log lacks %s:\n%s", want, log)
		}
	}
	for _, quiet := range []string{"rate_limit_event", `"signature"`, `"subtype":"init"`, "message_start", "tool_use_result"} {
		if strings.Contains(string(log), quiet) {
			t.Fatalf("log keeps %s:\n%s", quiet, log)
		}
	}

	// The run's rate_limit_event is Claude's quota, and a restart keeps it.
	SetStateDir(state)
	before := time.Unix(1793580600, 0).Add(-time.Hour)
	seen := claudeObserved(before)
	if len(seen) != 2 || seen[0].Window != "five_hour" || *seen[0].Used != 24 || seen[1].Window != "seven_day" || *seen[1].Used != 51 ||
		seen[1].Duration.Duration() != 7*24*time.Hour || seen[1].Source != SourceClaudeStream || seen[1].SampledAt.IsZero() {
		t.Fatalf("%+v", seen)
	}
	if q, err := claudeQuota(seen); err != nil || q.UsedPercent != 51 || q.WindowMinutes != 7*24*60 || q.Key != "claude" || !q.SampledAt.Equal(seen[1].SampledAt) {
		t.Fatalf("the window closest to its limit: %+v %v", q, err)
	}
	if after := claudeObserved(time.Unix(1793580600, 0).Add(time.Minute)); len(after) != 1 || after[0].Window != "seven_day" {
		t.Fatalf("a window that reset says nothing any more: %+v", after)
	}
	if _, err := claudeQuota(claudeObserved(time.Unix(1793772000, 0))); err != ErrNoQuota {
		t.Fatalf("with every window reset the quota is unknown: %v", err)
	}
}

func TestClaudeUsageCountsEveryModel(t *testing.T) {
	result := `{"type":"result","subtype":"success","result":"ok","total_cost_usd":0.5,` +
		`"usage":{"input_tokens":10,"cache_read_input_tokens":100,"cache_creation_input_tokens":20,"output_tokens":30,"iterations":[{"input_tokens":10,"cache_read_input_tokens":100,"cache_creation_input_tokens":20,"output_tokens":30}]},` +
		`"modelUsage":{"claude-opus-5-5":{"inputTokens":10,"outputTokens":30,"cacheReadInputTokens":100,"cacheCreationInputTokens":20,"thinkingTokens":5,"contextWindow":1000000},` +
		`"claude-haiku-5-5":{"inputTokens":7,"outputTokens":3,"cacheReadInputTokens":50,"cacheCreationInputTokens":0,"thinkingTokens":0,"contextWindow":200000}}}`
	u := usageClaude(result + "\n")
	if u == nil || u.CostUSD == nil || *u.CostUSD != 0.5 {
		t.Fatalf("%+v", u)
	}
	got := *u
	got.CostUSD = nil
	if got != (Usage{InputTokens: 17, CacheReadTokens: 150, CacheWriteTokens: 20, OutputTokens: 33, ReasoningTokens: 5, ContextTokens: 160, ContextWindow: 1000000}) {
		t.Fatalf("a subagent's model counts too, the context stays the main thread's: %+v", got)
	}
}

func TestClaudeUsageOfACutOffRun(t *testing.T) {
	b, err := os.ReadFile("testdata/claude/stream.ndjson")
	if err != nil {
		t.Fatal(err)
	}
	var cut strings.Builder
	for _, l := range lines(string(b)) {
		if !strings.Contains(l, `"total_cost_usd"`) { // the result event
			cut.WriteString(l + "\n")
		}
	}
	u := usageClaude(cut.String())
	if u == nil || *u != (Usage{InputTokens: 4, CacheReadTokens: 63139, CacheWriteTokens: 35541, OutputTokens: 190, ReasoningTokens: 23, ContextTokens: 56842}) {
		t.Fatalf("without the result each call's final usage adds up, unpriced: %+v", u)
	}
	if !AllowedModel("claude", "claude-opus-5-5") || AllowedModel("codex", "claude-opus-5-5") {
		t.Fatal("AllowedModel")
	}
}

const piStream = `{"type":"session","version":3,"id":"pi-1","cwd":"/repo"}
{"type":"agent_start"}
{"type":"message_start","message":{"role":"user","content":[{"type":"text","text":"Read README.md"}]}}
{"type":"message_update","assistantMessageEvent":{"type":"thinking_delta","delta":"Reading."},"message":{"role":"assistant","content":[{"type":"thinking","thinking":"Reading."}]}}
{"type":"message_end","message":{"role":"assistant","content":[{"type":"toolCall","id":"c1","name":"read","arguments":{"path":"README.md"}}],"stopReason":"toolUse","usage":{"input":1200,"output":30,"cacheRead":0,"cacheWrite":800,"totalTokens":2030,"cost":{"total":0.004}}}}
{"type":"tool_execution_start","toolCallId":"c1","toolName":"read","args":{"path":"README.md"}}
{"type":"tool_execution_update","toolCallId":"c1","partialResult":{"content":[{"type":"text","text":"# Pro"}]}}
{"type":"tool_execution_end","toolCallId":"c1","toolName":"read","result":{"content":[{"type":"text","text":"# Probe"}]},"isError":false}
{"type":"message_update","assistantMessageEvent":{"type":"text_delta","delta":"A heading."},"message":{"role":"assistant","content":[{"type":"text","text":"A heading."}]}}
{"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"A heading."}],"stopReason":"stop","usage":{"input":40,"output":12,"cacheRead":2000,"cacheWrite":0,"totalTokens":2052,"cost":{"total":0.001}}}}
{"type":"agent_end"}
`

func TestPiRunReadsItsStream(t *testing.T) {
	streamFile := filepath.Join(t.TempDir(), "pi.ndjson")
	if err := os.WriteFile(streamFile, []byte(piStream), 0o644); err != nil {
		t.Fatal(err)
	}
	sent := installFakePrintCLI(t, "pi", streamFile)
	logPath := filepath.Join(t.TempDir(), "run.agent.log")
	res, err := Pi{}.Run(context.Background(), Request{
		Prompt: "Read README.md", Effort: "low", ReadOnly: true, Dir: t.TempDir(), LogPath: logPath, Env: os.Environ(),
	}, "openai/gpt-x")
	if err != nil {
		t.Fatal(err)
	}
	if res.Output != "A heading." || res.SessionID != "pi-1" {
		t.Fatalf("%+v", res)
	}
	u := res.Usage
	if u == nil || u.CostUSD == nil || *u.CostUSD < 0.00499 || *u.CostUSD > 0.00501 {
		t.Fatalf("Pi's own price wins: %+v", u)
	}
	got := *u
	got.CostUSD = nil
	if got != (Usage{InputTokens: 1240, CacheReadTokens: 2000, CacheWriteTokens: 800, OutputTokens: 42, ContextTokens: 2052}) {
		t.Fatalf("usage %+v", got)
	}
	args, _ := os.ReadFile(sent)
	for _, want := range []string{"--mode\njson\n", "--session-id\n", "--model\nopenai/gpt-x\n", "--thinking\nlow\n", "--tools\nread,grep,find,ls\n", "--\nRead README.md\n"} {
		if !strings.Contains(string(args), want) {
			t.Fatalf("args lack %q:\n%s", want, args)
		}
	}
	log, _ := os.ReadFile(logPath)
	for _, want := range []string{`"delta":"A heading."`, `"type":"tool_execution_start"`, `"type":"tool_execution_end"`, `"dispatch":"usage"`} {
		if !strings.Contains(string(log), want) {
			t.Fatalf("log lacks %s:\n%s", want, log)
		}
	}
	for _, quiet := range []string{"tool_execution_update", "agent_start", `"content":[{"type":"thinking"`} {
		if strings.Contains(string(log), quiet) {
			t.Fatalf("log keeps %s:\n%s", quiet, log)
		}
	}
	if got := parsePiModels("provider  model        context  max-out  thinking  images\nopenai    gpt-x        400K     128K     yes       yes\n"); strings.Join(got, ",") != "openai/gpt-x" {
		t.Fatal(got)
	}
}
