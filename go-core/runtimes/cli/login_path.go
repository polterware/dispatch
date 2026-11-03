package main

import (
	"bytes"
	"context"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"time"
)

const loginPathMarker = "__DISPATCH_PATH__"

// adoptLoginPath gives the core the PATH a terminal has. An app opened from
// the Finder starts the core with launchd's PATH (/usr/bin:/bin:…), where no
// agent CLI lives, so every agent read as not installed and a run could not
// find node or the CLI itself. The login shell's PATH comes first, then the
// one the core was given, then the usual install folders that exist.
func adoptLoginPath() {
	home, _ := os.UserHomeDir()
	login := loginShellPath(os.Getenv("SHELL"), 5*time.Second)
	common := []string{"/opt/homebrew/bin", "/usr/local/bin"}
	if home != "" {
		common = append(common, filepath.Join(home, ".local", "bin"))
	}
	_ = os.Setenv("PATH", mergePath(login, os.Getenv("PATH"), common))
}

// loginShellPath asks the user's interactive login shell for its PATH, empty
// when it cannot say in time.
func loginShellPath(shell string, timeout time.Duration) string {
	if shell == "" {
		shell = "/bin/zsh"
	}
	ctx, cancel := context.WithTimeout(context.Background(), timeout)
	defer cancel()
	cmd := exec.CommandContext(ctx, shell, "-l", "-i", "-c", `printf '\n`+loginPathMarker+`%s\n' "$PATH"`)
	cmd.WaitDelay = time.Second // a child that keeps stdout open does not hold the core
	var out bytes.Buffer
	cmd.Stdout = &out
	if err := cmd.Run(); err != nil && out.Len() == 0 {
		return ""
	}
	return pathAfterMarker(out.String())
}

// pathAfterMarker reads the PATH line a shell printed among whatever its
// startup files print.
func pathAfterMarker(out string) string {
	for _, line := range strings.Split(out, "\n") {
		if rest, ok := strings.CutPrefix(line, loginPathMarker); ok {
			return strings.TrimSpace(rest)
		}
	}
	return ""
}

// mergePath joins the login PATH, the current one, and the common folders
// that exist, each folder once, in that order.
func mergePath(login, current string, common []string) string {
	seen := map[string]bool{}
	var dirs []string
	add := func(dir string) {
		if dir == "" || seen[dir] {
			return
		}
		seen[dir] = true
		dirs = append(dirs, dir)
	}
	for _, dir := range filepath.SplitList(login) {
		add(dir)
	}
	for _, dir := range filepath.SplitList(current) {
		add(dir)
	}
	for _, dir := range common {
		if st, err := os.Stat(dir); err == nil && st.IsDir() {
			add(dir)
		}
	}
	return strings.Join(dirs, string(filepath.ListSeparator))
}
