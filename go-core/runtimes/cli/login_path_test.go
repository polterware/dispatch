package main

import (
	"os"
	"path/filepath"
	"testing"
	"time"
)

func TestMergePathPutsTheLoginShellFirst(t *testing.T) {
	extra := t.TempDir()
	got := mergePath("/a:/b", "/usr/bin:/a", []string{extra, filepath.Join(extra, "missing")})
	if want := "/a:/b:/usr/bin:" + extra; got != want {
		t.Fatalf("got %q, want %q", got, want)
	}
}

func TestLoginShellPathReadsPastStartupNoise(t *testing.T) {
	if got := pathAfterMarker("welcome!\n" + loginPathMarker + "/x:/y\nbye\n"); got != "/x:/y" {
		t.Fatal(got)
	}
	shell := filepath.Join(t.TempDir(), "sh")
	script := "#!/bin/sh\necho motd\nPATH=/from/login:$PATH\neval \"$4\"\n"
	if err := os.WriteFile(shell, []byte(script), 0o755); err != nil {
		t.Fatal(err)
	}
	if got := loginShellPath(shell, 5*time.Second); filepath.SplitList(got)[0] != "/from/login" {
		t.Fatalf("got %q", got)
	}
	if got := loginShellPath(filepath.Join(t.TempDir(), "none"), time.Second); got != "" {
		t.Fatalf("a missing shell says nothing: %q", got)
	}
}
