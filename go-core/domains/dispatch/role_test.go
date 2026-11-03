package dispatch

import (
	"os"
	"path/filepath"
	"testing"
	"time"

	adapter "github.com/polterware/dispatch/go-core/providers/adapters"
)

func TestApplyProfileExplicitWins(t *testing.T) {
	o := Options{Tier: adapter.TierHard, Prompt: "do it"}
	o.Filters.Model = "pinned"
	p := Profile{Tier: "easy", Model: "profile", ReadOnly: true, Timeout: "2m", Instructions: "be brief", Prefer: []string{"codex"}}
	if err := ApplyProfile(&o, "reviewer", p); err != nil {
		t.Fatal(err)
	}
	if o.Tier != adapter.TierHard || o.Filters.Model != "pinned" || o.TierFloor != adapter.TierEasy {
		t.Fatalf("explicit values lost: %+v", o)
	}
	if !o.ReadOnly || o.Timeout != 2*time.Minute || o.Profile != "reviewer" {
		t.Fatalf("profile not applied: %+v", o)
	}
	if o.Prompt != "be brief\n\ndo it" {
		t.Fatalf("prompt = %q", o.Prompt)
	}
}

func TestLoadAllProfilesProjectWins(t *testing.T) {
	dir := t.TempDir()
	user, project := filepath.Join(dir, "u.json"), filepath.Join(dir, "p.json")
	os.WriteFile(user, []byte(`{"agents":{"a":{"tier":"easy"},"b":{"tier":"easy"}}}`), 0o644)
	os.WriteFile(project, []byte(`{"agents":{"a":{"tier":"hard"}}}`), 0o644)
	m, err := LoadAllProfiles(user, project)
	if err != nil {
		t.Fatal(err)
	}
	if m["a"].Tier != "hard" || m["b"].Tier != "easy" {
		t.Fatalf("merge wrong: %+v", m)
	}
	if _, err := LoadProfiles(filepath.Join(dir, "missing.json")); err != nil {
		t.Fatal(err)
	}
}

func TestValidateProfile(t *testing.T) {
	known := map[string]bool{"codex": true}
	if got := ValidateProfile("x", Profile{Tier: "hard", Prefer: []string{"codex"}}, known); len(got) != 0 {
		t.Fatalf("unexpected problems: %v", got)
	}
	got := ValidateProfile("x", Profile{Tier: "huge", Timeout: "soon", Prefer: []string{"codex"}, Avoid: []string{"codex"}}, known)
	if len(got) != 3 {
		t.Fatalf("want 3 problems, got %v", got)
	}
	if len(ValidateProfile("e", Profile{}, known)) != 1 {
		t.Fatal("empty role not flagged")
	}
}

func TestSaveProfileKeepsTheOthersAndRemoves(t *testing.T) {
	path := filepath.Join(t.TempDir(), "agents.json")
	if err := SaveProfile(path, "reviewer", Profile{Tier: "hard", ReadOnly: true}, false); err != nil {
		t.Fatal(err)
	}
	if err := SaveProfile(path, "tester", Profile{Tier: "normal", Instructions: "Write tests."}, false); err != nil {
		t.Fatal(err)
	}
	if err := SaveProfile(path, "reviewer", Profile{Tier: "easy"}, false); err != nil {
		t.Fatal(err)
	}
	got, err := LoadProfiles(path)
	if err != nil || len(got) != 2 || got["reviewer"].Tier != "easy" || got["tester"].Instructions != "Write tests." {
		t.Fatalf("profiles = %+v, %v", got, err)
	}
	if err := SaveProfile(path, "tester", Profile{}, true); err != nil {
		t.Fatal(err)
	}
	if err := SaveProfile(path, "tester", Profile{}, true); err == nil {
		t.Fatal("removing a missing role must fail")
	}
	if got, _ := LoadProfiles(path); len(got) != 1 {
		t.Fatalf("after remove = %+v", got)
	}
}

func TestValidProfileName(t *testing.T) {
	for name, want := range map[string]bool{"reviewer": true, "code-review2": true, "": false, "2fast": false, "Reviewer": false, "a b": false, "a/b": false} {
		if ValidProfileName(name) != want {
			t.Errorf("%q = %v", name, !want)
		}
	}
}
