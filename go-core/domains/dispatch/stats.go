package dispatch

import (
	"context"
	"encoding/json"
	"fmt"
	"math"
	"regexp"
	"sort"
	"strings"
	"time"
)

// MinSamples is how many runs a comparative ranking or a measured routing
// term needs.
const MinSamples = 5

const (
	StatsSchema = "dispatch.stats.v1"
	Schema      = StatsSchema
)

// RateLimitRe matches rate-limit and quota messages across agent CLIs.
var RateLimitRe = regexp.MustCompile(`(?i)rate.?limit|quota|429|too many requests|usage limit|exhausted`)

// I64 returns a pointer to n for optional millisecond fields.
func I64(n int64) *int64 { return &n }

// F64 returns a pointer to v for optional probability and cost fields.
func F64(v float64) *float64 { return &v }

// I returns a pointer to n for optional count fields.
func I(n int) *int { return &n }

// ObservedMs returns the agent wall time when known: RunMs first, then
// FinishedAt minus StartedAt (or CreatedAt for older records).
func (r Run) ObservedMs() (int64, bool) {
	if r.RunMs != nil && *r.RunMs >= 0 {
		return *r.RunMs, true
	}
	start := r.StartedAt
	if start.IsZero() {
		start = r.CreatedAt
	}
	if start.IsZero() || r.FinishedAt.IsZero() || r.FinishedAt.Before(start) {
		return 0, false
	}
	return r.FinishedAt.Sub(start).Milliseconds(), true
}

// StatsOptions selects the aggregation. A zero Since includes every run.
type StatsOptions struct {
	ProjectID string
	By        string
	Since     time.Time
	Now       time.Time
}

// Report is the dispatch.stats.v1 document.
type Report struct {
	Schema     string   `json:"schema"`
	Since      string   `json:"since,omitempty"`
	By         string   `json:"by"`
	MinSamples int      `json:"minSamples"`
	InFlight   int      `json:"inFlight,omitempty"`
	Rows       []Row    `json:"rows"`
	Rankings   Rankings `json:"rankings"`
	Note       string   `json:"note,omitempty"`
}

// Row is one group. A nil percentile means the duration is unknown.
type Row struct {
	Key         string  `json:"key"`
	Runs        int     `json:"runs"`
	SuccessPct  float64 `json:"successPct"`
	FirstTryPct float64 `json:"firstTryPct"`
	P50Ms       *int64  `json:"p50Ms,omitempty"`
	P90Ms       *int64  `json:"p90Ms,omitempty"`
	Duration    string  `json:"duration,omitempty"` // "unknown" when no samples
	DurationN   int     `json:"durationSamples"`
	Fallbacks   int     `json:"fallbacks"`
	RateLimits  int     `json:"rateLimits"`
	Orphans     int     `json:"orphans"`
	Spend       Spend   `json:"spend"`
}

// Spend sums what a group's runs reported spending. UsageRuns is how many
// reported usage at all, and CostUSD sums only the CostRuns that were priced.
type Spend struct {
	UsageRuns        int     `json:"usageRuns"`
	InputTokens      int64   `json:"inputTokens"`
	CacheReadTokens  int64   `json:"cacheReadTokens"`
	CacheWriteTokens int64   `json:"cacheWriteTokens"`
	OutputTokens     int64   `json:"outputTokens"`
	ReasoningTokens  int64   `json:"reasoningTokens"`
	CostUSD          float64 `json:"costUsd"`
	CostRuns         int     `json:"costRuns"`
}

func (s *Spend) add(u *Usage) {
	if u == nil {
		return
	}
	s.UsageRuns++
	s.InputTokens += u.InputTokens
	s.CacheReadTokens += u.CacheReadTokens
	s.CacheWriteTokens += u.CacheWriteTokens
	s.OutputTokens += u.OutputTokens
	s.ReasoningTokens += u.ReasoningTokens
	if u.CostUSD != nil {
		s.CostUSD += *u.CostUSD
		s.CostRuns++
	}
}

// Rankings compare models and providers. A non-empty Reason means the rank
// was withheld; Key is then "unknown".
type Rankings struct {
	MostUsedModel    StatRank `json:"mostUsedModel"`
	MostUsedProvider StatRank `json:"mostUsedProvider"`
	SlowestModel     StatRank `json:"slowestModel"`
	MostFailedModel  StatRank `json:"mostFailedModel"`
}

// StatRank is one ranking. Comparative ranks require MinSamples.
type StatRank struct {
	Key        string   `json:"key"`
	Runs       int      `json:"runs,omitempty"`
	Samples    int      `json:"samples,omitempty"`
	P50Ms      *int64   `json:"p50Ms,omitempty"`
	SuccessPct *float64 `json:"successPct,omitempty"`
	Reason     string   `json:"reason,omitempty"`
}

// ValidBy reports whether --by accepts name.
func ValidBy(name string) bool {
	switch name {
	case "model", "agent", "provider", "tier", "effort", "area", "day", "week", "requested", "resolved":
		return true
	default:
		return false
	}
}

// ParseSince parses 30m, 12h, 7d, 2w, or all. The time is the inclusive start.
func ParseSince(s string, now time.Time) (time.Time, error) {
	s = strings.TrimSpace(strings.ToLower(s))
	if s == "" || s == "all" || s == "0" {
		return time.Time{}, nil
	}
	i := 0
	for i < len(s) && s[i] >= '0' && s[i] <= '9' {
		i++
	}
	if i == 0 || i == len(s) {
		return time.Time{}, fmt.Errorf("bad --since %q (want 7d, 24h, 30m, 2w, all)", s)
	}
	n := 0
	for _, c := range s[:i] {
		n = n*10 + int(c-'0')
	}
	var unit time.Duration
	switch s[i:] {
	case "m":
		unit = time.Minute
	case "h":
		unit = time.Hour
	case "d":
		unit = 24 * time.Hour
	case "w":
		unit = 7 * 24 * time.Hour
	default:
		return time.Time{}, fmt.Errorf("bad --since %q (want 7d, 24h, 30m, 2w, all)", s)
	}
	return now.Add(-time.Duration(n) * unit), nil
}

// FromStore aggregates runs from the persistence store.
func FromStore(s Store, opt StatsOptions) (Report, error) {
	if s == nil {
		return Compute(nil, opt)
	}
	recs, err := s.ListRuns(context.Background(), opt.ProjectID, 0, 0)
	if err != nil {
		return Report{}, err
	}
	if opt.By == "area" {
		// A run keeps no area; its routing decision does.
		decs, err := s.ListRouteDecisions(context.Background(), opt.ProjectID)
		if err != nil {
			return Report{}, err
		}
		areas := make(map[string]Area, len(decs))
		for _, d := range decs {
			areas[d.RunID] = d.Area
		}
		for i := range recs {
			recs[i].Area = areas[recs[i].ID]
		}
	}
	return Compute(recs, opt)
}

// Compute aggregates recs. Pending and running runs are counted as inFlight
// and left out of the rates.
func Compute(recs []Run, opt StatsOptions) (Report, error) {
	if opt.By == "" {
		opt.By = "model"
	}
	if !ValidBy(opt.By) {
		return Report{}, fmt.Errorf("unknown --by %q", opt.By)
	}
	if opt.Now.IsZero() {
		opt.Now = time.Now()
	}
	rep := Report{
		Schema: StatsSchema, By: opt.By, MinSamples: MinSamples,
		Rows: []Row{},
		Note: "a missing field is unknown; duration uses runMs, else finishedAt-startedAt/createdAt",
	}
	if !opt.Since.IsZero() {
		rep.Since = opt.Since.UTC().Format(time.RFC3339)
	}
	var kept []Run
	for _, r := range recs {
		if !opt.Since.IsZero() && (r.CreatedAt.IsZero() || r.CreatedAt.Before(opt.Since)) {
			continue
		}
		if r.Status == StatusPending || r.Status == StatusRunning {
			rep.InFlight++
			continue
		}
		kept = append(kept, r)
	}
	rep.Rows = rowsOf(statsBuckets(kept, opt.By), opt.By)
	rep.Rankings = rankAllStats(kept)
	return rep, nil
}

type statsAcc struct {
	runs, ok, first, fallbacks, rates, orphans int
	durs                                       []int64
	spend                                      Spend
}

func statsBuckets(recs []Run, by string) map[string]*statsAcc {
	out := map[string]*statsAcc{}
	for _, r := range recs {
		k := statsGroupKey(r, by)
		a := out[k]
		if a == nil {
			a = &statsAcc{}
			out[k] = a
		}
		a.runs++
		if r.Status == StatusCompleted {
			a.ok++
			if len(r.Attempts) == 0 {
				a.first++
			}
		}
		if len(r.Attempts) > 0 {
			a.fallbacks++
		}
		if hitRateLimit(r) {
			a.rates++
		}
		if r.Status == StatusOrphaned {
			a.orphans++
		}
		if ms, ok := r.ObservedMs(); ok {
			a.durs = append(a.durs, ms)
		}
		a.spend.add(r.Usage)
	}
	return out
}

func statsGroupKey(r Run, by string) string {
	switch by {
	case "requested", "resolved":
		f := r.Requested
		if by == "resolved" {
			f = r.Resolved
		}
		if f == nil {
			return "unknown"
		}
		b, _ := json.Marshal(f)
		return string(b)
	case "model":
		return statsKnown(r.Model)
	case "agent":
		return statsKnown(r.Agent)
	case "provider":
		return statsKnown(r.Provider)
	case "tier":
		return statsKnown(r.Tier)
	case "effort":
		return statsKnown(r.Effort)
	case "area":
		return statsKnown(strings.Join(append(append([]string{}, r.Area.Kinds...), r.Area.Tech...), ", "))
	case "day":
		if r.CreatedAt.IsZero() {
			return "unknown"
		}
		return r.CreatedAt.UTC().Format("2006-01-02")
	case "week":
		if r.CreatedAt.IsZero() {
			return "unknown"
		}
		y, w := r.CreatedAt.UTC().ISOWeek()
		return fmt.Sprintf("%04d-W%02d", y, w)
	default:
		return "unknown"
	}
}

func statsKnown(s string) string {
	if s == "" {
		return "unknown"
	}
	return s
}

func rowsOf(groups map[string]*statsAcc, by string) []Row {
	rows := make([]Row, 0, len(groups))
	for k, a := range groups {
		row := Row{
			Key: k, Runs: a.runs,
			SuccessPct:  statsRatio(a.ok, a.runs),
			FirstTryPct: statsRatio(a.first, a.runs),
			DurationN:   len(a.durs),
			Fallbacks:   a.fallbacks,
			RateLimits:  a.rates,
			Orphans:     a.orphans,
			Spend:       a.spend,
		}
		if p, ok := pctile(a.durs, 0.50); ok {
			row.P50Ms = I64(p)
		}
		if p, ok := pctile(a.durs, 0.90); ok {
			row.P90Ms = I64(p)
		}
		if row.DurationN == 0 {
			row.Duration = "unknown"
		}
		rows = append(rows, row)
	}
	sort.Slice(rows, func(i, j int) bool {
		if by == "day" || by == "week" {
			return rows[i].Key < rows[j].Key
		}
		if rows[i].Runs != rows[j].Runs {
			return rows[i].Runs > rows[j].Runs
		}
		return rows[i].Key < rows[j].Key
	})
	return rows
}

func statsRatio(n, d int) float64 {
	if d == 0 {
		return 0
	}
	return math.Round(float64(n)/float64(d)*1000) / 10
}

func pctile(xs []int64, p float64) (int64, bool) {
	if len(xs) == 0 {
		return 0, false
	}
	s := append([]int64(nil), xs...)
	sort.Slice(s, func(i, j int) bool { return s[i] < s[j] })
	idx := int(math.Ceil(p*float64(len(s)))) - 1
	if idx < 0 {
		idx = 0
	}
	if idx >= len(s) {
		idx = len(s) - 1
	}
	return s[idx], true
}

func hitRateLimit(r Run) bool {
	if r.ExitClass == "rate_limit" || RateLimitRe.MatchString(r.Error) {
		return true
	}
	for _, a := range r.Attempts {
		if a.ExitClass == "rate_limit" || RateLimitRe.MatchString(a.Error) {
			return true
		}
	}
	return false
}

func rankAllStats(recs []Run) Rankings {
	return Rankings{
		MostUsedModel:    mostUsedStat(statsBuckets(recs, "model")),
		MostUsedProvider: mostUsedStat(statsBuckets(recs, "provider")),
		SlowestModel:     slowestStat(statsBuckets(recs, "model")),
		MostFailedModel:  mostFailedStat(statsBuckets(recs, "model")),
	}
}

func withheldRank(reason string) StatRank {
	return StatRank{Key: "unknown", Reason: reason}
}

func mostUsedStat(groups map[string]*statsAcc) StatRank {
	if len(groups) == 0 {
		return withheldRank("no runs in range")
	}
	best := ""
	bestN := -1
	for k, a := range groups {
		if a.runs > bestN || (a.runs == bestN && (best == "" || k < best)) {
			best, bestN = k, a.runs
		}
	}
	return StatRank{Key: best, Runs: bestN}
}

func slowestStat(groups map[string]*statsAcc) StatRank {
	var best *StatRank
	var bestP90 int64
	for k, a := range groups {
		if len(a.durs) < MinSamples {
			continue
		}
		p50, _ := pctile(a.durs, 0.50)
		p90, _ := pctile(a.durs, 0.90)
		cand := StatRank{Key: k, Runs: a.runs, Samples: len(a.durs), P50Ms: I64(p50)}
		if best == nil || p50 > *best.P50Ms || (p50 == *best.P50Ms && (p90 > bestP90 || (p90 == bestP90 && k < best.Key))) {
			c := cand
			best = &c
			bestP90 = p90
		}
	}
	if best == nil {
		return withheldRank(fmt.Sprintf("insufficient samples (minimum %d)", MinSamples))
	}
	return *best
}

func mostFailedStat(groups map[string]*statsAcc) StatRank {
	bestK := ""
	var best *statsAcc
	for k, a := range groups {
		if a.runs < MinSamples {
			continue
		}
		if best == nil || worseStat(a, best, k, bestK) {
			best, bestK = a, k
		}
	}
	if best == nil {
		return withheldRank(fmt.Sprintf("insufficient samples (minimum %d)", MinSamples))
	}
	p := statsRatio(best.ok, best.runs)
	return StatRank{Key: bestK, Runs: best.runs, SuccessPct: &p}
}

func worseStat(a, b *statsAcc, ak, bk string) bool {
	ap, bp := statsRatio(a.ok, a.runs), statsRatio(b.ok, b.runs)
	if ap != bp {
		return ap < bp
	}
	if a.runs != b.runs {
		return a.runs > b.runs
	}
	return ak < bk
}

// Text renders the table and the rankings. Unknown durations say "unknown".
func (r Report) Text() string {
	var b strings.Builder
	since := r.Since
	if since == "" {
		since = "all"
	}
	fmt.Fprintf(&b, "since %s  by %s  minSamples %d\n", since, r.By, r.MinSamples)
	if r.InFlight > 0 {
		fmt.Fprintf(&b, "inFlight %d (fora das taxas)\n", r.InFlight)
	}
	if len(r.Rows) == 0 {
		b.WriteString("no runs in range\n")
	} else {
		fmt.Fprintf(&b, "%-24s %5s %6s %6s %12s %12s %9s %10s %7s\n",
			r.By, "runs", "ok%", "first%", "p50", "p90", "fallbacks", "rateLimits", "orphans")
		for _, row := range r.Rows {
			fmt.Fprintf(&b, "%-24s %5d %6.1f %6.1f %12s %12s %9d %10d %7d\n",
				statsTrunc(row.Key, 24), row.Runs, row.SuccessPct, row.FirstTryPct,
				statsFmtDur(row.P50Ms), statsFmtDur(row.P90Ms), row.Fallbacks, row.RateLimits, row.Orphans)
		}
	}
	b.WriteString(r.Rankings.MostUsedModel.line("most used model"))
	b.WriteString(r.Rankings.MostUsedProvider.line("most used provider"))
	b.WriteString(r.Rankings.SlowestModel.line("slowest model"))
	b.WriteString(r.Rankings.MostFailedModel.line("most failed model"))
	return b.String()
}

func (r StatRank) line(label string) string {
	if r.Reason != "" || r.Key == "" {
		reason := r.Reason
		if reason == "" {
			reason = "unknown"
		}
		return fmt.Sprintf("%s: unknown (%s)\n", label, reason)
	}
	switch {
	case r.P50Ms != nil:
		return fmt.Sprintf("%s: %s (p50 %s, %d samples)\n", label, r.Key, statsFmtDur(r.P50Ms), r.Samples)
	case r.SuccessPct != nil:
		return fmt.Sprintf("%s: %s (%.1f%% ok, %d runs)\n", label, r.Key, *r.SuccessPct, r.Runs)
	default:
		return fmt.Sprintf("%s: %s (%d runs)\n", label, r.Key, r.Runs)
	}
}

func statsFmtDur(ms *int64) string {
	if ms == nil {
		return "unknown"
	}
	if *ms < 1000 {
		return fmt.Sprintf("%dms", *ms)
	}
	d := time.Duration(*ms) * time.Millisecond
	return d.Round(100 * time.Millisecond).String()
}

func statsTrunc(s string, n int) string {
	if len(s) <= n {
		return s
	}
	if n <= 1 {
		return s[:n]
	}
	return s[:n-1] + "…"
}
