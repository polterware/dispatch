package adapters

import (
	"context"
	"os/exec"
	"strings"

	"github.com/polterware/dispatch/go-core/providers/sandbox"
)

// Cursor drives the Cursor CLI. The binary is cursor-agent: a bare `agent`
// on PATH may resolve to Grok's alias. Sessions are pre-allocated with
// `create-chat` and selected with --resume.
func Cursor() Adapter {
	return cliAdapter{
		name: "cursor", binary: "cursor-agent",
		caps:        Capabilities{SessionResume: true, StructuredOutput: true, ReadOnly: true, FileEditing: true},
		flags:       cursorProbeFlags,
		listArgs:    []string{"models"},
		parseModels: parseCursorModels,
		newSession: func(ctx context.Context) (string, error) {
			cmd := exec.CommandContext(ctx, Bin("cursor"), "create-chat")
			cmd.Env = sandbox.Environ()
			out, err := cmd.Output()
			return strings.TrimSpace(string(out)), err
		},
		args: func(r Request, model, session string, _ bool) []string {
			a := []string{"-p", "--output-format", "stream-json", "--stream-partial-output", "--trust"}
			if r.ReadOnly {
				a = append(a, "--mode", "ask")
			} else {
				a = append(a, "--force", "--sandbox", "enabled")
			}
			if session != "" {
				a = append(a, "--resume", session)
			}
			if model != "" {
				a = append(a, "--model", model)
			}
			if r.Dir != "" {
				a = append(a, "--workspace", r.Dir)
			}
			return append(a, r.Prompt)
		},
		parse: parseCursorStream,
		usage: usageCursor,
	}
}

// Grok drives the Grok Build CLI in single-turn mode. A fresh run is given
// a caller-chosen session UUID (-s) so it can be resumed with --resume.
func Grok() Adapter {
	return cliAdapter{
		name: "grok", binary: "grok",
		caps:        Capabilities{SessionResume: true, StructuredOutput: true, ReadOnly: true, FileEditing: true},
		flags:       grokProbeFlags,
		listArgs:    []string{"models"},
		parseModels: parseGrokModels,
		newSession:  func(context.Context) (string, error) { return newUUID(), nil },
		args: func(r Request, model, session string, resume bool) []string {
			a := []string{"--output-format", "streaming-json"}
			if r.ReadOnly {
				a = append(a, "--permission-mode", "plan")
			} else {
				a = append(a, "--permission-mode", "acceptEdits")
			}
			if resume {
				a = append(a, "--resume", session)
			} else if session != "" {
				a = append(a, "-s", session)
			}
			if model != "" {
				a = append(a, "--model", model)
			}
			if r.Dir != "" {
				a = append(a, "--cwd", r.Dir)
			}
			// -p takes the prompt as its value.
			return append(a, "-p", r.Prompt)
		},
		parse: parseGrokStream,
		usage: usageGrok,
	}
}

var cursorProbeFlags = []probeFlag{
	{Name: "-p"},
	{Name: "--output-format", Capability: "structuredOutput"},
	{Name: "--stream-partial-output"},
	{Name: "--trust"},
	{Name: "--mode", Capability: "readOnly"},
	{Name: "--force", Capability: "fileEditing"},
	{Name: "--sandbox"},
	{Name: "--resume", Capability: "sessionResume"},
	{Name: "--model"},
	{Name: "--workspace"},
	{Name: "create-chat"},
}

var grokProbeFlags = []probeFlag{
	{Name: "--output-format", Capability: "structuredOutput"},
	{Name: "--permission-mode", Capability: "readOnly"},
	{Name: "--resume", Capability: "sessionResume"},
	{Name: "-s"},
	{Name: "--model"},
	{Name: "--cwd"},
	{Name: "-p"},
}

// Known returns the adapters dispatch knows about: the built-in ones, then
// the ACP agents the user added.
func Known() []Adapter {
	return append([]Adapter{Codex{}, Claude{}, Cursor(), Grok(), Antigravity{}, OpenCode(),
		Pi{}, Gemini(), Qwen(), Copilot(), Kimi()}, customAdapters()...)
}

var (
	_ Prober        = Codex{}
	_ Prober        = Antigravity{}
	_ Prober        = cliAdapter{}
	_ Prober        = acpAdapter{}
	_ Resumer       = acpAdapter{}
	_ UsageProvider = Codex{}
	_ UsageProvider = Antigravity{}
	_ UsageProvider = Claude{}
	_ Resumer       = Codex{}
	_ Resumer       = Antigravity{}
	_ Resumer       = cliAdapter{}
)
