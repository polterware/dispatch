package setup

import (
	"os"
	"path/filepath"
	"testing"

	"github.com/polterware/dispatch/go-core/providers/skill"
)

func fixture(t *testing.T) (home, binary, state string) {
	t.Helper()
	t.Setenv("CLAUDE_CONFIG_DIR", "")
	home = t.TempDir()
	binary = filepath.Join(t.TempDir(), "dispatch")
	if err := os.WriteFile(binary, []byte("new"), 0o755); err != nil {
		t.Fatal(err)
	}
	return home, binary, filepath.Join(home, "state")
}

func skillState(r Report, agent string) string {
	for _, s := range r.Skills {
		if s.Agent == agent {
			return s.State
		}
	}
	return ""
}

func TestReadStartsMissing(t *testing.T) {
	home, binary, state := fixture(t)
	r := Read(home, binary, state)
	if r.CLI.State != Missing || r.Hook.On || len(r.Skills) == 0 {
		t.Fatalf("report = %+v", r)
	}
	for _, s := range r.Skills {
		if s.State != Missing {
			t.Fatalf("%s = %s", s.Agent, s.State)
		}
	}
}

func TestLinkCLIReplacesAnOlderCopy(t *testing.T) {
	home, binary, state := fixture(t)
	path := CLIPath(home)
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, []byte("old"), 0o755); err != nil {
		t.Fatal(err)
	}
	if got := Read(home, binary, state).CLI.State; got != Other {
		t.Fatalf("before link = %s", got)
	}
	if _, err := LinkCLI(home, binary); err != nil {
		t.Fatal(err)
	}
	if got := Read(home, binary, state).CLI.State; got != Linked {
		t.Fatalf("after link = %s", got)
	}
}

func TestInstallSkillOverwritesAnOutdatedCopy(t *testing.T) {
	home, binary, state := fixture(t)
	dir := skill.Targets(home)["codex"].Dir
	if err := os.MkdirAll(filepath.Join(dir, skill.Name), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(dir, skill.Name, "SKILL.md"), []byte("old"), 0o644); err != nil {
		t.Fatal(err)
	}
	r := Read(home, binary, state)
	if skillState(r, "codex") != Outdated || skillState(r, "grok") != Outdated {
		t.Fatalf("codex and grok share a folder: %+v", r.Skills)
	}
	if _, err := InstallSkill(home, "codex"); err != nil {
		t.Fatal(err)
	}
	r = Read(home, binary, state)
	if skillState(r, "codex") != Current || skillState(r, "claude") != Missing {
		t.Fatalf("skills = %+v", r.Skills)
	}
	if _, err := InstallSkill(home, "nope"); err == nil {
		t.Fatal("an unknown agent must be refused")
	}
}

func TestSetHook(t *testing.T) {
	home, binary, state := fixture(t)
	if err := SetHook(home, binary, state, true); err != nil {
		t.Fatal(err)
	}
	if h := Read(home, binary, state).Hook; !h.On || !h.Current {
		t.Fatalf("hook on = %+v", h)
	}
	if err := SetHook(home, binary, state, false); err != nil {
		t.Fatal(err)
	}
	if h := Read(home, binary, state).Hook; h.On || h.Command != "" {
		t.Fatalf("hook off = %+v", h)
	}
}
