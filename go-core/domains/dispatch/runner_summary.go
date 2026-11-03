package dispatch

import (
	"fmt"
	"regexp"
	"time"

	adapter "github.com/polterware/dispatch/go-core/providers/adapters"
)

const SummarySchema = "dispatch.run-summary.v1"

var (
	conflictRe = regexp.MustCompile(`(?i)merge conflict|conflict marker|\bCONFLICT\b|<<<<<<<`)
	crashRe    = regexp.MustCompile(`(?i)\bpanic:|goroutine \d+ \[|segmentation fault|core dumped|fatal signal|signal: (?:segmentation|abort|bus|killed)|exit status 13[479]`)
)

// EffortOf and ProviderOf retain the telemetry API.
func EffortOf(model string) string          { return adapter.EffortOf(model) }
func ProviderOf(agent, model string) string { return adapter.ProviderOf(agent, model) }

// classOf labels an agent error. "failed" is the generic bucket.
func classOf(msg string) string {
	switch {
	case adapter.CreditRe.MatchString(msg):
		return "credits"
	case rateLimited.MatchString(msg):
		return "rate_limit"
	case conflictRe.MatchString(msg):
		return "merge_conflict"
	case crashRe.MatchString(msg):
		return "crash"
	default:
		return "failed"
	}
}

// Classify refines ExitClass with credits, rate_limit, crash and merge_conflict.
func Classify(rec Run) string {
	if rec.Status == StatusFailed {
		if n := len(rec.Attempts); n > 0 {
			switch rec.Attempts[n-1].ExitClass {
			case "credits", "rate_limit", "crash", "merge_conflict":
				return rec.Attempts[n-1].ExitClass
			}
		}
		if cls := classOf(rec.Error); cls != "failed" {
			return cls
		}
	}
	if rec.Status == StatusCompleted && (conflictRe.MatchString(rec.Note) || conflictRe.MatchString(rec.Error)) {
		return "merge_conflict"
	}
	return ExitClass(rec.Status)
}

// tries is how many agents were invoked. Failed attempts are recorded; the
// final agent (success, timeout, cancel, orphan, or the one still running)
// is not in that slice.
func tries(rec Run) int {
	n := len(rec.Attempts)
	switch rec.Status {
	case StatusCompleted, StatusTimedOut, StatusCancelled, StatusOrphaned, StatusRunning:
		if rec.Agent != "" {
			n++
		}
	}
	return n
}

// RunSummary is the dispatch.run-summary.v1 document. It never carries the
// agent's free text; that stays behind `dispatch result <id> --section agent`.
type RunSummary struct {
	Requested    *Filters          `json:"requested,omitempty"`
	Resolved     *Filters          `json:"resolved,omitempty"`
	Schema       string            `json:"schema"`
	ID           string            `json:"id"`
	Status       string            `json:"status"`
	Agent        string            `json:"agent,omitempty"`
	Model        string            `json:"model,omitempty"`
	Tier         string            `json:"tier,omitempty"`
	Provider     string            `json:"provider,omitempty"`
	Effort       string            `json:"effort,omitempty"`
	DurationMs   *int64            `json:"durationMs,omitempty"`
	QueuedMs     *int64            `json:"queuedMs,omitempty"`
	RunMs        *int64            `json:"runMs,omitempty"`
	FilesChanged *int              `json:"filesChanged,omitempty"`
	Insertions   *int              `json:"insertions,omitempty"`
	Deletions    *int              `json:"deletions,omitempty"`
	Usage        *Usage            `json:"usage,omitempty"`
	Attempts     int               `json:"attempts"`
	ExitClass    string            `json:"exitClass,omitempty"`
	Branch       string            `json:"branch,omitempty"`
	Worktree     string            `json:"worktree,omitempty"`
	Error        string            `json:"error,omitempty"`
	Note         string            `json:"note,omitempty"`
	SessionID    string            `json:"sessionId,omitempty"`
	ResumedOf    string            `json:"resumedOf,omitempty"`
	Details      map[string]string `json:"details"`
}

// Summarize builds a RunSummary. Absent numbers stay absent.
func Summarize(rec Run) RunSummary {
	s := RunSummary{
		Schema:    SummarySchema,
		Requested: rec.Requested, Resolved: rec.Resolved,
		ID:           rec.ID,
		Status:       rec.Status,
		Agent:        rec.Agent,
		Model:        rec.Model,
		Tier:         rec.Tier,
		Provider:     rec.Provider,
		Effort:       rec.Effort,
		QueuedMs:     rec.QueuedMs,
		RunMs:        rec.RunMs,
		FilesChanged: rec.FilesChanged,
		Insertions:   rec.Insertions,
		Deletions:    rec.Deletions,
		Usage:        rec.Usage,
		Attempts:     tries(rec),
		ExitClass:    rec.ExitClass,
		Branch:       rec.Branch,
		Worktree:     rec.Worktree,
		Error:        rec.Error,
		Note:         rec.Note,
		SessionID:    rec.SessionID,
		ResumedOf:    rec.ResumedOf,
		Details:      detailCmds(rec.ID),
	}
	if ms, ok := rec.ObservedMs(); ok {
		s.DurationMs = I64(ms)
	}
	return s
}

func detailCmds(id string) map[string]string {
	sec := func(name string) string {
		return "dispatch result " + id + " --section " + name
	}
	return map[string]string{
		"summary":  sec("summary"),
		"files":    sec("files"),
		"agent":    sec("agent"),
		"attempts": sec("attempts"),
		"meta":     sec("meta"),
	}
}

// Text is the three-line human rendering. Unknown fields say "unknown".
func (s RunSummary) Text() string {
	line1 := fmt.Sprintf("%s %s/%s tier=%s %s %s attempts=%d",
		s.Status, summaryUnk(s.Agent), summaryUnk(s.Model), summaryUnk(s.Tier), summaryFmtMs(s.DurationMs), summaryFmtFiles(s), s.Attempts)
	line2 := fmt.Sprintf("branch=%s worktree=%s", summaryUnk(s.Branch), summaryUnk(s.Worktree))
	line3 := fmt.Sprintf("details: dispatch result %s --section summary|files|agent|attempts|meta", s.ID)
	if s.Error != "" {
		line2 += " error=" + s.Error
	}
	if s.Note != "" {
		line2 += " note=" + s.Note
	}
	if s.Requested != nil {
		line2 += fmt.Sprintf(" requested=%+v resolved=%+v", *s.Requested, s.Resolved)
	}
	return line1 + "\n" + line2 + "\n" + line3 + "\n"
}

func summaryUnk(s string) string {
	if s == "" {
		return "unknown"
	}
	return s
}

func summaryFmtMs(ms *int64) string {
	if ms == nil {
		return "unknown"
	}
	if *ms < 1000 {
		return fmt.Sprintf("%dms", *ms)
	}
	return (time.Duration(*ms) * time.Millisecond).Round(100 * time.Millisecond).String()
}

func summaryFmtFiles(s RunSummary) string {
	if s.FilesChanged == nil {
		return "files=unknown"
	}
	if s.Insertions == nil || s.Deletions == nil {
		return fmt.Sprintf("files=%d +/- unknown", *s.FilesChanged)
	}
	return fmt.Sprintf("files=%d +%d -%d", *s.FilesChanged, *s.Insertions, *s.Deletions)
}
