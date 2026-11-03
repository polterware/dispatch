package main

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"sort"
	"strconv"
	"strings"
	"time"

	"github.com/polterware/dispatch/go-core/domains/dispatch"
	"github.com/polterware/dispatch/go-core/providers/adapters"
	"golang.org/x/term"
)

const (
	envCallerSession   = "DISPATCH_CALLER_SESSION"
	envCallerAgent     = "DISPATCH_CALLER_AGENT"
	maxCallerHookInput = 1 << 20
)

type callerProcess struct {
	Parent  int
	Started time.Time
	Args    []string
}

type callerProcessLookup func(context.Context, int) (callerProcess, error)

func dispatchCaller(dbPath string) dispatch.Caller {
	policy, _ := dispatch.LoadConfig(dispatch.PolicyConfigPath(dispatchStateDir(dbPath)))
	ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
	defer cancel()
	return resolveDispatchCaller(ctx, os.Getenv, os.Getppid(), lookupCallerProcess, policy)
}

func resolveDispatchCaller(ctx context.Context, getenv func(string) string, parent int, lookup callerProcessLookup, policy dispatch.Policy) dispatch.Caller {
	caller := func(agent, session string) dispatch.Caller {
		return dispatch.Caller{Agent: agent, SessionID: session, ConversationID: agent + ":" + session, Label: agent}
	}
	for _, pair := range [][2]string{{envCallerAgent, envCallerSession}, {envAgentLabel, envSession}} {
		agent, session := strings.TrimSpace(getenv(pair[0])), strings.TrimSpace(getenv(pair[1]))
		if agent != "" && session != "" {
			return caller(agent, session)
		}
	}
	// Agents that export their own session id to the shells they run.
	for _, pair := range [][2]string{{"codex", "CODEX_THREAD_ID"}, {"codex", "CODEX_SESSION_ID"}, {"opencode", "OPENCODE_SESSION_ID"}} {
		if session := strings.TrimSpace(getenv(pair[1])); session != "" {
			return caller(pair[0], session)
		}
	}
	binaries := map[string]string{"agent": "cursor"}
	add := func(agent string, args []string) {
		if binary := callerBinary(args); binary != "" {
			binaries[binary] = agent
		}
	}
	for agent, binary := range adapters.DefaultBinaries() {
		add(agent, []string{binary})
	}
	// Stable ordering makes duplicate configured basenames deterministic.
	keys := make([]string, 0, len(policy.Binaries)+len(policy.ACPAgents))
	for agent := range policy.Binaries {
		keys = append(keys, agent)
	}
	sort.Strings(keys)
	for _, agent := range keys {
		add(agent, []string{policy.Binaries[agent]})
	}
	keys = keys[:0]
	for agent := range policy.ACPAgents {
		keys = append(keys, agent)
	}
	sort.Strings(keys)
	for _, agent := range keys {
		add(agent, policy.ACPAgents[agent])
	}
	seen := map[int]bool{}
	for pid := parent; pid > 1 && !seen[pid] && len(seen) < 128; {
		seen[pid] = true
		process, err := lookup(ctx, pid)
		if err != nil {
			break
		}
		if agent := binaries[callerBinary(process.Args)]; agent != "" && !process.Started.IsZero() {
			return caller(agent, fmt.Sprintf("pid-%d-%d", pid, process.Started.Unix()))
		}
		pid = process.Parent
	}
	return dispatch.Caller{}
}

func callerBinary(args []string) string {
	if len(args) == 0 {
		return ""
	}
	binary := filepath.Base(args[0])
	switch binary {
	case "node", "bun", "deno", "python", "python3":
		if len(args) < 2 {
			return ""
		}
		binary = filepath.Base(args[1])
	}
	return binary
}

func lookupCallerProcess(ctx context.Context, pid int) (callerProcess, error) {
	cmd := exec.CommandContext(ctx, "ps", "-o", "ppid=,lstart=,command=", "-p", strconv.Itoa(pid))
	cmd.Env = append(os.Environ(), "LC_ALL=C")
	out, err := cmd.Output()
	if err != nil {
		return callerProcess{}, err
	}
	process, err := parseCallerProcess(string(out))
	if err != nil {
		return process, err
	}
	// ps prints argv unquoted, so an executable under a path with spaces
	// ("Application Support") splits; comm holds the whole path on its line.
	exe := exec.CommandContext(ctx, "ps", "-o", "comm=", "-p", strconv.Itoa(pid))
	exe.Env = cmd.Env
	if comm, err := exe.Output(); err == nil {
		process.Args = withCallerExecutable(process.Args, strings.TrimSpace(string(comm)))
	}
	return process, nil
}

// withCallerExecutable puts the executable path back together as argv[0] when
// the unquoted command line split it.
func withCallerExecutable(args []string, exe string) []string {
	if exe == "" || len(args) == 0 || !strings.Contains(exe, " ") {
		return args
	}
	joined := strings.Join(args, " ")
	if !strings.HasPrefix(joined, exe) {
		return args
	}
	return append([]string{exe}, callerArgs(joined[len(exe):])...)
}

func parseCallerProcess(output string) (callerProcess, error) {
	fields := strings.Fields(output)
	if len(fields) < 7 {
		return callerProcess{}, fmt.Errorf("invalid process record")
	}
	parent, err := strconv.Atoi(fields[0])
	if err != nil {
		return callerProcess{}, err
	}
	started, err := time.ParseInLocation("Mon Jan 2 15:04:05 2006", strings.Join(fields[1:6], " "), time.Local)
	if err != nil {
		return callerProcess{}, err
	}
	return callerProcess{Parent: parent, Started: started, Args: callerArgs(strings.Join(fields[6:], " "))}, nil
}

// callerArgs reads quoted command words without evaluating any shell expressions.
func callerArgs(command string) []string {
	var out []string
	var word strings.Builder
	var quote rune
	escaped := false
	for _, c := range command {
		switch {
		case escaped:
			word.WriteRune(c)
			escaped = false
		case c == '\\' && quote != '\'':
			escaped = true
		case quote != 0:
			if c == quote {
				quote = 0
			} else {
				word.WriteRune(c)
			}
		case c == '\'' || c == '"':
			quote = c
		case c == ' ' || c == '\t':
			if word.Len() > 0 {
				out = append(out, word.String())
				word.Reset()
			}
		default:
			word.WriteRune(c)
		}
	}
	if escaped {
		word.WriteRune('\\')
	}
	if word.Len() > 0 {
		out = append(out, word.String())
	}
	return out
}

func recordDispatchHookCaller(input io.Reader, terminal bool, envFile string) error {
	if terminal {
		return nil
	}
	data, err := io.ReadAll(io.LimitReader(input, maxCallerHookInput+1))
	if err != nil || len(data) > maxCallerHookInput {
		return nil
	}
	var payload struct {
		SessionID string `json:"session_id"`
	}
	if json.Unmarshal(data, &payload) != nil || strings.TrimSpace(payload.SessionID) == "" || envFile == "" {
		return nil
	}
	file, err := os.OpenFile(envFile, os.O_APPEND|os.O_CREATE|os.O_WRONLY, 0o600)
	if err != nil {
		return err
	}
	defer file.Close()
	quote := func(value string) string { return "'" + strings.ReplaceAll(value, "'", "'\\''") + "'" }
	_, err = fmt.Fprintf(file, "export %s=%s\nexport %s=%s\n", envCallerSession, quote(payload.SessionID), envCallerAgent, quote("claude-code"))
	return err
}

func recordDispatchHookStdin() error {
	return recordDispatchHookCaller(os.Stdin, term.IsTerminal(int(os.Stdin.Fd())), os.Getenv("CLAUDE_ENV_FILE"))
}
