package dispatch

import (
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"

	adapter "github.com/polterware/dispatch/go-core/providers/adapters"
)

// roleClassifier answers every task with probs and, when the task has roles,
// with role; it keeps the roles it was asked.
type roleClassifier struct {
	probs [3]float64
	role  RoleAnswer
	asked *map[string]string
}

func (roleClassifier) Name() string { return "fake" }

func (f roleClassifier) Classify(_ context.Context, t Task) (Difficulty, error) {
	if f.asked != nil {
		*f.asked = t.Roles
	}
	diff, err := NewDifficulty(&f.probs, "")
	if err == nil && len(t.Roles) > 0 {
		role := f.role
		diff.Role = &role
	}
	return diff, err
}

func pickDeps(t *testing.T, ran *[]string, c Classifier) Deps {
	d := testDeps(t, ran, fakeAdapter{name: "a", out: "ok", ran: ran}, fakeAdapter{name: "b", out: "ok", ran: ran})
	d.Table.Tiers[adapter.TierNormal] = []Entry{{Agent: "a", Model: "normal"}}
	d.Table.Tiers[adapter.TierHard] = []Entry{{Agent: "a", Model: "hard"}}
	d.Policy = DefaultPolicy()
	d.Policy.Discover = false
	d.Profiles = BuiltinProfiles()
	d.Classifier = c
	return d
}

func TestPickRoleNeedsAClearAnswer(t *testing.T) {
	roles := map[string]string{"reviewer": "r", "tester": "t", GeneralRole: GeneralWhen}
	for _, tc := range []struct {
		answer RoleAnswer
		want   string
	}{
		{RoleAnswer{Probs: map[string]float64{"reviewer": 0.8, "tester": 0.1, GeneralRole: 0.1}}, "reviewer"},
		{RoleAnswer{Probs: map[string]float64{"reviewer": 0.45, "tester": 0.4, GeneralRole: 0.15}}, GeneralRole},
		{RoleAnswer{Probs: map[string]float64{"ghost": 0.9}}, GeneralRole},
		{RoleAnswer{Role: "tester"}, "tester"},
		{RoleAnswer{Role: "ghost"}, GeneralRole},
	} {
		if got := PickRole(tc.answer, roles); got != tc.want {
			t.Errorf("%+v: got %s want %s", tc.answer, got, tc.want)
		}
	}
	if PickRole(RoleAnswer{Role: "tester"}, nil) != "" {
		t.Fatal("no roles picks nothing")
	}
}

func TestPickRolesSkipsManualAndUndescribedProfiles(t *testing.T) {
	roles := PickRoles(map[string]Profile{"a": {When: "x"}, "b": {When: "y", Manual: true}, "c": {Tier: "hard"}})
	if len(roles) != 2 || roles["a"] != "x" || roles[GeneralRole] == "" {
		t.Fatalf("%+v", roles)
	}
	if PickRoles(map[string]Profile{"c": {Tier: "hard"}}) != nil {
		t.Fatal("no pickable profile asks no role")
	}
}

func TestAutoPicksASubagentWithoutChangingAccess(t *testing.T) {
	var ran []string
	var asked map[string]string
	// An easy task the classifier calls a review: the reviewer's hard tier is a floor.
	d := pickDeps(t, &ran, roleClassifier{probs: [3]float64{0.9, 0.08, 0.02}, role: RoleAnswer{Probs: map[string]float64{"reviewer": 0.9, GeneralRole: 0.1}}, asked: &asked})
	r, err := d.Run(context.Background(), Options{Prompt: "review branch x"})
	if err != nil || r.Status != StatusCompleted {
		t.Fatalf("%+v %v", r, err)
	}
	if asked["reviewer"] == "" || asked[GeneralRole] == "" {
		t.Fatalf("roles asked %+v", asked)
	}
	if r.Profile != "reviewer" || r.ReadOnly || r.Tier != "hard" || r.Difficulty == nil || DifficultyTier(*r.Difficulty) != adapter.TierHard {
		t.Fatalf("pick applied wrong: profile %q readonly %v tier %s", r.Profile, r.ReadOnly, r.Tier)
	}
	if !strings.HasPrefix(r.Prompt, builtinProfiles["reviewer"].Instructions) || r.TimeoutSec != 15*60 {
		t.Fatalf("instructions or timeout missing: %q %d", r.Prompt, r.TimeoutSec)
	}
	if r.RolePick == nil || !r.RolePick.Auto || r.RolePick.Role != "reviewer" || r.RolePick.Probs["reviewer"] != 0.9 {
		t.Fatalf("pick record %+v", r.RolePick)
	}
	if !strings.Contains(r.Note, "role reviewer picked (P=0.90)") || !strings.Contains(r.Note, "raised to its hard tier") {
		t.Fatalf("note %q", r.Note)
	}
}

func TestAutoStaysGeneralWhenNoRoleIsClear(t *testing.T) {
	var ran []string
	d := pickDeps(t, &ran, roleClassifier{probs: [3]float64{0.1, 0.8, 0.1}, role: RoleAnswer{Probs: map[string]float64{"reviewer": 0.4, "tester": 0.35, GeneralRole: 0.25}}})
	r, err := d.Run(context.Background(), Options{Prompt: "do a mix of things", ReadOnly: true})
	if err != nil || r.Profile != "" || r.Prompt != "do a mix of things" || r.RolePick == nil || r.RolePick.Role != GeneralRole {
		t.Fatalf("%+v %v", r, err)
	}
}

func TestNoPickWhenTheRequestNamesOneOrThePolicyIsOff(t *testing.T) {
	var ran []string
	var asked map[string]string
	c := roleClassifier{probs: [3]float64{0.1, 0.8, 0.1}, role: RoleAnswer{Role: "tester"}, asked: &asked}
	d := pickDeps(t, &ran, c)
	o := Options{Prompt: "x", ReadOnly: true}
	if err := ApplyProfile(&o, "explorer", Profile{Instructions: "look"}); err != nil {
		t.Fatal(err)
	}
	if r, err := d.Run(context.Background(), o); err != nil || r.Profile != "explorer" || r.RolePick != nil || asked != nil {
		t.Fatalf("named: %+v %v asked %v", r, err, asked)
	}
	if r, err := d.Run(context.Background(), Options{Prompt: "x", ReadOnly: true, NoProfile: true}); err != nil || r.Profile != "" || r.RolePick != nil || asked != nil {
		t.Fatalf("--as none: %+v %v", r, err)
	}
	d.Policy.Roles = false
	if r, err := d.Run(context.Background(), Options{Prompt: "x", ReadOnly: true}); err != nil || r.Profile != "" || asked != nil {
		t.Fatalf("policy off: %+v %v", r, err)
	}
}

func TestRoleBoardRoutesItsBandInOrderAndFallsBackWhenEmpty(t *testing.T) {
	var ran []string
	d := pickDeps(t, &ran, roleClassifier{probs: [3]float64{0.1, 0.8, 0.1}, role: RoleAnswer{Role: "tester"}})
	p := d.Profiles["tester"]
	p.Tiers = []RoleEntry{{Tier: "normal", Agent: "b", Model: "first"}, {Tier: "normal", Agent: "a", Model: "second"}, {Tier: "hard", Agent: "a", Model: "deep"}}
	d.Profiles["tester"] = p
	route, err := d.Route(context.Background(), Options{Prompt: "add tests"})
	if err != nil || route.Role != "tester" || len(route.Candidates) != 2 || route.Winner.Agent != "b" || route.Winner.Model != "first" || route.Candidates[1].Model != "second" {
		t.Fatalf("route %+v %v", route, err)
	}
	r, err := d.Run(context.Background(), Options{Prompt: "add tests"})
	if err != nil || r.Profile != "tester" || r.Model != "first" || r.Tier != "normal" {
		t.Fatalf("run %+v %v", r, err)
	}
	// An easy task: the tester board has no easy models, so the general route takes it.
	d.Classifier = roleClassifier{probs: [3]float64{0.95, 0.04, 0.01}, role: RoleAnswer{Role: "tester"}}
	p.Tier = ""
	d.Profiles["tester"] = p
	route, err = d.Route(context.Background(), Options{Prompt: "rename a test"})
	if err != nil || route.Role != "tester" || route.Winner == nil || route.Winner.Model == "first" || route.Winner.Model == "deep" {
		t.Fatalf("empty band must use the tier table: %+v %v", route, err)
	}
}

func TestNamedRoleTierIsAFloorNotAFixedTier(t *testing.T) {
	var ran []string
	d := pickDeps(t, &ran, roleClassifier{probs: [3]float64{0.1, 0.1, 0.8}})
	o := Options{Prompt: "find where X is", ReadOnly: true}
	if err := ApplyProfile(&o, "explorer", d.Profiles["explorer"]); err != nil {
		t.Fatal(err)
	}
	// The explorer's easy floor does not cap a hard task.
	r, err := d.Run(context.Background(), o)
	if err != nil || r.Tier != "hard" || r.RequestedTier != string(TierAuto) {
		t.Fatalf("%+v %v", r, err)
	}
	// Without a classifier the floor lifts the default normal tier.
	d.Classifier = nil
	o = Options{Prompt: "review", ReadOnly: true}
	ApplyProfile(&o, "reviewer", d.Profiles["reviewer"])
	if r, err := d.Run(context.Background(), o); err != nil || r.Tier != "hard" {
		t.Fatalf("no classifier: %+v %v", r, err)
	}
}

func TestBuiltinProfilesMergeUnderTheFiles(t *testing.T) {
	dir := t.TempDir()
	user := filepath.Join(dir, "roles.json")
	// An agents.json an earlier Dispatch wrote is read while no roles.json exists.
	os.WriteFile(filepath.Join(dir, "agents.json"), []byte(`{"agents":{"reviewer":{"tier":"normal","readonly":true},"mine":{"when":"w"}}}`), 0o644)
	m, err := LoadAllProfiles(user, "")
	if err != nil {
		t.Fatal(err)
	}
	if m["reviewer"].Tier != "normal" || m["reviewer"].When != builtinProfiles["reviewer"].When {
		t.Fatalf("an older copy of a built-in keeps its When: %+v", m["reviewer"])
	}
	if _, ok := m["planner"]; !ok || m["mine"].When != "w" {
		t.Fatalf("%+v", m)
	}
	if ValidProfileName(GeneralRole) || ValidProfileName("none") {
		t.Fatal("general and none are reserved")
	}
	known := map[string]bool{"codex": true}
	if got := ValidateProfile("x", Profile{Tiers: []RoleEntry{{Tier: "normal", Agent: "ghost", Model: "m"}, {Tier: "normal", Agent: "codex"}, {Tier: "huge", Agent: "codex", Model: "m"}}}, known); len(got) != 3 {
		t.Fatalf("board problems %v", got)
	}
	if err := SaveProfile(user, "mine", Profile{When: "w2"}, false); err != nil {
		t.Fatal(err)
	}
	if m, _ := LoadAllProfiles(user, ""); m["mine"].When != "w2" || m["reviewer"].Tier != "normal" {
		t.Fatalf("the first save moves agents.json into roles.json: %+v", m)
	}
	if got := ValidateProfile("x", Profile{Manual: true}, known); len(got) != 0 {
		t.Fatalf("a manual-only override is a profile: %v", got)
	}
}
