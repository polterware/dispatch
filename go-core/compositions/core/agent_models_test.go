package core

import (
	"context"
	"errors"
	"sync/atomic"
	"testing"
	"time"

	"github.com/polterware/dispatch/go-core/providers/adapters"
)

// listingAgent counts how often its model list is asked and answers with err when set.
type listingAgent struct {
	name  string
	asked *atomic.Int32
	err   error
}

func (a listingAgent) Name() string                        { return a.name }
func (a listingAgent) Binary() string                      { return a.name }
func (a listingAgent) Capabilities() adapters.Capabilities { return adapters.Capabilities{} }
func (a listingAgent) Quota(context.Context) (adapters.Quota, error) {
	return adapters.Quota{}, adapters.ErrNoQuota
}
func (a listingAgent) Run(context.Context, adapters.Request, string) (adapters.Result, error) {
	return adapters.Result{}, adapters.ErrNotImplemented
}
func (a listingAgent) Models(context.Context) ([]adapters.Model, error) {
	n := a.asked.Add(1)
	if a.err != nil {
		return nil, a.err
	}
	return []adapters.Model{{ID: "m-" + string(rune('0'+n))}}, nil
}

func forgetAgentModels(name string) {
	agentModelsCache.Lock()
	delete(agentModelsCache.entries, name)
	agentModelsCache.Unlock()
}

func TestAgentModelsAnswerAtOnceAndRefreshBehind(t *testing.T) {
	core := &Core{DBDir: t.TempDir()}
	ctx := context.Background()
	a := listingAgent{name: "lister-" + t.Name(), asked: &atomic.Int32{}}
	t.Cleanup(func() { forgetAgentModels(a.name) })

	if m, err := core.agentModels(ctx, a); err != nil || len(m) != 1 || m[0].ID != "m-1" {
		t.Fatalf("%v %v", m, err)
	}
	if m, _ := core.agentModels(ctx, a); m[0].ID != "m-1" || a.asked.Load() != 1 {
		t.Fatalf("a fresh list is not asked again: %v, asked %d", m, a.asked.Load())
	}

	// A stale list answers at once while the CLI is asked again behind it.
	agentModelsCache.Lock()
	entry := agentModelsCache.entries[a.name]
	agentModelsCache.Unlock()
	entry.mu.Lock()
	entry.at = time.Now().Add(-2 * agentModelsGap)
	entry.mu.Unlock()
	if m, _ := core.agentModels(ctx, a); m[0].ID != "m-1" {
		t.Fatalf("the stale list answers: %v", m)
	}
	deadline := time.Now().Add(5 * time.Second)
	for {
		entry.mu.Lock()
		done := !entry.refreshing
		entry.mu.Unlock()
		if done || time.Now().After(deadline) {
			break
		}
		time.Sleep(10 * time.Millisecond)
	}
	if m, _ := core.agentModels(ctx, a); m[0].ID != "m-2" || a.asked.Load() != 2 {
		t.Fatalf("the refreshed list: %v, asked %d", m, a.asked.Load())
	}

	// A core that starts again answers with the list the last one kept.
	forgetAgentModels(a.name)
	again := &Core{DBDir: core.DBDir}
	if m, err := again.agentModels(ctx, a); err != nil || m[0].ID != "m-2" || a.asked.Load() != 2 {
		t.Fatalf("the kept list, without asking: %v %v, asked %d", m, err, a.asked.Load())
	}
}

func TestAgentModelsKeepAFailure(t *testing.T) {
	core := &Core{DBDir: t.TempDir()}
	ctx := context.Background()
	broken := errors.New("not signed in")
	a := listingAgent{name: "broken-" + t.Name(), asked: &atomic.Int32{}, err: broken}
	t.Cleanup(func() { forgetAgentModels(a.name) })
	for range 2 {
		if _, err := core.agentModels(ctx, a); !errors.Is(err, broken) {
			t.Fatalf("%v", err)
		}
	}
	if a.asked.Load() != 1 {
		t.Fatalf("an agent that cannot list holds up one read, not every one: asked %d", a.asked.Load())
	}

	canceled, cancel := context.WithCancel(ctx)
	cancel()
	b := listingAgent{name: "left-" + t.Name(), asked: &atomic.Int32{}, err: context.Canceled}
	t.Cleanup(func() { forgetAgentModels(b.name) })
	_, _ = core.agentModels(canceled, b)
	_, _ = core.agentModels(ctx, b)
	if b.asked.Load() != 2 {
		t.Fatalf("a caller that left keeps nothing: asked %d", b.asked.Load())
	}
}
