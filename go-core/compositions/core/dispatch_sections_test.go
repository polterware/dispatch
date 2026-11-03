package core

import (
	"context"
	"path/filepath"
	"testing"
	"time"

	"connectrpc.com/connect"
	"github.com/polterware/dispatch/go-core/domains/dispatch"
	dispatchv1 "github.com/polterware/dispatch/go-core/platform/gen/dispatch/v1"
	"github.com/polterware/dispatch/go-core/providers/sqlite"
)

func sectionTestCore(t *testing.T) *Core {
	t.Helper()
	t.Setenv("DISPATCH_HOME", t.TempDir())
	store, err := sqlite.New(filepath.Join(t.TempDir(), "dispatch.db"))
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { store.Close() })
	core := NewCore(store, testToken, nil)
	core.DBDir = t.TempDir()
	return core
}

func TestSectionHandlers(t *testing.T) {
	core := sectionTestCore(t)
	ctx := context.Background()
	create, err := core.SaveSection(ctx, connect.NewRequest(&dispatchv1.SaveSectionRequest{Name: "Section", Position: 2}))
	if err != nil {
		t.Fatal(err)
	}
	id := create.Msg.Section.Id
	if id == "" || create.Msg.Section.CreatedAt == "" {
		t.Fatalf("section=%+v", create.Msg.Section)
	}
	updated, err := core.SaveSection(ctx, connect.NewRequest(&dispatchv1.SaveSectionRequest{Id: id, Name: "Renamed", Position: 0}))
	if err != nil || updated.Msg.Section.Name != "Renamed" || updated.Msg.Section.Position != 0 {
		t.Fatalf("updated=%+v error=%v", updated, err)
	}
	for _, kind := range []string{"conversation", "run"} {
		if _, err := core.MoveToSection(ctx, connect.NewRequest(&dispatchv1.MoveToSectionRequest{Kind: kind, Ref: "ref", SectionId: id})); err != nil {
			t.Fatal(err)
		}
	}
	listed, err := core.ListSections(ctx, connect.NewRequest(&dispatchv1.ListSectionsRequest{}))
	if err != nil || len(listed.Msg.Sections) != 1 || len(listed.Msg.Sections[0].Items) != 2 {
		t.Fatalf("listed=%+v error=%v", listed, err)
	}
	if listed.Msg.Sections[0].Items[0].Kind != "conversation" || listed.Msg.Sections[0].Items[1].Position != 1 {
		t.Fatal("item mapping lost order")
	}
	if _, err := core.MoveToSection(ctx, connect.NewRequest(&dispatchv1.MoveToSectionRequest{Kind: "conversation", Ref: "ref"})); err != nil {
		t.Fatal(err)
	}
	if _, err := core.DeleteSection(ctx, connect.NewRequest(&dispatchv1.DeleteSectionRequest{Id: id})); err != nil {
		t.Fatal(err)
	}
	listed, err = core.ListSections(ctx, connect.NewRequest(&dispatchv1.ListSectionsRequest{}))
	if err != nil || len(listed.Msg.Sections) != 0 {
		t.Fatalf("deleted=%+v error=%v", listed, err)
	}
	_, err = core.SaveSection(ctx, connect.NewRequest(&dispatchv1.SaveSectionRequest{Name: " "}))
	if connect.CodeOf(err) != connect.CodeInvalidArgument {
		t.Fatalf("name: %v", err)
	}
	_, err = core.SaveSection(ctx, connect.NewRequest(&dispatchv1.SaveSectionRequest{Id: "missing", Name: "Section"}))
	if connect.CodeOf(err) != connect.CodeNotFound {
		t.Fatalf("unknown: %v", err)
	}
	_, err = core.DeleteSection(ctx, connect.NewRequest(&dispatchv1.DeleteSectionRequest{Id: "missing"}))
	if connect.CodeOf(err) != connect.CodeNotFound {
		t.Fatalf("delete unknown: %v", err)
	}
	_, err = core.MoveToSection(ctx, connect.NewRequest(&dispatchv1.MoveToSectionRequest{Kind: "invalid", Ref: "ref"}))
	if connect.CodeOf(err) != connect.CodeInvalidArgument {
		t.Fatalf("kind: %v", err)
	}
	_, err = core.MoveToSection(ctx, connect.NewRequest(&dispatchv1.MoveToSectionRequest{Kind: "run", Ref: "ref", SectionId: "missing"}))
	if connect.CodeOf(err) != connect.CodeNotFound {
		t.Fatalf("target: %v", err)
	}
}

func TestStartRunRecordsCallingConversation(t *testing.T) {
	t.Setenv("DISPATCH_DEPTH", "")
	t.Setenv("DISPATCH_NO_DELEGATE", "")
	core := sectionTestCore(t)
	dir := gitRepo(t)
	onlyGit(t)
	ctx := context.Background()
	if err := core.Store.SaveWorkspace(sqlite.Workspace{ID: "workspace", Name: "Workspace", Path: dir}); err != nil {
		t.Fatal(err)
	}
	res, err := core.StartRun(ctx, connect.NewRequest(&dispatchv1.StartRunRequest{
		ProjectId: "workspace", Tier: "easy", Prompt: "Read the repository", ReadOnly: true,
		Caller: &dispatchv1.DispatchCaller{Agent: "codex", SessionId: "chat", ConversationId: "codex:chat", Label: "Codex"},
	}))
	if err != nil {
		t.Fatal(err)
	}
	if res.Msg.Run.ConversationId != "codex:chat" || res.Msg.Run.CallerAgent != "codex" {
		t.Fatalf("response=%+v", res.Msg.Run)
	}
	// Wait for the async execution to finish before closing the store. No agent
	// CLI is on PATH, so this run fails without executing a real worker.
	deadline := time.Now().Add(10 * time.Second)
	for time.Now().Before(deadline) {
		run, err := core.Store.GetRun(ctx, res.Msg.Run.Id)
		if err != nil {
			t.Fatal(err)
		}
		if run.ConversationID != "codex:chat" || run.CallerAgent != "codex" || run.SessionID != "" {
			t.Fatalf("stored=%+v", run)
		}
		if isTerminalDispatchStatus(run.Status) {
			// Synchronize with finalization, which also holds this mutex.
			mu := core.dispatchRunMu(run.ID)
			mu.Lock()
			mu.Unlock()
			return
		}
		time.Sleep(20 * time.Millisecond)
	}
	t.Fatal("run did not finish")
}

func TestListRunsFiltersConversationBeforePagination(t *testing.T) {
	core := sectionTestCore(t)
	ctx := context.Background()
	for i, id := range []string{"first", "other", "last"} {
		conversation := "codex:chat"
		if id == "other" {
			conversation = "claude:chat"
		}
		if err := core.Store.SaveRun(ctx, dispatch.Run{ID: id, ProjectID: "workspace", ConversationID: conversation, CallerAgent: "codex", CreatedAt: time.Now().UTC().Add(time.Duration(i) * time.Second)}); err != nil {
			t.Fatal(err)
		}
	}
	res, err := core.ListRuns(ctx, connect.NewRequest(&dispatchv1.ListRunsRequest{ProjectId: "workspace", ConversationId: "codex:chat", Limit: 1, Offset: 1}))
	if err != nil || len(res.Msg.Runs) != 1 || res.Msg.Runs[0].Id != "first" || res.Msg.Runs[0].ConversationId != "codex:chat" || res.Msg.Runs[0].CallerAgent != "codex" {
		t.Fatalf("runs=%+v error=%v", res, err)
	}
}

func TestListRunsByConversationSpansWorkspaces(t *testing.T) {
	core := sectionTestCore(t)
	ctx := context.Background()
	for i, project := range []string{"workspace", "other-workspace"} {
		if err := core.Store.SaveRun(ctx, dispatch.Run{ID: project, ProjectID: project, ConversationID: "opencode:chat", CallerAgent: "opencode", CreatedAt: time.Now().UTC().Add(time.Duration(i) * time.Second)}); err != nil {
			t.Fatal(err)
		}
	}
	res, err := core.ListRuns(ctx, connect.NewRequest(&dispatchv1.ListRunsRequest{ConversationId: "opencode:chat"}))
	if err != nil || len(res.Msg.Runs) != 2 {
		t.Fatalf("runs=%+v error=%v", res, err)
	}
	if _, err := core.ListRuns(ctx, connect.NewRequest(&dispatchv1.ListRunsRequest{})); err == nil {
		t.Fatal("listing every workspace with no conversation was accepted")
	}
}
