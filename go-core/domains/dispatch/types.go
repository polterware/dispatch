// Package dispatch is the execution engine for delegated coding runs.
// A dispatch run is a delegation executed by the core. The core links it to an
// AgentRun (its audited session) and it may reference a Work card slug.
package dispatch

import (
	"context"
	"crypto/rand"
	"encoding/hex"
	"errors"
	"time"
)

const (
	StatusPending   = "PENDING"
	StatusRunning   = "RUNNING"
	StatusCompleted = "COMPLETED"
	StatusFailed    = "FAILED"
	StatusCancelled = "CANCELLED"
	StatusTimedOut  = "TIMED_OUT"
	StatusOrphaned  = "ORPHANED"
)

const (
	TierEasy   = "easy"
	TierNormal = "normal"
	TierHard   = "hard"
)

const (
	// EventOutput is a live output chunk. It is not an Activity entry.
	EventOutput = "output"
	// EventStarted, EventMerged, and EventFailed are the only dispatch rows
	// Activity shows.
	EventStarted = "started"
	EventMerged  = "merged"
	EventFailed  = "failed"
)

const (
	WorktreeActive    = "active"
	WorktreeReady     = "ready"
	WorktreeMerged    = "merged"
	WorktreeRemoved   = "removed"
	WorktreeAbandoned = "abandoned"
)

var (
	ErrUnknownRun      = errors.New("unknown dispatch run")
	ErrUnknownWorktree = errors.New("unknown worktree")
)

// Filters is the routing request or the route that actually ran.
type Filters struct {
	Provider string
	Model    string
	Effort   string
	Agent    string
	Priority string
	Relax    bool
}

// Attempt is one agent try inside a run, including a failed fallback.
type Attempt struct {
	Agent      string
	Model      string
	Error      string
	ExitClass  string
	DurationMs *int64
}

// Run is one delegated execution, linked to an AgentRun by AgentRunID.
//
// Tier fields:
//   - RequestedTier is the caller's tier before classification; it is empty
//     for tierless model/provider requests. On resume it is the previous
//     run's Tier, so it equals Tier.
//   - Tier is the routing tier. It differs from RequestedTier only on Auto,
//     whose RequestedTier is auto and whose Tier is the band of its difficulty.
//   - ExecutionTier is the tier of the candidate that actually ran. It differs from Tier when a
//     tierless filter or --relax resolves to a table candidate of another
//     tier, falls back to Tier when the candidate has none, and stays empty if
//     no attempt started or neither tier is known.
//
// Difficulty, DifficultyProbs, and Classifier are an Auto run's
// classification: d, P(easy|normal|hard) when the classifier gives them, and
// the classifier that answered. Other runs leave them empty.
// Caller identifies the calling conversation, never the worker's resume session.
type Caller struct {
	Agent          string
	SessionID      string
	ConversationID string
	Label          string
}

type Section struct {
	ID        string
	Name      string
	Position  int
	CreatedAt time.Time
	Items     []SectionItem
}

type SectionItem struct {
	Kind     string
	Ref      string
	Position int
}

type Run struct {
	ConversationID string
	CallerAgent    string
	ID             string
	ProjectID      string
	Status         string
	Tier           string
	RequestedTier  string
	ExecutionTier  string
	Agent          string
	Model          string
	Provider       string
	Effort         string
	Prompt         string
	Dir            string
	Repo           string
	Worktree       string
	Branch         string
	BaseSHA        string
	ReadOnly       bool
	NoWorktree     bool
	AgentRunID     string
	CardSlug       string
	// Area is the part of the project the run works in, resolved by the
	// caller; it is routing input, not stored (the decision log keeps it).
	Area        Area
	Output      string
	Error       string
	Note        string
	ExitCode    int
	ExitClass   string
	TimeoutSec  int
	SessionID   string
	ResumedOf   string
	ParentRunID string
	PID         int
	AgentPID    int
	ProcStart   string
	Profile     string
	Prefer      []string
	Avoid       []string
	// RolePick is how the classifier picked the run's role; nil when the
	// request named it or no pick was asked.
	RolePick        *RolePick
	Requested       *Filters
	Resolved        *Filters
	Attempts        []Attempt
	Classifier      string
	DifficultyProbs []float64
	Difficulty      *float64
	FilesChanged    *int
	Insertions      *int
	Deletions       *int
	QueuedMs        *int64
	RunMs           *int64
	// Usage is what the agents reported spending; nil when none did.
	Usage      *Usage
	CreatedAt  time.Time
	StartedAt  time.Time
	FinishedAt time.Time
	MergedAt   *time.Time
	MergeSHA   string

	// WorktreeStatus is read from the stored worktree, not persisted on the run.
	WorktreeStatus string
}

// RunEvent is an append-only fact about a run.
// Seq is assigned by the store when the caller leaves it at zero.
type RunEvent struct {
	ID        string
	RunID     string
	ProjectID string
	Kind      string
	Agent     string
	Tier      string
	Text      string
	Seq       int64
	At        time.Time
}

// Tier is one row of the declarative difficulty table.
type Tier struct {
	Name     string
	Provider string
	Agent    string
	Model    string
	Speed    int
	Strength int
	Position int
}

// Agent is the last probe of an installed CLI.
// UsedPercent is nil when the quota is unknown. Callers must not invent one.
type Agent struct {
	Name          string
	Bin           string
	Provider      string
	UsedPercent   *float64
	CooldownUntil *time.Time
}

// QuotaSnapshot is one official usage sample.
// UsedPercent is nil when the protocol did not report a figure.
type QuotaSnapshot struct {
	Agent         string
	Provider      string
	Window        string
	Source        string
	Confidence    string
	UsedPercent   *float64
	WindowMinutes int
	ResetsAt      time.Time
	SampledAt     time.Time
}

// WorktreeInfo is the core-owned git worktree for one run.
type WorktreeInfo struct {
	ID        string
	ProjectID string
	RunID     string
	CardSlug  string
	Path      string
	Branch    string
	BaseSHA   string
	Status    string
	Owner     string
}

// Store is the persistence boundary. The core implements it on SQLite.
// The engine must not open the database itself.
type Store interface {
	SaveRun(ctx context.Context, run Run) error
	GetRun(ctx context.Context, id string) (Run, error)
	ListRuns(ctx context.Context, projectID string, limit, offset int, conversationID ...string) ([]Run, error)
	AppendEvent(ctx context.Context, event RunEvent) error
	ListEvents(ctx context.Context, runID string, afterSeq int64, limit int) ([]RunEvent, error)
	ListHistory(ctx context.Context, projectID, agent, tier string, limit, offset int) ([]RunEvent, error)
	SaveWorktree(ctx context.Context, info WorktreeInfo) error
	ListWorktrees(ctx context.Context, projectID string) ([]WorktreeInfo, error)
	SaveQuota(ctx context.Context, snap QuotaSnapshot) error
	ListQuota(ctx context.Context, agent string, since time.Time) ([]QuotaSnapshot, error)
	ListTiers(ctx context.Context) ([]Tier, error)
	ReplaceTiers(ctx context.Context, tiers []Tier) error
	UpsertAgent(ctx context.Context, agent Agent) error
	ListAgents(ctx context.Context) ([]Agent, error)
	SetCooldown(ctx context.Context, agent string, until time.Time) error
	ClearCooldown(ctx context.Context, agent string) error
	// SaveRouteDecision stores the routing record of a new run, once per run.
	SaveRouteDecision(ctx context.Context, decision RouteDecision) error
	// GetRouteDecision returns a run's decision; ok is false when it has none.
	GetRouteDecision(ctx context.Context, runID string) (RouteDecision, bool, error)
	// ListRouteDecisions returns decisions, newest first; empty projectID lists all.
	ListRouteDecisions(ctx context.Context, projectID string) ([]RouteDecision, error)
}

// NewID returns a random 16-byte hex id.
func NewID() (string, error) {
	var buf [16]byte
	if _, err := rand.Read(buf[:]); err != nil {
		return "", err
	}
	return hex.EncodeToString(buf[:]), nil
}
