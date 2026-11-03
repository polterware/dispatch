// Package setup reports and changes how the agent CLIs on this machine reach
// dispatch: the `dispatch` command on the PATH, the dispatch skill in each
// agent's skills folder, the Claude Code SessionStart hook, and the Dispatch
// plugin in OpenCode.
package setup

import (
	"bytes"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"slices"
	"strings"

	"github.com/polterware/dispatch/go-core/providers/hook"
	"github.com/polterware/dispatch/go-core/providers/skill"
)

// CLI is the `dispatch` command agents run: Linked when it is this binary,
// Other when another file holds its place (an older copy), Missing otherwise.
type CLI struct {
	Path   string `json:"path"`
	State  string `json:"state"`
	Target string `json:"target,omitempty"`
	Binary string `json:"binary"`
}

// Skill is the dispatch skill for one agent: Current, Outdated (another
// version of it), or Missing.
type Skill struct {
	Agent string `json:"agent"`
	Path  string `json:"path"`
	State string `json:"state"`
}

// Hook is the Claude Code SessionStart hook: On when it is registered and the
// enabled flag is set; Command is what it runs, Current when that is this binary.
type Hook struct {
	On       bool   `json:"on"`
	Settings string `json:"settings"`
	Command  string `json:"command,omitempty"`
	Current  bool   `json:"current"`
}

// Report is the whole setup.
type Report struct {
	CLI      CLI      `json:"cli"`
	Skills   []Skill  `json:"skills"`
	Hook     Hook     `json:"hook"`
	OpenCode OpenCode `json:"opencode"`
}

const (
	Linked   = "linked"
	Other    = "other"
	Missing  = "missing"
	Current  = "current"
	Outdated = "outdated"
)

// CLIPath is where the `dispatch` command is linked: ~/.local/bin, the folder
// the agent CLIs install themselves into.
func CLIPath(home string) string { return filepath.Join(home, ".local", "bin", "dispatch") }

// Read reports the setup for binary, the dispatch executable agents should run.
func Read(home, binary, stateDir string) Report {
	r := Report{CLI: readCLI(home, binary), Skills: []Skill{}}
	r.OpenCode = readOpenCode(home, OpenCodeBinary(home, binary), stateDir)
	targets := skill.Targets(home)
	agents := make([]string, 0, len(targets))
	for name := range targets {
		agents = append(agents, name)
	}
	slices.Sort(agents)
	for _, agent := range agents {
		path := filepath.Join(targets[agent].Dir, skill.Name, "SKILL.md")
		state := Missing
		if b, err := os.ReadFile(path); err == nil {
			state = Outdated
			if bytes.Equal(b, skill.Content) {
				state = Current
			}
		}
		r.Skills = append(r.Skills, Skill{Agent: agent, Path: path, State: state})
	}
	settings := hook.SettingsPath(home)
	command := hook.RegisteredCommand(settings)
	r.Hook = Hook{
		On:       command != "" && hook.IsEnabled(stateDir),
		Settings: settings,
		Command:  command,
		Current:  command != "" && command == hook.HookCommand(binary),
	}
	return r
}

// OpenCodeBinary is the dispatch the OpenCode plugin runs: the linked command
// when it is this binary, so it follows the link, else binary itself.
func OpenCodeBinary(home, binary string) string {
	if readCLI(home, binary).State == Linked {
		return CLIPath(home)
	}
	return binary
}

func readCLI(home, binary string) CLI {
	c := CLI{Path: CLIPath(home), State: Missing, Binary: binary}
	info, err := os.Lstat(c.Path)
	if err != nil {
		return c
	}
	c.State = Other
	c.Target = c.Path
	if info.Mode()&os.ModeSymlink != 0 {
		if target, err := filepath.EvalSymlinks(c.Path); err == nil {
			c.Target = target
		}
	}
	if same(c.Target, binary) {
		c.State = Linked
	}
	return c
}

func same(a, b string) bool {
	ra, err := filepath.EvalSymlinks(a)
	if err != nil {
		return false
	}
	rb, err := filepath.EvalSymlinks(b)
	if err != nil {
		return false
	}
	return ra == rb
}

// LinkCLI points the `dispatch` command at binary, replacing whatever held its
// place: a link, or an older dispatch copy. It refuses a folder.
func LinkCLI(home, binary string) (string, error) {
	path := CLIPath(home)
	if info, err := os.Lstat(path); err == nil {
		if info.IsDir() {
			return path, fmt.Errorf("%s is a folder", path)
		}
		if err := os.Remove(path); err != nil {
			return path, err
		}
	}
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		return path, err
	}
	return path, os.Symlink(binary, path)
}

// InstallSkill writes the current skill for agent, or for every agent when it
// is empty, over any older copy. It returns the paths written.
func InstallSkill(home, agent string) ([]string, error) {
	targets := skill.Targets(home)
	agent = strings.TrimSpace(agent)
	if agent != "" {
		if _, ok := targets[agent]; !ok {
			return nil, fmt.Errorf("unknown agent %q", agent)
		}
	}
	var out []string
	seen := map[string]bool{}
	names := make([]string, 0, len(targets))
	for name := range targets {
		names = append(names, name)
	}
	slices.Sort(names)
	for _, name := range names {
		dir := targets[name].Dir
		if (agent != "" && name != agent) || seen[dir] {
			continue
		}
		seen[dir] = true
		path, err := skill.Install(dir, true)
		if err != nil {
			return out, err
		}
		out = append(out, path)
	}
	return out, nil
}

// SetHook registers the SessionStart hook for binary and sets the enabled
// flag, or removes both.
func SetHook(home, binary, stateDir string, on bool) error {
	settings := hook.SettingsPath(home)
	if !on {
		return errors.Join(hook.SetEnabled(stateDir, false), hook.Uninstall(settings))
	}
	if err := hook.SetEnabled(stateDir, true); err != nil {
		return err
	}
	return hook.Install(settings, binary)
}
