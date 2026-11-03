package sqlite_test

import (
	"context"
	"database/sql"
	"errors"
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/polterware/dispatch/go-core/domains/dispatch"
	"github.com/polterware/dispatch/go-core/providers/sqlite"
)

func TestConversationMigrationFrom0005(t *testing.T) {
	path := filepath.Join(t.TempDir(), "dispatch.db")
	db, err := sql.Open("sqlite", path)
	if err != nil {
		t.Fatal(err)
	}
	defer db.Close()
	for _, name := range []string{"0001_baseline.sql", "0002_drop_verify.sql", "0003_run_usage.sql", "0004_run_subagent.sql", "0005_run_role.sql"} {
		body, err := os.ReadFile(filepath.Join("migrations", name))
		if err != nil {
			t.Fatal(err)
		}
		if _, err := db.Exec(string(body)); err != nil {
			t.Fatal(err)
		}
	}
	if _, err := db.Exec(`PRAGMA user_version=5; INSERT INTO dispatch_runs(id, project_id, session_id, created_at) VALUES ('old','workspace','worker-session','2026-11-02T12:00:00Z')`); err != nil {
		t.Fatal(err)
	}
	if err := db.Close(); err != nil {
		t.Fatal(err)
	}
	store, err := sqlite.New(path)
	if err != nil {
		t.Fatal(err)
	}
	defer store.Close()
	old, err := store.GetRun(context.Background(), "old")
	if err != nil || old.SessionID != "worker-session" || old.ConversationID != "" || old.CallerAgent != "" {
		t.Fatalf("old=%+v error=%v", old, err)
	}
	var version, index int
	if err := store.DB.QueryRow(`PRAGMA user_version`).Scan(&version); err != nil || version != 6 {
		t.Fatalf("version=%d error=%v", version, err)
	}
	if err := store.DB.QueryRow(`SELECT COUNT(*) FROM pragma_index_info('dispatch_runs_conversation_created') WHERE name IN ('conversation_id','created_at')`).Scan(&index); err != nil || index != 2 {
		t.Fatalf("index=%d error=%v", index, err)
	}
	// Opening the database again must not rerun ALTER TABLE.
	again, err := sqlite.New(path)
	if err != nil {
		t.Fatal(err)
	}
	again.Close()
}

func TestRunsConversationRoundTripAndFilter(t *testing.T) {
	store := newStore(t)
	ctx := context.Background()
	now := time.Now().UTC()
	for i, run := range []dispatch.Run{
		{ID: "first", ProjectID: "one", ConversationID: "codex:chat", CallerAgent: "codex", SessionID: "worker-first"},
		{ID: "second", ProjectID: "two", ConversationID: "codex:chat", CallerAgent: "codex", SessionID: "worker-second"},
		{ID: "other", ProjectID: "one", ConversationID: "claude:other", CallerAgent: "claude"},
		{ID: "third", ProjectID: "one", ConversationID: "codex:chat", CallerAgent: "codex"},
	} {
		run.CreatedAt = now.Add(time.Duration(i) * time.Second)
		if err := store.SaveRun(ctx, run); err != nil {
			t.Fatal(err)
		}
	}
	got, err := store.GetRun(ctx, "first")
	if err != nil || got.ConversationID != "codex:chat" || got.CallerAgent != "codex" || got.SessionID != "worker-first" {
		t.Fatalf("run=%+v error=%v", got, err)
	}
	got.Note = "updated"
	if err := store.SaveRun(ctx, got); err != nil {
		t.Fatal(err)
	}
	got, err = store.GetRun(ctx, "first")
	if err != nil || got.ConversationID != "codex:chat" || got.Note != "updated" {
		t.Fatalf("updated=%+v error=%v", got, err)
	}
	all, err := store.ListRuns(ctx, "", 0, 0, "codex:chat")
	if err != nil || len(all) != 3 {
		t.Fatalf("all=%+v error=%v", all, err)
	}
	page, err := store.ListRuns(ctx, "one", 1, 1, "codex:chat")
	if err != nil || len(page) != 1 || page[0].ID != "first" {
		t.Fatalf("page=%+v error=%v", page, err)
	}
}

func TestSectionsStore(t *testing.T) {
	store := newStore(t)
	ctx := context.Background()
	save := func(id, name string, pos int) dispatch.Section {
		t.Helper()
		s, err := store.SaveSection(ctx, dispatch.Section{ID: id, Name: name, Position: pos})
		if err != nil {
			t.Fatal(err)
		}
		return s
	}
	list := func() []dispatch.Section {
		t.Helper()
		sections, err := store.ListSections(ctx)
		if err != nil {
			t.Fatal(err)
		}
		return sections
	}
	a, b := save("", "First", 0), save("", "Second", 1)
	if a.ID == "" || a.CreatedAt.IsZero() || len(list()) != 2 {
		t.Fatal("sections were not created")
	}
	renamed := save(a.ID, "Renamed", 2)
	if renamed.Name != "Renamed" || !renamed.CreatedAt.Equal(a.CreatedAt) || list()[0].ID != b.ID {
		t.Fatal("rename or reorder failed")
	}
	move := func(kind, ref, target string) {
		t.Helper()
		if err := store.MoveToSection(ctx, kind, ref, target); err != nil {
			t.Fatal(err)
		}
	}
	move("conversation", "codex:chat", a.ID)
	move("run", "run-1", a.ID)
	move("run", "run-1", a.ID)
	sections := list()
	if len(sections[1].Items) != 2 || sections[1].Items[0].Ref != "codex:chat" || sections[1].Items[1].Position != 1 {
		t.Fatalf("ordered items=%+v", sections)
	}
	move("conversation", "codex:chat", b.ID)
	sections = list()
	if len(sections[0].Items) != 1 || len(sections[1].Items) != 1 {
		t.Fatalf("move=%+v", sections)
	}
	move("run", "run-1", "")
	if len(list()[1].Items) != 0 {
		t.Fatal("move out failed")
	}
	if err := store.DeleteSection(ctx, b.ID); err != nil {
		t.Fatal(err)
	}
	var items int
	if err := store.DB.QueryRow(`SELECT COUNT(*) FROM dispatch_section_items`).Scan(&items); err != nil || items != 0 {
		t.Fatalf("cascade count=%d error=%v", items, err)
	}
	if sections = list(); len(sections) != 1 || sections[0].ID != a.ID {
		t.Fatalf("delete=%+v", sections)
	}
	if _, err := store.SaveSection(ctx, dispatch.Section{Name: " "}); !errors.Is(err, sqlite.ErrInvalidSection) {
		t.Fatalf("blank name: %v", err)
	}
	if _, err := store.SaveSection(ctx, dispatch.Section{Name: "x", Position: -1}); !errors.Is(err, sqlite.ErrInvalidSection) {
		t.Fatalf("negative position: %v", err)
	}
	if _, err := store.SaveSection(ctx, dispatch.Section{ID: "missing", Name: "x"}); !errors.Is(err, sqlite.ErrUnknownSection) {
		t.Fatalf("unknown: %v", err)
	}
	if err := store.MoveToSection(ctx, "invalid", "ref", a.ID); !errors.Is(err, sqlite.ErrInvalidSection) {
		t.Fatalf("kind: %v", err)
	}
	if err := store.MoveToSection(ctx, "run", "", a.ID); !errors.Is(err, sqlite.ErrInvalidSection) {
		t.Fatalf("ref: %v", err)
	}
	if err := store.MoveToSection(ctx, "run", "ref", "missing"); !errors.Is(err, sqlite.ErrUnknownSection) {
		t.Fatalf("target: %v", err)
	}
	if err := store.DeleteSection(ctx, "missing"); !errors.Is(err, sqlite.ErrUnknownSection) {
		t.Fatalf("delete unknown: %v", err)
	}
	// Check the SQL constraint too, independently of the Go validation.
	if _, err := store.DB.Exec(`INSERT INTO dispatch_section_items VALUES (?, 'invalid', 'ref', 0)`, a.ID); err == nil {
		t.Fatal("invalid kind accepted by SQLite")
	}
}
