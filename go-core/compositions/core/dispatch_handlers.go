package core

import (
	"context"
	"database/sql"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"sync"
	"time"
	"unicode/utf8"

	"connectrpc.com/connect"

	"github.com/polterware/dispatch/go-core/domains/dispatch"
	dispatchv1 "github.com/polterware/dispatch/go-core/platform/gen/dispatch/v1"
	"github.com/polterware/dispatch/go-core/platform/gen/dispatch/v1/dispatchv1connect"
	"github.com/polterware/dispatch/go-core/providers/adapters"
	"github.com/polterware/dispatch/go-core/providers/process"
	"github.com/polterware/dispatch/go-core/providers/worktree"
)

var _ dispatchv1connect.DispatchServiceHandler = (*Core)(nil)

const dispatchWatchTick = 150 * time.Millisecond

func (core *Core) initDispatchState() {
	core.dispatchOnce.Do(func() {
		core.dispatchCancels = map[string]context.CancelFunc{}
		core.dispatchFinish = map[string]*sync.Mutex{}
		core.dispatchLogOff = map[string]int64{}
		core.dispatchLogReady = map[string]struct{}{}
		core.dispatchFinalized = map[string]struct{}{}
	})
}

func (core *Core) putDispatchCancel(runID string, cancel context.CancelFunc) {
	core.initDispatchState()
	core.dispatchCancelMu.Lock()
	defer core.dispatchCancelMu.Unlock()
	core.dispatchCancels[runID] = cancel
}

func (core *Core) popDispatchCancel(runID string) (context.CancelFunc, bool) {
	core.initDispatchState()
	core.dispatchCancelMu.Lock()
	defer core.dispatchCancelMu.Unlock()
	cancel, ok := core.dispatchCancels[runID]
	if ok {
		delete(core.dispatchCancels, runID)
	}
	return cancel, ok
}

func (core *Core) dispatchRunMu(runID string) *sync.Mutex {
	core.initDispatchState()
	core.dispatchFinishMu.Lock()
	defer core.dispatchFinishMu.Unlock()
	mu, ok := core.dispatchFinish[runID]
	if !ok {
		mu = &sync.Mutex{}
		core.dispatchFinish[runID] = mu
	}
	return mu
}

// reconcileDispatchOnBoot marks RUNNING and PENDING runs whose runner died
// with the previous core as ORPHANED. ServeConnect calls this before accept.
func (core *Core) reconcileDispatchOnBoot() {
	if core == nil || core.Store == nil {
		return
	}
	ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
	defer cancel()
	deps, err := core.dispatchDeps(ctx)
	if err != nil {
		return
	}
	_ = deps.Reconcile(ctx)
}

func (core *Core) dispatchStateDir() string {
	if core.DBDir != "" {
		return filepath.Join(core.DBDir, "dispatch")
	}
	if core.Store != nil && core.Store.DB != nil {
		var seq int
		var name, file string
		if err := core.Store.DB.QueryRow(`PRAGMA database_list`).Scan(&seq, &name, &file); err == nil && file != "" && file != ":memory:" {
			return filepath.Join(filepath.Dir(file), "dispatch")
		}
	}
	return filepath.Join(os.TempDir(), "polterware-dispatch")
}

func (core *Core) dispatchDeps(ctx context.Context) (dispatch.Deps, error) {
	table, err := dispatch.LoadTiersFromStore(ctx, core.Store)
	if err != nil {
		return dispatch.Deps{}, err
	}
	return dispatch.Deps{
		Store:    core.Store,
		Adapters: adapters.Known(),
		Table:    table,
		StateDir: core.dispatchStateDir(),
	}, nil
}

func (core *Core) ListRuns(ctx context.Context, request *connect.Request[dispatchv1.ListRunsRequest]) (*connect.Response[dispatchv1.ListRunsResponse], error) {
	projectID := strings.TrimSpace(request.Msg.GetProjectId())
	conversationID := strings.TrimSpace(request.Msg.GetConversationId())
	// A conversation may start runs in several workspaces, so it lists across them.
	if projectID == "" && conversationID == "" {
		return nil, invalid("project id is required")
	}
	runs, err := core.Store.ListRuns(ctx, projectID, int(request.Msg.GetLimit()), int(request.Msg.GetOffset()), conversationID)
	if err != nil {
		return nil, internal(err)
	}
	out := make([]*dispatchv1.DispatchRun, 0, len(runs))
	for _, run := range runs {
		out = append(out, dispatchRunProto(run))
	}
	return connect.NewResponse(&dispatchv1.ListRunsResponse{Runs: out}), nil
}

func (core *Core) GetRun(ctx context.Context, request *connect.Request[dispatchv1.GetRunRequest]) (*connect.Response[dispatchv1.GetRunResponse], error) {
	id := strings.TrimSpace(request.Msg.GetId())
	if id == "" {
		return nil, invalid("run id is required")
	}
	run, err := core.Store.GetRun(ctx, id)
	if errors.Is(err, dispatch.ErrUnknownRun) || errors.Is(err, sql.ErrNoRows) {
		return nil, connect.NewError(connect.CodeNotFound, err)
	}
	if err != nil {
		return nil, internal(err)
	}
	return connect.NewResponse(&dispatchv1.GetRunResponse{Run: dispatchRunProto(run)}), nil
}

func (core *Core) StartRun(ctx context.Context, request *connect.Request[dispatchv1.StartRunRequest]) (*connect.Response[dispatchv1.StartRunResponse], error) {
	projectID := strings.TrimSpace(request.Msg.GetProjectId())
	if projectID == "" {
		return nil, invalid("project id is required")
	}
	tier := adapters.Tier(strings.TrimSpace(request.Msg.GetTier()))
	profileName := strings.TrimSpace(request.Msg.GetProfile())
	if tier != "" && tier != dispatch.TierAuto && !tier.Valid() {
		return nil, invalid("tier must be auto, easy, normal, or hard")
	}
	prompt := strings.TrimSpace(request.Msg.GetPrompt())
	if prompt == "" {
		return nil, invalid("prompt is required")
	}
	ws, err := core.workspace(ctx, projectID)
	if err != nil {
		return nil, invalid(err.Error())
	}

	// The run directory: the request's dir, else (in a Polterware project) its
	// repository, else the card's component repository, else the project's only
	// repository; a standalone workspace runs in its folder.
	dir := expandPath(request.Msg.GetDir())
	noWorktree := false
	reqWorktree := strings.TrimSpace(request.Msg.GetWorktree())
	if reqWorktree != "" {
		dir = expandPath(reqWorktree)
		noWorktree = true
	}
	hosted, err := core.hostRun(ctx, ws, strings.TrimSpace(request.Msg.GetCardSlug()), request.Msg.GetRepo(), dir)
	if err != nil {
		return nil, invalid(err.Error())
	}
	dir = hosted.Dir
	if abs, err := filepath.Abs(dir); err == nil {
		dir = abs
	}

	deps, err := core.dispatchDepsIn(ctx, dir)
	if err != nil {
		return nil, internal(err)
	}

	core.dispatchStartMu.Lock()
	existing, err := core.Store.ListRuns(ctx, projectID, 0, 0)
	if err != nil {
		core.dispatchStartMu.Unlock()
		return nil, internal(err)
	}
	for _, r := range existing {
		if r.Status != dispatch.StatusPending && r.Status != dispatch.StatusRunning {
			continue
		}
		if sameDispatchDir(r.Dir, dir) || (r.Worktree != "" && sameDispatchDir(r.Worktree, dir)) {
			core.dispatchStartMu.Unlock()
			return nil, invalid("another dispatch run is already active in " + dir)
		}
	}

	opts := dispatch.Options{
		Caller:     dispatch.Caller{Agent: request.Msg.GetCaller().GetAgent(), SessionID: request.Msg.GetCaller().GetSessionId(), ConversationID: request.Msg.GetCaller().GetConversationId(), Label: request.Msg.GetCaller().GetLabel()},
		ProjectID:  projectID,
		Area:       runArea(hosted),
		CardSlug:   strings.TrimSpace(request.Msg.GetCardSlug()),
		CardText:   hosted.CardText,
		Tier:       tier,
		Prompt:     prompt,
		Dir:        dir,
		ReadOnly:   request.Msg.GetReadOnly(),
		NoWorktree: noWorktree,
		Timeout:    time.Duration(request.Msg.GetTimeoutSec()) * time.Second,
		Filters: dispatch.Filters{
			Agent:    strings.TrimSpace(request.Msg.GetAgent()),
			Provider: strings.TrimSpace(request.Msg.GetProvider()),
			Model:    strings.TrimSpace(request.Msg.GetModel()),
			Effort:   strings.TrimSpace(request.Msg.GetEffort()),
			Priority: strings.TrimSpace(request.Msg.GetPriority()),
			Relax:    request.Msg.GetRelax(),
		},
	}
	if profileName != "" {
		if err := core.applyDispatchProfile(&opts, profileName); err != nil {
			core.dispatchStartMu.Unlock()
			return nil, invalid(err.Error())
		}
	}
	run, err := deps.Begin(opts)
	core.dispatchStartMu.Unlock()
	if err != nil {
		return nil, invalid(err.Error())
	}
	// A delegation is also an audited Run: the agent it launches writes under
	// this AgentRun, so Activity shows one history for both.
	if ws.PolterwareProjectID != "" {
		hctx, cancel := context.WithTimeout(ctx, hostTimeout)
		if id, aerr := core.Host.StartAgentRun(hctx, ws.PolterwareProjectID, dispatchRunTask(prompt), run.CardSlug); aerr == nil && id != "" {
			run.AgentRunID = id
			_ = core.Store.SaveRun(ctx, run)
		}
		cancel()
	}

	if reqWorktree != "" {
		run.Worktree = dir
		if root := worktree.Root(dir); root != "" {
			run.Repo = root
			if br, berr := worktree.CurrentBranch(dir); berr == nil && br != "HEAD" {
				run.Branch = br
			}
			if head, herr := worktree.Head(dir); herr == nil {
				run.BaseSHA = head
			}
		}
		if err := core.Store.SaveRun(ctx, run); err != nil {
			return nil, internal(err)
		}
		_ = saveRunWorktree(ctx, core.Store, run)
	}

	if err := core.Store.AppendEvent(ctx, dispatch.RunEvent{
		RunID:     run.ID,
		ProjectID: run.ProjectID,
		Kind:      dispatch.EventStarted,
		Agent:     run.Agent,
		Tier:      run.Tier,
		Text:      run.Prompt,
		At:        time.Now().UTC(),
	}); err != nil {
		return nil, internal(err)
	}

	execCtx, execCancel := context.WithCancel(context.Background())
	core.putDispatchCancel(run.ID, execCancel)
	go func(initial dispatch.Run) {
		defer func() {
			core.popDispatchCancel(initial.ID)
			execCancel()
		}()
		mu := core.dispatchRunMu(initial.ID)
		mu.Lock()
		defer mu.Unlock()
		current := initial
		if latest, err := core.Store.GetRun(context.Background(), initial.ID); err == nil {
			current = latest
		}
		if isTerminalDispatchStatus(current.Status) {
			core.finalizeDispatchRun(context.Background(), current)
			return
		}
		finalRun, _ := deps.Execute(execCtx, current)
		core.finalizeDispatchRun(context.Background(), finalRun)
	}(run)

	return connect.NewResponse(&dispatchv1.StartRunResponse{Run: dispatchRunProto(run)}), nil
}

// stopForeignDispatchProcs stops the agent tree and a runner that is not this
// process. The core's own PID is never signaled; in-process runs end through
// the Execute context instead.
func (core *Core) stopForeignDispatchProcs(ctx context.Context, run dispatch.Run) error {
	owner := run.PID
	if owner == os.Getpid() {
		owner = 0
	}
	if run.AgentPID <= 0 && owner <= 0 {
		return nil
	}
	tree, err := process.Capture(run.AgentPID)
	if err != nil {
		return err
	}
	if err := tree.Include(owner, run.ProcStart); err != nil {
		return err
	}
	return tree.Stop(ctx, 5*time.Second)
}

func (core *Core) CancelRun(ctx context.Context, request *connect.Request[dispatchv1.CancelRunRequest]) (*connect.Response[dispatchv1.CancelRunResponse], error) {
	id := strings.TrimSpace(request.Msg.GetId())
	if id == "" {
		return nil, invalid("run id is required")
	}
	execCancel, hadCancel := core.popDispatchCancel(id)
	if hadCancel {
		execCancel()
	}

	run, err := core.Store.GetRun(ctx, id)
	if errors.Is(err, dispatch.ErrUnknownRun) || errors.Is(err, sql.ErrNoRows) {
		return nil, connect.NewError(connect.CodeNotFound, err)
	}
	if err != nil {
		return nil, internal(err)
	}

	selfOwned := hadCancel || run.PID == os.Getpid()
	if !selfOwned {
		deps, derr := core.dispatchDeps(ctx)
		if derr != nil {
			return nil, internal(derr)
		}
		run, err = deps.Cancel(ctx, id)
		if errors.Is(err, dispatch.ErrUnknownRun) || errors.Is(err, sql.ErrNoRows) {
			return nil, connect.NewError(connect.CodeNotFound, err)
		}
		if err != nil {
			return nil, invalid(err.Error())
		}
		if !isTerminalDispatchStatus(run.Status) {
			run.Status = dispatch.StatusCancelled
			if run.Error == "" {
				run.Error = "cancelled"
			}
			run.FinishedAt = time.Now().UTC()
			if err := core.Store.SaveRun(ctx, run); err != nil {
				return nil, internal(err)
			}
		}
		core.finalizeDispatchRun(ctx, run)
		return connect.NewResponse(&dispatchv1.CancelRunResponse{Run: dispatchRunProto(run)}), nil
	}

	alreadyTerminal := isTerminalDispatchStatus(run.Status)
	if alreadyTerminal && !hadCancel {
		return nil, invalid(fmt.Sprintf("run %s already %s", run.ID, run.Status))
	}
	if !alreadyTerminal {
		if err := core.stopForeignDispatchProcs(ctx, run); err != nil {
			return nil, invalid(err.Error())
		}
	}

	mu := core.dispatchRunMu(id)
	mu.Lock()
	defer mu.Unlock()
	if latest, lerr := core.Store.GetRun(ctx, id); lerr == nil {
		run = latest
	}
	if !isTerminalDispatchStatus(run.Status) {
		run.Status = dispatch.StatusCancelled
		if run.Error == "" {
			run.Error = "cancelled"
		}
		run.FinishedAt = time.Now().UTC()
		if err := core.Store.SaveRun(ctx, run); err != nil {
			return nil, internal(err)
		}
	} else if run.Status != dispatch.StatusCancelled {
		return nil, invalid(fmt.Sprintf("run %s already %s", run.ID, run.Status))
	}
	core.finalizeDispatchRun(ctx, run)
	return connect.NewResponse(&dispatchv1.CancelRunResponse{Run: dispatchRunProto(run)}), nil
}

func (core *Core) Result(ctx context.Context, request *connect.Request[dispatchv1.ResultRequest]) (*connect.Response[dispatchv1.ResultResponse], error) {
	id := strings.TrimSpace(request.Msg.GetId())
	if id == "" {
		return nil, invalid("run id is required")
	}
	run, err := core.Store.GetRun(ctx, id)
	if errors.Is(err, dispatch.ErrUnknownRun) || errors.Is(err, sql.ErrNoRows) {
		return nil, connect.NewError(connect.CodeNotFound, err)
	}
	if err != nil {
		return nil, internal(err)
	}

	section := strings.TrimSpace(request.Msg.GetSection())
	if section == "" {
		section = "summary"
	}
	offset := int(request.Msg.GetOffset())
	limit := int(request.Msg.GetLimit())

	var raw string
	switch section {
	case "summary":
		maxLines := limit
		if maxLines <= 0 {
			maxLines = 40
		}
		raw, _ = dispatch.Summary(run.Output, maxLines, 4096)
	case "files":
		b, err := json.Marshal(map[string]any{
			"files_changed": run.FilesChanged,
			"insertions":    run.Insertions,
			"deletions":     run.Deletions,
		})
		if err != nil {
			return nil, internal(err)
		}
		raw = string(b)
	case "attempts":
		attempts := run.Attempts
		if attempts == nil {
			attempts = []dispatch.Attempt{}
		}
		b, err := json.Marshal(attempts)
		if err != nil {
			return nil, internal(err)
		}
		raw = string(b)
	case "meta":
		metaRun := run
		metaRun.Output = ""
		b, err := json.Marshal(metaRun)
		if err != nil {
			return nil, internal(err)
		}
		raw = string(b)
	case "output", "agent":
		raw = run.Output
	default:
		return nil, invalid("unknown section " + section)
	}

	content, total, _ := dispatch.Page(raw, offset, limit)
	return connect.NewResponse(&dispatchv1.ResultResponse{
		Content: content,
		Total:   int32(total),
	}), nil
}

func (core *Core) WatchRun(ctx context.Context, request *connect.Request[dispatchv1.WatchRunRequest], stream *connect.ServerStream[dispatchv1.WatchRunResponse]) error {
	id := strings.TrimSpace(request.Msg.GetId())
	if id == "" {
		return invalid("run id is required")
	}
	run, err := core.Store.GetRun(ctx, id)
	if errors.Is(err, dispatch.ErrUnknownRun) || errors.Is(err, sql.ErrNoRows) {
		return connect.NewError(connect.CodeNotFound, err)
	}
	if err != nil {
		return internal(err)
	}

	core.life.streams.Add(1)
	defer func() {
		core.life.streams.Add(-1)
		core.touch()
	}()

	lastSeq := request.Msg.GetAfterSeq()
	initialEvents, err := core.Store.ListEvents(ctx, id, lastSeq, 0)
	if err != nil {
		return internal(err)
	}
	for _, ev := range initialEvents {
		if err := stream.Send(watchRunProto(ev)); err != nil {
			return err
		}
		if ev.Seq > lastSeq {
			lastSeq = ev.Seq
		}
	}

	logPath := filepath.Join(core.dispatchStateDir(), "logs", id+".agent.log")
	ticker := time.NewTicker(dispatchWatchTick)
	defer ticker.Stop()

	for {
		if err := core.syncAgentLogOutput(ctx, run, logPath); err != nil {
			return internal(err)
		}
		events, err := core.Store.ListEvents(ctx, id, lastSeq, 0)
		if err != nil {
			return internal(err)
		}
		for _, ev := range events {
			if err := stream.Send(watchRunProto(ev)); err != nil {
				return err
			}
			if ev.Seq > lastSeq {
				lastSeq = ev.Seq
			}
		}

		current, err := core.Store.GetRun(ctx, id)
		if err != nil {
			return internal(err)
		}
		run = current
		if isTerminalDispatchStatus(run.Status) {
			if err := core.syncAgentLogOutput(ctx, run, logPath); err != nil {
				return internal(err)
			}
			tailEvents, err := core.Store.ListEvents(ctx, id, lastSeq, 0)
			if err != nil {
				return internal(err)
			}
			for _, ev := range tailEvents {
				if err := stream.Send(watchRunProto(ev)); err != nil {
					return err
				}
				if ev.Seq > lastSeq {
					lastSeq = ev.Seq
				}
			}
			return nil
		}

		select {
		case <-ctx.Done():
			return nil
		case <-core.Done():
			return nil
		case <-ticker.C:
		}
	}
}

func (core *Core) syncAgentLogOutput(ctx context.Context, run dispatch.Run, logPath string) error {
	f, err := os.Open(logPath)
	if err != nil {
		return nil
	}
	defer f.Close()
	info, err := f.Stat()
	if err != nil || info.Size() == 0 {
		return nil
	}
	size := info.Size()

	core.initDispatchState()
	core.dispatchWatchMu.Lock()
	defer core.dispatchWatchMu.Unlock()

	if _, ready := core.dispatchLogReady[run.ID]; !ready {
		off, err := core.seededLogOffset(ctx, run.ID)
		if err != nil {
			return err
		}
		core.dispatchLogOff[run.ID] = off
		core.dispatchLogReady[run.ID] = struct{}{}
	}
	off := core.dispatchLogOff[run.ID]
	if off > size {
		off = size
	}
	if off == size {
		core.dispatchLogOff[run.ID] = off
		return nil
	}
	if _, err := f.Seek(off, io.SeekStart); err != nil {
		return err
	}
	buf := make([]byte, size-off)
	n, err := io.ReadFull(f, buf)
	if err != nil && !errors.Is(err, io.ErrUnexpectedEOF) && !errors.Is(err, io.EOF) {
		return err
	}
	emit := completeRunePrefix(buf[:n])
	if len(emit) == 0 {
		return nil
	}
	if err := core.Store.AppendEvent(ctx, dispatch.RunEvent{
		RunID:     run.ID,
		ProjectID: run.ProjectID,
		Kind:      dispatch.EventOutput,
		Agent:     run.Agent,
		Tier:      run.Tier,
		Text:      string(emit),
		At:        time.Now().UTC(),
	}); err != nil {
		return err
	}
	core.dispatchLogOff[run.ID] = off + int64(len(emit))
	return nil
}

func (core *Core) seededLogOffset(ctx context.Context, runID string) (int64, error) {
	events, err := core.Store.ListEvents(ctx, runID, 0, 0)
	if err != nil {
		return 0, err
	}
	var n int64
	for _, ev := range events {
		if ev.Kind == dispatch.EventOutput {
			n += int64(len(ev.Text))
		}
	}
	return n, nil
}

// completeRunePrefix drops a trailing incomplete UTF-8 sequence so a chunk
// never splits a rune. Invalid but complete bytes are kept.
func completeRunePrefix(b []byte) []byte {
	if len(b) == 0 || utf8.Valid(b) {
		return b
	}
	max := utf8.UTFMax - 1
	if max > len(b) {
		max = len(b)
	}
	for n := 1; n <= max; n++ {
		tail := b[len(b)-n:]
		if utf8.RuneStart(tail[0]) && !utf8.FullRune(tail) {
			return b[:len(b)-n]
		}
	}
	return b
}

func (core *Core) ListWorktrees(ctx context.Context, request *connect.Request[dispatchv1.ListWorktreesRequest]) (*connect.Response[dispatchv1.ListWorktreesResponse], error) {
	projectID := strings.TrimSpace(request.Msg.GetProjectId())
	if projectID == "" {
		return nil, invalid("project id is required")
	}
	items, err := core.Store.ListWorktrees(ctx, projectID)
	if err != nil {
		return nil, internal(err)
	}
	out := make([]*dispatchv1.DispatchWorktree, 0, len(items))
	for _, item := range items {
		out = append(out, dispatchWorktreeProto(item))
	}
	return connect.NewResponse(&dispatchv1.ListWorktreesResponse{Worktrees: out}), nil
}

func (core *Core) ListQuota(ctx context.Context, request *connect.Request[dispatchv1.ListQuotaRequest]) (*connect.Response[dispatchv1.ListQuotaResponse], error) {
	now := time.Now().UTC()
	since, err := parseQuotaSince(request.Msg.GetSince(), now)
	if err != nil {
		return nil, invalid(err.Error())
	}
	agentFilter := strings.TrimSpace(request.Msg.GetAgent())
	core.probeAgentsAndQuotaOnce(ctx, agentFilter, now)

	snaps, err := core.Store.ListQuota(ctx, agentFilter, since)
	if err != nil {
		return nil, internal(err)
	}
	out := make([]*dispatchv1.DispatchQuotaSnapshot, 0, len(snaps))
	for _, snap := range snaps {
		out = append(out, dispatchQuotaProto(snap))
	}
	return connect.NewResponse(&dispatchv1.ListQuotaResponse{Snapshots: out}), nil
}

func (core *Core) ListTiers(ctx context.Context, _ *connect.Request[dispatchv1.ListTiersRequest]) (*connect.Response[dispatchv1.ListTiersResponse], error) {
	rows, err := core.Store.ListTiers(ctx)
	if err != nil {
		return nil, internal(err)
	}
	if len(rows) == 0 {
		if err := core.Store.ReplaceTiers(ctx, dispatch.Default().Rows()); err != nil {
			return nil, internal(err)
		}
		rows, err = core.Store.ListTiers(ctx)
		if err != nil {
			return nil, internal(err)
		}
	}
	out := make([]*dispatchv1.DispatchTier, 0, len(rows))
	for _, row := range rows {
		out = append(out, dispatchTierProto(row))
	}
	return connect.NewResponse(&dispatchv1.ListTiersResponse{Tiers: out}), nil
}

// ReplaceTiers stores a new tier table. Rows keep their request order within each tier.
func (core *Core) ReplaceTiers(ctx context.Context, request *connect.Request[dispatchv1.ReplaceTiersRequest]) (*connect.Response[dispatchv1.ReplaceTiersResponse], error) {
	table := dispatch.Table{Tiers: map[adapters.Tier][]dispatch.Entry{}}
	for _, row := range request.Msg.GetTiers() {
		tier := adapters.Tier(strings.TrimSpace(row.GetName()))
		if !tier.Valid() {
			return nil, invalid(fmt.Sprintf("unknown tier %q", row.GetName()))
		}
		table.Tiers[tier] = append(table.Tiers[tier], dispatch.Entry{
			Provider: strings.TrimSpace(row.GetProvider()),
			Agent:    strings.TrimSpace(row.GetAgent()),
			Model:    strings.TrimSpace(row.GetModel()),
			Speed:    int(row.GetSpeed()),
			Strength: int(row.GetStrength()),
		})
	}
	validated, err := table.Validated()
	if err != nil {
		return nil, invalid(err.Error())
	}
	if err := core.Store.ReplaceTiers(ctx, validated.Rows()); err != nil {
		return nil, internal(err)
	}
	rows, err := core.Store.ListTiers(ctx)
	if err != nil {
		return nil, internal(err)
	}
	out := make([]*dispatchv1.DispatchTier, 0, len(rows))
	for _, row := range rows {
		out = append(out, dispatchTierProto(row))
	}
	return connect.NewResponse(&dispatchv1.ReplaceTiersResponse{Tiers: out}), nil
}

// ListModels asks every installed agent CLI for its models, in parallel and bounded by a timeout.
func (core *Core) ListModels(ctx context.Context, _ *connect.Request[dispatchv1.ListModelsRequest]) (*connect.Response[dispatchv1.ListModelsResponse], error) {
	return connect.NewResponse(core.listModels(ctx)), nil
}

func (core *Core) listModels(ctx context.Context) *dispatchv1.ListModelsResponse {
	type listed struct {
		models  []*dispatchv1.DispatchModel
		problem string
	}
	known := adapters.Known()
	results := make([]listed, len(known))
	var wg sync.WaitGroup
	for i, a := range known {
		if _, err := exec.LookPath(a.Binary()); err != nil {
			continue
		}
		wg.Add(1)
		go func(i int, a adapters.Adapter) {
			defer wg.Done()
			listCtx, cancel := context.WithTimeout(ctx, 20*time.Second)
			defer cancel()
			models, err := core.agentModels(listCtx, a)
			if err != nil {
				results[i].problem = fmt.Sprintf("%s: %v", a.Name(), err)
				return
			}
			for _, m := range models {
				results[i].models = append(results[i].models, &dispatchv1.DispatchModel{
					Agent:    a.Name(),
					Model:    m.ID,
					Provider: adapters.ProviderOf(a.Name(), m.ID),
					Tier:     string(m.Tier),
				})
			}
		}(i, a)
	}
	wg.Wait()
	response := &dispatchv1.ListModelsResponse{}
	for _, r := range results {
		response.Models = append(response.Models, r.models...)
		if r.problem != "" {
			response.Problems = append(response.Problems, r.problem)
		}
	}
	return response
}

func (core *Core) ListAgents(ctx context.Context, _ *connect.Request[dispatchv1.ListAgentsRequest]) (*connect.Response[dispatchv1.ListAgentsResponse], error) {
	core.probeAgentsAndQuotaOnce(ctx, "", time.Now().UTC())
	agents, err := core.Store.ListAgents(ctx)
	if err != nil {
		return nil, internal(err)
	}
	installed := map[string]bool{}
	for _, a := range adapters.Known() {
		if _, err := exec.LookPath(a.Binary()); err == nil {
			installed[a.Name()] = true
		}
	}
	out := make([]*dispatchv1.DispatchAgent, 0, len(agents))
	for _, agent := range agents {
		row := dispatchAgentProto(agent)
		row.Installed = installed[agent.Name]
		out = append(out, row)
	}
	return connect.NewResponse(&dispatchv1.ListAgentsResponse{Agents: out}), nil
}

func (core *Core) ListHistory(ctx context.Context, request *connect.Request[dispatchv1.ListHistoryRequest]) (*connect.Response[dispatchv1.ListHistoryResponse], error) {
	events, err := core.Store.ListHistory(
		ctx,
		strings.TrimSpace(request.Msg.GetProjectId()),
		strings.TrimSpace(request.Msg.GetAgent()),
		strings.TrimSpace(request.Msg.GetTier()),
		int(request.Msg.GetLimit()),
		int(request.Msg.GetOffset()),
	)
	if err != nil {
		return nil, internal(err)
	}
	out := make([]*dispatchv1.DispatchRunEvent, 0, len(events))
	for _, ev := range events {
		out = append(out, dispatchRunEventProto(ev))
	}
	return connect.NewResponse(&dispatchv1.ListHistoryResponse{Events: out}), nil
}

func (core *Core) CheckTiers(ctx context.Context, _ *connect.Request[dispatchv1.CheckTiersRequest]) (*connect.Response[dispatchv1.CheckTiersResponse], error) {
	table, err := dispatch.LoadTiersFromStore(ctx, core.Store)
	if err != nil {
		return nil, internal(err)
	}
	known := adapters.Known()
	adapterByName := make(map[string]adapters.Adapter, len(known))
	for _, a := range known {
		adapterByName[a.Name()] = a
	}

	var problems []string
	problems = append(problems, table.FieldProblems()...)
	problems = append(problems, table.CapabilityProblems(known)...)

	type modelQuery struct {
		installed bool
		models    []adapters.Model
		err       error
	}
	queried := map[string]modelQuery{}
	// Ask the table's installed agents for their models at once, through the shared cache.
	var wg sync.WaitGroup
	var mu sync.Mutex
	for _, tier := range []adapters.Tier{adapters.TierEasy, adapters.TierNormal, adapters.TierHard} {
		for _, entry := range table.Tiers[tier] {
			a, ok := adapterByName[entry.Agent]
			if _, seen := queried[entry.Agent]; !ok || seen {
				continue
			}
			queried[entry.Agent] = modelQuery{}
			if _, lerr := exec.LookPath(a.Binary()); lerr != nil {
				continue
			}
			wg.Add(1)
			go func(name string, a adapters.Adapter) {
				defer wg.Done()
				models, err := core.agentModels(ctx, a)
				mu.Lock()
				queried[name] = modelQuery{installed: true, models: models, err: err}
				mu.Unlock()
			}(entry.Agent, a)
		}
	}
	wg.Wait()
	for _, tier := range []adapters.Tier{adapters.TierEasy, adapters.TierNormal, adapters.TierHard} {
		for _, entry := range table.Tiers[tier] {
			_, ok := adapterByName[entry.Agent]
			if !ok {
				problems = append(problems, fmt.Sprintf("%s: unknown agent %s", tier, entry.Agent))
				continue
			}
			q := queried[entry.Agent]
			if !q.installed {
				continue
			}
			if q.err != nil {
				problems = append(problems, fmt.Sprintf("%s: %s/%s: %v", tier, entry.Agent, entry.Model, q.err))
				continue
			}
			listed := false
			for _, m := range q.models {
				if m.ID == entry.Model || adapters.BaseModel(m.ID) == adapters.BaseModel(entry.Model) {
					listed = true
					break
				}
			}
			if !listed {
				problems = append(problems, fmt.Sprintf("%s: %s/%s: model not listed by installed agent", tier, entry.Agent, entry.Model))
			}
		}
	}

	return connect.NewResponse(&dispatchv1.CheckTiersResponse{Problems: problems}), nil
}

func (core *Core) Stats(ctx context.Context, request *connect.Request[dispatchv1.StatsRequest]) (*connect.Response[dispatchv1.StatsResponse], error) {
	now := time.Now().UTC()
	since, err := dispatch.ParseSince(request.Msg.GetSince(), now)
	if err != nil {
		return nil, invalid(err.Error())
	}
	report, err := dispatch.FromStore(core.Store, dispatch.StatsOptions{
		ProjectID: strings.TrimSpace(request.Msg.GetProjectId()),
		By:        strings.TrimSpace(request.Msg.GetBy()),
		Since:     since,
		Now:       now,
	})
	if err != nil {
		return nil, invalid(err.Error())
	}
	rows := make([]*dispatchv1.DispatchStatsRow, 0, len(report.Rows))
	for _, r := range report.Rows {
		row := &dispatchv1.DispatchStatsRow{
			Key:         r.Key,
			Runs:        int32(r.Runs),
			SuccessPct:  r.SuccessPct,
			FirstTryPct: r.FirstTryPct,
			Fallbacks:   int32(r.Fallbacks),
			RateLimits:  int32(r.RateLimits),
			Orphans:     int32(r.Orphans),

			UsageRuns:        int32(r.Spend.UsageRuns),
			InputTokens:      r.Spend.InputTokens,
			CacheReadTokens:  r.Spend.CacheReadTokens,
			CacheWriteTokens: r.Spend.CacheWriteTokens,
			OutputTokens:     r.Spend.OutputTokens,
			ReasoningTokens:  r.Spend.ReasoningTokens,
			CostUsd:          r.Spend.CostUSD,
			CostRuns:         int32(r.Spend.CostRuns),
		}
		if r.P50Ms != nil {
			v := *r.P50Ms
			row.P50Ms = &v
		}
		if r.P90Ms != nil {
			v := *r.P90Ms
			row.P90Ms = &v
		}
		rows = append(rows, row)
	}
	return connect.NewResponse(&dispatchv1.StatsResponse{
		By:       report.By,
		Since:    report.Since,
		InFlight: int32(report.InFlight),
		Rows:     rows,
		Rankings: &dispatchv1.DispatchStatsRankings{
			MostUsedModel:    dispatchStatRankProto(report.Rankings.MostUsedModel),
			MostUsedProvider: dispatchStatRankProto(report.Rankings.MostUsedProvider),
			SlowestModel:     dispatchStatRankProto(report.Rankings.SlowestModel),
			MostFailedModel:  dispatchStatRankProto(report.Rankings.MostFailedModel),
		},
	}), nil
}

func (core *Core) Capacity(_ context.Context, request *connect.Request[dispatchv1.CapacityRequest]) (*connect.Response[dispatchv1.CapacityResponse], error) {
	now := time.Now().UTC()
	since, err := parseQuotaSince(request.Msg.GetSince(), now)
	if err != nil {
		return nil, invalid(err.Error())
	}
	report, err := dispatch.CapacityFromStore(core.Store, since, now)
	if err != nil {
		return nil, internal(err)
	}
	keys := make([]*dispatchv1.DispatchCapacityKey, 0, len(report.Keys))
	for _, k := range report.Keys {
		item := &dispatchv1.DispatchCapacityKey{
			Key:      k.Key,
			Windows:  int32(k.Windows),
			HitFull:  int32(k.HitFull),
			Leftover: k.Leftover,
			Pace:     k.Pace,
		}
		if k.ConsumedPct != nil {
			v := *k.ConsumedPct
			item.ConsumedPct = &v
		}
		if k.LeftoverPct != nil {
			v := *k.LeftoverPct
			item.LeftoverPct = &v
		}
		keys = append(keys, item)
	}
	return connect.NewResponse(&dispatchv1.CapacityResponse{
		Since: report.Since,
		Note:  report.Note,
		Keys:  keys,
	}), nil
}

func dispatchStatRankProto(r dispatch.StatRank) *dispatchv1.DispatchStatRank {
	out := &dispatchv1.DispatchStatRank{
		Key:     r.Key,
		Runs:    int32(r.Runs),
		Samples: int32(r.Samples),
		Reason:  r.Reason,
	}
	if r.P50Ms != nil {
		v := *r.P50Ms
		out.P50Ms = &v
	}
	if r.SuccessPct != nil {
		v := *r.SuccessPct
		out.SuccessPct = &v
	}
	return out
}

// agentProbeGap is how long a probe of the agent CLIs' usage stays fresh. Each probe runs every
// installed CLI's usage command, seconds in all, so Runs, Usage, and Models share one.
const agentProbeGap = time.Minute

var agentProbes = struct {
	sync.Mutex
	last map[string]time.Time
}{last: map[string]time.Time{}}

// probeAgentsAndQuotaOnce probes at most once per agentProbeGap for the same filter. Calls that
// arrive while a probe runs wait for it and then read what it stored instead of probing again.
func (core *Core) probeAgentsAndQuotaOnce(ctx context.Context, filterAgent string, now time.Time) {
	agentProbes.Lock()
	defer agentProbes.Unlock()
	fresh := func(key string) bool {
		at, ok := agentProbes.last[key]
		return ok && now.Sub(at) < agentProbeGap
	}
	// A full probe covers every agent; one agent's probe covers only that agent.
	if fresh("") || (filterAgent != "" && fresh(filterAgent)) {
		return
	}
	core.probeAgentsAndQuota(ctx, filterAgent, now)
	agentProbes.last[filterAgent] = time.Now().UTC()
}

func (core *Core) probeAgentsAndQuota(ctx context.Context, filterAgent string, now time.Time) {
	for _, a := range adapters.Known() {
		if filterAgent != "" && a.Name() != filterAgent {
			continue
		}
		provider := adapters.ProviderOf(a.Name(), "")
		emp := dispatch.EmpiricalQuota(core.Store, a.Name(), now)
		agentRow := dispatch.Agent{Name: a.Name(), Bin: a.Binary(), Provider: provider}
		// source and confidence describe the quota snapshots this probe saves.
		source, confidence := emp.Source, string(emp.Confidence)
		if !emp.ResetAt.IsZero() {
			t := emp.ResetAt
			agentRow.CooldownUntil = &t
		}

		_, installedErr := exec.LookPath(a.Binary())
		installed := installedErr == nil

		if up, ok := a.(adapters.UsageProvider); ok {
			source = up.UsageSource()
			if installed {
				if samples, err := up.Usage(ctx); err == nil && len(samples) > 0 {
					for _, s := range samples {
						at := now
						if !s.SampledAt.IsZero() {
							at = s.SampledAt
						}
						var usedCopy *float64
						if s.Used != nil {
							v := *s.Used
							usedCopy = &v
							if agentRow.UsedPercent == nil {
								agentRow.UsedPercent = &v
							}
						}
						_ = core.Store.SaveQuota(ctx, dispatch.QuotaSnapshot{
							Agent:         a.Name(),
							Provider:      provider,
							Window:        s.Window,
							Source:        s.Source,
							Confidence:    string(s.Confidence),
							UsedPercent:   usedCopy,
							WindowMinutes: int(s.Duration.Duration().Minutes()),
							ResetsAt:      s.ResetAt,
							SampledAt:     at,
						})
					}
					_ = core.Store.UpsertAgent(ctx, agentRow)
					continue
				}
			}
		}

		if installed {
			if q, err := a.Quota(ctx); err == nil {
				used := q.UsedPercent
				agentRow.UsedPercent = &used
				_ = core.Store.SaveQuota(ctx, dispatch.QuotaSnapshot{
					Agent:         a.Name(),
					Provider:      provider,
					Window:        "default",
					Source:        source,
					Confidence:    string(adapters.ConfidenceOfficial),
					UsedPercent:   &used,
					WindowMinutes: q.WindowMinutes,
					ResetsAt:      q.ResetsAt,
					SampledAt:     now,
				})
				_ = core.Store.UpsertAgent(ctx, agentRow)
				continue
			}
		}

		_ = core.Store.SaveQuota(ctx, dispatch.QuotaSnapshot{
			Agent:       a.Name(),
			Provider:    provider,
			Window:      emp.Window,
			Source:      source,
			Confidence:  confidence,
			UsedPercent: nil,
			ResetsAt:    emp.ResetAt,
			SampledAt:   now,
		})
		_ = core.Store.UpsertAgent(ctx, agentRow)
	}
}

// dispatchRunTask is the one-line task an AgentRun shows for a delegation.
func dispatchRunTask(prompt string) string {
	line := strings.TrimSpace(strings.SplitN(strings.TrimSpace(prompt), "\n", 2)[0])
	if len(line) > 160 {
		line = line[:160]
	}
	if line == "" {
		line = "delegated run"
	}
	return line
}

// completeLinkedAgentRun closes the AgentRun a delegation started.
func (core *Core) completeLinkedAgentRun(run dispatch.Run) {
	if run.AgentRunID == "" {
		return
	}
	ctx, cancel := context.WithTimeout(context.Background(), hostTimeout)
	defer cancel()
	_ = core.Host.CompleteAgentRun(ctx, run.AgentRunID)
}

func (core *Core) finalizeDispatchRun(ctx context.Context, run dispatch.Run) {
	if core == nil || core.Store == nil || run.ID == "" {
		return
	}
	core.initDispatchState()
	core.dispatchWatchMu.Lock()
	defer core.dispatchWatchMu.Unlock()
	if _, done := core.dispatchFinalized[run.ID]; done {
		return
	}
	store := core.Store
	if latest, err := store.GetRun(ctx, run.ID); err == nil && isTerminalDispatchStatus(latest.Status) {
		run = latest
	}
	core.completeLinkedAgentRun(run)
	events, _ := store.ListEvents(ctx, run.ID, 0, 0)
	if dispatchTerminalRecorded(events) {
		_ = saveRunWorktree(ctx, store, run)
		core.dispatchFinalized[run.ID] = struct{}{}
		delete(core.dispatchLogOff, run.ID)
		delete(core.dispatchLogReady, run.ID)
		return
	}
	_ = saveRunWorktree(ctx, store, run)

	hasKind := func(kind string) bool {
		for _, ev := range events {
			if ev.Kind == kind {
				return true
			}
		}
		return false
	}

	now := time.Now().UTC()
	switch run.Status {
	case dispatch.StatusFailed, dispatch.StatusTimedOut, dispatch.StatusCancelled:
		if !hasKind(dispatch.EventFailed) {
			text := run.Error
			if text == "" {
				text = strings.ToLower(run.Status)
			}
			_ = store.AppendEvent(ctx, dispatch.RunEvent{
				RunID:     run.ID,
				ProjectID: run.ProjectID,
				Kind:      dispatch.EventFailed,
				Agent:     run.Agent,
				Tier:      run.Tier,
				Text:      text,
				At:        now,
			})
		}
	}
	if run.MergedAt != nil && !run.MergedAt.IsZero() && !hasKind(dispatch.EventMerged) {
		_ = store.AppendEvent(ctx, dispatch.RunEvent{
			RunID:     run.ID,
			ProjectID: run.ProjectID,
			Kind:      dispatch.EventMerged,
			Agent:     run.Agent,
			Tier:      run.Tier,
			Text:      run.MergeSHA,
			At:        now,
		})
	}
	core.dispatchFinalized[run.ID] = struct{}{}
	delete(core.dispatchLogOff, run.ID)
	delete(core.dispatchLogReady, run.ID)
}

func dispatchTerminalRecorded(events []dispatch.RunEvent) bool {
	for _, ev := range events {
		if ev.Kind == dispatch.EventFailed {
			return true
		}
	}
	return false
}

func saveRunWorktree(ctx context.Context, store dispatch.Store, run dispatch.Run) error {
	if run.Worktree == "" {
		return nil
	}
	status := dispatch.WorktreeActive
	switch run.Status {
	case dispatch.StatusCompleted:
		status = dispatch.WorktreeReady
	case dispatch.StatusFailed, dispatch.StatusCancelled, dispatch.StatusTimedOut, dispatch.StatusOrphaned:
		status = dispatch.WorktreeAbandoned
	}
	return store.SaveWorktree(ctx, dispatch.WorktreeInfo{
		ID:        run.ID,
		ProjectID: run.ProjectID,
		RunID:     run.ID,
		CardSlug:  run.CardSlug,
		Path:      run.Worktree,
		Branch:    run.Branch,
		BaseSHA:   run.BaseSHA,
		Status:    status,
		Owner:     run.Agent,
	})
}

func sameDispatchDir(a, b string) bool {
	if a == "" || b == "" {
		return false
	}
	if absA, err := filepath.Abs(a); err == nil {
		a = absA
	}
	if absB, err := filepath.Abs(b); err == nil {
		b = absB
	}
	return filepath.Clean(a) == filepath.Clean(b)
}

func isTerminalDispatchStatus(status string) bool {
	switch status {
	case dispatch.StatusCompleted, dispatch.StatusFailed, dispatch.StatusCancelled, dispatch.StatusTimedOut, dispatch.StatusOrphaned:
		return true
	default:
		return false
	}
}

func parseQuotaSince(raw string, now time.Time) (time.Time, error) {
	s := strings.TrimSpace(raw)
	if s == "" {
		return time.Time{}, nil
	}
	if t, err := time.Parse(time.RFC3339Nano, s); err == nil {
		return t.UTC(), nil
	}
	if t, err := time.Parse(time.RFC3339, s); err == nil {
		return t.UTC(), nil
	}
	return dispatch.ParseSince(s, now)
}

func formatDispatchTime(t time.Time) string {
	if t.IsZero() {
		return ""
	}
	return t.UTC().Format(time.RFC3339)
}

func dispatchRunProto(run dispatch.Run) *dispatchv1.DispatchRun {
	out := &dispatchv1.DispatchRun{
		ConversationId: run.ConversationID, CallerAgent: run.CallerAgent,
		Id:          run.ID,
		ProjectId:   run.ProjectID,
		Status:      run.Status,
		Tier:        run.Tier,
		Agent:       run.Agent,
		Model:       run.Model,
		Provider:    run.Provider,
		Effort:      run.Effort,
		Prompt:      run.Prompt,
		CardSlug:    run.CardSlug,
		AgentRunId:  run.AgentRunID,
		Dir:         run.Dir,
		Repo:        run.Repo,
		Worktree:    run.Worktree,
		Branch:      run.Branch,
		BaseSha:     run.BaseSHA,
		ReadOnly:    run.ReadOnly,
		ExitCode:    int32(run.ExitCode),
		ExitClass:   run.ExitClass,
		Error:       run.Error,
		Note:        run.Note,
		CreatedAt:   formatDispatchTime(run.CreatedAt),
		StartedAt:   formatDispatchTime(run.StartedAt),
		FinishedAt:  formatDispatchTime(run.FinishedAt),
		MergeSha:    run.MergeSHA,
		ResumedOf:   run.ResumedOf,
		ParentRunId: run.ParentRunID,
		Priority:    dispatchPriority(run),
		Attempts:    dispatchAttemptsProto(run.Attempts),
	}
	out.WorktreeStatus = run.WorktreeStatus
	if run.FilesChanged != nil {
		v := int32(*run.FilesChanged)
		out.FilesChanged = &v
	}
	if run.Insertions != nil {
		v := int32(*run.Insertions)
		out.Insertions = &v
	}
	if run.Deletions != nil {
		v := int32(*run.Deletions)
		out.Deletions = &v
	}
	if run.QueuedMs != nil {
		v := *run.QueuedMs
		out.QueuedMs = &v
	}
	if run.RunMs != nil {
		v := *run.RunMs
		out.RunMs = &v
	}
	if run.MergedAt != nil && !run.MergedAt.IsZero() {
		out.MergedAt = formatDispatchTime(*run.MergedAt)
	}
	if u := run.Usage; u != nil {
		out.Usage = &dispatchv1.DispatchUsage{
			InputTokens:      u.InputTokens,
			CacheReadTokens:  u.CacheReadTokens,
			CacheWriteTokens: u.CacheWriteTokens,
			OutputTokens:     u.OutputTokens,
			ReasoningTokens:  u.ReasoningTokens,
			CostUsd:          u.CostUSD,
			CostSource:       u.CostSource,
			ContextTokens:    u.ContextTokens,
			ContextWindow:    u.ContextWindow,
		}
	}
	return out
}

func dispatchPriority(run dispatch.Run) string {
	if run.Resolved != nil && run.Resolved.Priority != "" {
		return run.Resolved.Priority
	}
	if run.Requested != nil {
		return run.Requested.Priority
	}
	return ""
}

func dispatchAttemptsProto(attempts []dispatch.Attempt) []*dispatchv1.DispatchAttempt {
	if len(attempts) == 0 {
		return nil
	}
	out := make([]*dispatchv1.DispatchAttempt, 0, len(attempts))
	for _, attempt := range attempts {
		item := &dispatchv1.DispatchAttempt{
			Agent:     attempt.Agent,
			Model:     attempt.Model,
			Error:     attempt.Error,
			ExitClass: attempt.ExitClass,
		}
		if attempt.DurationMs != nil {
			ms := *attempt.DurationMs
			item.DurationMs = &ms
		}
		out = append(out, item)
	}
	return out
}

func dispatchRunEventProto(ev dispatch.RunEvent) *dispatchv1.DispatchRunEvent {
	return &dispatchv1.DispatchRunEvent{
		Id:        ev.ID,
		RunId:     ev.RunID,
		ProjectId: ev.ProjectID,
		Kind:      ev.Kind,
		Agent:     ev.Agent,
		Tier:      ev.Tier,
		Text:      ev.Text,
		Seq:       ev.Seq,
		At:        formatDispatchTime(ev.At),
	}
}

func watchRunProto(ev dispatch.RunEvent) *dispatchv1.WatchRunResponse {
	return &dispatchv1.WatchRunResponse{
		Seq:  ev.Seq,
		Kind: ev.Kind,
		Text: ev.Text,
		At:   formatDispatchTime(ev.At),
	}
}

func dispatchWorktreeProto(wt dispatch.WorktreeInfo) *dispatchv1.DispatchWorktree {
	return &dispatchv1.DispatchWorktree{
		Id:        wt.ID,
		ProjectId: wt.ProjectID,
		RunId:     wt.RunID,
		CardSlug:  wt.CardSlug,
		Path:      wt.Path,
		Branch:    wt.Branch,
		BaseSha:   wt.BaseSHA,
		Status:    wt.Status,
		Owner:     wt.Owner,
	}
}

func dispatchQuotaProto(snap dispatch.QuotaSnapshot) *dispatchv1.DispatchQuotaSnapshot {
	out := &dispatchv1.DispatchQuotaSnapshot{
		Agent:         snap.Agent,
		Provider:      snap.Provider,
		Window:        snap.Window,
		Source:        snap.Source,
		Confidence:    snap.Confidence,
		WindowMinutes: int32(snap.WindowMinutes),
		ResetsAt:      formatDispatchTime(snap.ResetsAt),
		SampledAt:     formatDispatchTime(snap.SampledAt),
	}
	if snap.UsedPercent != nil {
		v := *snap.UsedPercent
		out.UsedPercent = &v
	}
	return out
}

func dispatchTierProto(tier dispatch.Tier) *dispatchv1.DispatchTier {
	return &dispatchv1.DispatchTier{
		Name:     tier.Name,
		Provider: tier.Provider,
		Agent:    tier.Agent,
		Model:    tier.Model,
		Speed:    int32(tier.Speed),
		Strength: int32(tier.Strength),
		Position: int32(tier.Position),
	}
}

func dispatchAgentProto(agent dispatch.Agent) *dispatchv1.DispatchAgent {
	out := &dispatchv1.DispatchAgent{
		Name:     agent.Name,
		Bin:      agent.Bin,
		Provider: agent.Provider,
	}
	if agent.UsedPercent != nil {
		v := *agent.UsedPercent
		out.UsedPercent = &v
	}
	if agent.CooldownUntil != nil && !agent.CooldownUntil.IsZero() {
		out.CooldownUntil = formatDispatchTime(*agent.CooldownUntil)
	}
	return out
}
