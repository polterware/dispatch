package main

import (
	"context"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"os"
	"strings"

	"connectrpc.com/connect"

	"github.com/polterware/dispatch/go-core/platform/coreclient"
	dispatchv1 "github.com/polterware/dispatch/go-core/platform/gen/dispatch/v1"
)

// Polterware's hook sets these for an agent session, so a run started from it
// joins that session's AgentRun.
const (
	envSession    = "POLTERWARE_SESSION"
	envAgentLabel = "POLTERWARE_AGENT_LABEL"
)

// dial returns clients for the core that owns dbPath, starting it when needed.
func dial(dbPath string) (coreclient.Clients, error) {
	return coreclient.Connect(dbPath, "cli")
}

// resolveWorkspace returns the workspace with id, else the one holding dir
// (the cwd when empty).
func resolveWorkspace(c coreclient.Clients, id, dir string) (*dispatchv1.Workspace, error) {
	if strings.TrimSpace(id) == "" && strings.TrimSpace(dir) == "" {
		cwd, err := os.Getwd()
		if err != nil {
			return nil, err
		}
		dir = cwd
	}
	res, err := c.Workspaces.ResolveWorkspace(context.Background(), connect.NewRequest(&dispatchv1.ResolveWorkspaceRequest{Id: id, Dir: dir}))
	if err != nil {
		return nil, err
	}
	return res.Msg.GetWorkspace(), nil
}

func writeJSON(v any) error {
	enc := json.NewEncoder(os.Stdout)
	enc.SetIndent("", "  ")
	return enc.Encode(v)
}

// exitError ends the process with code after the command printed its output.
type exitError struct{ code int }

func (e exitError) Error() string { return fmt.Sprintf("exit status %d", e.code) }

func fail(err error) {
	var exit exitError
	if errors.As(err, &exit) {
		os.Exit(exit.code)
	}
	var connectErr *connect.Error
	if errors.As(err, &connectErr) {
		fmt.Fprintf(os.Stderr, "dispatch: %s\n", connectErr.Message())
		os.Exit(1)
	}
	fmt.Fprintf(os.Stderr, "dispatch: %v\n", err)
	os.Exit(1)
}

// parseInterspersed allows flags after positional arguments.
func parseInterspersed(flags *flag.FlagSet, args []string) ([]string, error) {
	var positional []string
	for {
		if err := flags.Parse(args); err != nil {
			return nil, err
		}
		rest := flags.Args()
		if len(rest) == 0 {
			return positional, nil
		}
		positional = append(positional, rest[0])
		args = rest[1:]
	}
}

func oneLine(s string) string { return strings.Join(strings.Fields(s), " ") }

func shortID(id string) string {
	if len(id) > 12 {
		return id[:12]
	}
	return id
}
