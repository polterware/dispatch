package main

import (
	"context"
	"errors"
	"flag"
	"fmt"
	"os"
	"os/signal"
	"path/filepath"
	"syscall"
	"time"

	"github.com/polterware/dispatch/go-core/compositions/core"
	"github.com/polterware/dispatch/go-core/platform/coreclient"
	"github.com/polterware/dispatch/go-core/platform/runstate"
	"github.com/polterware/dispatch/go-core/providers/polterware"
	storepkg "github.com/polterware/dispatch/go-core/providers/sqlite"
)

// runServe runs the core: the only process that opens the store. The app,
// the CLI, and Polterware attach to it through core.json.
func runServe(dbPath string, args []string) error {
	fs := flag.NewFlagSet("serve", flag.ContinueOnError)
	listenAddress := fs.String("listen", "127.0.0.1:0", "loopback listen address")
	sessionToken := fs.String("token", os.Getenv("PWDISPATCH_SESSION_TOKEN"), "session bearer token (default: minted)")
	idle := fs.Duration("idle", idleFromEnv(), "exit after this long without clients (0, the default, keeps running)")
	if err := fs.Parse(args); err != nil {
		return err
	}

	lock, err := runstate.LockCore(dbPath)
	if errors.Is(err, runstate.ErrCoreLocked) {
		if existing, ok := runstate.ReadCore(dbPath); ok {
			fmt.Fprintf(os.Stderr, "dispatch core already running for this database (pid %d, %s)\n", existing.PID, existing.Endpoint)
		} else {
			fmt.Fprintln(os.Stderr, "dispatch core already running for this database")
		}
		os.Exit(3)
	}
	if err != nil {
		return fmt.Errorf("core lock: %w", err)
	}
	defer lock.Close()
	adoptLoginPath()

	store, err := storepkg.New(dbPath)
	if err != nil {
		return fmt.Errorf("db open: %w", err)
	}
	defer store.Close()

	token := *sessionToken
	if token == "" {
		if token, err = core.MintToken(); err != nil {
			return fmt.Errorf("token: %w", err)
		}
	}
	ln, endpoint, err := core.Listen(*listenAddress)
	if err != nil {
		return fmt.Errorf("listen: %w", err)
	}
	fmt.Fprintf(os.Stderr, "dispatch core listening on %s with session-token authentication\n", endpoint)

	polterwareDB := polterware.DefaultDB()
	c := core.NewCore(store, token, &polterware.Client{DBPath: polterwareDB})
	c.DBDir = filepath.Dir(dbPath)
	if exe, err := coreclient.Executable(); err == nil {
		c.Build = coreclient.BuildID(exe)
	}
	if n, err := c.SeedFromPolterware(context.Background(), polterwareDB); err != nil {
		fmt.Fprintf(os.Stderr, "import from Polterware: %v\n", err)
	} else if n > 0 {
		fmt.Fprintf(os.Stderr, "imported %d tier entries from Polterware\n", n)
	}
	hs := core.ServeConnect(ln, c)

	if err := runstate.WriteJSON(runstate.CorePath(dbPath), runstate.Core{
		PID:       os.Getpid(),
		Endpoint:  endpoint,
		Token:     token,
		DBPath:    dbPath,
		Build:     c.Build,
		StartedAt: time.Now().UTC(),
	}); err != nil {
		fmt.Fprintf(os.Stderr, "run state: %v\n", err)
	}
	defer runstate.RemoveCore(dbPath, os.Getpid())

	sig := make(chan os.Signal, 1)
	signal.Notify(sig, syscall.SIGINT, syscall.SIGTERM, syscall.SIGHUP)
	ticker := time.NewTicker(30 * time.Second)
	defer ticker.Stop()
wait:
	for {
		select {
		case <-sig:
			break wait
		case <-c.Done():
			break wait
		case <-ticker.C:
			if *idle > 0 && c.Idle(*idle) {
				fmt.Fprintf(os.Stderr, "dispatch core idle for %s; exiting\n", *idle)
				break wait
			}
		}
	}
	c.Stop()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	_ = hs.Shutdown(ctx)
	return nil
}

func runStop(dbPath string) error {
	if _, ok := runstate.ReadCore(dbPath); !ok {
		fmt.Println("no dispatch core is running for this database")
		return nil
	}
	if err := coreclient.Stop(dbPath); err != nil {
		return err
	}
	fmt.Println("dispatch core stopped")
	return nil
}

func idleFromEnv() time.Duration {
	if value := os.Getenv("PWDISPATCH_CORE_IDLE"); value != "" {
		if d, err := time.ParseDuration(value); err == nil {
			return d
		}
	}
	return 0
}
