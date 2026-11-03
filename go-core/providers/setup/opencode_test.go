package setup

import (
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func openCodeConfig(t *testing.T, home string) string {
	t.Helper()
	t.Setenv("XDG_CONFIG_HOME", "")
	return filepath.Join(home, ".config", "opencode", "opencode.json")
}

func TestOpenCodePluginInstallUpdateAndRemove(t *testing.T) {
	home, binary, state := fixture(t)
	config := openCodeConfig(t, home)
	if got := Read(home, binary, state).OpenCode; got.State != Missing || !got.On || got.Config != config {
		t.Fatalf("before install = %+v", got)
	}
	dir, err := InstallOpenCodePlugin(home, binary, state)
	if err != nil {
		t.Fatal(err)
	}
	if dir != filepath.Join(state, "opencode-plugin") {
		t.Fatalf("dir = %s", dir)
	}
	for _, name := range []string{"package.json", "server.js", "rpc.js", "gui-experimental.cjs"} {
		if _, err := os.Stat(filepath.Join(dir, name)); err != nil {
			t.Fatalf("%s: %v", name, err)
		}
	}
	body, err := os.ReadFile(config)
	if err != nil {
		t.Fatal(err)
	}
	var parsed struct {
		Schema  string          `json:"$schema"`
		Plugins []openCodeEntry `json:"plugins"`
	}
	if err := json.Unmarshal(body, &parsed); err != nil {
		t.Fatalf("config is not JSON: %v\n%s", err, body)
	}
	if len(parsed.Plugins) != 1 || parsed.Plugins[0].Package != dir || parsed.Plugins[0].Options["binary"] != binary ||
		parsed.Plugins[0].Options["state"] != filepath.Join(state, OpenCodeSwitch) || parsed.Schema == "" {
		t.Fatalf("config = %s", body)
	}
	if got := Read(home, binary, state).OpenCode.State; got != Current {
		t.Fatalf("after install = %s", got)
	}

	if err := os.WriteFile(filepath.Join(dir, "server.js"), []byte("export default {}\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if got := Read(home, binary, state).OpenCode.State; got != Outdated {
		t.Fatalf("other files = %s", got)
	}
	other := filepath.Join(t.TempDir(), "dispatch")
	if got := Read(home, other, state).OpenCode.State; got != Outdated {
		t.Fatalf("other binary = %s", got)
	}
	if _, err := InstallOpenCodePlugin(home, binary, state); err != nil {
		t.Fatal(err)
	}
	if got := Read(home, binary, state).OpenCode.State; got != Current {
		t.Fatalf("after update = %s", got)
	}
	after, _ := os.ReadFile(config)
	if strings.Count(string(after), `"package":`) != 1 {
		t.Fatalf("an update must replace the entry, not add one:\n%s", after)
	}

	if err := SetOpenCodeDelegation(state, false); err != nil {
		t.Fatal(err)
	}
	if got := Read(home, binary, state).OpenCode; got.On || got.State != Current {
		t.Fatalf("delegation off = %+v", got)
	}

	if err := RemoveOpenCodePlugin(home, state); err != nil {
		t.Fatal(err)
	}
	if got := Read(home, binary, state).OpenCode; got.State != Missing || !got.On {
		t.Fatalf("after remove = %+v", got)
	}
	if _, err := os.Stat(dir); !os.IsNotExist(err) {
		t.Fatalf("plugin folder should be gone: %v", err)
	}
	left, _ := os.ReadFile(config)
	if strings.Contains(string(left), dir) {
		t.Fatalf("entry left behind:\n%s", left)
	}
	if err := json.Unmarshal(left, &parsed); err != nil {
		t.Fatalf("config after remove is not JSON: %v\n%s", err, left)
	}
	if err := RemoveOpenCodePlugin(home, state); err != nil {
		t.Fatalf("removing twice: %v", err)
	}
}

func TestEditOpenCodeConfigKeepsTheRestOfTheFile(t *testing.T) {
	dir := "/state/opencode-plugin"
	entry := []byte(`{"package":"/state/opencode-plugin","options":{"binary":"/bin/dispatch"}}`)
	cases := []struct {
		name, src, want string
	}{
		{
			name: "no plugins",
			src:  "{\n  // models\n  \"model\": \"a/b\"\n}\n",
			want: "{\n  \"plugins\": [\n    " + string(entry) + "\n  ],\n  // models\n  \"model\": \"a/b\"\n}\n",
		},
		{
			name: "empty object",
			src:  "{}",
			want: "{\n  \"plugins\": [\n    " + string(entry) + "\n  ]\n}",
		},
		{
			name: "empty list",
			src:  "{\"plugins\": []}",
			want: "{\"plugins\": [\n    " + string(entry) + "\n  ]}",
		},
		{
			name: "other plugins",
			src:  "{\n  \"plugins\": [\n    \"acme\", /* keep */\n    \"./local\"\n  ]\n}",
			want: "{\n  \"plugins\": [\n    \"acme\", /* keep */\n    \"./local\",\n    " + string(entry) + "\n  ]\n}",
		},
		{
			name: "older entry as a path",
			src:  "{\"plugins\": [\"acme\", \"/state/opencode-plugin\", \"b\"]}",
			want: "{\"plugins\": [\"acme\", " + string(entry) + ", \"b\"]}",
		},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			config := filepath.Join(t.TempDir(), "opencode.jsonc")
			if err := os.WriteFile(config, []byte(c.src), 0o644); err != nil {
				t.Fatal(err)
			}
			if err := editOpenCodeConfig(config, dir, entry); err != nil {
				t.Fatal(err)
			}
			got, _ := os.ReadFile(config)
			if string(got) != c.want {
				t.Fatalf("got\n%s\nwant\n%s", got, c.want)
			}
		})
	}
}

func TestEditOpenCodeConfigKeepsTheFileMode(t *testing.T) {
	config := filepath.Join(t.TempDir(), "opencode.json")
	if err := os.WriteFile(config, []byte("{}"), 0o600); err != nil {
		t.Fatal(err)
	}
	if err := editOpenCodeConfig(config, "/d", []byte(`"/d"`)); err != nil {
		t.Fatal(err)
	}
	info, err := os.Stat(config)
	if err != nil {
		t.Fatal(err)
	}
	if info.Mode().Perm() != 0o600 {
		t.Fatalf("mode = %v, want 0600", info.Mode().Perm())
	}
	if left, _ := filepath.Glob(filepath.Join(filepath.Dir(config), ".opencode.json.*.tmp")); len(left) != 0 {
		t.Fatalf("temporary files left: %v", left)
	}
}

func TestEditOpenCodeConfigRemovesOnlyItsEntry(t *testing.T) {
	dir := "/state/opencode-plugin"
	ours := `{"package": "/state/opencode-plugin", "options": {}}`
	cases := []struct{ src, want string }{
		{"{\"plugins\": [\"a\", " + ours + ", \"b\"]}", "{\"plugins\": [\"a\", \"b\"]}"},
		{"{\"plugins\": [\"a\", " + ours + "]}", "{\"plugins\": [\"a\"]}"},
		{"{\"plugins\": [" + ours + "], \"model\": \"x\"}", "{\"plugins\": [], \"model\": \"x\"}"},
		{"{\"plugins\": [\"a\"]}", "{\"plugins\": [\"a\"]}"},
	}
	for _, c := range cases {
		config := filepath.Join(t.TempDir(), "opencode.json")
		if err := os.WriteFile(config, []byte(c.src), 0o644); err != nil {
			t.Fatal(err)
		}
		if err := editOpenCodeConfig(config, dir, nil); err != nil {
			t.Fatal(err)
		}
		if got, _ := os.ReadFile(config); string(got) != c.want {
			t.Fatalf("got %s, want %s", got, c.want)
		}
	}
}

func TestEditOpenCodeConfigRefusesWhatItCannotRead(t *testing.T) {
	for _, src := range []string{"[]", "{\"plugins\": \"acme\"}", "{\"a\": "} {
		config := filepath.Join(t.TempDir(), "opencode.json")
		if err := os.WriteFile(config, []byte(src), 0o644); err != nil {
			t.Fatal(err)
		}
		if err := editOpenCodeConfig(config, "/d", []byte(`"x"`)); err == nil {
			t.Fatalf("%s: want an error", src)
		}
		if got, _ := os.ReadFile(config); string(got) != src {
			t.Fatalf("%s was changed to %s", src, got)
		}
	}
}

func TestOpenCodeConfigPrefersJSONC(t *testing.T) {
	home := t.TempDir()
	t.Setenv("XDG_CONFIG_HOME", "")
	dir := filepath.Join(home, ".config", "opencode")
	if got := OpenCodeConfigPath(home); got != filepath.Join(dir, "opencode.json") {
		t.Fatalf("default = %s", got)
	}
	if err := os.MkdirAll(dir, 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(dir, "opencode.jsonc"), []byte("{}"), 0o644); err != nil {
		t.Fatal(err)
	}
	if got := OpenCodeConfigPath(home); got != filepath.Join(dir, "opencode.jsonc") {
		t.Fatalf("with jsonc = %s", got)
	}
	xdg := t.TempDir()
	t.Setenv("XDG_CONFIG_HOME", xdg)
	if got := OpenCodeConfigPath(home); got != filepath.Join(xdg, "opencode", "opencode.json") {
		t.Fatalf("xdg = %s", got)
	}
}
