// Package polterware is Dispatch's link to Polterware: it attaches to
// Polterware's local core and reads it through DispatchHostService, the only
// Polterware contract Dispatch imports. Without Polterware every call answers
// ErrNoHost and Dispatch works on folders alone.
package polterware

import (
	"context"
	"errors"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"sync"
	"syscall"
	"time"

	"connectrpc.com/connect"

	localv1 "github.com/polterware/dispatch/go-core/platform/gen/polterware/local/v1"
	"github.com/polterware/dispatch/go-core/platform/gen/polterware/local/v1/localv1connect"
	"github.com/polterware/dispatch/go-core/platform/runstate"
)

// ErrNoHost means Polterware is not installed or its core did not answer.
var ErrNoHost = errors.New("polterware is not available")

type Project struct {
	ID   string
	Name string
}

// Run is what Polterware settles for a run: where it works, its area, and the
// card text the classifier reads.
type Run struct {
	Dir      string
	Kinds    []string
	Tech     []string
	CardText string
}

type Card struct {
	Slug, Title, Milestone, Stage string
}

type Repository struct {
	Name, Remote, LocalPath string
}

// Host is what Dispatch reads from Polterware.
type Host interface {
	Projects(ctx context.Context) ([]Project, error)
	ResolveProject(ctx context.Context, id, dir string) (Project, error)
	ResolveRun(ctx context.Context, projectID, cardSlug, repo, dir string) (Run, error)
	Cards(ctx context.Context, projectID string) ([]Card, []Repository, error)
	StartAgentRun(ctx context.Context, projectID, task, cardSlug string) (string, error)
	CompleteAgentRun(ctx context.Context, id string) error
}

// None is the host when Polterware is absent.
type None struct{}

func (None) Projects(context.Context) ([]Project, error) { return nil, ErrNoHost }
func (None) ResolveProject(context.Context, string, string) (Project, error) {
	return Project{}, ErrNoHost
}
func (None) ResolveRun(context.Context, string, string, string, string) (Run, error) {
	return Run{}, ErrNoHost
}
func (None) Cards(context.Context, string) ([]Card, []Repository, error) { return nil, nil, ErrNoHost }
func (None) StartAgentRun(context.Context, string, string, string) (string, error) {
	return "", ErrNoHost
}
func (None) CompleteAgentRun(context.Context, string) error { return ErrNoHost }

// DefaultDB is Polterware's database: POLTERWARE_CORE_DB, else the one the
// Polterware Desktop and CLI share (<user config dir>/polterware/local.db).
func DefaultDB() string {
	if db := strings.TrimSpace(os.Getenv("POLTERWARE_CORE_DB")); db != "" {
		return db
	}
	dir, err := os.UserConfigDir()
	if err != nil {
		home, _ := os.UserHomeDir()
		dir = filepath.Join(home, ".config")
	}
	return filepath.Join(dir, "polterware", "local.db")
}

// Client attaches to Polterware's core for DBPath on each call: a running
// core through its core.json (the same layout Dispatch uses), else one it
// starts with the `polterware` binary on PATH.
type Client struct {
	DBPath string
	// Binary is the polterware executable; empty looks it up on PATH.
	Binary string

	mu sync.Mutex
}

func (c *Client) service(ctx context.Context) (localv1connect.DispatchHostServiceClient, error) {
	c.mu.Lock()
	defer c.mu.Unlock()
	core, ok := runstate.ReadCore(c.DBPath)
	if !ok {
		var err error
		if core, err = c.start(ctx); err != nil {
			return nil, err
		}
	}
	httpClient := &http.Client{Timeout: 10 * time.Second}
	return localv1connect.NewDispatchHostServiceClient(httpClient, strings.TrimSuffix(core.Endpoint, "/"),
		connect.WithInterceptors(bearer{token: core.Token})), nil
}

// start launches Polterware's core when the database exists, so a person who
// never opened Polterware is not given one.
func (c *Client) start(ctx context.Context) (runstate.Core, error) {
	if _, err := os.Stat(c.DBPath); err != nil {
		return runstate.Core{}, ErrNoHost
	}
	bin := c.Binary
	if bin == "" {
		found, err := exec.LookPath("polterware")
		if err != nil {
			return runstate.Core{}, ErrNoHost
		}
		bin = found
	}
	cmd := exec.Command(bin, "-db", c.DBPath, "serve", "--listen", "127.0.0.1:0")
	cmd.SysProcAttr = &syscall.SysProcAttr{Setsid: true}
	if err := cmd.Start(); err != nil {
		return runstate.Core{}, ErrNoHost
	}
	go func() { _ = cmd.Wait() }()
	deadline := time.Now().Add(10 * time.Second)
	for time.Now().Before(deadline) {
		if core, ok := runstate.ReadCore(c.DBPath); ok {
			return core, nil
		}
		select {
		case <-ctx.Done():
			return runstate.Core{}, ctx.Err()
		case <-time.After(100 * time.Millisecond):
		}
	}
	return runstate.Core{}, ErrNoHost
}

// unavailable maps a transport failure to ErrNoHost and keeps the core's own
// answers (an unknown project, a repository to pick).
func unavailable(err error) error {
	var ce *connect.Error
	if errors.As(err, &ce) && ce.Code() != connect.CodeUnavailable && ce.Code() != connect.CodeUnimplemented && ce.Code() != connect.CodeUnknown {
		return errors.New(ce.Message())
	}
	return ErrNoHost
}

func (c *Client) Projects(ctx context.Context) ([]Project, error) {
	svc, err := c.service(ctx)
	if err != nil {
		return nil, err
	}
	res, err := svc.ListHostProjects(ctx, connect.NewRequest(&localv1.ListHostProjectsRequest{}))
	if err != nil {
		return nil, unavailable(err)
	}
	out := make([]Project, 0, len(res.Msg.GetProjects()))
	for _, p := range res.Msg.GetProjects() {
		out = append(out, Project{ID: p.GetId(), Name: p.GetName()})
	}
	return out, nil
}

func (c *Client) ResolveProject(ctx context.Context, id, dir string) (Project, error) {
	svc, err := c.service(ctx)
	if err != nil {
		return Project{}, err
	}
	res, err := svc.ResolveHostProject(ctx, connect.NewRequest(&localv1.ResolveHostProjectRequest{Id: id, Dir: dir}))
	if err != nil {
		return Project{}, unavailable(err)
	}
	p := res.Msg.GetProject()
	return Project{ID: p.GetId(), Name: p.GetName()}, nil
}

func (c *Client) ResolveRun(ctx context.Context, projectID, cardSlug, repo, dir string) (Run, error) {
	svc, err := c.service(ctx)
	if err != nil {
		return Run{}, err
	}
	res, err := svc.ResolveRun(ctx, connect.NewRequest(&localv1.ResolveRunRequest{ProjectId: projectID, CardSlug: cardSlug, Repo: repo, Dir: dir}))
	if err != nil {
		return Run{}, unavailable(err)
	}
	m := res.Msg
	return Run{Dir: m.GetDir(), Kinds: m.GetKinds(), Tech: m.GetTech(), CardText: m.GetCardText()}, nil
}

func (c *Client) Cards(ctx context.Context, projectID string) ([]Card, []Repository, error) {
	svc, err := c.service(ctx)
	if err != nil {
		return nil, nil, err
	}
	res, err := svc.ListHostCards(ctx, connect.NewRequest(&localv1.ListHostCardsRequest{ProjectId: projectID}))
	if err != nil {
		return nil, nil, unavailable(err)
	}
	cards := make([]Card, 0, len(res.Msg.GetCards()))
	for _, card := range res.Msg.GetCards() {
		cards = append(cards, Card{Slug: card.GetSlug(), Title: card.GetTitle(), Milestone: card.GetMilestone(), Stage: card.GetStage()})
	}
	repos := make([]Repository, 0, len(res.Msg.GetRepositories()))
	for _, r := range res.Msg.GetRepositories() {
		repos = append(repos, Repository{Name: r.GetName(), Remote: r.GetRemote(), LocalPath: r.GetLocalPath()})
	}
	return cards, repos, nil
}

func (c *Client) StartAgentRun(ctx context.Context, projectID, task, cardSlug string) (string, error) {
	svc, err := c.service(ctx)
	if err != nil {
		return "", err
	}
	res, err := svc.StartHostAgentRun(ctx, connect.NewRequest(&localv1.StartHostAgentRunRequest{ProjectId: projectID, Task: task, CardSlug: cardSlug}))
	if err != nil {
		return "", unavailable(err)
	}
	return res.Msg.GetAgentRunId(), nil
}

func (c *Client) CompleteAgentRun(ctx context.Context, id string) error {
	svc, err := c.service(ctx)
	if err != nil {
		return err
	}
	if _, err := svc.CompleteHostAgentRun(ctx, connect.NewRequest(&localv1.CompleteHostAgentRunRequest{AgentRunId: id})); err != nil {
		return unavailable(err)
	}
	return nil
}

type bearer struct{ token string }

func (b bearer) WrapUnary(next connect.UnaryFunc) connect.UnaryFunc {
	return func(ctx context.Context, request connect.AnyRequest) (connect.AnyResponse, error) {
		request.Header().Set("Authorization", "Bearer "+b.token)
		request.Header().Set("Polterware-Client", "dispatch")
		return next(ctx, request)
	}
}

func (b bearer) WrapStreamingClient(next connect.StreamingClientFunc) connect.StreamingClientFunc {
	return next
}

func (b bearer) WrapStreamingHandler(next connect.StreamingHandlerFunc) connect.StreamingHandlerFunc {
	return next
}
