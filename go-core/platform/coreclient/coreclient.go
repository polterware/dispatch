// Package coreclient finds, starts, and talks to the Dispatch core. The core
// (`dispatch serve`) is the only process that opens the dispatch store; the
// CLI and the Desktop reach it through core.json.
package coreclient

import (
	"context"
	"errors"
	"fmt"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"syscall"
	"time"

	"connectrpc.com/connect"

	dispatchv1 "github.com/polterware/dispatch/go-core/platform/gen/dispatch/v1"
	"github.com/polterware/dispatch/go-core/platform/gen/dispatch/v1/dispatchv1connect"
	"github.com/polterware/dispatch/go-core/platform/runstate"
)

// ClientHeader names the calling client (desktop, cli, polterware).
const ClientHeader = "Pwdispatch-Client"

// Executable resolves this binary; the core is its own `serve` subcommand.
func Executable() (string, error) {
	exe, err := os.Executable()
	if err != nil {
		return "", fmt.Errorf("resolve dispatch executable: %w", err)
	}
	if resolved, err := filepath.EvalSymlinks(exe); err == nil {
		exe = resolved
	}
	return exe, nil
}

// BuildID identifies a binary by modification time and size
// ("<unix seconds>-<bytes>"). The Desktop computes the same value.
func BuildID(exe string) string {
	info, err := os.Stat(exe)
	if err != nil {
		return ""
	}
	return fmt.Sprintf("%d-%d", info.ModTime().Unix(), info.Size())
}

// NewerBuild reports whether build a is a newer binary than build b.
func NewerBuild(a, b string) bool {
	return buildTime(a) > buildTime(b)
}

func buildTime(build string) int64 {
	head, _, _ := strings.Cut(build, "-")
	value, _ := strconv.ParseInt(head, 10, 64)
	return value
}

// Ensure returns a live core for dbPath. It starts one when none is running
// and replaces one that runs an older build than this binary.
func Ensure(dbPath string) (runstate.Core, error) {
	exe, err := Executable()
	if err != nil {
		return runstate.Core{}, err
	}
	build := BuildID(exe)
	for attempt := 0; attempt < 3; attempt++ {
		core, ok := runstate.ReadCore(dbPath)
		if !ok {
			return Start(exe, dbPath)
		}
		ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
		health, err := New(core, "cli").Health.Check(ctx, connect.NewRequest(&dispatchv1.CheckRequest{}))
		cancel()
		if err != nil {
			// Listening but not answering yet (just started): retry shortly.
			time.Sleep(200 * time.Millisecond)
			continue
		}
		if NewerBuild(build, health.Msg.Build) {
			if err := Stop(dbPath); err != nil {
				return runstate.Core{}, fmt.Errorf("replace older core: %w", err)
			}
			continue
		}
		return core, nil
	}
	return runstate.Core{}, fmt.Errorf("core for %s is not responding; see %s", dbPath, runstate.LogPath(dbPath))
}

// Start launches a detached core and waits for its core.json. When another
// core wins the start race, that core is returned.
func Start(exe, dbPath string) (runstate.Core, error) {
	if core, ok := runstate.ReadCore(dbPath); ok {
		return core, nil
	}
	if err := runstate.EnsureDir(dbPath); err != nil {
		return runstate.Core{}, err
	}
	logFile, err := os.OpenFile(runstate.LogPath(dbPath), os.O_CREATE|os.O_WRONLY|os.O_APPEND, 0o600)
	if err != nil {
		return runstate.Core{}, err
	}
	defer logFile.Close()

	cmd := exec.Command(exe, "-db", dbPath, "serve", "--listen", "127.0.0.1:0")
	cmd.Stdout = logFile
	cmd.Stderr = logFile
	cmd.SysProcAttr = &syscall.SysProcAttr{Setsid: true}
	if err := cmd.Start(); err != nil {
		return runstate.Core{}, err
	}
	exited := make(chan error, 1)
	go func() { exited <- cmd.Wait() }()

	deadline := time.After(15 * time.Second)
	for {
		if core, ok := runstate.ReadCore(dbPath); ok {
			return core, nil
		}
		select {
		case err := <-exited:
			// Exit 3: another core took the lock first; attach to it.
			for i := 0; i < 20; i++ {
				if core, ok := runstate.ReadCore(dbPath); ok {
					return core, nil
				}
				time.Sleep(100 * time.Millisecond)
			}
			return runstate.Core{}, fmt.Errorf("core exited before listening (%v); see %s", err, runstate.LogPath(dbPath))
		case <-deadline:
			return runstate.Core{}, fmt.Errorf("timed out waiting for core; see %s", runstate.LogPath(dbPath))
		case <-time.After(100 * time.Millisecond):
		}
	}
}

// Stop asks the running core to shut down and waits for it to exit.
func Stop(dbPath string) error {
	core, ok := runstate.ReadCore(dbPath)
	if !ok {
		return nil
	}
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()
	if _, err := New(core, "cli").Health.Shutdown(ctx, connect.NewRequest(&dispatchv1.ShutdownRequest{Reason: "stop"})); err != nil {
		if err := syscall.Kill(core.PID, syscall.SIGTERM); err != nil && !errors.Is(err, syscall.ESRCH) {
			return err
		}
	}
	for i := 0; i < 50; i++ {
		if !runstate.PIDAlive(core.PID) {
			return nil
		}
		time.Sleep(100 * time.Millisecond)
	}
	return fmt.Errorf("core pid %d did not exit", core.PID)
}

// Clients are the typed Connect clients of one core.
type Clients struct {
	Health     dispatchv1connect.HealthServiceClient
	Dispatch   dispatchv1connect.DispatchServiceClient
	Workspaces dispatchv1connect.WorkspaceServiceClient
}

// New builds clients for core; client names the caller (ClientHeader).
func New(core runstate.Core, client string) Clients {
	httpClient := &http.Client{}
	options := connect.WithInterceptors(HeaderInterceptor{Token: core.Token, Client: client})
	base := strings.TrimSuffix(core.Endpoint, "/")
	return Clients{
		Health:     dispatchv1connect.NewHealthServiceClient(httpClient, base, options),
		Dispatch:   dispatchv1connect.NewDispatchServiceClient(httpClient, base, options),
		Workspaces: dispatchv1connect.NewWorkspaceServiceClient(httpClient, base, options),
	}
}

// Connect ensures a core for dbPath and returns clients for it.
func Connect(dbPath, client string) (Clients, error) {
	core, err := Ensure(dbPath)
	if err != nil {
		return Clients{}, err
	}
	return New(core, client), nil
}

// HeaderInterceptor sends the bearer token and the client name.
type HeaderInterceptor struct {
	Token  string
	Client string
}

func (h HeaderInterceptor) set(header http.Header) {
	header.Set("Authorization", "Bearer "+h.Token)
	if h.Client != "" {
		header.Set(ClientHeader, h.Client)
	}
}

func (h HeaderInterceptor) WrapUnary(next connect.UnaryFunc) connect.UnaryFunc {
	return func(ctx context.Context, request connect.AnyRequest) (connect.AnyResponse, error) {
		h.set(request.Header())
		return next(ctx, request)
	}
}

func (h HeaderInterceptor) WrapStreamingClient(next connect.StreamingClientFunc) connect.StreamingClientFunc {
	return func(ctx context.Context, spec connect.Spec) connect.StreamingClientConn {
		conn := next(ctx, spec)
		h.set(conn.RequestHeader())
		return conn
	}
}

func (h HeaderInterceptor) WrapStreamingHandler(next connect.StreamingHandlerFunc) connect.StreamingHandlerFunc {
	return next
}
