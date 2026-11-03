package dispatch

import (
	"context"
	"time"

	adapter "github.com/polterware/dispatch/go-core/providers/adapters"
)

const empiricalWindow = 24 * time.Hour

// EmpiricalQuota is the risk sample for an agent with no official quota.
// Used is always nil.
func EmpiricalQuota(st Store, agent string, now time.Time) adapter.QuotaSample {
	if now.IsZero() {
		now = time.Now()
	}
	ctx := context.Background()
	until := storeCooldowns(ctx, st, now)[agent]
	n := 0
	if st != nil {
		recs, err := st.ListRuns(ctx, "", 0, 0)
		if err == nil {
			cutoff := now.Add(-empiricalWindow)
			for _, r := range recs {
				if !r.FinishedAt.After(cutoff) && !r.CreatedAt.After(cutoff) {
					continue
				}
				if r.Agent == agent && rateLimited.MatchString(r.Error) {
					n++
				}
				for _, at := range r.Attempts {
					if at.Agent == agent && (at.ExitClass == "rate_limit" || rateLimited.MatchString(at.Error)) {
						n++
					}
				}
			}
		}
	}
	return adapter.EmpiricalSample(now, until, n)
}
