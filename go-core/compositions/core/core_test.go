package core

import (
	"context"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"testing"
	"time"

	"connectrpc.com/connect"

	"github.com/polterware/dispatch/go-core/domains/dispatch"
	dispatchv1 "github.com/polterware/dispatch/go-core/platform/gen/dispatch/v1"
	"github.com/polterware/dispatch/go-core/platform/gen/dispatch/v1/dispatchv1connect"
	"github.com/polterware/dispatch/go-core/providers/polterware"
	storepkg "github.com/polterware/dispatch/go-core/providers/sqlite"
)

const testToken = "test-session-token"

func startCore(t *testing.T) (*Core, string) {
	return startCoreWith(t, nil)
}

func startCoreWith(t *testing.T, host polterware.Host) (*Core, string) {
	t.Helper()
	t.Setenv("DISPATCH_HOME", t.TempDir())
	store, err := storepkg.New(filepath.Join(t.TempDir(), "dispatch.db"))
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = store.Close() })
	core := NewCore(store, testToken, host)
	listener, url, err := Listen("127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	server := ServeConnect(listener, core)
	t.Cleanup(func() { _ = server.Close() })
	return core, url
}

func withToken[T any](request *connect.Request[T], token string) *connect.Request[T] {
	if token != "" {
		request.Header().Set("Authorization", "Bearer "+token)
	}
	return request
}

// fakeHost is a Polterware with one project whose repository is cloned at dir.
type fakeHost struct {
	dir     string
	started *[]string
	done    *[]string
}

func (h fakeHost) Projects(context.Context) ([]polterware.Project, error) {
	return []polterware.Project{{ID: "proj-1", Name: "Shop"}}, nil
}

func (h fakeHost) ResolveProject(_ context.Context, id, dir string) (polterware.Project, error) {
	if id == "proj-1" || (id == "" && dir == h.dir) {
		return polterware.Project{ID: "proj-1", Name: "Shop"}, nil
	}
	return polterware.Project{}, polterware.ErrNoHost
}

func (h fakeHost) ResolveRun(_ context.Context, _, cardSlug, _, dir string) (polterware.Run, error) {
	if dir == "" {
		dir = h.dir
	}
	text := ""
	if cardSlug != "" {
		text = "## card: " + cardSlug
	}
	return polterware.Run{Dir: dir, Kinds: []string{"web-app"}, Tech: []string{"Go"}, CardText: text}, nil
}

func (h fakeHost) Cards(context.Context, string) ([]polterware.Card, []polterware.Repository, error) {
	return []polterware.Card{{Slug: "checkout", Title: "Checkout", Milestone: "v1", Stage: "build"}},
		[]polterware.Repository{{Name: "shop", LocalPath: h.dir}}, nil
}

func (h fakeHost) StartAgentRun(_ context.Context, _, task, _ string) (string, error) {
	*h.started = append(*h.started, task)
	return "agent-run-1", nil
}

func (h fakeHost) CompleteAgentRun(_ context.Context, id string) error {
	*h.done = append(*h.done, id)
	return nil
}

func gitRepo(t *testing.T) string {
	t.Helper()
	dir := t.TempDir()
	for _, args := range [][]string{{"init", "-q"}, {"-c", "user.email=t@t", "-c", "user.name=t", "commit", "-q", "--allow-empty", "-m", "init"}} {
		if out, err := exec.Command("git", append([]string{"-C", dir}, args...)...).CombinedOutput(); err != nil {
			t.Fatalf("git %v: %v %s", args, err, out)
		}
	}
	return dir
}

// onlyGit leaves git alone on PATH, so no agent CLI of this machine runs.
func onlyGit(t *testing.T) {
	t.Helper()
	git, err := exec.LookPath("git")
	if err != nil {
		t.Skip("git is required")
	}
	bin := t.TempDir()
	if err := os.Symlink(git, filepath.Join(bin, "git")); err != nil {
		t.Fatal(err)
	}
	t.Setenv("PATH", bin)
}

func TestWorkspacesListPolterwareProjectsAndFolders(t *testing.T) {
	repo := gitRepo(t)
	var started, done []string
	_, url := startCoreWith(t, fakeHost{dir: repo, started: &started, done: &done})
	client := dispatchv1connect.NewWorkspaceServiceClient(http.DefaultClient, url)
	ctx := context.Background()

	folder := t.TempDir()
	added, err := client.AddWorkspace(ctx, withToken(connect.NewRequest(&dispatchv1.AddWorkspaceRequest{Path: folder}), testToken))
	if err != nil || added.Msg.Workspace.Path != folder || added.Msg.Workspace.PolterwareProjectId != "" {
		t.Fatalf("add folder: %+v %v", added, err)
	}
	// A folder inside a Polterware project's clone resolves to the project.
	linked, err := client.AddWorkspace(ctx, withToken(connect.NewRequest(&dispatchv1.AddWorkspaceRequest{Path: repo}), testToken))
	if err != nil || linked.Msg.Workspace.Id != "proj-1" || linked.Msg.Workspace.PolterwareProjectId != "proj-1" {
		t.Fatalf("add clone: %+v %v", linked, err)
	}
	list, err := client.ListWorkspaces(ctx, withToken(connect.NewRequest(&dispatchv1.ListWorkspacesRequest{}), testToken))
	if err != nil || !list.Msg.Linked || len(list.Msg.Workspaces) != 2 {
		t.Fatalf("list: %+v %v", list, err)
	}
	cards, err := client.ListWorkspaceCards(ctx, withToken(connect.NewRequest(&dispatchv1.ListWorkspaceCardsRequest{WorkspaceId: "proj-1"}), testToken))
	if err != nil || len(cards.Msg.Cards) != 1 || cards.Msg.Cards[0].Slug != "checkout" || len(cards.Msg.Repositories) != 1 {
		t.Fatalf("cards: %+v %v", cards, err)
	}
	if _, err := client.RemoveWorkspace(ctx, withToken(connect.NewRequest(&dispatchv1.RemoveWorkspaceRequest{Id: "proj-1"}), testToken)); err == nil {
		t.Fatal("a linked workspace must not be removed")
	}
}

func TestStartRunInALinkedWorkspaceOpensAndClosesTheAgentRun(t *testing.T) {
	repo := gitRepo(t)
	var started, done []string
	core, url := startCoreWith(t, fakeHost{dir: repo, started: &started, done: &done})
	onlyGit(t)
	client := dispatchv1connect.NewDispatchServiceClient(http.DefaultClient, url)
	ctx := context.Background()

	if _, err := client.StartRun(ctx, withToken(connect.NewRequest(&dispatchv1.StartRunRequest{ProjectId: "elsewhere", Prompt: "x", Tier: "easy"}), testToken)); err == nil {
		t.Fatal("an unknown workspace must be refused")
	}
	res, err := client.StartRun(ctx, withToken(connect.NewRequest(&dispatchv1.StartRunRequest{ProjectId: "proj-1", Prompt: "fix the footer", Tier: "easy", CardSlug: "checkout", ReadOnly: true}), testToken))
	if err != nil {
		t.Fatal(err)
	}
	run := res.Msg.Run
	if real, _ := filepath.EvalSymlinks(repo); run.Dir != repo && run.Dir != real {
		t.Fatalf("dir %q, want the project's clone %q", run.Dir, repo)
	}
	if run.AgentRunId != "agent-run-1" || len(started) != 1 {
		t.Fatalf("agent run %q started=%v", run.AgentRunId, started)
	}
	deadline := time.Now().Add(20 * time.Second)
	for time.Now().Before(deadline) {
		if r, err := core.Store.GetRun(ctx, run.Id); err == nil && r.Status != dispatch.StatusPending && r.Status != dispatch.StatusRunning {
			break
		}
		time.Sleep(100 * time.Millisecond)
	}
	for time.Now().Before(deadline) && len(done) == 0 {
		time.Sleep(50 * time.Millisecond)
	}
	if len(done) != 1 || done[0] != "agent-run-1" {
		t.Fatalf("agent run not completed: %v", done)
	}
}

func TestStandaloneWorkspaceRunsInItsFolder(t *testing.T) {
	core, _ := startCore(t)
	folder := t.TempDir()
	ws, err := core.workspaceAt(context.Background(), folder, "")
	if err != nil || ws.Path != folder || ws.PolterwareProjectID != "" {
		t.Fatalf("%+v %v", ws, err)
	}
	run, err := core.hostRun(context.Background(), ws, "", "", "")
	if err != nil || run.Dir != folder || run.CardText != "" {
		t.Fatalf("%+v %v", run, err)
	}
	again, err := core.workspaceAt(context.Background(), filepath.Join(folder), "")
	if err != nil || again.ID != ws.ID {
		t.Fatalf("the same folder must give the same workspace: %+v %v", again, err)
	}
}
