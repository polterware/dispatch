package classifier

import (
	"context"
	"encoding/json"
	"errors"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"sync/atomic"
	"testing"

	"github.com/polterware/dispatch/go-core/domains/dispatch"
	adapter "github.com/polterware/dispatch/go-core/providers/adapters"
)

func testKey(t *testing.T) Key {
	t.Setenv("CLASSIFIER_TEST_KEY", "secret-value")
	return Key{Env: "CLASSIFIER_TEST_KEY", Account: "test"}
}

func TestJevRetriesARateLimitAndReadsTheProbabilities(t *testing.T) {
	var calls int32
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if atomic.AddInt32(&calls, 1) == 1 {
			w.WriteHeader(429)
			return
		}
		if r.Header.Get("Authorization") != "Bearer secret-value" {
			t.Errorf("auth %q", r.Header.Get("Authorization"))
		}
		var body struct {
			Model     string            `json:"model"`
			State     map[string]string `json:"state"`
			Questions map[string]struct {
				Type     string   `json:"type"`
				Criteria []string `json:"criteria"`
			} `json:"questions"`
		}
		json.NewDecoder(r.Body).Decode(&body)
		q := body.Questions["difficulty"]
		if body.Model != "jev-latest" || body.State["task"] != "fix the footer" || q.Type != "score" || len(q.Criteria) != 3 {
			t.Errorf("body %+v", body)
		}
		w.Write([]byte(`{"answers":{"difficulty":{"score":0.1,"probabilities":{"0":0.9,"1":0.08,"2":0.02}}}}`))
	}))
	defer srv.Close()
	d, err := Jev{URL: srv.URL, Key: testKey(t)}.Classify(context.Background(), dispatch.Task{Text: "fix the footer"})
	if err != nil || calls != 2 || d.Tier != adapter.TierEasy || !d.HasProbs || d.Provider != "jev" || d.Model != "jev-latest" {
		t.Fatalf("%+v %v calls=%d", d, err, calls)
	}
}

func TestJevAsksTheRoleAsAChoiceInTheSameRequest(t *testing.T) {
	roles := map[string]string{"reviewer": "Review a diff.", dispatch.GeneralRole: dispatch.GeneralWhen}
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		var body struct {
			Questions map[string]struct {
				Type     string          `json:"type"`
				Criteria json.RawMessage `json:"criteria"`
			} `json:"questions"`
		}
		json.NewDecoder(r.Body).Decode(&body)
		var criteria map[string]string
		json.Unmarshal(body.Questions["role"].Criteria, &criteria)
		if body.Questions["role"].Type != "choice" || criteria["reviewer"] != "Review a diff." || body.Questions["difficulty"].Type != "score" {
			t.Errorf("questions %+v", body.Questions)
		}
		w.Write([]byte(`{"answers":{"difficulty":{"probabilities":{"0":0.1,"1":0.8,"2":0.1}},"role":{"choice":"reviewer","confidence":0.7,"probabilities":{"reviewer":0.85,"general":0.15}}}}`))
	}))
	defer srv.Close()
	d, err := Jev{URL: srv.URL, Key: testKey(t)}.Classify(context.Background(), dispatch.Task{Text: "review branch x", Roles: roles})
	if err != nil || d.Role == nil || d.Role.Role != "reviewer" || d.Role.Probs["reviewer"] != 0.85 || *d.Role.Confidence != 0.7 {
		t.Fatalf("%+v %v", d.Role, err)
	}
	if dispatch.PickRole(*d.Role, roles) != "reviewer" {
		t.Fatal("pick")
	}
}

func TestJevReportsAnHTTPError(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		w.WriteHeader(401)
		w.Write([]byte(`{"error":"bad key"}`))
	}))
	defer srv.Close()
	_, err := Jev{URL: srv.URL, Key: testKey(t)}.Classify(context.Background(), dispatch.Task{Text: "x"})
	if err == nil || !strings.Contains(err.Error(), "HTTP 401") || strings.Contains(err.Error(), "secret-value") {
		t.Fatalf("%v", err)
	}
}

func TestOpenAIReadsLevelsAndRefusals(t *testing.T) {
	answer := `{"answers":[{"name":"difficulty","score":2,"confidence":0.8,"probabilities":[{"index":0,"label":"easy","probability":0.1},{"index":1,"label":"normal","probability":0.2},{"index":2,"label":"hard","probability":0.7}]}]}`
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		var body struct {
			Input     string `json:"input"`
			Questions []struct {
				Name   string              `json:"name"`
				Levels []map[string]string `json:"levels"`
			} `json:"questions"`
		}
		json.NewDecoder(r.Body).Decode(&body)
		if body.Input == "refuse" {
			w.Write([]byte(`{"type":"refusal","refusal":"policy"}`))
			return
		}
		if len(body.Questions) != 1 || body.Questions[0].Name != "difficulty" || len(body.Questions[0].Levels) != 3 || body.Questions[0].Levels[2]["label"] != "hard" {
			t.Errorf("body %+v", body)
		}
		w.Write([]byte(answer))
	}))
	defer srv.Close()
	o := OpenAI{URL: srv.URL, Key: testKey(t)}
	d, err := o.Classify(context.Background(), dispatch.Task{Text: "migrate billing"})
	if err != nil || d.Tier != adapter.TierHard || d.Confidence == nil || *d.Confidence != 0.8 {
		t.Fatalf("%+v %v", d, err)
	}
	if _, err := o.Classify(context.Background(), dispatch.Task{Text: "refuse"}); err == nil || !strings.Contains(err.Error(), "refused") {
		t.Fatalf("refusal: %v", err)
	}
	p, err := parseLevelProbs(json.RawMessage(`{"easy":0.5,"normal":0.5}`))
	if err != nil || p != [3]float64{0.5, 0.5, 0} {
		t.Fatalf("map form %v %v", p, err)
	}
}

func TestCLIParsesTheTier(t *testing.T) {
	for out, want := range map[string]adapter.Tier{
		`{"tier": "hard"}`: adapter.TierHard,
		"Thinking about easy vs hard...\n{\"tier\":\"normal\"}": adapter.TierNormal,
		"easy": adapter.TierEasy,
	} {
		if got, err := ParseTier(out); err != nil || got != want {
			t.Errorf("%q: %s %v", out, got, err)
		}
	}
	if _, err := ParseTier(CLIPrompt("x", nil)); err == nil {
		t.Fatal("the echoed prompt alone must not parse")
	}
	if _, err := ParseTier("easy or hard"); err == nil {
		t.Fatal("an ambiguous reply must fail")
	}
	var asked string
	c := CLI{Agent: "cursor", Model: "fast", Run: func(_ context.Context, p string) (string, error) { asked = p; return `{"tier":"easy"}`, nil }}
	d, err := c.Classify(context.Background(), dispatch.Task{Text: "rename a var"})
	if err != nil || d.Tier != adapter.TierEasy || d.HasProbs || d.Model != "cursor/fast" || !strings.HasSuffix(asked, "Task:\nrename a var") {
		t.Fatalf("%+v %v", d, err)
	}
	if _, err := (CLI{Run: func(context.Context, string) (string, error) { return "", errors.New("quota") }}).Classify(context.Background(), dispatch.Task{}); err == nil {
		t.Fatal("run error must surface")
	}
	if d.Role != nil || strings.Contains(asked, "role") {
		t.Fatalf("no roles asked, got %+v in %q", d.Role, asked)
	}
	roles := map[string]string{"tester": "Write tests.", dispatch.GeneralRole: dispatch.GeneralWhen}
	c.Run = func(_ context.Context, p string) (string, error) {
		asked = p
		return `{"tier":"normal","role":"tester"}`, nil
	}
	d, err = c.Classify(context.Background(), dispatch.Task{Text: "add tests", Roles: roles})
	if err != nil || d.Role == nil || d.Role.Role != "tester" || !strings.Contains(asked, "- tester: Write tests.") {
		t.Fatalf("%+v %v %q", d.Role, err, asked)
	}
	if ParseRole(CLIPrompt("x", roles)) != "" {
		t.Fatal("the echoed prompt alone must not name a role")
	}
}

// fakeKeychain keeps items in memory; legacy holds the ones security(1) saved.
type fakeKeychain struct {
	items, legacy map[string]string
	reads         int
	failSet       error
}

func newFakeKeychain() *fakeKeychain {
	return &fakeKeychain{items: map[string]string{}, legacy: map[string]string{}}
}

func (f *fakeKeychain) Has(account string) (bool, error) { _, ok := f.items[account]; return ok, nil }
func (f *fakeKeychain) Get(account string) (string, error) {
	f.reads++
	return f.items[account], nil
}
func (f *fakeKeychain) Set(account, value string) error {
	if f.failSet != nil {
		return f.failSet
	}
	if _, ok := f.legacy[account]; ok {
		return errors.New("duplicate item")
	}
	f.items[account] = value
	return nil
}
func (f *fakeKeychain) Delete(account string) error           { delete(f.items, account); return nil }
func (f *fakeKeychain) Legacy(account string) (string, error) { return f.legacy[account], nil }
func (f *fakeKeychain) DropLegacy(account string) error       { delete(f.legacy, account); return nil }

// TestMain keeps every test off the person's Keychain; the darwin test that
// uses the real one builds its own store under a test service.
func TestMain(m *testing.M) {
	keychain = newFakeKeychain()
	os.Exit(m.Run())
}

func useFakeKeychain(t *testing.T) *fakeKeychain {
	t.Helper()
	original := keychain
	fake := newFakeKeychain()
	keychain = fake
	t.Cleanup(func() { keychain = original })
	return fake
}

func TestKeyReadsTheEnvironmentThenTheKeychain(t *testing.T) {
	fake := useFakeKeychain(t)
	home := t.TempDir()
	t.Setenv("HOME", home)
	t.Setenv("CLASSIFIER_TEST_KEY", "")
	k := Key{Env: "CLASSIFIER_TEST_KEY", Account: "test"}
	if k.Status() != "missing" {
		t.Fatal(k.Status())
	}
	// The old ~/.config file is no longer read.
	os.MkdirAll(filepath.Join(home, ".config", "test"), 0o700)
	os.WriteFile(filepath.Join(home, ".config", "test", "key"), []byte("from-file\n"), 0o600)
	if v, src, err := k.read(); v != "" || src != "missing" || err != nil {
		t.Fatalf("%q from %s, %v", v, src, err)
	}
	fake.items["test"], fake.reads = "from-keychain", 0
	if k.Status() != "keychain" || fake.reads != 0 {
		t.Fatalf("%s after %d reads: the status must not read the value", k.Status(), fake.reads)
	}
	if v, src, _ := k.read(); v != "from-keychain" || src != "keychain" {
		t.Fatalf("%q from %s", v, src)
	}
	t.Setenv("CLASSIFIER_TEST_KEY", "from-env")
	if v, src, _ := k.read(); k.Status() != "env" || v != "from-env" || src != "env" {
		t.Fatalf("%s: %q from %s", k.Status(), v, src)
	}
	if _, err := (Jev{URL: "http://127.0.0.1:1", Key: Key{Env: "NONE_SET_HERE", Account: "none"}}).Classify(context.Background(), dispatch.Task{}); err == nil ||
		!strings.Contains(err.Error(), "NONE_SET_HERE") || !strings.Contains(err.Error(), "dispatch config key set jev") || strings.Contains(err.Error(), ".config") {
		t.Fatalf("%v", err)
	}
}

func TestKeySavedAndClearedInTheKeychain(t *testing.T) {
	fake := useFakeKeychain(t)
	t.Setenv("CLASSIFIER_TEST_KEY", "")
	k := Key{Env: "CLASSIFIER_TEST_KEY", Account: "test"}
	if err := k.Save("  sk-secret  "); err != nil {
		t.Fatal(err)
	}
	if fake.items["test"] != "sk-secret" || k.Status() != "keychain" {
		t.Fatalf("%q, %s", fake.items["test"], k.Status())
	}
	if err := k.Save("sk-other"); err != nil || fake.items["test"] != "sk-other" {
		t.Fatalf("replace: %q, %v", fake.items["test"], err)
	}
	if err := k.Clear(); err != nil || k.Status() != "missing" {
		t.Fatalf("%s, %v", k.Status(), err)
	}
	if err := k.Clear(); err != nil {
		t.Fatalf("clearing a missing key: %v", err)
	}
	if err := k.Save(" "); err == nil {
		t.Fatal("an empty key must be refused")
	}
	if err := (Key{Env: "CLASSIFIER_TEST_KEY"}).Save("sk"); err == nil {
		t.Fatal("a key with no Keychain account must be refused")
	}
	fake.failSet = errors.New("locked")
	if err := k.Save("sk-secret"); err == nil || strings.Contains(err.Error(), "sk-secret") {
		t.Fatalf("a failed save must say so without the value: %v", err)
	}
}

func TestKeyMovesAnItemSavedThroughSecurity(t *testing.T) {
	t.Setenv("TYPESAFE_API_KEY", "")
	t.Setenv("OPENAI_API_KEY", "")
	fake := useFakeKeychain(t)
	// Jev's old item had another account: it moves to jev, and the old one goes.
	fake.legacy["typesafe"] = "sk-typesafe"
	if JevKey.Status() != "keychain" || fake.items["jev"] != "sk-typesafe" || fake.legacy["typesafe"] != "" {
		t.Fatalf("jev: %+v", fake)
	}
	// OpenAI's had the same account: the old item goes first, then the key is saved again.
	fake.legacy["openai"] = "sk-openai"
	if v, src, err := OpenAIKey.read(); v != "sk-openai" || src != "keychain" || err != nil || len(fake.legacy) != 0 {
		t.Fatalf("openai: %q from %s, %v, %+v", v, src, err, fake)
	}
	// A key saved since wins over an old one left behind.
	fake.items["jev"] = "sk-new"
	fake.legacy["typesafe"] = "sk-old"
	if v, _, _ := JevKey.read(); v != "sk-new" || len(fake.legacy) != 0 {
		t.Fatalf("%q, %+v", v, fake)
	}
	// Saving or clearing drops an old item too.
	fake.legacy["typesafe"] = "sk-old"
	if err := JevKey.Clear(); err != nil || len(fake.legacy) != 0 || JevKey.Status() != "missing" {
		t.Fatalf("%v, %+v", err, fake)
	}
	fake.legacy["openai"] = "sk-old"
	if err := OpenAIKey.Save("sk-new"); err != nil || fake.items["openai"] != "sk-new" || len(fake.legacy) != 0 {
		t.Fatalf("%v, %+v", err, fake)
	}
}
