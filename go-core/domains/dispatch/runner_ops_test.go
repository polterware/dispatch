package dispatch

import (
	"context"
	"os"
	"path/filepath"
	"testing"

	"github.com/polterware/dispatch/go-core/providers/worktree"
)

func completedWorktree(t *testing.T) (Deps, Run) {
	t.Helper()
	var ran []string
	d := testDeps(t, &ran)
	root := t.TempDir()
	testLifecycleGit(t, root, "init", "-q")
	testLifecycleGit(t, root, "commit", "--allow-empty", "-qm", "base")
	wt := filepath.Join(t.TempDir(), "worktree")
	branch, err := worktree.Create(root, wt, "merge-test")
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(wt, "result.txt"), []byte("result\n"), 0600); err != nil {
		t.Fatal(err)
	}
	testLifecycleGit(t, wt, "add", "result.txt")
	testLifecycleGit(t, wt, "commit", "-qm", "result")
	rec := Run{ID: "merge-test", Status: StatusCompleted, Repo: root, Worktree: wt, Branch: branch}
	if err := d.Store.SaveRun(context.Background(), rec); err != nil {
		t.Fatal(err)
	}
	return d, rec
}

func assertWorktreeStatus(t *testing.T, d Deps, id, want string) {
	t.Helper()
	trees, err := d.Store.ListWorktrees(context.Background(), "")
	if err != nil {
		t.Fatal(err)
	}
	for _, tree := range trees {
		if tree.RunID == id {
			if tree.Status != want {
				t.Fatalf("worktree status = %q, want %q", tree.Status, want)
			}
			return
		}
	}
	t.Fatal("missing worktree record")
}

func TestMergeWorktreeLifecycle(t *testing.T) {
	for _, keep := range []bool{false, true} {
		name := "remove"
		if keep {
			name = "keep"
		}
		t.Run(name, func(t *testing.T) {
			d, rec := completedWorktree(t)
			ctx := context.Background()
			res, err := d.Merge(ctx, rec.ID, keep)
			if err != nil || !res.OK || res.Cleaned == keep {
				t.Fatalf("merge = %+v, %v", res, err)
			}
			saved, err := d.Store.GetRun(ctx, rec.ID)
			if err != nil || saved.MergedAt == nil || saved.MergeSHA == "" {
				t.Fatalf("saved run = %+v, %v", saved, err)
			}
			if b, err := os.ReadFile(filepath.Join(rec.Repo, "result.txt")); err != nil || string(b) != "result\n" {
				t.Fatalf("merged result = %q, %v", b, err)
			}
			if keep {
				assertWorktreeStatus(t, d, rec.ID, WorktreeMerged)
				if _, err := os.Stat(rec.Worktree); err != nil {
					t.Fatal(err)
				}
				if _, err := d.Clean(ctx, rec.ID, false); err != nil {
					t.Fatal(err)
				}
			}
			assertWorktreeStatus(t, d, rec.ID, WorktreeRemoved)
			if _, err := os.Stat(rec.Worktree); !os.IsNotExist(err) {
				t.Fatalf("worktree still exists: %v", err)
			}
			testLifecycleGit(t, rec.Repo, "merge-base", "--is-ancestor", saved.MergeSHA, "HEAD")
		})
	}
}

func TestCleanUnmergedWorktree(t *testing.T) {
	d, rec := completedWorktree(t)
	ctx := context.Background()
	if _, err := d.Clean(ctx, rec.ID, true); err != nil {
		t.Fatal(err)
	}
	assertWorktreeStatus(t, d, rec.ID, WorktreeAbandoned)
	if _, err := os.Stat(rec.Worktree); !os.IsNotExist(err) {
		t.Fatalf("worktree still exists: %v", err)
	}
}
