package main

import (
	"context"
	"encoding/json"
	"errors"
	"os"
	"os/exec"
	"path/filepath"
	"reflect"
	"strings"
	"testing"
	"time"

	"github.com/polterware/dispatch/go-core/domains/dispatch"
	"github.com/polterware/dispatch/go-core/providers/hook"
)

func TestDispatchCallerEnvironmentOrder(t *testing.T) {
	tests := []struct {
		name           string
		env            map[string]string
		agent, session string
	}{
		{"dispatch", map[string]string{envCallerSession: "chat", envCallerAgent: "claude-code", envSession: "pw", envAgentLabel: "claude", "CODEX_THREAD_ID": "thread"}, "claude-code", "chat"},
		{"polterware", map[string]string{envCallerSession: "incomplete", envSession: "pw", envAgentLabel: "claude", "CODEX_THREAD_ID": "thread"}, "claude", "pw"},
		{"codex thread", map[string]string{"CODEX_THREAD_ID": "thread", "CODEX_SESSION_ID": "session"}, "codex", "thread"},
		{"codex session", map[string]string{"CODEX_SESSION_ID": "session"}, "codex", "session"},
		{"opencode session", map[string]string{"OPENCODE_SESSION_ID": "ses_chat"}, "opencode", "ses_chat"},
		{"plugin over opencode shell", map[string]string{envCallerSession: "ses_root", envCallerAgent: "opencode", "OPENCODE_SESSION_ID": "ses_child"}, "opencode", "ses_root"},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			lookup := func(context.Context, int) (callerProcess, error) {
				t.Fatal("environment identity must skip process lookup")
				return callerProcess{}, nil
			}
			got := resolveDispatchCaller(context.Background(), func(k string) string { return tt.env[k] }, 20, lookup, dispatch.Policy{})
			if got.Agent != tt.agent || got.SessionID != tt.session || got.ConversationID != tt.agent+":"+tt.session || got.Label != tt.agent {
				t.Fatalf("caller: %+v", got)
			}
		})
	}
}

func TestDispatchCallerProcessWalk(t *testing.T) {
	started := time.Unix(1793630000, 0)
	for _, tt := range []struct {
		name   string
		args   []string
		policy dispatch.Policy
		agent  string
	}{
		{"registry", []string{"/opt/bin/codex", "--model", "x"}, dispatch.Policy{}, "codex"},
		{"cursor alias", []string{"/bin/agent"}, dispatch.Policy{}, "cursor"},
		{"node script", []string{"/bin/node", "/bin/claude"}, dispatch.Policy{}, "claude"},
		{"bun script", []string{"bun", "/bin/grok"}, dispatch.Policy{}, "grok"},
		{"deno script", []string{"deno", "/bin/pi"}, dispatch.Policy{}, "pi"},
		{"python script", []string{"python", "/bin/qwen"}, dispatch.Policy{}, "qwen"},
		{"configured", []string{"/other/codex-custom"}, dispatch.Policy{Binaries: map[string]string{"codex": "/opt/codex-custom"}}, "codex"},
		{"custom ACP", []string{"node", "/opt/local-agent.js", "acp"}, dispatch.Policy{ACPAgents: map[string][]string{"custom": {"node", "/opt/local-agent.js", "acp"}}}, "custom"},
	} {
		t.Run(tt.name, func(t *testing.T) {
			var visited []int
			lookup := func(_ context.Context, pid int) (callerProcess, error) {
				visited = append(visited, pid)
				switch pid {
				case 30:
					return callerProcess{Parent: 20, Started: started, Args: []string{"/bin/zsh", "-c", "codex"}}, nil
				case 20:
					return callerProcess{Parent: 10, Started: started, Args: tt.args}, nil
				default:
					t.Fatal("walk must stop at nearest agent")
					return callerProcess{}, nil
				}
			}
			got := resolveDispatchCaller(context.Background(), func(string) string { return "" }, 30, lookup, tt.policy)
			if got.Agent != tt.agent || got.ConversationID != tt.agent+":pid-20-1793630000" || !reflect.DeepEqual(visited, []int{30, 20}) {
				t.Fatalf("caller=%+v visited=%v", got, visited)
			}
		})
	}
}

func TestDispatchCallerNone(t *testing.T) {
	for _, tt := range []struct {
		name   string
		lookup callerProcessLookup
	}{
		{"no agent", func(context.Context, int) (callerProcess, error) {
			return callerProcess{Parent: 1, Args: []string{"sh", "codex"}}, nil
		}},
		{"lookup failed", func(context.Context, int) (callerProcess, error) { return callerProcess{}, errors.New("unavailable") }},
		{"cycle", func(context.Context, int) (callerProcess, error) {
			return callerProcess{Parent: 20, Args: []string{"zsh"}}, nil
		}},
		{"unknown start", func(context.Context, int) (callerProcess, error) {
			return callerProcess{Parent: 1, Args: []string{"codex"}}, nil
		}},
	} {
		t.Run(tt.name, func(t *testing.T) {
			if got := resolveDispatchCaller(context.Background(), func(string) string { return "" }, 20, tt.lookup, dispatch.Policy{}); got != (dispatch.Caller{}) {
				t.Fatalf("caller=%+v", got)
			}
		})
	}
}

func TestParseCallerProcess(t *testing.T) {
	got, err := parseCallerProcess(` 12 Mon Nov  2 13:04:05 2026 /usr/bin/node '/opt/agent files/claude' --flag`)
	if err != nil || got.Parent != 12 || got.Started.Year() != 2026 || !reflect.DeepEqual(got.Args, []string{"/usr/bin/node", "/opt/agent files/claude", "--flag"}) {
		t.Fatalf("process=%+v error=%v", got, err)
	}
	for _, bad := range []string{"", "x Mon Nov 2 13:04:05 2026 codex", "12 invalid date data here now codex"} {
		if _, err := parseCallerProcess(bad); err == nil {
			t.Fatalf("accepted %q", bad)
		}
	}
}

func TestWithCallerExecutable(t *testing.T) {
	exe := "/Users/me/Library/Application Support/Claude/claude.app/Contents/MacOS/claude"
	split := callerArgs(exe + " --output-format stream-json")
	if callerBinary(split) == "claude" {
		t.Fatalf("expected the unquoted path to split, got %q", split)
	}
	got := withCallerExecutable(split, exe)
	if !reflect.DeepEqual(got, []string{exe, "--output-format", "stream-json"}) || callerBinary(got) != "claude" {
		t.Fatalf("args=%q", got)
	}
	plain := []string{"/usr/bin/node", "/opt/bin/gemini"}
	if got := withCallerExecutable(plain, "/usr/bin/node"); !reflect.DeepEqual(got, plain) {
		t.Fatalf("args=%q", got)
	}
}

func TestDispatchHookCaller(t *testing.T) {
	envFile := filepath.Join(t.TempDir(), "env")
	if err := os.WriteFile(envFile, []byte("export EXISTING='kept'\n"), 0o600); err != nil {
		t.Fatal(err)
	}
	session := "chat'$(printf injected); with spaces"
	payload, _ := json.Marshal(map[string]string{"session_id": session})
	if err := recordDispatchHookCaller(strings.NewReader(string(payload)), false, envFile); err != nil {
		t.Fatal(err)
	}
	// Sourcing the file checks that apostrophes and shell expressions remain literal.
	cmd := exec.Command("/bin/sh", "-c", `. "$1"; printf '%s\n%s\n%s' "$EXISTING" "$DISPATCH_CALLER_SESSION" "$DISPATCH_CALLER_AGENT"`, "sh", envFile)
	output, err := cmd.Output()
	if err != nil || string(output) != "kept\n"+session+"\nclaude-code" {
		t.Fatalf("env=%q error=%v", output, err)
	}
	before, _ := os.ReadFile(envFile)
	for _, input := range []string{"", "{", `{}`, `{"session_id":3}`, `{"session_id":""}`, strings.Repeat("x", maxCallerHookInput+1)} {
		if err := recordDispatchHookCaller(strings.NewReader(input), false, envFile); err != nil {
			t.Fatal(err)
		}
	}
	if err := recordDispatchHookCaller(strings.NewReader(string(payload)), true, envFile); err != nil {
		t.Fatal(err)
	}
	after, _ := os.ReadFile(envFile)
	if string(before) != string(after) {
		t.Fatal("invalid or terminal input changed the file")
	}
}

func TestRunDispatchHookKeepsContext(t *testing.T) {
	dbPath := filepath.Join(t.TempDir(), "dispatch.db")
	if err := hook.SetEnabled(dispatchStateDir(dbPath), true); err != nil {
		t.Fatal(err)
	}
	input, err := os.CreateTemp(t.TempDir(), "stdin")
	if err != nil {
		t.Fatal(err)
	}
	defer input.Close()
	input.WriteString(`{"session_id":"hook-chat"}`)
	input.Seek(0, 0)
	envFile := filepath.Join(t.TempDir(), "env")
	t.Setenv("CLAUDE_ENV_FILE", envFile)
	old := os.Stdin
	os.Stdin = input
	t.Cleanup(func() { os.Stdin = old })
	capture, err := os.CreateTemp(t.TempDir(), "stdout")
	if err != nil {
		t.Fatal(err)
	}
	defer capture.Close()
	oldOut := os.Stdout
	os.Stdout = capture
	t.Cleanup(func() { os.Stdout = oldOut })
	if err := runDispatchHook(dbPath); err != nil {
		t.Fatal(err)
	}
	os.Stdout = oldOut
	captured, err := os.ReadFile(capture.Name())
	if err != nil {
		t.Fatal(err)
	}
	output := string(captured)
	if output != hook.ContextBlock() {
		t.Fatalf("context=%q", output)
	}
	data, err := os.ReadFile(envFile)
	if err != nil || !strings.Contains(string(data), "DISPATCH_CALLER_SESSION='hook-chat'") {
		t.Fatalf("env=%q error=%v", data, err)
	}
}
