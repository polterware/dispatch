package classifier

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"strings"
	"time"

	"github.com/polterware/dispatch/go-core/domains/dispatch"
)

const (
	openAIURL   = "https://api.openai.com/v1/decisions"
	openAIModel = "gpt-6-luna"
)

// OpenAI asks OpenAI's Decisions API one score question about the task. The
// request and answer field names live only in this file. It does not pick a
// subagent: a run it classifies stays general unless the request names one.
type OpenAI struct {
	Model  string
	URL    string
	Key    Key
	Client *http.Client
}

func (OpenAI) Name() string { return dispatch.ClassifierOpenAI }

type openAIAnswer struct {
	Type    string `json:"type"`
	Refusal string `json:"refusal"`
	Answers []struct {
		Name          string          `json:"name"`
		Probabilities json.RawMessage `json:"probabilities"`
		Confidence    *float64        `json:"confidence"`
	} `json:"answers"`
}

var levelLabels = [3]string{"easy", "normal", "hard"}

func (o OpenAI) Classify(ctx context.Context, t dispatch.Task) (dispatch.Difficulty, error) {
	k := o.Key
	if k.Env == "" {
		k = OpenAIKey
	}
	key, _, err := k.read()
	if err != nil {
		return dispatch.Difficulty{}, fmt.Errorf("no OpenAI key: %w", err)
	}
	if key == "" {
		return dispatch.Difficulty{}, fmt.Errorf("no OpenAI key: save one in Settings › Classifier or with `dispatch config key set openai`, or set %s", k.Env)
	}
	levels := make([]map[string]string, len(dispatch.DifficultyLevels))
	for i, d := range dispatch.DifficultyLevels {
		levels[i] = map[string]string{"label": levelLabels[i], "description": d}
	}
	model := orDefault(o.Model, openAIModel)
	body := map[string]any{
		"model": model,
		"input": t.Text,
		"questions": []map[string]any{{
			"name":         "difficulty",
			"type":         "score",
			"instructions": dispatch.DifficultyQuestion,
			"levels":       levels,
		}},
	}
	start := time.Now()
	var out openAIAnswer
	if err := postJSON(ctx, o.Client, orDefault(o.URL, openAIURL), key, body, &out); err != nil {
		return dispatch.Difficulty{}, err
	}
	if out.Type == "refusal" || out.Refusal != "" {
		return dispatch.Difficulty{}, fmt.Errorf("openai refused: %s", orDefault(out.Refusal, "no reason"))
	}
	for _, a := range out.Answers {
		if a.Name != "difficulty" {
			continue
		}
		p, err := parseLevelProbs(a.Probabilities)
		if err != nil {
			return dispatch.Difficulty{}, err
		}
		diff, err := dispatch.NewDifficulty(&p, "")
		if err != nil {
			return dispatch.Difficulty{}, err
		}
		diff.Provider, diff.Model, diff.Confidence, diff.Latency = o.Name(), model, a.Confidence, time.Since(start)
		return diff, nil
	}
	return dispatch.Difficulty{}, fmt.Errorf("openai: no difficulty answer")
}

// parseLevelProbs reads probabilities given as a list of {index, label,
// probability} entries or as an object keyed by index or label.
func parseLevelProbs(raw json.RawMessage) ([3]float64, error) {
	var p [3]float64
	var list []struct {
		Index       *int    `json:"index"`
		Label       string  `json:"label"`
		Probability float64 `json:"probability"`
	}
	if json.Unmarshal(raw, &list) == nil && len(list) > 0 {
		for _, e := range list {
			if i := levelIndex(e.Index, e.Label); i >= 0 {
				p[i] += e.Probability
			}
		}
		return p, nil
	}
	var m map[string]float64
	if json.Unmarshal(raw, &m) == nil && len(m) > 0 {
		for k, v := range m {
			var idx *int
			if len(k) == 1 && k[0] >= '0' && k[0] <= '2' {
				n := int(k[0] - '0')
				idx = &n
			}
			if i := levelIndex(idx, k); i >= 0 {
				p[i] += v
			}
		}
		return p, nil
	}
	return p, fmt.Errorf("openai: unreadable probabilities")
}

func levelIndex(index *int, label string) int {
	if index != nil && *index >= 0 && *index < 3 {
		return *index
	}
	for i, l := range levelLabels {
		if strings.EqualFold(strings.TrimSpace(label), l) {
			return i
		}
	}
	return -1
}
