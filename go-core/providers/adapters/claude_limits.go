package adapters

import (
	"encoding/json"
	"math"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"sync"
	"time"
)

// claudeLimits is a rate_limit_event's rate_limit_info: each subscription
// window with how much of it is used (0 to 1) and when it resets.
type claudeLimits struct {
	Windows map[string]struct {
		Utilization *float64 `json:"utilization"`
		ResetsAt    int64    `json:"resetsAt"`
	} `json:"unifiedWindows"`
}

// claudeWindow is the span of a window Claude Code names, zero for one it
// does not name yet.
func claudeWindow(name string) time.Duration {
	switch {
	case name == "five_hour":
		return 5 * time.Hour
	case strings.HasPrefix(name, "seven_day"):
		return 7 * 24 * time.Hour
	}
	return 0
}

// samples turns each window with a figure into an official sample, the one
// closest to its limit last, as the latest sample a reader sees.
func (l *claudeLimits) samples(at time.Time) []QuotaSample {
	var out []QuotaSample
	for name, w := range l.Windows {
		span := claudeWindow(name)
		if w.Utilization == nil || w.ResetsAt == 0 || span == 0 {
			continue
		}
		used := math.Round(*w.Utilization*10000) / 100
		out = append(out, QuotaSample{
			Window:     name,
			Used:       &used,
			ResetAt:    time.Unix(w.ResetsAt, 0).UTC(),
			Duration:   Span(span),
			Source:     SourceClaudeStream,
			Confidence: ConfidenceOfficial,
			SampledAt:  at,
		})
	}
	sort.Slice(out, func(i, j int) bool {
		if *out[i].Used != *out[j].Used {
			return *out[i].Used < *out[j].Used
		}
		return out[i].Window < out[j].Window
	})
	return out
}

// observed keeps the windows of the last rate_limit_event a Claude run
// printed, Claude Code's only report of its quota, and a file in the state
// directory when the core set one, so a restart keeps them.
var observed struct {
	sync.Mutex
	dir    string
	claude []QuotaSample
	loaded bool
}

const claudeLimitsFile = "claude-rate-limits.json"

// SetStateDir is where adapters keep what they observed across restarts.
func SetStateDir(dir string) {
	observed.Lock()
	defer observed.Unlock()
	observed.dir, observed.claude, observed.loaded = dir, nil, false
}

func rememberClaudeLimits(s []QuotaSample) {
	if len(s) == 0 {
		return
	}
	observed.Lock()
	defer observed.Unlock()
	observed.claude, observed.loaded = s, true
	if observed.dir == "" {
		return
	}
	if b, err := json.Marshal(s); err == nil {
		_ = os.WriteFile(filepath.Join(observed.dir, claudeLimitsFile), b, 0o600)
	}
}

// claudeObserved is the remembered windows that have not reset by now: a
// reset window's figure says nothing about it any more.
func claudeObserved(now time.Time) []QuotaSample {
	observed.Lock()
	defer observed.Unlock()
	if !observed.loaded && observed.dir != "" {
		if b, err := os.ReadFile(filepath.Join(observed.dir, claudeLimitsFile)); err == nil {
			_ = json.Unmarshal(b, &observed.claude)
		}
		observed.loaded = true
	}
	var out []QuotaSample
	for _, s := range observed.claude {
		if s.Used != nil && s.ResetAt.After(now) {
			out = append(out, s)
		}
	}
	return out
}

// claudeQuota is the window closest to its limit, since either one stops a
// run.
func claudeQuota(s []QuotaSample) (Quota, error) {
	var top *QuotaSample
	for i := range s {
		if s[i].Used != nil && (top == nil || *s[i].Used > *top.Used) {
			top = &s[i]
		}
	}
	if top == nil {
		return Quota{}, ErrNoQuota
	}
	return Quota{
		Key:           "claude",
		UsedPercent:   *top.Used,
		WindowMinutes: int(top.Duration.Duration().Minutes()),
		ResetsAt:      top.ResetAt,
		SampledAt:     top.SampledAt,
	}, nil
}
