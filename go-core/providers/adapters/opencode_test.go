package adapters

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestOpenCodeRunsOverACP(t *testing.T) {
	a := OpenCode().(acpAdapter)
	if got := strings.Join(a.spec.command(true), " "); got != "acp" {
		t.Fatal(got)
	}
	if a.spec.readMode != "plan" || !a.spec.caps.ReadOnly {
		t.Fatal("a read-only opencode run must select its plan agent")
	}
	if ProviderOf("opencode", "openrouter/some-model") != "openrouter" {
		t.Fatal(ProviderOf("opencode", "openrouter/some-model"))
	}
	if got := parseOpenCodeModels("opencode/a\nnoise line\nx/y\n"); len(got) != 2 {
		t.Fatalf("%v", got)
	}
}

func TestBinaryOverrideAndDetect(t *testing.T) {
	dir := t.TempDir()
	bin := filepath.Join(dir, "my-opencode")
	if err := os.WriteFile(bin, []byte("#!/bin/sh\n"), 0o755); err != nil {
		t.Fatal(err)
	}
	t.Setenv("PATH", t.TempDir())
	t.Cleanup(func() { SetBinaries(nil) })

	if Bin("opencode") != "opencode" {
		t.Fatal("default binary expected")
	}
	SetBinaries(map[string]string{"opencode": bin})
	if OpenCode().Binary() != bin || Bin("codex") != "codex" {
		t.Fatalf("override not applied: %s", OpenCode().Binary())
	}
	var got Detection
	for _, d := range Detect() {
		if d.Agent == "opencode" {
			got = d
		}
	}
	if got.Source != "configured" || got.Path != bin {
		t.Fatalf("%+v", got)
	}
	SetBinaries(map[string]string{"opencode": filepath.Join(dir, "gone")})
	for _, d := range Detect() {
		if d.Agent == "opencode" && d.Source != "invalid" {
			t.Fatalf("%+v", d)
		}
	}
	if ValidBinary("relative/path") == nil || ValidBinary(filepath.Join(dir, "gone")) == nil || ValidBinary(bin) != nil {
		t.Fatal("ValidBinary")
	}
}
