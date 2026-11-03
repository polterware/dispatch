package dispatch

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"time"

	adapter "github.com/polterware/dispatch/go-core/providers/adapters"
)

// Profile is a role: the kind of work a run does. Instructions go before the
// task; When describes the tasks it fits, for the classifier that picks a
// role; Tier is a floor on the task's difficulty; ReadOnly and Timeout bound
// the run. Tiers is the role's own board: per tier, its models in order, the
// first the primary and the rest fallbacks. A tier with none uses the general
// board (the tier table). Manual keeps the role out of the pick, so only
// `--role` gives it.
type Profile struct {
	Provider     string      `json:"provider,omitempty"`
	Model        string      `json:"model,omitempty"`
	Effort       string      `json:"effort,omitempty"`
	Agent        string      `json:"agent,omitempty"`
	Relax        bool        `json:"relax,omitempty"`
	Priority     string      `json:"priority,omitempty"`
	Tier         string      `json:"tier,omitempty"`
	ReadOnly     bool        `json:"readonly,omitempty"`
	Instructions string      `json:"instructions,omitempty"`
	When         string      `json:"when,omitempty"`
	Tiers        []RoleEntry `json:"tiers,omitempty"`
	Manual       bool        `json:"manual,omitempty"`
	Prefer       []string    `json:"prefer,omitempty"`
	Avoid        []string    `json:"avoid,omitempty"`
	Timeout      string      `json:"timeout,omitempty"`
}

// RoleEntry is one card of a role's board: a model in a tier, in board order.
type RoleEntry struct {
	Tier     string `json:"tier"`
	Agent    string `json:"agent"`
	Model    string `json:"model"`
	Provider string `json:"provider,omitempty"`
}

// Board is the role's own tier table; a tier it leaves empty is absent.
func (p Profile) Board() Table {
	rows := make([]Tier, 0, len(p.Tiers))
	for i, e := range p.Tiers {
		rows = append(rows, Tier{Name: e.Tier, Agent: e.Agent, Model: e.Model, Provider: e.Provider, Position: i})
	}
	return TableFromRows(rows)
}

// rolesFile is roles.json; Agents reads the agents.json an earlier Dispatch wrote.
type rolesFile struct {
	Roles  map[string]Profile `json:"roles,omitempty"`
	Agents map[string]Profile `json:"agents,omitempty"`
}

// GeneralRole is the classifier's answer when no role fits: the run keeps
// the plain route. It is reserved and never a profile.
const GeneralRole = "general"

// GeneralWhen is how the classifier reads the general answer.
const GeneralWhen = "None of the other roles fits well: a mixed or unusual task, or one that names no clear kind of work."

// builtinProfiles are the roles every install starts with. A role of the same
// name in roles.json replaces one; manual keeps it out of the pick.
var builtinProfiles = map[string]Profile{
	"explorer": {
		Tier: "easy", ReadOnly: true, Timeout: "10m",
		When:         "Find where something is in the code or explain how existing code works. A question to answer, not a change to make.",
		Instructions: "Answer from the code. Name the files and lines that matter. Do not change anything.",
	},
	"implementer": {
		Tier:         "normal",
		When:         "Make a change: build a feature, fix a known bug, or refactor code, with the approach already clear.",
		Instructions: "Make the change, keep it focused, and run the checks and tests that cover it.",
	},
	"tester": {
		Tier:         "normal",
		When:         "Write or fix tests for code that exists, or run the tests and report what fails.",
		Instructions: "Write and run tests for the described change. Report what fails and why.",
	},
	"reviewer": {
		Tier: "hard", ReadOnly: true, Timeout: "15m",
		When:         "Review a diff, a branch, or a pull request for bugs. Judging work already done, not doing it.",
		Instructions: "Review the diff. Point out only real bugs, with file and line.",
	},
	"planner": {
		Tier: "hard", ReadOnly: true, Timeout: "20m",
		When:         "Design how to build something before building it: weigh approaches, plan the steps, or find the cause of a hard bug.",
		Instructions: "Plan the work. Weigh the approaches, name the files involved, and list the steps. Do not change anything.",
	},
}

// BuiltinProfiles returns a copy of the roles every install starts with.
func BuiltinProfiles() map[string]Profile {
	out := make(map[string]Profile, len(builtinProfiles))
	for k, v := range builtinProfiles {
		out[k] = v
	}
	return out
}

// IsBuiltinProfile reports a name the install ships a role for.
func IsBuiltinProfile(name string) bool {
	_, ok := builtinProfiles[name]
	return ok
}

// GlobalProfilesPath is the user's roles file inside the dispatch state dir.
func GlobalProfilesPath(stateDir string) string { return filepath.Join(stateDir, "roles.json") }

// ProjectProfilesPath is where a repository keeps its roles.
func ProjectProfilesPath(dir string) string { return filepath.Join(dir, ".dispatch", "roles.json") }

// LoadProfiles reads path; a missing file is an empty set. With no roles.json
// it reads the agents.json beside it, which an earlier Dispatch wrote, until a
// save writes roles.json.
func LoadProfiles(path string) (map[string]Profile, error) {
	b, err := os.ReadFile(path)
	if os.IsNotExist(err) && filepath.Base(path) == "roles.json" {
		path = filepath.Join(filepath.Dir(path), "agents.json")
		b, err = os.ReadFile(path)
	}
	if os.IsNotExist(err) {
		return nil, nil
	}
	if err != nil {
		return nil, err
	}
	var f rolesFile
	if err := json.Unmarshal(b, &f); err != nil {
		return nil, fmt.Errorf("%s: %w", path, err)
	}
	if f.Roles != nil {
		return f.Roles, nil
	}
	return f.Agents, nil
}

// LoadAllProfiles merges the built-in roles, the user file, and the
// project file; the later one wins by name. A file's profile with no When
// keeps the built-in one's, so an older copy of a built-in is still picked.
func LoadAllProfiles(userPath, projectPath string) (map[string]Profile, error) {
	out := BuiltinProfiles()
	for _, p := range []string{userPath, projectPath} {
		m, err := LoadProfiles(p)
		if err != nil {
			return nil, err
		}
		for k, v := range m {
			if v.When == "" {
				v.When = builtinProfiles[k].When
			}
			out[k] = v
		}
	}
	return out, nil
}

// ValidProfileName is a name a run can take with --role: lowercase letters,
// digits, and dashes, starting with a letter. general and none are reserved.
func ValidProfileName(name string) bool {
	if name == "" || name == GeneralRole || name == "none" || len(name) > 40 || name[0] < 'a' || name[0] > 'z' {
		return false
	}
	for _, r := range name {
		if !(r >= 'a' && r <= 'z' || r >= '0' && r <= '9' || r == '-') {
			return false
		}
	}
	return true
}

// SaveProfile writes p as name in path, replacing a profile of that name and
// keeping the others; remove drops it instead. A missing file starts empty.
func SaveProfile(path, name string, p Profile, remove bool) error {
	profiles, err := LoadProfiles(path)
	if err != nil {
		return err
	}
	if profiles == nil {
		profiles = map[string]Profile{}
	}
	if remove {
		if _, ok := profiles[name]; !ok {
			return fmt.Errorf("no role %q in %s", name, path)
		}
		delete(profiles, name)
	} else {
		profiles[name] = p
	}
	b, err := json.MarshalIndent(rolesFile{Roles: profiles}, "", "  ")
	if err != nil {
		return err
	}
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		return err
	}
	return os.WriteFile(path, append(b, '\n'), 0o644)
}

// ProfileNames returns sorted profile names.
func ProfileNames(m map[string]Profile) []string {
	out := make([]string, 0, len(m))
	for k := range m {
		out = append(out, k)
	}
	sort.Strings(out)
	return out
}

// ValidateProfile reports problems with one profile. known holds agent names.
func ValidateProfile(name string, p Profile, known map[string]bool) []string {
	var out []string
	add := func(f string, a ...any) { out = append(out, name+": "+fmt.Sprintf(f, a...)) }
	if p.Effort != "" && !adapter.ValidEffort(p.Effort) {
		add("invalid effort %q", p.Effort)
	}
	if p.Priority != "" && !ValidPriority(p.Priority) {
		add("invalid priority %q (want speed|balanced|quality)", p.Priority)
	}
	if p.Agent != "" && !known[p.Agent] {
		add("unknown agent %q", p.Agent)
	}
	if p.Tier != "" && !adapter.Tier(p.Tier).Valid() {
		add("invalid tier %q (want easy|normal|hard)", p.Tier)
	}
	if p.Timeout != "" {
		if _, err := time.ParseDuration(p.Timeout); err != nil {
			add("invalid timeout %q", p.Timeout)
		}
	}
	for _, a := range p.Prefer {
		if !known[a] {
			add("prefer: unknown agent %q", a)
		}
	}
	for _, a := range p.Avoid {
		if !known[a] {
			add("avoid: unknown agent %q", a)
		}
	}
	for _, a := range p.Prefer {
		for _, b := range p.Avoid {
			if a == b {
				add("agent %q is both preferred and avoided", a)
			}
		}
	}
	for i, e := range p.Tiers {
		switch {
		case !adapter.Tier(e.Tier).Valid():
			add("tiers[%d]: invalid tier %q (want easy|normal|hard)", i, e.Tier)
		case !known[e.Agent]:
			add("tiers[%d]: unknown agent %q", i, e.Agent)
		case e.Model == "":
			add("tiers[%d]: model is required", i)
		}
	}
	if len(p.Tiers) > 0 && (p.Model != "" || p.Provider != "") {
		add("its board and a model or provider both pick the model; keep one")
	}
	if p.Provider == "" && p.Model == "" && p.Agent == "" && p.Effort == "" && p.Priority == "" && p.Instructions == "" && p.Tier == "" && p.When == "" && len(p.Tiers) == 0 && !p.Manual && len(p.Prefer) == 0 && len(p.Avoid) == 0 {
		add("empty role")
	}
	return out
}

// Pickable is a profile the classifier may pick: one that says when it fits
// and is not manual.
func (p Profile) Pickable() bool { return !p.Manual && strings.TrimSpace(p.When) != "" }

// PickRoles is what the classifier chooses from: each pickable profile's When
// by name, plus general. Nil when no profile is pickable.
func PickRoles(profiles map[string]Profile) map[string]string {
	out := map[string]string{}
	for name, p := range profiles {
		if p.Pickable() && name != GeneralRole {
			out[name] = strings.TrimSpace(p.When)
		}
	}
	if len(out) == 0 {
		return nil
	}
	out[GeneralRole] = GeneralWhen
	return out
}

// PickMin is the probability a role needs to be picked: below it the run
// stays general, because a wrong role prefixes the wrong instructions.
const PickMin = 0.5

// RolePick is how the classifier chose a run's role, with its probability per
// role; general when none fit.
type RolePick struct {
	Auto       bool               `json:"auto"`
	Classifier string             `json:"classifier,omitempty"`
	Role       string             `json:"role"`
	Probs      map[string]float64 `json:"probs,omitempty"`
	Confidence *float64           `json:"confidence,omitempty"`
}

// PickRole is the role an answer picks: the most likely one when it reaches
// PickMin and is a known pickable role, else general. A classifier that names
// a role without probabilities picks it outright.
func PickRole(answer RoleAnswer, roles map[string]string) string {
	if len(roles) == 0 {
		return ""
	}
	if len(answer.Probs) == 0 {
		if _, ok := roles[answer.Role]; ok {
			return answer.Role
		}
		return GeneralRole
	}
	best, bestP := GeneralRole, -1.0
	names := make([]string, 0, len(answer.Probs))
	for name := range answer.Probs {
		names = append(names, name)
	}
	sort.Strings(names)
	for _, name := range names {
		if p := answer.Probs[name]; p > bestP {
			best, bestP = name, p
		}
	}
	if _, ok := roles[best]; !ok || bestP < PickMin {
		return GeneralRole
	}
	return best
}

// ApplyProfile fills o from the role the request named. Values already set on
// o (explicit request fields) win over the role. Its tier is a floor on the
// task's difficulty, not a fixed tier, so the role's board still follows how
// hard the task is.
func ApplyProfile(o *Options, name string, p Profile) error {
	if o.Filters.Provider == "" {
		o.Filters.Provider = p.Provider
	}
	if o.Filters.Model == "" {
		o.Filters.Model = p.Model
	}
	if o.Filters.Effort == "" {
		o.Filters.Effort = p.Effort
	}
	if o.Filters.Agent == "" {
		o.Filters.Agent = p.Agent
	}
	if !o.Filters.Relax {
		o.Filters.Relax = p.Relax
	}
	if o.Filters.Priority == "" {
		o.Filters.Priority = p.Priority
	}
	if adapter.Tier(p.Tier).Valid() {
		o.TierFloor = adapter.Tier(p.Tier)
	}
	o.ReadOnly = o.ReadOnly || p.ReadOnly
	if o.Timeout == 0 && p.Timeout != "" {
		d, err := time.ParseDuration(p.Timeout)
		if err != nil {
			return fmt.Errorf("role %s: invalid timeout %q", name, p.Timeout)
		}
		o.Timeout = d
	}
	if p.Instructions != "" && o.Resume == "" {
		o.Prompt = p.Instructions + "\n\n" + o.Prompt
	}
	o.Profile, o.Prefer, o.Avoid, o.RoleBoard = name, p.Prefer, p.Avoid, p.Board()
	return nil
}

// applyPicked fills o from a role the classifier picked. Unlike --role it
// never changes access, so a run asked to edit is never made read-only, and a
// model or provider pin, which Auto cannot take, is left out.
func applyPicked(o *Options, name string, p Profile) error {
	q := p
	q.ReadOnly, q.Model, q.Provider = false, "", ""
	return ApplyProfile(o, name, q)
}

// raiseToFloor lifts a classified difficulty to the role's floor tier.
func raiseToFloor(diff *Difficulty, floor adapter.Tier, role string) string {
	if i := tierIndex(floor); i >= 0 && diff != nil && tierIndex(DifficultyTier(diff.D)) < i {
		diff.D, diff.Tier = bandMid[i], floor
		return fmt.Sprintf("role %s: raised to its %s tier", role, floor)
	}
	return ""
}
