package sqlite_test

import (
	"context"
	"testing"

	"github.com/polterware/dispatch/go-core/domains/dispatch"
)

func TestRunReadsStoredWorktreeStatus(t *testing.T) {
	store := newStore(t)
	ctx := context.Background()
	if err := store.SaveRun(ctx, dispatch.Run{ID: "run", ProjectID: "workspace"}); err != nil {
		t.Fatal(err)
	}
	for _, status := range []string{"", dispatch.WorktreeMerged, dispatch.WorktreeRemoved} {
		if status != "" {
			if err := store.SaveWorktree(ctx, dispatch.WorktreeInfo{ID: "tree", RunID: "run", ProjectID: "workspace", Status: status}); err != nil {
				t.Fatal(err)
			}
		}
		got, err := store.GetRun(ctx, "run")
		if err != nil || got.WorktreeStatus != status {
			t.Fatalf("get status = %q, %v; want %q", got.WorktreeStatus, err, status)
		}
		runs, err := store.ListRuns(ctx, "workspace", 0, 0)
		if err != nil || len(runs) != 1 {
			t.Fatalf("list runs = %+v, %v", runs, err)
		}
		if runs[0].WorktreeStatus != status {
			t.Fatalf("list status = %q, want %q", runs[0].WorktreeStatus, status)
		}
	}
}
