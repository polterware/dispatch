package dispatch

import (
	"context"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"strconv"
	"strings"
	"syscall"
	"time"

	"github.com/polterware/dispatch/go-core/providers/process"
	"github.com/polterware/dispatch/go-core/providers/worktree"
)

func processStart(ctx context.Context, pid int) string {
	out, err := exec.CommandContext(ctx, "ps", "-o", "lstart=", "-p", strconv.Itoa(pid)).Output()
	if err != nil {
		return ""
	}
	return strings.TrimSpace(string(out))
}

func pidAlive(pid int) bool {
	if pid == 0 {
		return false
	}
	err := syscall.Kill(pid, 0)
	return err == nil || errors.Is(err, syscall.EPERM)
}

// Reconcile preserves orphaned work so it can be inspected or recovered.
func (d Deps) Reconcile(ctx context.Context) error {
	if d.Store == nil {
		return nil
	}
	recs, err := d.Store.ListRuns(ctx, "", 0, 0)
	if err != nil {
		return err
	}
	for _, rec := range recs {
		if rec.Status != StatusPending && rec.Status != StatusRunning {
			continue
		}
		if err := ctx.Err(); err != nil {
			return err
		}
		note := "runner process exited unexpectedly"
		if rec.PID > 0 && pidAlive(rec.PID) {
			if rec.ProcStart == "" {
				continue
			} // Legacy records have no identity.
			start := processStart(ctx, rec.PID)
			if err := ctx.Err(); err != nil {
				return err
			}
			if start == "" && pidAlive(rec.PID) {
				return fmt.Errorf("cannot verify process identity for run %s", rec.ID)
			}
			if start == rec.ProcStart {
				continue
			}
			note = "runner process identity changed (PID reused)"
		}
		// Avoid replacing a final record written while ps was running.
		latest, err := d.Store.GetRun(ctx, rec.ID)
		if err != nil {
			return err
		}
		if latest.Status != rec.Status || latest.PID != rec.PID || latest.ProcStart != rec.ProcStart {
			continue
		}
		rec = latest
		if rec.AgentPID > 0 && (pidAlive(rec.AgentPID) || pidAlive(-rec.AgentPID)) {
			tree, err := process.Capture(rec.AgentPID)
			if err != nil {
				return err
			}
			if err := tree.Stop(ctx, 5*time.Second); err != nil {
				return err
			}
		}
		rec.Status, rec.Note, rec.FinishedAt = StatusOrphaned, joinNote(rec.Note, note), d.now()
		rec.ExitCode = 1
		completeTelemetry(&rec, rec.FinishedAt)
		if err := d.Store.SaveRun(ctx, rec); err != nil {
			return err
		}
	}
	return nil
}

// Cancel waits for both the owner and the agent group before recording success.
func (d Deps) Cancel(ctx context.Context, id string) (Run, error) {
	if err := d.Reconcile(ctx); err != nil {
		return Run{}, err
	}
	rec, err := d.Store.GetRun(ctx, id)
	if err != nil {
		return rec, err
	}
	orphan := rec.Status == StatusOrphaned
	if terminalStatus(rec.Status) && !orphan {
		return rec, fmt.Errorf("run %s already %s", rec.ID, rec.Status)
	}
	ownerPID := rec.PID
	if orphan {
		ownerPID = 0
	}
	if ownerPID <= 0 && rec.AgentPID <= 0 {
		return rec, nil
	}
	tree, err := process.Capture(rec.AgentPID)
	if err != nil {
		return rec, err
	}
	if err := tree.Include(ownerPID, rec.ProcStart); err != nil {
		return rec, err
	}
	if err := tree.Stop(ctx, 5*time.Second); err != nil {
		return rec, err
	}
	if latest, err := d.Store.GetRun(ctx, id); err != nil {
		return rec, err
	} else {
		rec = latest
	}
	if orphan {
		return rec, nil
	}
	rec.Status, rec.Error, rec.FinishedAt = StatusCancelled, "cancelled", d.now()
	completeTelemetry(&rec, rec.FinishedAt)
	return rec, d.Store.SaveRun(ctx, rec)
}

// Recover commits preserved work, without claiming that it was verified.
func (d Deps) Recover(ctx context.Context, id string) (Run, string, error) {
	rec, err := d.Store.GetRun(ctx, id)
	if err != nil {
		return rec, "", err
	}
	if rec.Status != StatusOrphaned {
		return rec, "", fmt.Errorf("run %s is %s; recovery requires ORPHANED", rec.ID, rec.Status)
	}
	if rec.Worktree == "" {
		return rec, "", fmt.Errorf("run %s has no worktree", rec.ID)
	}
	if info, err := os.Stat(rec.Worktree); err != nil || !info.IsDir() {
		return rec, "", fmt.Errorf("worktree %s does not exist", rec.Worktree)
	}
	if err := ctx.Err(); err != nil {
		return rec, "", err
	}
	if rec.AgentPID > 0 && pidAlive(-rec.AgentPID) {
		return rec, "", fmt.Errorf("agent group still exists; cancel run %s before recovery", rec.ID)
	}
	stat, err := worktree.Recover(rec.Worktree, rec.Branch, rec.BaseSHA, rec.ID)
	if err != nil {
		return rec, "", err
	}
	rec.Status, rec.Note, rec.FinishedAt = StatusCompleted, "recovered from orphan; unverified", d.now()
	rec.Error, rec.ExitCode = "", 0
	completeTelemetry(&rec, rec.FinishedAt)
	return rec, stat, d.Store.SaveRun(ctx, rec)
}
