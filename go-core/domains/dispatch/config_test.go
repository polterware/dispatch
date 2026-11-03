package dispatch

import (
	"bytes"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestLoadMissingAndPriority(t *testing.T) {
	dir := t.TempDir()
	missing := filepath.Join(dir, "config.json")
	f, err := LoadConfig(missing)
	if err != nil || f.Priority != "" || !f.Discover {
		t.Fatal(err, f)
	}
	if err := os.WriteFile(missing, []byte(" \n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if f, err = LoadConfig(missing); err != nil || f.Priority != "" || !f.Discover {
		t.Fatal(err, f)
	}
	if err := os.WriteFile(missing, []byte(`{"priority":"quality"}`), 0o644); err != nil {
		t.Fatal(err)
	}
	if f, err = LoadConfig(missing); err != nil || f.Priority != "quality" || !f.Discover {
		t.Fatal(err, f)
	}
	if err := os.WriteFile(missing, []byte(`{"priority":"turbo"}`), 0o644); err != nil {
		t.Fatal(err)
	}
	if _, err := LoadConfig(missing); err == nil {
		t.Fatal("accepted invalid priority")
	}
	if err := os.WriteFile(missing, []byte(`{`), 0o644); err != nil {
		t.Fatal(err)
	}
	if _, err := LoadConfig(missing); err == nil {
		t.Fatal("accepted broken json")
	}
}

func TestLoadAllMergesProjectAndDeny(t *testing.T) {
	d := t.TempDir()
	g, p := filepath.Join(d, "global.json"), filepath.Join(d, "project.json")
	os.WriteFile(g, []byte(`{"priority":"speed","discover":true,"allow":{"providers":["openai"]},"deny":{"agents":["cursor"]}}`), 0o600)
	os.WriteFile(p, []byte(`{"priority":"quality","allow":{"models":["gpt"]},"deny":{"agents":["grok"],"providers":["google"]}}`), 0o600)
	f, err := LoadAllConfig(g, p)
	if err != nil || f.Priority != "quality" || !f.Discover || len(f.Allow.Models) != 1 || len(f.Deny.Agents) != 2 || len(f.Deny.Providers) != 1 {
		t.Fatalf("%+v %v", f, err)
	}
}

func TestRejectsWrongKnownTypesAndWritePreservesUnknown(t *testing.T) {
	p := filepath.Join(t.TempDir(), "config.json")
	for _, b := range []string{`{"discover":"yes"}`, `{"allow":[]}`, `{"deny":{"agents":"codex"}}`} {
		if err := os.WriteFile(p, []byte(b), 0o600); err != nil {
			t.Fatal(err)
		}
		if _, err := LoadConfig(p); err == nil {
			t.Fatalf("accepted %s", b)
		}
	}
	if err := os.WriteFile(p, []byte(`{"future":{"x":1},"priority":"speed"}`), 0o600); err != nil {
		t.Fatal(err)
	}
	if err := WriteConfig(p, Policy{Priority: "quality", Discover: true, Allow: Rules{Providers: []string{"openai"}}}); err != nil {
		t.Fatal(err)
	}
	b, _ := os.ReadFile(p)
	if !bytes.Contains(b, []byte(`"future"`)) || !bytes.Contains(b, []byte(`"quality"`)) {
		t.Fatalf("%s", b)
	}
}

func TestBinariesRoundTripAndGlobalOnly(t *testing.T) {
	dir := t.TempDir()
	g, p := filepath.Join(dir, "g.json"), filepath.Join(dir, "p.json")
	if err := WriteConfigKey(g, "binaries", map[string]string{"opencode": "/x/opencode-cli"}); err != nil {
		t.Fatal(err)
	}
	if err := WriteConfigKey(p, "binaries", map[string]string{"codex": "/evil/codex"}); err != nil {
		t.Fatal(err)
	}
	f, err := LoadAllConfig(g, p)
	if err != nil || f.Binaries["opencode"] != "/x/opencode-cli" || len(f.Binaries) != 1 {
		t.Fatalf("%+v %v", f, err)
	}
	if err := WriteConfigKey(g, "binaries", nil); err != nil {
		t.Fatal(err)
	}
	if f, _ := LoadConfig(g); len(f.Binaries) != 0 {
		t.Fatalf("not cleared: %+v", f.Binaries)
	}
}

func TestACPAgentsAreGlobalOnlyAndValidated(t *testing.T) {
	dir := t.TempDir()
	g, p := filepath.Join(dir, "g.json"), filepath.Join(dir, "p.json")
	if err := WriteConfigKey(g, "acp_agents", map[string][]string{"mine": {"/opt/mine", "acp"}}); err != nil {
		t.Fatal(err)
	}
	if err := WriteConfigKey(p, "acp_agents", map[string][]string{"evil": {"/tmp/evil"}}); err != nil {
		t.Fatal(err)
	}
	f, err := LoadAllConfig(g, p)
	if err != nil || len(f.ACPAgents) != 1 || f.ACPAgents["mine"][1] != "acp" {
		t.Fatalf("%+v %v", f.ACPAgents, err)
	}
	if err := WriteConfigKey(g, "acp_agents", map[string][]string{"codex": {"/x"}}); err != nil {
		t.Fatal(err)
	}
	if _, err := LoadConfig(g); err == nil || !strings.Contains(err.Error(), "built-in") {
		t.Fatalf("a built-in agent's name must be refused: %v", err)
	}
}

func TestDiscoverDefaultsOnAndExplicitFalseWins(t *testing.T) {
	dir := t.TempDir()
	global, project := filepath.Join(dir, "global.json"), filepath.Join(dir, "project.json")
	for _, body := range []string{"", "{}", `{"discover":true}`, `{"discover":false}`} {
		if err := os.WriteFile(global, []byte(body), 0o600); err != nil {
			t.Fatal(err)
		}
		want := body != `{"discover":false}`
		f, err := LoadConfig(global)
		if err != nil || f.Discover != want {
			t.Fatalf("%q: %+v, %v", body, f, err)
		}
		f, err = LoadAllConfig(global, project)
		if err != nil || f.Discover != want {
			t.Fatalf("merged %q: %+v, %v", body, f, err)
		}
	}
	if err := os.WriteFile(global, []byte(`{"discover":true}`), 0o600); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(project, []byte(`{"discover":false}`), 0o600); err != nil {
		t.Fatal(err)
	}
	f, err := LoadAllConfig(global, project)
	if err != nil || f.Discover {
		t.Fatalf("project override: %+v, %v", f, err)
	}
}
