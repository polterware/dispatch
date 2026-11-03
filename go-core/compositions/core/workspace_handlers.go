package core

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"errors"
	"os"
	"path/filepath"
	"strings"
	"time"

	"connectrpc.com/connect"

	"github.com/polterware/dispatch/go-core/domains/dispatch"
	dispatchv1 "github.com/polterware/dispatch/go-core/platform/gen/dispatch/v1"
	"github.com/polterware/dispatch/go-core/platform/gen/dispatch/v1/dispatchv1connect"
	"github.com/polterware/dispatch/go-core/providers/polterware"
	storepkg "github.com/polterware/dispatch/go-core/providers/sqlite"
)

var _ dispatchv1connect.WorkspaceServiceHandler = (*Core)(nil)

// hostTimeout bounds one read of Polterware, so a slow host never holds a run.
const hostTimeout = 5 * time.Second

// workspace returns the workspace with id. A Polterware project Dispatch has
// not seen yet becomes a linked workspace with the project's id.
func (core *Core) workspace(ctx context.Context, id string) (storepkg.Workspace, error) {
	ws, err := core.Store.GetWorkspace(id)
	if !errors.Is(err, storepkg.ErrNoWorkspace) {
		return ws, err
	}
	hctx, cancel := context.WithTimeout(ctx, hostTimeout)
	defer cancel()
	project, herr := core.Host.ResolveProject(hctx, id, "")
	if herr != nil || project.ID != id {
		return storepkg.Workspace{}, errors.New("unknown workspace " + id)
	}
	ws = storepkg.Workspace{ID: project.ID, Name: project.Name, PolterwareProjectID: project.ID}
	return ws, core.Store.SaveWorkspace(ws)
}

// workspaceAt returns the workspace that holds dir: the Polterware project
// whose repository is cloned there, else a folder added before, else a new
// standalone workspace for dir itself.
func (core *Core) workspaceAt(ctx context.Context, dir, name string) (storepkg.Workspace, error) {
	dir = filepath.Clean(expandPath(dir))
	hctx, cancel := context.WithTimeout(ctx, hostTimeout)
	defer cancel()
	if project, err := core.Host.ResolveProject(hctx, "", dir); err == nil && project.ID != "" {
		ws, err := core.Store.GetWorkspace(project.ID)
		if errors.Is(err, storepkg.ErrNoWorkspace) {
			ws = storepkg.Workspace{ID: project.ID, Name: project.Name, PolterwareProjectID: project.ID}
			err = core.Store.SaveWorkspace(ws)
		}
		return ws, err
	}
	if ws, err := core.Store.WorkspaceAt(dir); !errors.Is(err, storepkg.ErrNoWorkspace) {
		return ws, err
	}
	if info, err := os.Stat(dir); err != nil || !info.IsDir() {
		return storepkg.Workspace{}, errors.New("not a folder: " + dir)
	}
	if strings.TrimSpace(name) == "" {
		name = filepath.Base(dir)
	}
	sum := sha256.Sum256([]byte(dir))
	ws := storepkg.Workspace{ID: "ws-" + hex.EncodeToString(sum[:6]), Name: name, Path: dir}
	return ws, core.Store.SaveWorkspace(ws)
}

// hostRun settles a run in ws: a linked workspace asks Polterware for the
// directory (the request's, else the repository's clone, else the card's
// component repository, else the only one), the area, and the card text; a
// standalone one, or Polterware being away, runs in dir or the folder.
func (core *Core) hostRun(ctx context.Context, ws storepkg.Workspace, cardSlug, repo, dir string) (polterware.Run, error) {
	if ws.PolterwareProjectID == "" {
		if dir == "" {
			dir = ws.Path
		}
		if dir == "" {
			return polterware.Run{}, errors.New("the run needs a directory")
		}
		return polterware.Run{Dir: dir}, nil
	}
	hctx, cancel := context.WithTimeout(ctx, hostTimeout)
	defer cancel()
	run, err := core.Host.ResolveRun(hctx, ws.PolterwareProjectID, cardSlug, repo, dir)
	if errors.Is(err, polterware.ErrNoHost) {
		if dir == "" {
			return polterware.Run{}, errors.New("Polterware is not running: pass the run's directory")
		}
		return polterware.Run{Dir: dir}, nil
	}
	if err != nil {
		return polterware.Run{}, err
	}
	if run.Dir == "" {
		run.Dir = dir
	}
	return run, nil
}

// runHints is the area and card text of a dry run (Route, Classify); an
// unknown workspace or an absent Polterware gives none.
func (core *Core) runHints(ctx context.Context, workspaceID, cardSlug, dir string) polterware.Run {
	workspaceID = strings.TrimSpace(workspaceID)
	if workspaceID == "" {
		return polterware.Run{}
	}
	ws, err := core.Store.GetWorkspace(workspaceID)
	if err != nil || ws.PolterwareProjectID == "" {
		return polterware.Run{}
	}
	run, err := core.hostRun(ctx, ws, strings.TrimSpace(cardSlug), "", dir)
	if err != nil {
		return polterware.Run{}
	}
	return run
}

func runArea(run polterware.Run) dispatch.Area {
	return dispatch.NewArea(strings.Join(run.Kinds, ","), strings.Join(run.Tech, ","))
}

func workspaceProto(ws storepkg.Workspace) *dispatchv1.Workspace {
	return &dispatchv1.Workspace{
		Id:                  ws.ID,
		Name:                ws.Name,
		Path:                ws.Path,
		PolterwareProjectId: ws.PolterwareProjectID,
		CreatedAt:           ws.CreatedAt.UTC().Format(time.RFC3339),
	}
}

// ListWorkspaces lists Polterware's projects (kept as linked workspaces) and
// the folders added on their own.
func (core *Core) ListWorkspaces(ctx context.Context, _ *connect.Request[dispatchv1.ListWorkspacesRequest]) (*connect.Response[dispatchv1.ListWorkspacesResponse], error) {
	hctx, cancel := context.WithTimeout(ctx, hostTimeout)
	defer cancel()
	projects, herr := core.Host.Projects(hctx)
	linked := herr == nil
	for _, p := range projects {
		ws, err := core.Store.GetWorkspace(p.ID)
		if errors.Is(err, storepkg.ErrNoWorkspace) || (err == nil && ws.Name != p.Name) {
			ws = storepkg.Workspace{ID: p.ID, Name: p.Name, PolterwareProjectID: p.ID, CreatedAt: ws.CreatedAt}
			if err := core.Store.SaveWorkspace(ws); err != nil {
				return nil, internal(err)
			}
		} else if err != nil {
			return nil, internal(err)
		}
	}
	live := map[string]bool{}
	for _, p := range projects {
		live[p.ID] = true
	}
	items, err := core.Store.ListWorkspaces()
	if err != nil {
		return nil, internal(err)
	}
	out := &dispatchv1.ListWorkspacesResponse{Linked: linked}
	for _, ws := range items {
		// A project Polterware no longer lists (archived) is hidden while it answers.
		if linked && ws.PolterwareProjectID != "" && !live[ws.PolterwareProjectID] {
			continue
		}
		out.Workspaces = append(out.Workspaces, workspaceProto(ws))
	}
	return connect.NewResponse(out), nil
}

func (core *Core) AddWorkspace(ctx context.Context, request *connect.Request[dispatchv1.AddWorkspaceRequest]) (*connect.Response[dispatchv1.AddWorkspaceResponse], error) {
	path := strings.TrimSpace(request.Msg.GetPath())
	if path == "" {
		return nil, invalid("path is required")
	}
	ws, err := core.workspaceAt(ctx, path, request.Msg.GetName())
	if err != nil {
		return nil, invalid(err.Error())
	}
	return connect.NewResponse(&dispatchv1.AddWorkspaceResponse{Workspace: workspaceProto(ws)}), nil
}

// RemoveWorkspace forgets a standalone folder; its runs stay in the store.
func (core *Core) RemoveWorkspace(_ context.Context, request *connect.Request[dispatchv1.RemoveWorkspaceRequest]) (*connect.Response[dispatchv1.RemoveWorkspaceResponse], error) {
	ws, err := core.Store.GetWorkspace(strings.TrimSpace(request.Msg.GetId()))
	if err != nil {
		return nil, invalid(err.Error())
	}
	if ws.PolterwareProjectID != "" {
		return nil, invalid("a Polterware project's workspace follows the project")
	}
	if err := core.Store.DeleteWorkspace(ws.ID); err != nil {
		return nil, internal(err)
	}
	return connect.NewResponse(&dispatchv1.RemoveWorkspaceResponse{}), nil
}

func (core *Core) ResolveWorkspace(ctx context.Context, request *connect.Request[dispatchv1.ResolveWorkspaceRequest]) (*connect.Response[dispatchv1.ResolveWorkspaceResponse], error) {
	if id := strings.TrimSpace(request.Msg.GetId()); id != "" {
		ws, err := core.workspace(ctx, id)
		if err != nil {
			return nil, invalid(err.Error())
		}
		return connect.NewResponse(&dispatchv1.ResolveWorkspaceResponse{Workspace: workspaceProto(ws)}), nil
	}
	dir := strings.TrimSpace(request.Msg.GetDir())
	if dir == "" {
		return nil, invalid("id or dir is required")
	}
	ws, err := core.workspaceAt(ctx, dir, "")
	if err != nil {
		return nil, invalid(err.Error())
	}
	return connect.NewResponse(&dispatchv1.ResolveWorkspaceResponse{Workspace: workspaceProto(ws)}), nil
}

// ListWorkspaceCards lists a linked workspace's open cards and repositories;
// a standalone workspace, or an absent Polterware, has none.
func (core *Core) ListWorkspaceCards(ctx context.Context, request *connect.Request[dispatchv1.ListWorkspaceCardsRequest]) (*connect.Response[dispatchv1.ListWorkspaceCardsResponse], error) {
	out := &dispatchv1.ListWorkspaceCardsResponse{}
	ws, err := core.Store.GetWorkspace(strings.TrimSpace(request.Msg.GetWorkspaceId()))
	if err != nil || ws.PolterwareProjectID == "" {
		return connect.NewResponse(out), nil
	}
	hctx, cancel := context.WithTimeout(ctx, hostTimeout)
	defer cancel()
	cards, repos, err := core.Host.Cards(hctx, ws.PolterwareProjectID)
	if err != nil {
		return connect.NewResponse(out), nil
	}
	for _, c := range cards {
		out.Cards = append(out.Cards, &dispatchv1.WorkspaceCard{Slug: c.Slug, Title: c.Title, Milestone: c.Milestone, Stage: c.Stage})
	}
	for _, r := range repos {
		out.Repositories = append(out.Repositories, &dispatchv1.WorkspaceRepository{Name: r.Name, Remote: r.Remote, LocalPath: r.LocalPath})
	}
	return connect.NewResponse(out), nil
}

// expandPath resolves a leading ~ to the home folder.
func expandPath(path string) string {
	path = strings.TrimSpace(path)
	if path == "~" || strings.HasPrefix(path, "~/") {
		if home, err := os.UserHomeDir(); err == nil {
			return filepath.Join(home, strings.TrimPrefix(path, "~"))
		}
	}
	return path
}
