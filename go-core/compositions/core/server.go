// Package core is Polterware Dispatch's local core (`dispatch serve`): the
// only process that opens the dispatch store. The CLI, the Desktop, and
// Polterware attach to it through core.json.
package core

import (
	"context"
	"crypto/rand"
	"crypto/subtle"
	"encoding/hex"
	"fmt"
	"net"
	"net/http"
	"os"
	"strings"
	"sync"
	"sync/atomic"
	"time"

	"connectrpc.com/connect"
	"golang.org/x/net/http2"
	"golang.org/x/net/http2/h2c"

	"github.com/polterware/dispatch/go-core/domains/dispatch"
	"github.com/polterware/dispatch/go-core/platform/coreclient"
	dispatchv1 "github.com/polterware/dispatch/go-core/platform/gen/dispatch/v1"
	"github.com/polterware/dispatch/go-core/platform/gen/dispatch/v1/dispatchv1connect"
	"github.com/polterware/dispatch/go-core/providers/polterware"
	storepkg "github.com/polterware/dispatch/go-core/providers/sqlite"
)

type Core struct {
	Store   *storepkg.Store
	Token   string
	Started time.Time
	// Build identifies the core binary (coreclient.BuildID).
	Build string
	// DBDir is the database directory; the dispatch state (tiers.json, the
	// catalog prices, and live logs) lives in its dispatch folder.
	DBDir string
	// Host reads Polterware: projects, run directories, cards, and AgentRuns.
	// Without Polterware it answers polterware.ErrNoHost.
	Host polterware.Host

	life lifecycle

	// Dispatch coordination is per core. Maps are lazy-initialized so a zero
	// Core does not share cancel, watch, or finalize state with another core.
	dispatchOnce      sync.Once
	dispatchStartMu   sync.Mutex
	dispatchCancelMu  sync.Mutex
	dispatchWatchMu   sync.Mutex
	dispatchFinishMu  sync.Mutex
	dispatchCancels   map[string]context.CancelFunc
	dispatchFinish    map[string]*sync.Mutex
	dispatchLogOff    map[string]int64
	dispatchLogReady  map[string]struct{}
	dispatchFinalized map[string]struct{}
}

// NewCore wires the core around the only open store of this database.
func NewCore(store *storepkg.Store, token string, host polterware.Host) *Core {
	if host == nil {
		host = polterware.None{}
	}
	core := &Core{
		Store:   store,
		Token:   token,
		Started: time.Now().UTC(),
		Host:    host,
		life:    lifecycle{done: make(chan struct{})},
	}
	core.touch()
	return core
}

func MintToken() (string, error) {
	var value [32]byte
	if _, err := rand.Read(value[:]); err != nil {
		return "", err
	}
	return hex.EncodeToString(value[:]), nil
}

func Listen(address string) (net.Listener, string, error) {
	listener, err := net.Listen("tcp", address)
	if err != nil {
		return nil, "", err
	}
	return listener, "http://" + listener.Addr().String(), nil
}

func ServeConnect(listener net.Listener, core *Core) *http.Server {
	// Runs left RUNNING by a dead core become ORPHANED before the server accepts work.
	core.reconcileDispatchOnBoot()
	_ = dispatch.ApplyAgentConfig(core.dispatchStateDir())
	mux := http.NewServeMux()
	options := []connect.HandlerOption{connect.WithInterceptors(interceptor{core: core})}
	mux.Handle(dispatchv1connect.NewHealthServiceHandler(core, options...))
	mux.Handle(dispatchv1connect.NewDispatchServiceHandler(core, options...))
	mux.Handle(dispatchv1connect.NewWorkspaceServiceHandler(core, options...))
	server := &http.Server{Handler: h2c.NewHandler(mux, &http2.Server{}), ReadHeaderTimeout: 5 * time.Second}
	go func() { _ = server.Serve(listener) }()
	return server
}

// lifecycle tracks what keeps the daemon alive: recent requests and open run
// streams.
type lifecycle struct {
	lastActive atomic.Int64
	streams    atomic.Int32
	done       chan struct{}
	stopOnce   sync.Once
}

func (core *Core) touch() { core.life.lastActive.Store(time.Now().UnixNano()) }

// Done closes when the core was asked to stop.
func (core *Core) Done() <-chan struct{} { return core.life.done }

// Stop asks the core to shut down; runServe owns the actual exit.
func (core *Core) Stop() { core.life.stopOnce.Do(func() { close(core.life.done) }) }

// Idle reports whether nothing used the core for at least after.
func (core *Core) Idle(after time.Duration) bool {
	if core.life.streams.Load() > 0 {
		return false
	}
	return time.Since(time.Unix(0, core.life.lastActive.Load())) >= after
}

// clientOf names the calling client from the client header; the Desktop is
// the default.
func clientOf(header http.Header) string {
	if client := strings.TrimSpace(header.Get(coreclient.ClientHeader)); client != "" {
		return client
	}
	return "desktop"
}

// interceptor authenticates every unary and streaming call with the session
// token and records activity for idle exit. Health.Check answers without it,
// so a client can tell a live core from a stale core.json.
type interceptor struct{ core *Core }

func (i interceptor) authorize(header http.Header) error {
	value := header.Get("Authorization")
	candidate := strings.TrimPrefix(value, "Bearer ")
	if !strings.HasPrefix(value, "Bearer ") || subtle.ConstantTimeCompare([]byte(candidate), []byte(i.core.Token)) != 1 {
		return connect.NewError(connect.CodeUnauthenticated, fmt.Errorf("invalid or missing session token"))
	}
	i.core.touch()
	return nil
}

func (i interceptor) WrapUnary(next connect.UnaryFunc) connect.UnaryFunc {
	return func(ctx context.Context, request connect.AnyRequest) (connect.AnyResponse, error) {
		if request.Spec().Procedure != dispatchv1connect.HealthServiceCheckProcedure {
			if err := i.authorize(request.Header()); err != nil {
				return nil, err
			}
		}
		return next(ctx, request)
	}
}

func (i interceptor) WrapStreamingClient(next connect.StreamingClientFunc) connect.StreamingClientFunc {
	return next
}

func (i interceptor) WrapStreamingHandler(next connect.StreamingHandlerFunc) connect.StreamingHandlerFunc {
	return func(ctx context.Context, conn connect.StreamingHandlerConn) error {
		if err := i.authorize(conn.RequestHeader()); err != nil {
			return err
		}
		i.core.life.streams.Add(1)
		defer func() {
			i.core.life.streams.Add(-1)
			i.core.touch()
		}()
		return next(ctx, conn)
	}
}

func (core *Core) Check(context.Context, *connect.Request[dispatchv1.CheckRequest]) (*connect.Response[dispatchv1.CheckResponse], error) {
	return connect.NewResponse(&dispatchv1.CheckResponse{
		Status:    "ok",
		Version:   "0.1.0",
		UptimeSec: time.Since(core.Started).Seconds(),
		Build:     core.Build,
		Pid:       int32(os.Getpid()),
	}), nil
}

func (core *Core) Shutdown(context.Context, *connect.Request[dispatchv1.ShutdownRequest]) (*connect.Response[dispatchv1.ShutdownResponse], error) {
	// Let the response reach the caller before the server stops.
	time.AfterFunc(100*time.Millisecond, core.Stop)
	return connect.NewResponse(&dispatchv1.ShutdownResponse{}), nil
}

func invalid(message string) error {
	return connect.NewError(connect.CodeInvalidArgument, fmt.Errorf("%s", message))
}

func internal(err error) error { return connect.NewError(connect.CodeInternal, err) }
