package adapters

import (
	"context"
	"fmt"
	"os/exec"
	"regexp"
	"slices"
	"sort"
	"strings"

	"github.com/polterware/dispatch/go-core/providers/sandbox"
)

// The agents dispatch runs over ACP, each started the way its CLI documents.
// A read-only run rejects every permission request to edit, delete, move,
// or execute; readMode or an approval flag adds the agent's own read-only
// mode where it has one.

// Gemini drives `gemini --acp`; --approval-mode plan is its read-only mode.
func Gemini() Adapter {
	return acpAdapter{spec: acpSpec{
		name:    "gemini",
		command: approvalModeCommand("--acp", "plan"),
		caps:    Capabilities{SessionResume: true, StructuredOutput: true, ReadOnly: true, FileEditing: true},
		flags:   []probeFlag{{Name: "--acp", Capability: "structuredOutput"}, {Name: "--approval-mode", Capability: "readOnly"}},
	}}
}

// Qwen drives `qwen --acp`; --approval-mode plan is its read-only mode.
func Qwen() Adapter {
	return acpAdapter{spec: acpSpec{
		name:    "qwen",
		command: approvalModeCommand("--acp", "plan"),
		caps:    Capabilities{SessionResume: true, StructuredOutput: true, ReadOnly: true, FileEditing: true},
		flags:   []probeFlag{{Name: "--acp", Capability: "structuredOutput"}, {Name: "--approval-mode", Capability: "readOnly"}},
	}}
}

// Copilot drives `copilot --acp`, which asks before every tool it runs.
func Copilot() Adapter {
	return acpAdapter{spec: acpSpec{
		name:    "copilot",
		command: func(bool) []string { return []string{"--acp"} },
		caps:    Capabilities{SessionResume: true, StructuredOutput: true, ReadOnly: true, FileEditing: true},
		flags:   []probeFlag{{Name: "--acp", Capability: "structuredOutput"}},
	}}
}

// Kimi drives `kimi acp`.
func Kimi() Adapter {
	return acpAdapter{spec: acpSpec{
		name:    "kimi",
		command: func(bool) []string { return []string{"acp"} },
		caps:    Capabilities{SessionResume: true, StructuredOutput: true, ReadOnly: true, FileEditing: true},
		flags:   []probeFlag{{Name: "acp", Capability: "structuredOutput"}},
	}}
}

// OpenCode drives `opencode acp`. Its plan agent is the read-only mode, and
// the models come from `opencode models` as provider/model.
func OpenCode() Adapter {
	return acpAdapter{spec: acpSpec{
		name:       "opencode",
		command:    func(bool) []string { return []string{"acp"} },
		readMode:   "plan",
		caps:       Capabilities{SessionResume: true, StructuredOutput: true, ReadOnly: true, FileEditing: true},
		flags:      []probeFlag{{Name: "acp", Capability: "structuredOutput"}},
		listModels: listModelsWith("opencode", []string{"models"}, parseOpenCodeModels),
	}}
}

func approvalModeCommand(acp, readMode string) func(bool) []string {
	return func(readOnly bool) []string {
		if readOnly {
			return []string{acp, "--approval-mode", readMode}
		}
		return []string{acp}
	}
}

// listModelsWith lists an agent's models from a CLI subcommand, without
// Claude models outside Claude's own CLI.
func listModelsWith(agent string, args []string, parse func(string) []string) func(context.Context) ([]Model, error) {
	return func(ctx context.Context) ([]Model, error) {
		cmd := exec.CommandContext(ctx, Bin(agent), args...)
		cmd.Env = sandbox.Environ()
		out, err := cmd.Output()
		if err != nil {
			return nil, err
		}
		var models []Model
		for _, id := range parse(string(out)) {
			if AllowedModel(agent, id) {
				models = append(models, Model{ID: id})
			}
		}
		return models, nil
	}
}

// customAgentName is the form an added agent's name takes.
var customAgentName = regexp.MustCompile(`^[a-z][a-z0-9-]{0,31}$`)

// SetCustomAgents replaces the ACP agents the user added (`dispatch agents
// add <name> -- <command…>`): each name runs its command, whose first word
// is the executable. A name that is already a built-in agent is ignored.
func SetCustomAgents(m map[string][]string) {
	binMu.Lock()
	defer binMu.Unlock()
	customAgents = map[string][]string{}
	for name, command := range m {
		if ValidCustomAgent(name, command) == nil {
			customAgents[name] = slices.Clone(command)
		}
	}
}

// ValidCustomAgent checks an added agent before it is saved.
func ValidCustomAgent(name string, command []string) error {
	if !customAgentName.MatchString(name) {
		return fmt.Errorf("invalid agent name %q (lowercase letters, digits, and dashes, starting with a letter)", name)
	}
	if _, builtIn := defaultBinaries[name]; builtIn {
		return fmt.Errorf("%s is a built-in agent", name)
	}
	if len(command) == 0 || strings.TrimSpace(command[0]) == "" {
		return fmt.Errorf("%s: the command that starts its ACP server is empty", name)
	}
	return nil
}

var customAgents = map[string][]string{}

// customAdapters are the added agents, by name.
func customAdapters() []Adapter {
	binMu.RLock()
	defer binMu.RUnlock()
	names := make([]string, 0, len(customAgents))
	for name := range customAgents {
		names = append(names, name)
	}
	sort.Strings(names)
	out := make([]Adapter, 0, len(names))
	for _, name := range names {
		args := slices.Clone(customAgents[name][1:])
		out = append(out, acpAdapter{spec: acpSpec{
			name:    name,
			command: func(bool) []string { return args },
			caps:    Capabilities{SessionResume: true, StructuredOutput: true, ReadOnly: true, FileEditing: true},
		}})
	}
	return out
}
