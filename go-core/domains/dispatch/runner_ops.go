package dispatch

import (
	"context"
	"fmt"
	"strings"

	"github.com/polterware/dispatch/go-core/providers/sandbox"
	"github.com/polterware/dispatch/go-core/providers/worktree"
)

// MergeResult is the JSON body of a merge. Status is "merged" or
// "merge_conflict"; OK is true only for the first.
type MergeResult struct {
	OK        bool     `json:"ok"`
	ID        string   `json:"id"`
	Status    string   `json:"status"`
	Merged    string   `json:"merged,omitempty"`
	Git       string   `json:"git,omitempty"`
	Cleaned   bool     `json:"cleaned,omitempty"`
	Note      string   `json:"note,omitempty"`
	Conflicts []string `json:"conflicts,omitempty"`
	Hint      string   `json:"hint,omitempty"`
	Revert    string   `json:"revert,omitempty"`
}

// Merge integrates a COMPLETED run's worktree branch into the repository's
// current branch. It checks for conflicts before touching the working tree. A
// refusal (wrong status, uncommitted overlap) is an error; a conflict is a
// result with OK false.
func (d Deps) Merge(ctx context.Context, id string, keep bool) (MergeResult, error) {
	rec, err := d.Store.GetRun(ctx, id)
	if err != nil {
		return MergeResult{}, err
	}
	res := MergeResult{ID: rec.ID}
	if rec.Branch == "" || rec.Repo == "" {
		return res, fmt.Errorf("run %s has no worktree branch to merge", rec.ID)
	}
	if rec.Status != StatusCompleted {
		return res, fmt.Errorf("run %s is %s; only COMPLETED runs can be merged", rec.ID, rec.Status)
	}

	target, _ := worktree.CurrentBranch(rec.Repo)
	unlock, err := worktree.LockMerge(rec.Repo, target)
	if err != nil {
		return res, redactErr(err)
	}
	defer unlock()

	overlapping, err := worktree.UncommittedOverlaps(rec.Repo, rec.Branch)
	if err != nil {
		return res, redactErr(err)
	}
	if len(overlapping) > 0 {
		return res, fmt.Errorf("working tree has uncommitted changes in files touched by %s: %s; commit or stash them before merging", rec.Branch, strings.Join(overlapping, ", "))
	}

	if worktree.SupportsMergeTree(rec.Repo) {
		tree, err := worktree.MergeTree(rec.Repo, "HEAD", rec.Branch)
		if err != nil {
			return res, redactErr(err)
		}
		if !tree.Clean {
			rec.ExitClass = "merge_conflict"
			if err := d.Store.SaveRun(ctx, rec); err != nil {
				return res, err
			}
			res.Status, res.Conflicts = "merge_conflict", tree.Conflicts
			res.Hint = fmt.Sprintf("ask the agent to rebase branch %s, or resolve the conflicts by hand", rec.Branch)
			return res, nil
		}
	} else {
		res.Note = "git does not support merge-tree --write-tree; skipped the pre-merge conflict check"
	}

	out, err := worktree.Merge(rec.Repo, rec.Branch, rec.ID)
	if err != nil {
		return res, redactErr(err)
	}
	mergeSHA, _ := worktree.Head(rec.Repo)
	res.Merged, res.Git = rec.Branch, out
	if !keep {
		res.Cleaned = worktree.Remove(rec.Repo, rec.Worktree, rec.Branch, false) == nil
	}
	res.OK, res.Status, res.Revert = true, "merged", "git revert -m 1 "+mergeSHA

	now := d.now()
	rec.MergedAt, rec.MergeSHA = &now, mergeSHA
	if err := d.Store.SaveRun(ctx, rec); err != nil {
		return res, err
	}
	_ = d.Store.AppendEvent(ctx, RunEvent{
		RunID: rec.ID, ProjectID: rec.ProjectID, Kind: EventMerged,
		Agent: rec.Agent, Tier: rec.Tier, Text: rec.Branch, At: now,
	})
	status := WorktreeMerged
	if res.Cleaned {
		status = WorktreeRemoved
	}
	_ = d.Store.SaveWorktree(ctx, WorktreeInfo{
		ID: rec.ID, ProjectID: rec.ProjectID, RunID: rec.ID, CardSlug: rec.CardSlug,
		Path: rec.Worktree, Branch: rec.Branch, BaseSHA: rec.BaseSHA,
		Status: status, Owner: rec.Agent,
	})
	return res, nil
}

func redactErr(err error) error { return fmt.Errorf("%s", sandbox.Redact(err.Error())) }

// Clean discards a run's worktree and branch. Without force it refuses a
// branch that is not merged.
func (d Deps) Clean(ctx context.Context, id string, force bool) (Run, error) {
	rec, err := d.Store.GetRun(ctx, id)
	if err != nil {
		return rec, err
	}
	if rec.Worktree == "" {
		return rec, fmt.Errorf("run %s has no worktree", rec.ID)
	}
	if err := worktree.Remove(rec.Repo, rec.Worktree, rec.Branch, force); err != nil {
		return rec, redactErr(err)
	}
	status := WorktreeAbandoned
	if rec.MergedAt != nil {
		status = WorktreeRemoved
	}
	_ = d.Store.SaveWorktree(ctx, WorktreeInfo{
		ID: rec.ID, ProjectID: rec.ProjectID, RunID: rec.ID, CardSlug: rec.CardSlug,
		Path: rec.Worktree, Branch: rec.Branch, BaseSHA: rec.BaseSHA,
		Status: status, Owner: rec.Agent,
	})
	return rec, nil
}
