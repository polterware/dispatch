package dispatch

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"

	adapter "github.com/polterware/dispatch/go-core/providers/adapters"
)

type fakeAdapter struct {
	name string
	out  string
	err  error
	ran  *[]string
}

func (f fakeAdapter) Name() string   { return f.name }
func (f fakeAdapter) Binary() string { return f.name }
func (f fakeAdapter) Capabilities() adapter.Capabilities {
	return adapter.Capabilities{SessionResume: true, ReadOnly: true, FileEditing: true}
}
func (f fakeAdapter) Models(context.Context) ([]adapter.Model, error) { return nil, nil }
func (f fakeAdapter) Quota(context.Context) (adapter.Quota, error) {
	return adapter.Quota{}, adapter.ErrNoQuota
}
func (f fakeAdapter) Run(_ context.Context, r adapter.Request, m string) (adapter.Result, error) {
	tag := f.name + "/" + m
	if r.ResumeSession != "" {
		tag += "@" + r.ResumeSession
	}
	*f.ran = append(*f.ran, tag)
	return adapter.Result{Output: f.out, SessionID: "sess-" + f.name}, f.err
}

func testDeps(t *testing.T, ran *[]string, a ...adapter.Adapter) Deps {
	t.Chdir(t.TempDir())
	t.Setenv(depthEnv, "")
	t.Setenv(noDelegateEnv, "")
	stateDir := t.TempDir()
	return Deps{
		Adapters: a,
		Table: Table{Tiers: map[adapter.Tier][]Entry{
			adapter.TierEasy: {{Agent: "a", Model: "m1"}, {Agent: "b", Model: "m2"}, {Agent: "ghost", Model: "x"}},
		}},
		Store:       newFakeStoreWithDir(stateDir),
		StateDir:    stateDir,
		Now:         time.Now,
		Installed:   func(b string) bool { return b != "ghost" },
		WorktreeDir: filepath.Join(t.TempDir(), "wt"),
	}
}

func TestFallback(t *testing.T) {
	var ran []string
	d := testDeps(t, &ran,
		fakeAdapter{name: "a", err: errors.New("rate limited"), ran: &ran},
		fakeAdapter{name: "b", out: "hello\n", ran: &ran})
	rec, err := d.Run(context.Background(), Options{Tier: adapter.TierEasy, Prompt: "p"})
	if err != nil || rec.Status != StatusCompleted || rec.Agent != "b" || len(rec.Attempts) != 1 {
		t.Fatalf("%+v %v", rec, err)
	}
	if got, _ := d.Store.GetRun(context.Background(), rec.ID); got.Status != StatusCompleted {
		t.Fatalf("not persisted: %+v", got)
	}
}

func TestAllFail(t *testing.T) {
	var ran []string
	d := testDeps(t, &ran, fakeAdapter{name: "a", err: errors.New("x"), ran: &ran}, fakeAdapter{name: "b", err: errors.New("y"), ran: &ran})
	rec, _ := d.Run(context.Background(), Options{Tier: adapter.TierEasy, Prompt: "p"})
	if rec.Status != StatusFailed || len(ran) != 2 {
		t.Fatalf("%+v ran=%v", rec, ran)
	}
}

func TestTimeoutStopsFallback(t *testing.T) {
	var ran []string
	d := testDeps(t, &ran, fakeAdapter{name: "a", err: context.DeadlineExceeded, ran: &ran}, fakeAdapter{name: "b", out: "x", ran: &ran})
	rec, _ := d.Run(context.Background(), Options{Tier: adapter.TierEasy, Prompt: "p", Timeout: time.Second})
	if rec.Status != StatusTimedOut || len(ran) != 1 {
		t.Fatalf("%+v ran=%v", rec, ran)
	}
}

func TestRecursionRefused(t *testing.T) {
	var ran []string
	d := testDeps(t, &ran)
	t.Setenv(depthEnv, "1")
	if _, err := d.Run(context.Background(), Options{Tier: adapter.TierEasy}); !errors.Is(err, ErrRecursive) {
		t.Fatalf("got %v", err)
	}
}

func TestNoDelegateRefused(t *testing.T) {
	var ran []string
	d := testDeps(t, &ran)
	t.Setenv(noDelegateEnv, "1")
	if _, err := d.Run(context.Background(), Options{Tier: adapter.TierEasy}); !errors.Is(err, ErrRecursive) {
		t.Fatalf("got %v, want ErrRecursive", err)
	}
}

type fakeEnvAdapter struct {
	fakeAdapter
	capturedEnv *[]string
}

func (f fakeEnvAdapter) Run(ctx context.Context, r adapter.Request, m string) (adapter.Result, error) {
	*f.capturedEnv = append([]string(nil), r.Env...)
	return f.fakeAdapter.Run(ctx, r, m)
}

func TestChildEnvironmentVariables(t *testing.T) {
	var ran []string
	var env []string
	f := fakeEnvAdapter{
		fakeAdapter: fakeAdapter{name: "a", out: "ok", ran: &ran},
		capturedEnv: &env,
	}
	d := testDeps(t, &ran, f)
	t.Setenv(runIDEnv, "parent-run-456")

	rec, err := d.Run(context.Background(), Options{Tier: adapter.TierEasy, Prompt: "p"})
	if err != nil {
		t.Fatal(err)
	}

	mustContain := map[string]string{
		"DISPATCH_DEPTH":         "1",
		"DISPATCH_RUN_ID":        rec.ID,
		"DISPATCH_PARENT_RUN_ID": "parent-run-456",
		"DISPATCH_NO_DELEGATE":   "1",
	}

	envMap := map[string]string{}
	for _, entry := range env {
		k, v, _ := strings.Cut(entry, "=")
		envMap[k] = v
	}

	for k, want := range mustContain {
		if got := envMap[k]; got != want {
			t.Errorf("env[%s] = %q, want %q", k, got, want)
		}
	}
}

func TestPageAndSummary(t *testing.T) {
	text := "a\nb\nc\nd\n"
	c, total, more := Page(text, 1, 2)
	if c != "b\nc\n" || total != 4 || !more {
		t.Fatalf("%q %d %v", c, total, more)
	}
	if _, _, more := Page(text, 2, 10); more {
		t.Fatal("no more expected")
	}
	s, trunc := Summary("l1\nl2\nl3\n", 2, 100)
	if s != "l1\nl2\n" || !trunc {
		t.Fatalf("%q %v", s, trunc)
	}
	if s, trunc := Summary("short", 10, 100); s != "short" || trunc {
		t.Fatalf("%q %v", s, trunc)
	}
}

func TestResumeUsesSameAgentAndSession(t *testing.T) {
	var ran []string
	d := testDeps(t, &ran, fakeAdapter{name: "a", out: "one", ran: &ran}, fakeAdapter{name: "b", out: "two", ran: &ran})
	first, err := d.Run(context.Background(), Options{Tier: adapter.TierEasy, Prompt: "p", ReadOnly: true})
	if err != nil || first.SessionID != "sess-a" {
		t.Fatalf("%+v %v", first, err)
	}
	ran = nil
	second, err := d.Run(context.Background(), Options{Prompt: "follow up", ReadOnly: true, Resume: first.ID})
	if err != nil || second.Status != StatusCompleted || second.ResumedOf != first.ID {
		t.Fatalf("%+v %v", second, err)
	}
	if len(ran) != 1 || ran[0] != "a/m1@sess-a" {
		t.Fatalf("ran = %v", ran)
	}
}

type editFake struct {
	fakeAdapter
	edit func(dir string)
}

func (f editFake) Run(ctx context.Context, r adapter.Request, m string) (adapter.Result, error) {
	if f.edit != nil {
		f.edit(r.Dir)
	}
	return f.fakeAdapter.Run(ctx, r, m)
}

func scriptRepo(t *testing.T, script string) string {
	t.Helper()
	root := t.TempDir()
	git := func(args ...string) {
		t.Helper()
		cmd := exec.Command("git", append([]string{"-C", root, "-c", "user.name=test", "-c", "user.email=test@localhost"}, args...)...)
		b, err := cmd.CombinedOutput()
		if err != nil {
			t.Fatalf("git %v: %v\n%s", args, err, b)
		}
	}
	git("init", "-q", "-b", "main")
	write := func(path, body string, mode os.FileMode) {
		t.Helper()
		full := filepath.Join(root, path)
		if err := os.MkdirAll(filepath.Dir(full), 0o755); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(full, []byte(body), mode); err != nil {
			t.Fatal(err)
		}
	}
	write("check.sh", script, 0o755)
	git("add", "-A")
	git("commit", "-qm", "base")
	return root
}

func TestWriteRunCommitsOnItsBranch(t *testing.T) {
	t.Setenv("DISPATCH_HOME", t.TempDir())
	root := t.TempDir()
	cmd := exec.Command("git", "-C", root, "-c", "user.name=test", "-c", "user.email=test@localhost", "init", "-q", "-b", "main")
	if b, err := cmd.CombinedOutput(); err != nil {
		t.Fatalf("%v %s", err, b)
	}
	if err := os.WriteFile(filepath.Join(root, "README"), []byte("hi\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	cmd = exec.Command("git", "-C", root, "-c", "user.name=test", "-c", "user.email=test@localhost", "add", "-A")
	if b, err := cmd.CombinedOutput(); err != nil {
		t.Fatalf("%v %s", err, b)
	}
	cmd = exec.Command("git", "-C", root, "-c", "user.name=test", "-c", "user.email=test@localhost", "commit", "-qm", "base")
	if b, err := cmd.CombinedOutput(); err != nil {
		t.Fatalf("%v %s", err, b)
	}
	var ran []string
	f := editFake{fakeAdapter: fakeAdapter{name: "a", out: "ok\n", ran: &ran}, edit: func(dir string) {
		os.WriteFile(filepath.Join(dir, "extra.txt"), []byte("x\n"), 0o644)
	}}
	d := testDeps(t, &ran, f)
	rec, err := d.Run(context.Background(), Options{Tier: adapter.TierEasy, Prompt: "p", Dir: root})
	if err != nil || rec.Status != StatusCompleted || rec.ExitClass != "ok" || !strings.Contains(rec.Note, "changes committed on "+rec.Branch) {
		t.Fatalf("%+v %v", rec, err)
	}
}

func TestRateLimitSetsCooldownAndDemotes(t *testing.T) {
	var ran []string
	d := testDeps(t, &ran,
		fakeAdapter{name: "a", err: errors.New("429 too many requests"), ran: &ran},
		fakeAdapter{name: "b", out: "ok", ran: &ran})
	_, _ = d.Run(context.Background(), Options{Tier: adapter.TierEasy, Prompt: "p", ReadOnly: true})
	if _, ok := storeCooldowns(context.Background(), d.Store, time.Now())["a"]; !ok {
		t.Fatal("expected cooldown for a")
	}
	ran = nil
	_, _ = d.Run(context.Background(), Options{Tier: adapter.TierEasy, Prompt: "p", ReadOnly: true})
	if len(ran) == 0 || ran[0] != "b/m2" {
		t.Fatalf("cooled-down agent should be tried last, ran = %v", ran)
	}
}

type capsFake struct {
	fakeAdapter
	caps adapter.Capabilities
}

func (c capsFake) Capabilities() adapter.Capabilities { return c.caps }

type probeFake struct {
	fakeAdapter
	report adapter.ProbeReport
}

func (p probeFake) Probe(context.Context) adapter.ProbeReport { return p.report }

type resumeToggle struct {
	fakeAdapter
	on *bool
}

func (r resumeToggle) Capabilities() adapter.Capabilities {
	return adapter.Capabilities{ReadOnly: true, FileEditing: true, SessionResume: *r.on}
}

func TestReadOnlyWithoutSupportIsRefused(t *testing.T) {
	var ran []string
	caps := adapter.Capabilities{FileEditing: true, SessionResume: true}
	d := testDeps(t, &ran,
		capsFake{fakeAdapter: fakeAdapter{name: "a", out: "x", ran: &ran}, caps: caps},
		capsFake{fakeAdapter: fakeAdapter{name: "b", out: "y", ran: &ran}, caps: caps})
	rec, err := d.Run(context.Background(), Options{Tier: adapter.TierEasy, Prompt: "p", ReadOnly: true})
	if err != nil {
		t.Fatal(err)
	}
	if len(ran) != 0 || rec.Status != StatusFailed || !strings.Contains(rec.Error, "does not support readOnly") {
		t.Fatalf("%+v ran=%v", rec, ran)
	}
	if strings.Contains(rec.Error, "no installed agent") {
		t.Fatal(rec.Error)
	}
}

func TestFileEditingWithoutSupportIsRefused(t *testing.T) {
	var ran []string
	d := testDeps(t, &ran, capsFake{
		fakeAdapter: fakeAdapter{name: "a", out: "x", ran: &ran},
		caps:        adapter.Capabilities{ReadOnly: true, SessionResume: true},
	})
	rec, err := d.Run(context.Background(), Options{Tier: adapter.TierEasy, Prompt: "p"})
	if err != nil {
		t.Fatal(err)
	}
	if len(ran) != 0 || rec.Status != StatusFailed || !strings.Contains(rec.Error, "fileEditing") {
		t.Fatalf("%+v ran=%v", rec, ran)
	}
}

func TestResumeWithoutSupportIsRefused(t *testing.T) {
	var ran []string
	on := true
	a := resumeToggle{fakeAdapter: fakeAdapter{name: "a", out: "one", ran: &ran}, on: &on}
	d := testDeps(t, &ran, a)
	first, err := d.Run(context.Background(), Options{Tier: adapter.TierEasy, Prompt: "p", ReadOnly: true})
	if err != nil || first.Status != StatusCompleted {
		t.Fatalf("%+v %v", first, err)
	}
	on = false
	ran = nil
	second, err := d.Run(context.Background(), Options{Prompt: "follow up", ReadOnly: true, Resume: first.ID})
	if err != nil {
		t.Fatal(err)
	}
	if len(ran) != 0 || second.Status != StatusFailed || !strings.Contains(second.Error, "session resume") {
		t.Fatalf("%+v ran=%v", second, ran)
	}
}

func TestUnverifiedProbeStillRuns(t *testing.T) {
	var ran []string
	a := probeFake{
		fakeAdapter: fakeAdapter{name: "a", out: "ok", ran: &ran},
		report:      adapter.ProbeReport{Status: adapter.StatusUnverified, Notes: []string{"version unknown"}},
	}
	d := testDeps(t, &ran, a)
	rec, err := d.Run(context.Background(), Options{Tier: adapter.TierEasy, Prompt: "p", ReadOnly: true})
	if err != nil || rec.Status != StatusCompleted || len(ran) != 1 {
		t.Fatalf("%+v %v ran=%v", rec, err, ran)
	}
	if strings.Contains(rec.Note, "DEGRADED") {
		t.Fatal(rec.Note)
	}
}

func TestDegradedProbeStillRunsUnlessCapabilityMissing(t *testing.T) {
	report := adapter.ProbeReport{
		Status: adapter.StatusDegraded,
		Notes:  []string{"missing flags: --mode"},
		Flags:  []adapter.FlagCheck{{Name: "--mode", Present: false, Capability: "readOnly"}},
	}
	t.Run("write", func(t *testing.T) {
		var ran []string
		a := probeFake{fakeAdapter: fakeAdapter{name: "a", out: "ok", ran: &ran}, report: report}
		d := testDeps(t, &ran, a)
		rec, err := d.Run(context.Background(), Options{Tier: adapter.TierEasy, Prompt: "p", NoWorktree: true})
		if err != nil || rec.Status != StatusCompleted || len(ran) != 1 {
			t.Fatalf("%+v %v ran=%v", rec, err, ran)
		}
		if !strings.Contains(rec.Note, "DEGRADED") {
			t.Fatalf("note %q", rec.Note)
		}
	})
	t.Run("readonly", func(t *testing.T) {
		var ran []string
		a := probeFake{fakeAdapter: fakeAdapter{name: "a", out: "ok", ran: &ran}, report: report}
		d := testDeps(t, &ran, a)
		rec, err := d.Run(context.Background(), Options{Tier: adapter.TierEasy, Prompt: "p", ReadOnly: true})
		if err != nil {
			t.Fatal(err)
		}
		if len(ran) != 0 || rec.Status != StatusFailed || !strings.Contains(rec.Error, "readOnly") || !strings.Contains(rec.Error, "--mode") {
			t.Fatalf("%+v ran=%v", rec, ran)
		}
	})
}

func TestDegradedCodexProbeDoesNotBreakRun(t *testing.T) {
	wd, err := os.Getwd()
	if err != nil {
		t.Fatal(err)
	}
	helpBytes, err := os.ReadFile(filepath.Join(wd, "../../providers/adapters/testdata/codex/0.99.0/help.txt"))
	if err != nil {
		t.Fatal(err)
	}
	verBytes, err := os.ReadFile(filepath.Join(wd, "../../providers/adapters/testdata/codex/0.99.0/version.txt"))
	if err != nil {
		t.Fatal(err)
	}
	help, ver := string(helpBytes), string(verBytes)
	run := func(t *testing.T, helpText string) (Run, string) {
		t.Helper()
		dir := t.TempDir()
		logPath := filepath.Join(dir, "log")
		verPath := filepath.Join(dir, "version.txt")
		helpPath := filepath.Join(dir, "help.txt")
		if err := os.WriteFile(verPath, []byte(ver), 0o644); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(helpPath, []byte(helpText), 0o644); err != nil {
			t.Fatal(err)
		}
		script := fmt.Sprintf("#!/bin/sh\nprintf '%%s\\n' \"$*\" >> %q\ncase \"$*\" in\n--version) /bin/cat %q ;;\n--help|\"exec --help\") /bin/cat %q ;;\nesac\n[ \"$1\" = app-server ] || exit 0\nwhile IFS= read -r line; do\n  case \"$line\" in\n  *'\"id\":1,'*) echo '{\"id\":1,\"result\":{}}' ;;\n  *'\"id\":2,'*) echo '{\"id\":2,\"result\":{\"thread\":{\"id\":\"th\"}}}' ;;\n  *'\"id\":3,'*) echo '{\"method\":\"item/completed\",\"params\":{\"item\":{\"type\":\"agentMessage\",\"id\":\"m\",\"text\":\"ok\",\"phase\":\"final_answer\"}}}'; echo '{\"method\":\"turn/completed\",\"params\":{\"turn\":{\"id\":\"t\",\"status\":\"completed\"}}}' ;;\n  esac\ndone\n", logPath, verPath, helpPath)
		if err := os.WriteFile(filepath.Join(dir, "codex"), []byte(script), 0o755); err != nil {
			t.Fatal(err)
		}
		t.Setenv("PATH", dir)
		var ran []string
		d := testDeps(t, &ran, adapter.Codex{})
		d.Table = Table{Tiers: map[adapter.Tier][]Entry{
			adapter.TierEasy: {{Agent: "codex", Model: "gpt-5.4"}},
		}}
		rec, err := d.Run(context.Background(), Options{Tier: adapter.TierEasy, Prompt: "p", ReadOnly: true})
		if err != nil {
			t.Fatal(err)
		}
		log, _ := os.ReadFile(logPath)
		return rec, string(log)
	}
	t.Run("flags present", func(t *testing.T) {
		rec, log := run(t, help)
		if rec.Status != StatusCompleted || strings.Contains(rec.Note, "DEGRADED") || !strings.Contains(rec.Output, "ok") {
			t.Fatalf("status %s note %q out %q err %q\n%s", rec.Status, rec.Note, rec.Output, rec.Error, log)
		}
	})
	t.Run("missing json still runs", func(t *testing.T) {
		mutated := strings.ReplaceAll(help, "--json", "--json-removed")
		rec, log := run(t, mutated)
		if rec.Status != StatusCompleted || !strings.Contains(rec.Output, "ok") {
			t.Fatalf("status %s err %q out %q\n%s", rec.Status, rec.Error, rec.Output, log)
		}
		if !strings.Contains(rec.Note, "DEGRADED") || !strings.Contains(rec.Note, "--json") {
			t.Fatalf("note %q", rec.Note)
		}
		if !strings.Contains(log, "app-server") {
			t.Fatalf("run did not execute:\n%s", log)
		}
	})
	t.Run("missing readonly flag refused", func(t *testing.T) {
		mutated := strings.ReplaceAll(help, "\n  -c ", "\n  -C ")
		if mutated == help {
			t.Fatal("fixture has no -c flag line")
		}
		rec, log := run(t, mutated)
		if rec.Status != StatusFailed || !strings.Contains(rec.Error, "readOnly") || !strings.Contains(rec.Error, "-c") {
			t.Fatalf("%s %q\n%s", rec.Status, rec.Error, log)
		}
		for _, line := range strings.Split(log, "\n") {
			if strings.HasPrefix(line, "exec") && line != "exec --help" {
				t.Fatalf("readonly run was not refused:\n%s", log)
			}
		}
	})
}

func TestEmpiricalQuota(t *testing.T) {
	st := newFakeStore()
	now := time.Now()
	if err := st.SaveRun(context.Background(), Run{
		ID: "1", Status: StatusFailed, Agent: "cursor", Error: "429 too many requests",
		CreatedAt: now, FinishedAt: now,
		Attempts: []Attempt{{Agent: "cursor", Error: "429 too many requests", ExitClass: "rate_limit"}},
	}); err != nil {
		t.Fatal(err)
	}
	s := EmpiricalQuota(st, "cursor", now)
	if s.Used != nil || s.Confidence != adapter.ConfidenceEmpirical || s.Risk != "recent_rate_limit" || s.Source != adapter.SourceEmpirical {
		t.Fatalf("%+v", s)
	}
	until := now.Add(time.Hour)
	if err := st.SetCooldown(context.Background(), "grok", until); err != nil {
		t.Fatal(err)
	}
	g := EmpiricalQuota(st, "grok", now)
	if g.Used != nil || g.Risk != "cooldown" || g.ResetAt.IsZero() {
		t.Fatalf("%+v", g)
	}
	raw, _ := json.Marshal(s)
	if strings.Contains(string(raw), `"used"`) {
		t.Fatal(string(raw))
	}
}

func TestCancelOrphanAndWait(t *testing.T) {
	var ran []string
	d := testDeps(t, &ran)
	rec := Run{ID: "r-1", Status: StatusRunning, PID: 999999, CreatedAt: time.Now()}
	_ = d.Store.SaveRun(context.Background(), rec)
	out, err := d.Cancel(context.Background(), "r-1")
	if err != nil || out.Status != StatusOrphaned {
		t.Fatalf("%+v %v", out, err)
	}
	rec2 := Run{ID: "r-2", Status: StatusRunning, PID: 999999, CreatedAt: time.Now()}
	_ = d.Store.SaveRun(context.Background(), rec2)
	w, err := d.Wait(context.Background(), "r-2", 10*time.Millisecond)
	if err != nil || w.Status != StatusOrphaned {
		t.Fatalf("%+v %v", w, err)
	}
}

type slowFail struct {
	fakeAdapter
	delay time.Duration
}

func (s slowFail) Run(ctx context.Context, r adapter.Request, m string) (adapter.Result, error) {
	time.Sleep(s.delay)
	return s.fakeAdapter.Run(ctx, r, m)
}

// RunMs describes the attempt that finished the run, not earlier failures,
// otherwise speed evidence blames the model that rescued the run.
func TestRunMsExcludesFailedFallbackAttempts(t *testing.T) {
	var ran []string
	d := testDeps(t, &ran,
		slowFail{fakeAdapter: fakeAdapter{name: "a", err: errors.New("boom"), ran: &ran}, delay: 400 * time.Millisecond},
		fakeAdapter{name: "b", out: "ok\n", ran: &ran})
	rec, err := d.Run(context.Background(), Options{Tier: adapter.TierEasy, Prompt: "p"})
	if err != nil || rec.Status != StatusCompleted || rec.Agent != "b" {
		t.Fatalf("%+v %v", rec, err)
	}
	if rec.RunMs == nil || *rec.RunMs >= 200 {
		t.Fatalf("RunMs %v includes the failed attempt", rec.RunMs)
	}
}

func TestCreditErrorCoolsDownModelScopeAndMarksLog(t *testing.T) {
	var ran []string
	d := testDeps(t, &ran,
		fakeAdapter{name: "a", err: errors.New("a: stopped early: ActionRequiredError: m1 is not included in your weekly usage"), ran: &ran},
		fakeAdapter{name: "b", out: "ok", ran: &ran})
	rec, _ := d.Run(context.Background(), Options{Tier: adapter.TierEasy, Prompt: "p", ReadOnly: true})
	if rec.Status != StatusCompleted || len(rec.Attempts) != 1 || rec.Attempts[0].ExitClass != "credits" {
		t.Fatalf("%+v", rec)
	}
	cool := storeCooldowns(context.Background(), d.Store, time.Now())
	if _, ok := cool["a"]; ok {
		t.Fatal("credit error must not cool the whole agent")
	}
	until, ok := cool["a:m1"]
	if !ok || until.Before(time.Now().Add(creditCooldownFor-time.Minute)) {
		t.Fatalf("cooldowns = %v", cool)
	}
	b, err := os.ReadFile(AgentLogPath(d.Store, rec.ID))
	if err != nil || !strings.Contains(string(b), "── dispatch: a/m1 failed (credits); trying b/m2 ──") {
		t.Fatalf("log = %q %v", b, err)
	}
	ran = nil
	_, _ = d.Run(context.Background(), Options{Tier: adapter.TierEasy, Prompt: "p", ReadOnly: true})
	if len(ran) != 1 || ran[0] != "b/m2" {
		t.Fatalf("a/m1 should be skipped, ran = %v", ran)
	}
}
