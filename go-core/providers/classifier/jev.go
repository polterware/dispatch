package classifier

import (
	"context"
	"fmt"
	"net/http"
	"strconv"
	"time"

	"github.com/polterware/dispatch/go-core/domains/dispatch"
)

const (
	jevURL   = "https://api.typesafe.ai/v1/systemone"
	jevModel = "jev-latest"
)

// Jev asks TypeSafe's System One model one score question about the task, and
// a choice of subagent when the task has roles.
type Jev struct {
	Model  string
	URL    string
	Key    Key
	Client *http.Client
}

func (Jev) Name() string { return dispatch.ClassifierJev }

type jevAnswer struct {
	Answers map[string]struct {
		Score         *float64           `json:"score"`
		Choice        string             `json:"choice"`
		Probabilities map[string]float64 `json:"probabilities"`
		Confidence    *float64           `json:"confidence"`
	} `json:"answers"`
}

func (j Jev) Classify(ctx context.Context, t dispatch.Task) (dispatch.Difficulty, error) {
	key, _, err := j.key().read()
	if err != nil {
		return dispatch.Difficulty{}, fmt.Errorf("no TypeSafe key: %w", err)
	}
	if key == "" {
		return dispatch.Difficulty{}, fmt.Errorf("no TypeSafe key: save one in Settings › Classifier or with `dispatch config key set jev`, or set %s", j.key().Env)
	}
	model := orDefault(j.Model, jevModel)
	questions := map[string]any{
		"difficulty": map[string]any{
			"type":         "score",
			"instructions": dispatch.DifficultyQuestion,
			"criteria":     dispatch.DifficultyLevels,
		},
	}
	// The role is a Choice over the same state, asked in the same request.
	if len(t.Roles) > 0 {
		questions["role"] = map[string]any{
			"type":         "choice",
			"instructions": dispatch.RoleQuestion,
			"criteria":     t.Roles,
		}
	}
	body := map[string]any{
		"model":     model,
		"state":     map[string]string{"task": t.Text},
		"questions": questions,
	}
	start := time.Now()
	var out jevAnswer
	if err := postJSON(ctx, j.Client, orDefault(j.URL, jevURL), key, body, &out); err != nil {
		return dispatch.Difficulty{}, err
	}
	a, ok := out.Answers["difficulty"]
	if !ok || len(a.Probabilities) == 0 {
		return dispatch.Difficulty{}, fmt.Errorf("jev: no difficulty answer")
	}
	var p [3]float64
	for i := range p {
		p[i] = a.Probabilities[strconv.Itoa(i)]
	}
	diff, err := dispatch.NewDifficulty(&p, "")
	if err != nil {
		return dispatch.Difficulty{}, err
	}
	diff.Provider, diff.Model, diff.Confidence, diff.Latency = j.Name(), model, a.Confidence, time.Since(start)
	if r, ok := out.Answers["role"]; ok && len(t.Roles) > 0 && (r.Choice != "" || len(r.Probabilities) > 0) {
		diff.Role = &dispatch.RoleAnswer{Role: r.Choice, Probs: r.Probabilities, Confidence: r.Confidence}
	}
	return diff, nil
}

func (j Jev) key() Key {
	if j.Key.Env == "" {
		return JevKey
	}
	return j.Key
}

func orDefault(v, d string) string {
	if v == "" {
		return d
	}
	return v
}
