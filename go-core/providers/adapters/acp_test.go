package adapters

import (
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

// fakeACP answers the handshake, a session, its setup calls, and one prompt
// the way an ACP agent does: chunks, a tool call, a permission request with a
// string id, a usage_update, and the prompt's usage. It records what dispatch
// sent it.
const fakeACP = `#!/bin/sh
[ "$1" = "acp" ] || exit 2
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$SENT"
  id=$(printf '%s' "$line" | sed -n 's/^{"id":\([0-9]*\),.*/\1/p')
  case "$line" in
  *'"method":"initialize"'*)
    echo '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1,"agentCapabilities":{"loadSession":true}}}' ;;
  *'"method":"session/new"'*)
    echo '{"jsonrpc":"2.0","id":2,"result":{"sessionId":"s-1","models":{"availableModels":[{"modelId":"m-a"},{"modelId":"m-b"}]},"modes":{"availableModes":[{"id":"build"},{"id":"plan"}]},"configOptions":[{"id":"effort","category":"thought_level","options":[{"value":"low"},{"value":"high"}]}]}}' ;;
  *'"method":"session/load"'*)
    echo '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s-0","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"OLD TURN"}}}}'
    echo '{"jsonrpc":"2.0","id":2,"result":{"modes":{"availableModes":[{"id":"plan"}]}}}' ;;
  *'"method":"session/set_'*)
    echo "{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{}}" ;;
  *'"method":"session/prompt"'*)
    echo "$id" > "$SENT.prompt"
    echo '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s-1","update":{"sessionUpdate":"available_commands_update","availableCommands":[]}}}'
    echo '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s-1","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"Looking"}}}}'
    echo '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s-1","update":{"sessionUpdate":"tool_call","toolCallId":"t1","title":"read","kind":"read","rawInput":{"filePath":"/repo/README.md"}}}}'
    echo '{"jsonrpc":"2.0","id":"p-1","method":"session/request_permission","params":{"sessionId":"s-1","toolCall":{"toolCallId":"t2","kind":"edit","title":"Edit README.md"},"options":[{"optionId":"ok","kind":"allow_once"},{"optionId":"no","kind":"reject_once"}]}}' ;;
  *'"id":"p-1"'*)
    prompt=$(cat "$SENT.prompt")
    echo '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s-1","update":{"sessionUpdate":"agent_message_chunk","messageId":"m2","content":{"type":"text","text":"Done."}}}}'
    echo '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s-1","update":{"sessionUpdate":"usage_update","used":5000,"size":200000,"cost":{"amount":0.25,"currency":"USD"}}}}'
    echo "{\"jsonrpc\":\"2.0\",\"id\":$prompt,\"result\":{\"stopReason\":\"${STOP:-end_turn}\",\"usage\":{\"inputTokens\":1000,\"outputTokens\":50,\"cachedReadTokens\":4000,\"thoughtTokens\":10,\"totalTokens\":5060}}}" ;;
  esac
done
`

func installFakeACP(t *testing.T, agent string) (sent string) {
	t.Helper()
	dir := t.TempDir()
	bin := filepath.Join(dir, agent)
	if err := os.WriteFile(bin, []byte(fakeACP), 0o755); err != nil {
		t.Fatal(err)
	}
	SetBinaries(map[string]string{agent: bin})
	t.Cleanup(func() { SetBinaries(nil) })
	sent = filepath.Join(dir, "sent.jsonl")
	t.Setenv("SENT", sent)
	return sent
}

func TestACPRunStreamsAReadOnlyTurn(t *testing.T) {
	sent := installFakeACP(t, "opencode")
	logPath := filepath.Join(t.TempDir(), "run.agent.log")
	res, err := OpenCode().Run(context.Background(), Request{
		Prompt: "look", Effort: "high", ReadOnly: true, Dir: t.TempDir(), LogPath: logPath, Env: os.Environ(),
	}, "m-a")
	if err != nil {
		t.Fatal(err)
	}
	if res.Output != "Done." || res.SessionID != "s-1" || res.ExitCode != 0 {
		t.Fatalf("%+v", res)
	}
	cost := 0.25
	if res.Usage == nil || res.Usage.CostUSD == nil || *res.Usage.CostUSD != cost {
		t.Fatalf("usage %+v", res.Usage)
	}
	u := *res.Usage
	u.CostUSD = nil
	if u != (Usage{InputTokens: 1000, CacheReadTokens: 4000, OutputTokens: 60, ReasoningTokens: 10, ContextTokens: 5000, ContextWindow: 200000}) {
		t.Fatalf("usage %+v", u)
	}

	log, _ := os.ReadFile(logPath)
	for _, want := range []string{`"sessionUpdate":"agent_message_chunk"`, `"text":"Looking"`, `"sessionUpdate":"tool_call"`,
		"[note] read-only run: refused edit Edit README.md", `"dispatch":"usage"`, `"context":5000`, `"window":200000`} {
		if !strings.Contains(string(log), want) {
			t.Fatalf("log lacks %s:\n%s", want, log)
		}
	}
	for _, quiet := range []string{"available_commands_update", `"sessionUpdate":"usage_update"`, `"sessionId"`} {
		if strings.Contains(string(log), quiet) {
			t.Fatalf("log keeps %s:\n%s", quiet, log)
		}
	}

	req, _ := os.ReadFile(sent)
	for _, want := range []string{`"method":"session/new"`, `"method":"session/set_model"`, `"modelId":"m-a"`,
		`"configId":"effort","sessionId":"s-1","value":"high"`, `"method":"session/set_mode"`, `"modeId":"plan"`,
		`"text":"look"`, `{"id":"p-1"`, `"optionId":"no"`} {
		if !strings.Contains(string(req), want) {
			t.Fatalf("sent lacks %s:\n%s", want, req)
		}
	}
}

func TestACPRunResumesAndReportsARefusal(t *testing.T) {
	sent := installFakeACP(t, "opencode")
	t.Setenv("STOP", "refusal")
	logPath := filepath.Join(t.TempDir(), "run.agent.log")
	res, err := OpenCode().(Resumer).Resume(context.Background(), Request{
		Prompt: "again", ReadOnly: true, Dir: t.TempDir(), ResumeSession: "s-0", LogPath: logPath, Env: os.Environ(),
	}, "")
	if err == nil || !strings.Contains(err.Error(), "refused") {
		t.Fatalf("a refusal must fail the run: %v", err)
	}
	if res.SessionID != "s-0" || res.Usage == nil || res.Usage.CostUSD != nil {
		t.Fatalf("a resumed session's cumulative cost must not be its turn's: %+v", res.Usage)
	}
	log, _ := os.ReadFile(logPath)
	if strings.Contains(string(log), "OLD TURN") {
		t.Fatalf("session/load's replay must stay out of the log:\n%s", log)
	}
	req, _ := os.ReadFile(sent)
	if !strings.Contains(string(req), `"method":"session/load"`) || !strings.Contains(string(req), `"sessionId":"s-0"`) {
		t.Fatalf("%s", req)
	}
}

func TestACPRunRefusesAnUnlistedModelAndAMissingReadMode(t *testing.T) {
	installFakeACP(t, "opencode")
	if _, err := OpenCode().Run(context.Background(), Request{Prompt: "p", Dir: t.TempDir(), Env: os.Environ()}, "m-z"); err == nil || !strings.Contains(err.Error(), `"m-z" is not offered`) {
		t.Fatalf("%v", err)
	}
	strict := acpAdapter{spec: acpSpec{name: "opencode", command: func(bool) []string { return []string{"acp"} }, readMode: "ask"}}
	if _, err := strict.Run(context.Background(), Request{Prompt: "p", ReadOnly: true, Dir: t.TempDir(), Env: os.Environ()}, ""); err == nil || !strings.Contains(err.Error(), "no ask mode") {
		t.Fatalf("%v", err)
	}
	if _, err := OpenCode().Run(context.Background(), Request{Prompt: "p", Dir: t.TempDir()}, "claude-opus-5-5"); err == nil {
		t.Fatal("claude models stay in Claude's own CLI")
	}
}

func TestACPModelsFromTheSession(t *testing.T) {
	installFakeACP(t, "kimi")
	kimi := acpAdapter{spec: acpSpec{name: "kimi", command: func(bool) []string { return []string{"acp"} }}}
	ids, err := kimi.listSessionModels(context.Background())
	if err != nil || strings.Join(ids, ",") != "m-a,m-b" {
		t.Fatalf("%v %v", ids, err)
	}
}

func TestCustomAgents(t *testing.T) {
	t.Cleanup(func() { SetCustomAgents(nil) })
	if ValidCustomAgent("codex", []string{"x"}) == nil || ValidCustomAgent("Bad Name", []string{"x"}) == nil || ValidCustomAgent("mine", nil) == nil {
		t.Fatal("ValidCustomAgent")
	}
	SetCustomAgents(map[string][]string{"mine": {"/opt/mine/bin/mine", "serve", "--acp"}, "codex": {"x"}})
	var found Adapter
	for _, a := range Known() {
		if a.Name() == "codex" && a.Binary() == "x" {
			t.Fatal("an added agent must not replace a built-in one")
		}
		if a.Name() == "mine" {
			found = a
		}
	}
	if found == nil || found.Binary() != "/opt/mine/bin/mine" {
		t.Fatalf("%v", found)
	}
	if got := strings.Join(found.(acpAdapter).spec.command(false), " "); got != "serve --acp" {
		t.Fatal(got)
	}
}
