package main

import (
	"context"
	"encoding/json"
	"fmt"
	"strings"
	"time"

	"connectrpc.com/connect"

	"github.com/polterware/dispatch/go-core/domains/dispatch"
	"github.com/polterware/dispatch/go-core/platform/coreclient"
	dispatchv1 "github.com/polterware/dispatch/go-core/platform/gen/dispatch/v1"
	"github.com/polterware/dispatch/go-core/platform/gen/dispatch/v1/dispatchv1connect"
)

type dispatchStartRequest struct {
	ProjectID  string
	Tier       string
	CardSlug   string
	Agent      string
	Worktree   string
	Dir        string
	ReadOnly   bool
	Provider   string
	Model      string
	Effort     string
	Priority   string
	Relax      bool
	TimeoutSec int
	Profile    string
	Prompt     string
	Caller     dispatch.Caller
}

type dispatchListRunsRequest struct {
	ConversationID string
	ProjectID      string
	Limit          int
	Offset         int
}

type dispatchResultRequest struct {
	RunID   string
	Section string
	Offset  int
	Limit   int
}

type dispatchResultResponse struct {
	Run        dispatch.Run `json:"run"`
	Section    string       `json:"section"`
	Offset     int          `json:"offset"`
	Limit      int          `json:"limit"`
	TotalLines int          `json:"total_lines"`
	HasMore    bool         `json:"has_more"`
	Content    string       `json:"content,omitempty"`
}

type dispatchHistoryRequest struct {
	ProjectID string
	Agent     string
	Tier      string
	Limit     int
	Offset    int
}

type dispatchTierCheck struct {
	OK       bool     `json:"ok"`
	Problems []string `json:"problems"`
}

type dispatchStatsRow struct {
	Key         string  `json:"key"`
	Runs        int     `json:"runs"`
	SuccessPct  float64 `json:"success_pct"`
	FirstTryPct float64 `json:"first_try_pct"`
	P50Ms       *int64  `json:"p50_ms,omitempty"`
	P90Ms       *int64  `json:"p90_ms,omitempty"`
	Fallbacks   int     `json:"fallbacks"`
	RateLimits  int     `json:"rate_limits"`
	Orphans     int     `json:"orphans"`

	UsageRuns        int     `json:"usage_runs"`
	InputTokens      int64   `json:"input_tokens"`
	CacheReadTokens  int64   `json:"cache_read_tokens"`
	CacheWriteTokens int64   `json:"cache_write_tokens"`
	OutputTokens     int64   `json:"output_tokens"`
	ReasoningTokens  int64   `json:"reasoning_tokens"`
	CostUSD          float64 `json:"cost_usd"`
	CostRuns         int     `json:"cost_runs"`
}

type dispatchStatsRequest struct {
	ProjectID string
	By        string
	Since     string
}

type dispatchStatsResponse struct {
	By       string             `json:"by"`
	Since    string             `json:"since,omitempty"`
	InFlight int                `json:"in_flight,omitempty"`
	Rows     []dispatchStatsRow `json:"rows"`
}

// dispatchClient is the local CLI/TUI boundary for the core's DispatchService.
type dispatchClient interface {
	ListRuns(ctx context.Context, req dispatchListRunsRequest) ([]dispatch.Run, error)
	GetRun(ctx context.Context, id string) (dispatch.Run, error)
	StartRun(ctx context.Context, req dispatchStartRequest) (dispatch.Run, error)
	CancelRun(ctx context.Context, id string) (dispatch.Run, error)
	Result(ctx context.Context, req dispatchResultRequest) (dispatchResultResponse, error)
	ListWorktrees(ctx context.Context, projectID string) ([]dispatch.WorktreeInfo, error)
	ListQuota(ctx context.Context, agent string, since time.Time) ([]dispatch.QuotaSnapshot, error)
	ListTiers(ctx context.Context) ([]dispatch.Tier, error)
	ListAgents(ctx context.Context) ([]dispatch.Agent, error)
	ListHistory(ctx context.Context, req dispatchHistoryRequest) ([]dispatch.RunEvent, error)
	CheckTiers(ctx context.Context) (dispatchTierCheck, error)
	Stats(ctx context.Context, req dispatchStatsRequest) (dispatchStatsResponse, error)
	WatchRun(ctx context.Context, id string, afterSeq int64) (<-chan dispatch.RunEvent, error)
}

type connectDispatchClient struct {
	svc dispatchv1connect.DispatchServiceClient
}

func newDispatchClient(c coreclient.Clients) dispatchClient {
	return &connectDispatchClient{svc: c.Dispatch}
}

func (c *connectDispatchClient) ListRuns(ctx context.Context, req dispatchListRunsRequest) ([]dispatch.Run, error) {
	res, err := c.svc.ListRuns(ctx, connect.NewRequest(&dispatchv1.ListRunsRequest{
		ProjectId:      req.ProjectID,
		ConversationId: req.ConversationID,
		Limit:          int32(req.Limit),
		Offset:         int32(req.Offset),
	}))
	if err != nil {
		return nil, wrapDispatchError(err)
	}
	items := res.Msg.GetRuns()
	out := make([]dispatch.Run, 0, len(items))
	for _, item := range items {
		out = append(out, dispatchRunFromProto(item))
	}
	return out, nil
}

func (c *connectDispatchClient) GetRun(ctx context.Context, id string) (dispatch.Run, error) {
	res, err := c.svc.GetRun(ctx, connect.NewRequest(&dispatchv1.GetRunRequest{
		Id: id,
	}))
	if err != nil {
		return dispatch.Run{}, wrapDispatchError(err)
	}
	return dispatchRunFromProto(res.Msg.GetRun()), nil
}

func (c *connectDispatchClient) StartRun(ctx context.Context, req dispatchStartRequest) (dispatch.Run, error) {
	res, err := c.svc.StartRun(ctx, connect.NewRequest(&dispatchv1.StartRunRequest{
		ProjectId:  req.ProjectID,
		Tier:       req.Tier,
		Prompt:     req.Prompt,
		CardSlug:   req.CardSlug,
		Agent:      req.Agent,
		Worktree:   req.Worktree,
		Dir:        req.Dir,
		ReadOnly:   req.ReadOnly,
		Provider:   req.Provider,
		Model:      req.Model,
		Effort:     req.Effort,
		Priority:   req.Priority,
		Relax:      req.Relax,
		TimeoutSec: int32(req.TimeoutSec),
		Profile:    req.Profile,
		Caller:     &dispatchv1.DispatchCaller{Agent: req.Caller.Agent, SessionId: req.Caller.SessionID, ConversationId: req.Caller.ConversationID, Label: req.Caller.Label},
	}))
	if err != nil {
		return dispatch.Run{}, wrapDispatchError(err)
	}
	return dispatchRunFromProto(res.Msg.GetRun()), nil
}

func (c *connectDispatchClient) CancelRun(ctx context.Context, id string) (dispatch.Run, error) {
	res, err := c.svc.CancelRun(ctx, connect.NewRequest(&dispatchv1.CancelRunRequest{
		Id: id,
	}))
	if err != nil {
		return dispatch.Run{}, wrapDispatchError(err)
	}
	return dispatchRunFromProto(res.Msg.GetRun()), nil
}

func (c *connectDispatchClient) Result(ctx context.Context, req dispatchResultRequest) (dispatchResultResponse, error) {
	section := strings.TrimSpace(req.Section)
	if section == "" {
		section = "summary"
	}
	res, err := c.svc.Result(ctx, connect.NewRequest(&dispatchv1.ResultRequest{
		Id:      req.RunID,
		Section: section,
		Offset:  int32(req.Offset),
		Limit:   int32(req.Limit),
	}))
	if err != nil {
		return dispatchResultResponse{}, wrapDispatchError(err)
	}
	var run dispatch.Run
	if fetched, runErr := c.GetRun(ctx, req.RunID); runErr == nil {
		run = fetched
	} else {
		run.ID = req.RunID
	}
	content := res.Msg.GetContent()
	trimmed := strings.TrimSpace(content)
	switch section {
	case "attempts":
		if trimmed != "" {
			var attempts []dispatch.Attempt
			if json.Unmarshal([]byte(trimmed), &attempts) == nil {
				run.Attempts = attempts
			}
		}
	case "meta":
		if trimmed != "" {
			var metaRun dispatch.Run
			if json.Unmarshal([]byte(trimmed), &metaRun) == nil {
				run = metaRun
			}
		}
	}
	total := int(res.Msg.GetTotal())
	offset := req.Offset
	if offset < 0 {
		offset = 0
	}
	hasMore := req.Limit > 0 && offset+req.Limit < total
	return dispatchResultResponse{
		Run:        run,
		Section:    section,
		Offset:     req.Offset,
		Limit:      req.Limit,
		TotalLines: total,
		HasMore:    hasMore,
		Content:    content,
	}, nil
}

func (c *connectDispatchClient) ListWorktrees(ctx context.Context, projectID string) ([]dispatch.WorktreeInfo, error) {
	res, err := c.svc.ListWorktrees(ctx, connect.NewRequest(&dispatchv1.ListWorktreesRequest{
		ProjectId: projectID,
	}))
	if err != nil {
		return nil, wrapDispatchError(err)
	}
	items := res.Msg.GetWorktrees()
	out := make([]dispatch.WorktreeInfo, 0, len(items))
	for _, item := range items {
		out = append(out, dispatchWorktreeFromProto(item))
	}
	return out, nil
}

func (c *connectDispatchClient) ListQuota(ctx context.Context, agent string, since time.Time) ([]dispatch.QuotaSnapshot, error) {
	sinceStr := ""
	if !since.IsZero() {
		sinceStr = since.UTC().Format(time.RFC3339)
	}
	res, err := c.svc.ListQuota(ctx, connect.NewRequest(&dispatchv1.ListQuotaRequest{
		Agent: agent,
		Since: sinceStr,
	}))
	if err != nil {
		return nil, wrapDispatchError(err)
	}
	items := res.Msg.GetSnapshots()
	out := make([]dispatch.QuotaSnapshot, 0, len(items))
	for _, item := range items {
		out = append(out, dispatchQuotaFromProto(item))
	}
	return out, nil
}

func (c *connectDispatchClient) ListTiers(ctx context.Context) ([]dispatch.Tier, error) {
	res, err := c.svc.ListTiers(ctx, connect.NewRequest(&dispatchv1.ListTiersRequest{}))
	if err != nil {
		return nil, wrapDispatchError(err)
	}
	items := res.Msg.GetTiers()
	out := make([]dispatch.Tier, 0, len(items))
	for _, item := range items {
		out = append(out, dispatchTierFromProto(item))
	}
	return out, nil
}

func (c *connectDispatchClient) ListAgents(ctx context.Context) ([]dispatch.Agent, error) {
	res, err := c.svc.ListAgents(ctx, connect.NewRequest(&dispatchv1.ListAgentsRequest{}))
	if err != nil {
		return nil, wrapDispatchError(err)
	}
	items := res.Msg.GetAgents()
	out := make([]dispatch.Agent, 0, len(items))
	for _, item := range items {
		out = append(out, dispatchAgentFromProto(item))
	}
	return out, nil
}

func (c *connectDispatchClient) ListHistory(ctx context.Context, req dispatchHistoryRequest) ([]dispatch.RunEvent, error) {
	res, err := c.svc.ListHistory(ctx, connect.NewRequest(&dispatchv1.ListHistoryRequest{
		ProjectId: req.ProjectID,
		Agent:     req.Agent,
		Tier:      req.Tier,
		Limit:     int32(req.Limit),
		Offset:    int32(req.Offset),
	}))
	if err != nil {
		return nil, wrapDispatchError(err)
	}
	items := res.Msg.GetEvents()
	out := make([]dispatch.RunEvent, 0, len(items))
	for _, item := range items {
		out = append(out, dispatchRunEventFromProto(item))
	}
	return out, nil
}

func (c *connectDispatchClient) CheckTiers(ctx context.Context) (dispatchTierCheck, error) {
	res, err := c.svc.CheckTiers(ctx, connect.NewRequest(&dispatchv1.CheckTiersRequest{}))
	if err != nil {
		return dispatchTierCheck{}, wrapDispatchError(err)
	}
	problems := res.Msg.GetProblems()
	return dispatchTierCheck{
		OK:       len(problems) == 0,
		Problems: problems,
	}, nil
}

func (c *connectDispatchClient) Stats(ctx context.Context, req dispatchStatsRequest) (dispatchStatsResponse, error) {
	res, err := c.svc.Stats(ctx, connect.NewRequest(&dispatchv1.StatsRequest{
		ProjectId: req.ProjectID,
		By:        req.By,
		Since:     req.Since,
	}))
	if err != nil {
		return dispatchStatsResponse{}, wrapDispatchError(err)
	}
	items := res.Msg.GetRows()
	rows := make([]dispatchStatsRow, 0, len(items))
	for _, item := range items {
		row := dispatchStatsRow{
			Key:         item.GetKey(),
			Runs:        int(item.GetRuns()),
			SuccessPct:  item.GetSuccessPct(),
			FirstTryPct: item.GetFirstTryPct(),
			Fallbacks:   int(item.GetFallbacks()),
			RateLimits:  int(item.GetRateLimits()),
			Orphans:     int(item.GetOrphans()),

			UsageRuns:        int(item.GetUsageRuns()),
			InputTokens:      item.GetInputTokens(),
			CacheReadTokens:  item.GetCacheReadTokens(),
			CacheWriteTokens: item.GetCacheWriteTokens(),
			OutputTokens:     item.GetOutputTokens(),
			ReasoningTokens:  item.GetReasoningTokens(),
			CostUSD:          item.GetCostUsd(),
			CostRuns:         int(item.GetCostRuns()),
		}
		if item.P50Ms != nil {
			v := *item.P50Ms
			row.P50Ms = &v
		}
		if item.P90Ms != nil {
			v := *item.P90Ms
			row.P90Ms = &v
		}
		rows = append(rows, row)
	}
	return dispatchStatsResponse{
		By:       res.Msg.GetBy(),
		Since:    res.Msg.GetSince(),
		InFlight: int(res.Msg.GetInFlight()),
		Rows:     rows,
	}, nil
}

func (c *connectDispatchClient) WatchRun(ctx context.Context, id string, afterSeq int64) (<-chan dispatch.RunEvent, error) {
	run, err := c.GetRun(ctx, id)
	if err != nil {
		return nil, err
	}
	stream, err := c.svc.WatchRun(ctx, connect.NewRequest(&dispatchv1.WatchRunRequest{
		Id:       id,
		AfterSeq: afterSeq,
	}))
	if err != nil {
		return nil, wrapDispatchError(err)
	}
	ch := make(chan dispatch.RunEvent, 16)
	go func() {
		defer close(ch)
		defer stream.Close()
		for stream.Receive() {
			msg := stream.Msg()
			ev := dispatch.RunEvent{
				RunID:     id,
				ProjectID: run.ProjectID,
				Agent:     run.Agent,
				Tier:      run.Tier,
				Kind:      msg.GetKind(),
				Text:      msg.GetText(),
				Seq:       msg.GetSeq(),
				At:        parseDispatchTime(msg.GetAt()),
			}
			select {
			case ch <- ev:
			case <-ctx.Done():
				return
			}
		}
	}()
	return ch, nil
}

func wrapDispatchError(err error) error {
	if err == nil {
		return nil
	}
	if connect.CodeOf(err) == connect.CodeNotFound {
		return connect.NewError(connect.CodeNotFound, fmt.Errorf("run not found: %w (%v)", dispatch.ErrUnknownRun, err))
	}
	return err
}

func parseDispatchTime(value string) time.Time {
	s := strings.TrimSpace(value)
	if s == "" {
		return time.Time{}
	}
	if t, err := time.Parse(time.RFC3339Nano, s); err == nil {
		return t
	}
	if t, err := time.Parse(time.RFC3339, s); err == nil {
		return t
	}
	return time.Time{}
}

func dispatchRunFromProto(p *dispatchv1.DispatchRun) dispatch.Run {
	if p == nil {
		return dispatch.Run{}
	}
	out := dispatch.Run{
		ConversationID: p.GetConversationId(), CallerAgent: p.GetCallerAgent(),
		ID:         p.GetId(),
		ProjectID:  p.GetProjectId(),
		Status:     p.GetStatus(),
		Tier:       p.GetTier(),
		Agent:      p.GetAgent(),
		Model:      p.GetModel(),
		Provider:   p.GetProvider(),
		Effort:     p.GetEffort(),
		Prompt:     p.GetPrompt(),
		CardSlug:   p.GetCardSlug(),
		AgentRunID: p.GetAgentRunId(),
		Dir:        p.GetDir(),
		Repo:       p.GetRepo(),
		Worktree:   p.GetWorktree(),
		Branch:     p.GetBranch(),
		BaseSHA:    p.GetBaseSha(),
		ReadOnly:   p.GetReadOnly(),
		ExitCode:   int(p.GetExitCode()),
		ExitClass:  p.GetExitClass(),
		Error:      p.GetError(),
		Note:       p.GetNote(),
		CreatedAt:  parseDispatchTime(p.GetCreatedAt()),
		StartedAt:  parseDispatchTime(p.GetStartedAt()),
		FinishedAt: parseDispatchTime(p.GetFinishedAt()),
		MergeSHA:   p.GetMergeSha(),
	}
	if p.FilesChanged != nil {
		v := int(*p.FilesChanged)
		out.FilesChanged = &v
	}
	if p.Insertions != nil {
		v := int(*p.Insertions)
		out.Insertions = &v
	}
	if p.Deletions != nil {
		v := int(*p.Deletions)
		out.Deletions = &v
	}
	if p.QueuedMs != nil {
		v := *p.QueuedMs
		out.QueuedMs = &v
	}
	if p.RunMs != nil {
		v := *p.RunMs
		out.RunMs = &v
	}
	if u := p.GetUsage(); u != nil {
		out.Usage = &dispatch.Usage{
			InputTokens:      u.GetInputTokens(),
			CacheReadTokens:  u.GetCacheReadTokens(),
			CacheWriteTokens: u.GetCacheWriteTokens(),
			OutputTokens:     u.GetOutputTokens(),
			ReasoningTokens:  u.GetReasoningTokens(),
			CostUSD:          u.CostUsd,
			CostSource:       u.GetCostSource(),
			ContextTokens:    u.GetContextTokens(),
			ContextWindow:    u.GetContextWindow(),
		}
	}
	if t := parseDispatchTime(p.GetMergedAt()); !t.IsZero() {
		out.MergedAt = &t
	}
	return out
}

func dispatchWorktreeFromProto(p *dispatchv1.DispatchWorktree) dispatch.WorktreeInfo {
	if p == nil {
		return dispatch.WorktreeInfo{}
	}
	return dispatch.WorktreeInfo{
		ID:        p.GetId(),
		ProjectID: p.GetProjectId(),
		RunID:     p.GetRunId(),
		CardSlug:  p.GetCardSlug(),
		Path:      p.GetPath(),
		Branch:    p.GetBranch(),
		BaseSHA:   p.GetBaseSha(),
		Status:    p.GetStatus(),
		Owner:     p.GetOwner(),
	}
}

func dispatchQuotaFromProto(p *dispatchv1.DispatchQuotaSnapshot) dispatch.QuotaSnapshot {
	if p == nil {
		return dispatch.QuotaSnapshot{}
	}
	out := dispatch.QuotaSnapshot{
		Agent:         p.GetAgent(),
		Provider:      p.GetProvider(),
		Window:        p.GetWindow(),
		Source:        p.GetSource(),
		Confidence:    p.GetConfidence(),
		WindowMinutes: int(p.GetWindowMinutes()),
		ResetsAt:      parseDispatchTime(p.GetResetsAt()),
		SampledAt:     parseDispatchTime(p.GetSampledAt()),
	}
	if p.UsedPercent != nil {
		v := *p.UsedPercent
		out.UsedPercent = &v
	}
	return out
}

func dispatchTierFromProto(p *dispatchv1.DispatchTier) dispatch.Tier {
	if p == nil {
		return dispatch.Tier{}
	}
	return dispatch.Tier{
		Name:     p.GetName(),
		Provider: p.GetProvider(),
		Agent:    p.GetAgent(),
		Model:    p.GetModel(),
		Speed:    int(p.GetSpeed()),
		Strength: int(p.GetStrength()),
		Position: int(p.GetPosition()),
	}
}

func dispatchAgentFromProto(p *dispatchv1.DispatchAgent) dispatch.Agent {
	if p == nil {
		return dispatch.Agent{}
	}
	out := dispatch.Agent{
		Name:     p.GetName(),
		Bin:      p.GetBin(),
		Provider: p.GetProvider(),
	}
	if p.UsedPercent != nil {
		v := *p.UsedPercent
		out.UsedPercent = &v
	}
	if t := parseDispatchTime(p.GetCooldownUntil()); !t.IsZero() {
		out.CooldownUntil = &t
	}
	return out
}

func dispatchRunEventFromProto(p *dispatchv1.DispatchRunEvent) dispatch.RunEvent {
	if p == nil {
		return dispatch.RunEvent{}
	}
	return dispatch.RunEvent{
		ID:        p.GetId(),
		RunID:     p.GetRunId(),
		ProjectID: p.GetProjectId(),
		Kind:      p.GetKind(),
		Agent:     p.GetAgent(),
		Tier:      p.GetTier(),
		Text:      p.GetText(),
		Seq:       p.GetSeq(),
		At:        parseDispatchTime(p.GetAt()),
	}
}
