package adapters

import (
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

// fakeAppServer answers the handshake and one turn the way `codex app-server`
// does, and records what dispatch sent it.
const fakeAppServer = `#!/bin/sh
[ "$1" = "app-server" ] || exit 2
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$SENT"
  case "$line" in
  *'"method":"initialize"'*) echo '{"id":1,"result":{}}' ;;
  *'"method":"thread/'*)
    echo '{"id":2,"result":{"thread":{"id":"th-1"}}}'
    echo '{"method":"mcpServer/startupStatus/updated","params":{"name":"x","status":"ready"}}' ;;
  *'"method":"turn/start"'*)
    echo '{"id":3,"result":{"turn":{"id":"tu-1","status":"inProgress"}}}'
    echo '{"method":"turn/started","params":{"threadId":"th-1","turn":{"id":"tu-1"}}}'
    echo '{"method":"item/started","params":{"threadId":"th-1","turnId":"tu-1","item":{"type":"userMessage","id":"u1"}}}'
    echo '{"method":"item/agentMessage/delta","params":{"threadId":"th-1","turnId":"tu-1","itemId":"m1","delta":"Hel"}}'
    echo '{"method":"item/completed","params":{"threadId":"th-1","turnId":"tu-1","item":{"type":"agentMessage","id":"m1","text":"Hello","phase":"commentary"}}}'
    echo '{"method":"item/started","params":{"threadId":"th-1","turnId":"tu-1","item":{"type":"commandExecution","id":"c1","command":"ls","status":"inProgress"}}}'
    echo '{"method":"thread/tokenUsage/updated","params":{"threadId":"th-1","turnId":"tu-1","tokenUsage":{"total":{"inputTokens":900},"last":{"inputTokens":1000,"cachedInputTokens":800,"outputTokens":50,"reasoningOutputTokens":20}}}}'
    echo '{"method":"item/completed","params":{"threadId":"th-1","turnId":"tu-1","item":{"type":"agentMessage","id":"m2","text":"Done.","phase":"final_answer"}}}'
    echo '{"method":"thread/tokenUsage/updated","params":{"threadId":"th-1","turnId":"tu-1","tokenUsage":{"last":{"inputTokens":500,"cachedInputTokens":100,"outputTokens":10},"modelContextWindow":272000}}}'
    echo "{\"method\":\"turn/completed\",\"params\":{\"threadId\":\"th-1\",\"turn\":{\"id\":\"tu-1\",\"status\":\"${STATUS:-completed}\"}}}" ;;
  esac
done
`

func installFakeAppServer(t *testing.T) (sent string) {
	t.Helper()
	dir := t.TempDir()
	bin := filepath.Join(dir, "codex")
	if err := os.WriteFile(bin, []byte(fakeAppServer), 0o755); err != nil {
		t.Fatal(err)
	}
	SetBinaries(map[string]string{"codex": bin})
	t.Cleanup(func() { SetBinaries(nil) })
	sent = filepath.Join(dir, "sent.jsonl")
	t.Setenv("SENT", sent)
	return sent
}

func TestCodexRunStreamsTheTurn(t *testing.T) {
	sent := installFakeAppServer(t)
	logPath := filepath.Join(t.TempDir(), "run.agent.log")
	res, err := Codex{}.Run(context.Background(), Request{
		Prompt: "do it", Effort: "high", ReadOnly: true, Dir: t.TempDir(), LogPath: logPath, Env: os.Environ(),
	}, "gpt-x")
	if err != nil {
		t.Fatal(err)
	}
	if res.Output != "Done." || res.SessionID != "th-1" || res.ExitCode != 0 {
		t.Fatalf("%+v", res)
	}
	if res.Usage == nil || *res.Usage != (Usage{InputTokens: 600, CacheReadTokens: 900, OutputTokens: 60, ReasoningTokens: 20, ContextTokens: 510, ContextWindow: 272000}) {
		t.Fatalf("usage %+v", res.Usage)
	}

	log, _ := os.ReadFile(logPath)
	for _, want := range []string{`"method":"item/agentMessage/delta"`, `"delta":"Hel"`, `"type":"commandExecution"`, `"method":"turn/completed"`, `"dispatch":"usage","input":600`, `"context":510`} {
		if !strings.Contains(string(log), want) {
			t.Fatalf("log lacks %s:\n%s", want, log)
		}
	}
	for _, quiet := range []string{"mcpServer", "tokenUsage", "userMessage", `"threadId"`, `"turnId"`} {
		if strings.Contains(string(log), quiet) {
			t.Fatalf("log keeps %s:\n%s", quiet, log)
		}
	}

	req, _ := os.ReadFile(sent)
	for _, want := range []string{`"method":"thread/start"`, `"sandbox":"read-only"`, `"approvalPolicy":"never"`, `"model":"gpt-x"`, `"effort":"high"`, `"text":"do it"`, `"threadId":"th-1"`} {
		if !strings.Contains(string(req), want) {
			t.Fatalf("requests lack %s:\n%s", want, req)
		}
	}
}

func TestCodexRunResumesAndReportsAFailedTurn(t *testing.T) {
	sent := installFakeAppServer(t)
	t.Setenv("STATUS", "failed")
	res, err := Codex{}.Resume(context.Background(), Request{Prompt: "again", ResumeSession: "th-1", Env: os.Environ()}, "")
	if err == nil || !strings.Contains(err.Error(), "failed") {
		t.Fatalf("err %v", err)
	}
	if res.SessionID != "th-1" {
		t.Fatalf("%+v", res)
	}
	req, _ := os.ReadFile(sent)
	if !strings.Contains(string(req), `"method":"thread/resume"`) || !strings.Contains(string(req), `"sandbox":"workspace-write"`) {
		t.Fatalf("requests:\n%s", req)
	}
}
