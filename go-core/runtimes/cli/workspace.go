package main

import (
	"context"
	"errors"
	"flag"
	"fmt"
	"os"
	"path/filepath"

	"connectrpc.com/connect"

	dispatchv1 "github.com/polterware/dispatch/go-core/platform/gen/dispatch/v1"
)

const workspaceUsage = `usage: dispatch workspace ls [--json]
       dispatch workspace add <folder> [--name <name>]
       dispatch workspace rm <id>`

// runWorkspace lists, adds, and removes workspaces. Polterware's projects are
// listed while Polterware answers; a folder is added on its own.
func runWorkspace(dbPath string, args []string) error {
	if len(args) == 0 {
		args = []string{"ls"}
	}
	sub, rest := args[0], args[1:]
	flags := flag.NewFlagSet("workspace "+sub, flag.ContinueOnError)
	flags.SetOutput(os.Stderr)
	asJSON := flags.Bool("json", false, "print JSON")
	name := flags.String("name", "", "workspace name (default: the folder's)")
	positional, err := parseInterspersed(flags, rest)
	if err != nil {
		return err
	}
	c, err := dial(dbPath)
	if err != nil {
		return err
	}
	ctx := context.Background()
	switch sub {
	case "ls", "list":
		res, err := c.Workspaces.ListWorkspaces(ctx, connect.NewRequest(&dispatchv1.ListWorkspacesRequest{}))
		if err != nil {
			return err
		}
		if *asJSON {
			return writeJSON(res.Msg)
		}
		if !res.Msg.GetLinked() {
			fmt.Println("Polterware is not running: only folders are listed")
		}
		if len(res.Msg.GetWorkspaces()) == 0 {
			fmt.Println("no workspaces; `dispatch run` in a folder adds it")
		}
		for _, ws := range res.Msg.GetWorkspaces() {
			where := ws.GetPath()
			if ws.GetPolterwareProjectId() != "" {
				where = "Polterware project"
			}
			fmt.Printf("%-16s  %-24s  %s\n", ws.GetId(), ws.GetName(), where)
		}
		return nil
	case "add":
		if len(positional) != 1 {
			return errors.New(workspaceUsage)
		}
		path, err := filepath.Abs(positional[0])
		if err != nil {
			return err
		}
		res, err := c.Workspaces.AddWorkspace(ctx, connect.NewRequest(&dispatchv1.AddWorkspaceRequest{Path: path, Name: *name}))
		if err != nil {
			return err
		}
		ws := res.Msg.GetWorkspace()
		fmt.Printf("workspace %s (%s)\n", ws.GetId(), ws.GetName())
		return nil
	case "rm", "remove":
		if len(positional) != 1 {
			return errors.New(workspaceUsage)
		}
		if _, err := c.Workspaces.RemoveWorkspace(ctx, connect.NewRequest(&dispatchv1.RemoveWorkspaceRequest{Id: positional[0]})); err != nil {
			return err
		}
		fmt.Println("removed", positional[0])
		return nil
	default:
		return errors.New(workspaceUsage)
	}
}
