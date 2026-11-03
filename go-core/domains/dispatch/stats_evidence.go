package dispatch

// ModelEvidence is measured routing evidence for one model inside one tier:
// how many runs finished and how long the completed ones took. Callers apply
// MinSamples; values below that stay available for inspection.
type ModelEvidence struct {
	Tier      string `json:"tier"`
	Model     string `json:"model"`
	Finished  int    `json:"finished"`
	DurationN int    `json:"durationSamples"`
	P50Ms     *int64 `json:"p50Ms,omitempty"`
}

// EvidenceKey is the map key for a tier and model. Tiers are not pooled:
// the same model name in easy and hard is two different tasks.
func EvidenceKey(tier, model string) string {
	return tier + "\x00" + model
}

// EvidenceByTierModel aggregates finished runs. Pending and running rows are
// skipped. Duration uses the same ObservedMs and p50 as the stats report.
func EvidenceByTierModel(recs []Run) map[string]ModelEvidence {
	type acc struct {
		finished int
		durs     []int64
	}
	groups := map[string]*acc{}
	labels := map[string][2]string{}
	for _, r := range recs {
		if r.Status == StatusPending || r.Status == StatusRunning || r.Model == "" {
			continue
		}
		key := EvidenceKey(r.Tier, r.Model)
		a := groups[key]
		if a == nil {
			a = &acc{}
			groups[key] = a
			labels[key] = [2]string{r.Tier, r.Model}
		}
		a.finished++
		// Only successful runs time the model; timeouts and cancels would skew speed.
		if ms, ok := r.ObservedMs(); ok && r.Status == StatusCompleted {
			a.durs = append(a.durs, ms)
		}
	}
	out := make(map[string]ModelEvidence, len(groups))
	for key, a := range groups {
		label := labels[key]
		ev := ModelEvidence{
			Tier: label[0], Model: label[1],
			Finished: a.finished, DurationN: len(a.durs),
		}
		if p, ok := pctile(a.durs, 0.50); ok {
			ev.P50Ms = I64(p)
		}
		out[key] = ev
	}
	return out
}
