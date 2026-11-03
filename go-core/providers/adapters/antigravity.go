package adapters

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"os/exec"
	"strings"
	"time"

	"github.com/polterware/dispatch/go-core/providers/sandbox"
)

// Antigravity drives the Antigravity CLI (agy) over ACP (`agy acp
// --sandbox`), which asks before each edit or command: a write run allows
// them, a read-only run refuses them. Effort is part of the model ID (e.g.
// gemini-3.8-flash-low). Quota comes from agy's own non-interactive `/usage`
// report, grouped by model family.
type Antigravity struct{}

var antigravityRun = acpAdapter{spec: acpSpec{
	name:       "antigravity",
	command:    func(bool) []string { return []string{"acp", "--sandbox"} },
	caps:       Capabilities{EffortMode: "model-suffix", SessionResume: true, StructuredOutput: true, ReadOnly: true, FileEditing: true, Usage: true},
	flags:      []probeFlag{{Sub: "acp", Name: "--stdio", Capability: "structuredOutput"}, {Sub: "acp", Name: "--sandbox"}},
	listModels: listModelsWith("antigravity", []string{"models"}, parseAgyModels),
}}

func (Antigravity) Name() string               { return "antigravity" }
func (Antigravity) Binary() string             { return Bin("antigravity") }
func (Antigravity) Capabilities() Capabilities { return antigravityRun.spec.caps }
func (Antigravity) UsageSource() string        { return SourceAgyUsage }
func (Antigravity) Probe(ctx context.Context) ProbeReport {
	return antigravityRun.Probe(ctx)
}
func (Antigravity) Models(ctx context.Context) ([]Model, error) {
	return antigravityRun.Models(ctx)
}
func (Antigravity) Run(ctx context.Context, r Request, m string) (Result, error) {
	return antigravityRun.Run(ctx, r, m)
}
func (Antigravity) Resume(ctx context.Context, r Request, m string) (Result, error) {
	return antigravityRun.Resume(ctx, r, m)
}

// Quota is unavailable without a model: use ModelQuotas.
func (Antigravity) Quota(context.Context) (Quota, error) { return Quota{}, ErrNoQuota }

// Usage lists every official bucket. It does not invent a percent.
func (Antigravity) Usage(ctx context.Context) ([]QuotaSample, error) {
	raw, err := agyUsageRaw(ctx)
	if err != nil {
		return nil, fmt.Errorf("%w: %v", ErrNoQuota, err)
	}
	return parseAgySamples(raw)
}

func agyUsageRaw(ctx context.Context) ([]byte, error) {
	ctx, cancel := context.WithTimeout(ctx, 90*time.Second)
	defer cancel()
	cmd := exec.CommandContext(ctx, Bin("antigravity"), "-p", "/usage", "--output-format", "json")
	cmd.Env = sandbox.Environ()
	ownGroup(cmd)
	var out bytes.Buffer
	cmd.Stdout = &out
	if err := cmd.Run(); err != nil {
		return nil, err
	}
	return out.Bytes(), nil
}

// ModelQuotas resolves each model to its group's quota.
func (Antigravity) ModelQuotas(ctx context.Context, models []string) (map[string]Quota, error) {
	raw, err := agyUsageRaw(ctx)
	if err != nil {
		return nil, fmt.Errorf("%w: %v", ErrNoQuota, err)
	}
	groups, err := parseAgyUsage(raw)
	if err != nil {
		return nil, fmt.Errorf("%w: %v", ErrNoQuota, err)
	}
	res := map[string]Quota{}
	for _, m := range models {
		g := "Claude and GPT models"
		if strings.Contains(strings.ToLower(m), "gemini") {
			g = "Gemini Models"
		}
		if q, ok := groups[g]; ok {
			q.Key = "antigravity:" + g
			res[m] = q
		}
	}
	return res, nil
}

// parseAgyUsage turns the /usage JSON into one Quota per group. It uses the
// weekly bucket, unless the 5-hour bucket is nearly exhausted (>= 90% used),
// in which case that bucket is the binding constraint.
func parseAgyUsage(raw []byte) (map[string]Quota, error) {
	var env struct {
		Command struct {
			Data struct {
				Groups []struct {
					Name    string `json:"name"`
					Buckets []struct {
						Window            string  `json:"window"`
						RemainingFraction float64 `json:"remaining_fraction"`
						ResetTime         string  `json:"reset_time"`
					} `json:"buckets"`
				} `json:"groups"`
			} `json:"data"`
		} `json:"command"`
	}
	if err := json.Unmarshal(raw, &env); err != nil {
		return nil, err
	}
	out := map[string]Quota{}
	for _, g := range env.Command.Data.Groups {
		var weekly, fiveH *Quota
		for _, b := range g.Buckets {
			reset, err := time.Parse(time.RFC3339, b.ResetTime)
			if err != nil {
				continue
			}
			q := Quota{UsedPercent: 100 * (1 - b.RemainingFraction), ResetsAt: reset}
			switch b.Window {
			case "weekly":
				q.WindowMinutes = 7 * 24 * 60
				weekly = &q
			case "5h":
				q.WindowMinutes = 5 * 60
				fiveH = &q
			}
		}
		chosen := weekly
		if fiveH != nil && (weekly == nil || fiveH.UsedPercent >= 90) {
			chosen = fiveH
		}
		if chosen != nil {
			out[g.Name] = *chosen
		}
	}
	if len(out) == 0 {
		return nil, fmt.Errorf("no usage groups in report")
	}
	return out, nil
}

// parseAgySamples turns every /usage bucket into an official QuotaSample.
// The window name is "<group>/<bucket>" so the two pools stay distinct.
func parseAgySamples(raw []byte) ([]QuotaSample, error) {
	var env struct {
		Command struct {
			Data struct {
				Groups []struct {
					Name    string `json:"name"`
					Buckets []struct {
						Window            string  `json:"window"`
						RemainingFraction float64 `json:"remaining_fraction"`
						ResetTime         string  `json:"reset_time"`
					} `json:"buckets"`
				} `json:"groups"`
			} `json:"data"`
		} `json:"command"`
	}
	if err := json.Unmarshal(raw, &env); err != nil {
		return nil, err
	}
	var out []QuotaSample
	for _, g := range env.Command.Data.Groups {
		for _, b := range g.Buckets {
			reset, err := time.Parse(time.RFC3339, b.ResetTime)
			if err != nil {
				continue
			}
			var dur time.Duration
			switch b.Window {
			case "weekly":
				dur = 7 * 24 * time.Hour
			case "5h":
				dur = 5 * time.Hour
			default:
				continue
			}
			used := 100 * (1 - b.RemainingFraction)
			out = append(out, QuotaSample{
				Window:     g.Name + "/" + b.Window,
				Used:       &used,
				ResetAt:    reset,
				Duration:   Span(dur),
				Source:     SourceAgyUsage,
				Confidence: ConfidenceOfficial,
			})
		}
	}
	if len(out) == 0 {
		return nil, fmt.Errorf("no usage groups in report")
	}
	return out, nil
}
