package classifier

import (
	"context"
	"fmt"
	"os"
	"regexp"
	"sort"
	"strings"
	"time"

	"github.com/polterware/dispatch/go-core/domains/dispatch"
	adapter "github.com/polterware/dispatch/go-core/providers/adapters"
)

// CLI asks a cheap model through an installed agent CLI to name the tier. It
// spends the person's quota and a few seconds per run, so it is opt-in. Run
// executes one read-only prompt and returns the agent's reply.
type CLI struct {
	Agent, Model string
	Run          func(ctx context.Context, prompt string) (string, error)
}

// NewCLI runs model through a, read-only in an empty temporary folder, with
// a short timeout.
func NewCLI(a adapter.Adapter, model string, timeout time.Duration) CLI {
	return CLI{Agent: a.Name(), Model: model, Run: func(ctx context.Context, prompt string) (string, error) {
		dir, err := os.MkdirTemp("", "dispatch-classify-")
		if err != nil {
			return "", err
		}
		defer os.RemoveAll(dir)
		res, err := a.Run(ctx, adapter.Request{Prompt: prompt, Tier: adapter.TierEasy, Dir: dir, ReadOnly: true, Timeout: timeout}, model)
		if err != nil {
			return "", err
		}
		if res.ExitCode != 0 {
			return "", fmt.Errorf("%s exited %d", a.Name(), res.ExitCode)
		}
		return res.Output, nil
	}}
}

func (CLI) Name() string { return dispatch.ClassifierCLI }

// CLIPrompt is the whole instruction the agent receives, the task included.
// With roles it also asks which one fits, by name.
func CLIPrompt(task string, roles map[string]string) string {
	var b strings.Builder
	b.WriteString("Classify a coding task. Judge from the text alone: do not read files, run commands, or start the work.\n\n")
	b.WriteString(dispatch.DifficultyQuestion + "\n")
	for _, l := range dispatch.DifficultyLevels {
		b.WriteString("- " + l + "\n")
	}
	if len(roles) == 0 {
		b.WriteString("\nReply with only JSON like {\"tier\": \"<easy, normal, or hard>\"}.\n\nTask:\n")
		b.WriteString(task)
		return b.String()
	}
	b.WriteString("\n" + dispatch.RoleQuestion + "\n")
	names := make([]string, 0, len(roles))
	for name := range roles {
		names = append(names, name)
	}
	sort.Strings(names)
	for _, name := range names {
		b.WriteString("- " + name + ": " + roles[name] + "\n")
	}
	b.WriteString("\nReply with only JSON like {\"tier\": \"<easy, normal, or hard>\", \"role\": \"<one role name>\"}.\n\nTask:\n")
	b.WriteString(task)
	return b.String()
}

func (c CLI) Classify(ctx context.Context, t dispatch.Task) (dispatch.Difficulty, error) {
	if c.Run == nil {
		return dispatch.Difficulty{}, fmt.Errorf("cli classifier: no agent in the easy tier")
	}
	start := time.Now()
	out, err := c.Run(ctx, CLIPrompt(t.Text, t.Roles))
	if err != nil {
		return dispatch.Difficulty{}, err
	}
	tier, err := ParseTier(out)
	if err != nil {
		return dispatch.Difficulty{}, err
	}
	diff, err := dispatch.NewDifficulty(nil, tier)
	if err != nil {
		return dispatch.Difficulty{}, err
	}
	diff.Provider, diff.Model, diff.Latency = c.Name(), strings.Trim(c.Agent+"/"+c.Model, "/"), time.Since(start)
	if role := ParseRole(out); role != "" && len(t.Roles) > 0 {
		diff.Role = &dispatch.RoleAnswer{Role: role}
	}
	return diff, nil
}

// roleJSON also matches the escaped quotes of a JSON event stream.
var roleJSON = regexp.MustCompile(`\\?"role\\?"\s*:\s*\\?"([a-z][a-z0-9-]*)\\?"`)

// ParseRole reads the last {"role": …} from an agent's reply, empty when none.
func ParseRole(out string) string {
	if m := roleJSON.FindAllStringSubmatch(strings.ToLower(out), -1); len(m) > 0 {
		return m[len(m)-1][1]
	}
	return ""
}

// tierJSON also matches the escaped quotes of a JSON event stream.
var tierJSON = regexp.MustCompile(`\\?"tier\\?"\s*:\s*\\?"(easy|normal|hard)\\?"`)
var tierWord = regexp.MustCompile(`\b(easy|normal|hard)\b`)

// ParseTier reads the tier from an agent's reply: the last {"tier": …}, else
// the reply when it is a single tier word.
func ParseTier(out string) (adapter.Tier, error) {
	low := strings.ToLower(out)
	if m := tierJSON.FindAllStringSubmatch(low, -1); len(m) > 0 {
		return adapter.Tier(m[len(m)-1][1]), nil
	}
	if words := tierWord.FindAllString(low, -1); len(words) == 1 {
		return adapter.Tier(words[0]), nil
	}
	return "", fmt.Errorf("cli classifier: no tier in the reply")
}
