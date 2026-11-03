// Command dispatch is Polterware Dispatch's binary: its local core (`serve`)
// and the commands that delegate tasks to other coding agents.
package main

import (
	"flag"
	"fmt"
	"os"
	"path/filepath"
)

// commands maps each top-level command to the name runDispatch knows it by.
var commands = map[string]string{
	"run":      "run",
	"runs":     "ls",
	"status":   "status",
	"wait":     "wait",
	"result":   "result",
	"cancel":   "cancel",
	"route":    "route",
	"classify": "classify",
	"models":   "models",
	"merge":    "merge",
	"clean":    "clean",
	"recover":  "recover",
	"roles":    "roles",
	"tiers":    "tiers",
	"usage":    "stats",
	"agents":   "agents",
	"doctor":   "doctor",
	"config":   "config",
	"skill":    "skill",
	"setup":    "setup",
	"update":   "update",
	"on":       "on",
	"off":      "off",
	"hook":     "hook",
}

func main() {
	dbPath := flag.String("db", defaultDB(), "sqlite path")
	flag.Usage = usage
	flag.Parse()
	if abs, err := filepath.Abs(*dbPath); err == nil {
		*dbPath = abs
	}
	args := flag.Args()
	if len(args) == 0 {
		usage()
		os.Exit(2)
	}
	command, rest := args[0], args[1:]

	// Only `serve` opens the store; every other command is a client of the core
	// and starts it when none is running.
	var err error
	switch command {
	case "serve":
		err = runServe(*dbPath, rest)
	case "stop":
		err = runStop(*dbPath)
	case "workspace", "workspaces":
		err = runWorkspace(*dbPath, rest)
	case "dispatch":
		// `dispatch dispatch hook` is the hook Polterware registered before.
		err = runDispatch(*dbPath, rest)
	case "help", "-h", "--help":
		usage()
	default:
		sub, ok := commands[command]
		if !ok {
			fmt.Fprintf(os.Stderr, "unknown command %q\n", command)
			usage()
			os.Exit(2)
		}
		err = runDispatch(*dbPath, append([]string{sub}, rest...))
	}
	if err != nil {
		fail(err)
	}
}

func usage() {
	fmt.Fprintf(os.Stderr, `Usage:
  dispatch [-db path] <command> [flags]

Runs:
%s
       dispatch runs | status <id> | cancel <id> | tiers [build] | usage | agents

Workspaces (Polterware projects when Polterware is installed, plus folders):
  dispatch workspace ls [--json]
  dispatch workspace add <folder> [--name <name>]
  dispatch workspace rm <id>

Core:
  dispatch serve [--listen 127.0.0.1:0] [--idle <dur>]
  dispatch stop

The database defaults to $PWDISPATCH_DATA_DIR/dispatch.db, else
~/Library/Application Support/Polterware Dispatch/dispatch.db.
`, dispatchUsage)
}

// defaultDB is $PWDISPATCH_DATA_DIR/dispatch.db, else the app's folder.
func defaultDB() string {
	if data := os.Getenv("PWDISPATCH_DATA_DIR"); data != "" {
		return filepath.Join(data, "dispatch.db")
	}
	home, err := os.UserHomeDir()
	if err != nil {
		return "dispatch.db"
	}
	return filepath.Join(home, "Library", "Application Support", "Polterware Dispatch", "dispatch.db")
}
