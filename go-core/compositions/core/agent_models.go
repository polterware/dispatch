package core

import (
	"context"
	"encoding/json"
	"errors"
	"os"
	"path/filepath"
	"sync"
	"time"

	"github.com/polterware/dispatch/go-core/providers/adapters"
)

// agentModelsGap is how long an agent CLI's model list stays fresh: models change rarely, and
// asking a CLI takes seconds each time Runs, Add model, Stats, or the tier check needs it.
const agentModelsGap = 5 * time.Minute

// agentModelsTimeout bounds a refresh asked in the background, which no caller waits on.
const agentModelsTimeout = 20 * time.Second

// agentModelsFile keeps each agent's last list in the state directory, so a core that starts
// answers with it at once.
const agentModelsFile = "agent-models.json"

type agentModelsEntry struct {
	mu         sync.Mutex
	at         time.Time
	models     []adapters.Model
	err        error
	refreshing bool
}

type savedAgentModels struct {
	At     time.Time        `json:"at"`
	Models []adapters.Model `json:"models"`
}

var agentModelsCache = struct {
	sync.Mutex
	entries map[string]*agentModelsEntry
}{entries: map[string]*agentModelsEntry{}}

// agentModels is the agent's model list. One asked within agentModelsGap is returned as is; an
// older one, or one a core before this one kept on disk, is returned at once while its CLI is
// asked again in the background. Only an agent never asked waits for its CLI, and calls for it
// meanwhile wait for that answer. A failure is kept like a list, so an agent that cannot list
// does not hold up every read; a refresh that fails keeps the list it had.
func (core *Core) agentModels(ctx context.Context, a adapters.Adapter) ([]adapters.Model, error) {
	agentModelsCache.Lock()
	entry := agentModelsCache.entries[a.Name()]
	if entry == nil {
		entry = &agentModelsEntry{}
		if saved, ok := core.savedAgentModels()[a.Name()]; ok {
			entry.at, entry.models = saved.At, saved.Models
		}
		agentModelsCache.entries[a.Name()] = entry
	}
	agentModelsCache.Unlock()

	entry.mu.Lock()
	defer entry.mu.Unlock()
	if !entry.at.IsZero() {
		if time.Since(entry.at) >= agentModelsGap && !entry.refreshing {
			entry.refreshing = true
			go core.refreshAgentModels(entry, a)
		}
		return entry.models, entry.err
	}
	models, err := a.Models(ctx)
	if errors.Is(err, context.Canceled) {
		return nil, err // the caller left; the next one asks again
	}
	entry.at, entry.models, entry.err = time.Now(), models, err
	if err == nil {
		core.saveAgentModels(a.Name(), entry.at, models)
	}
	return models, err
}

func (core *Core) refreshAgentModels(entry *agentModelsEntry, a adapters.Adapter) {
	ctx, cancel := context.WithTimeout(context.Background(), agentModelsTimeout)
	defer cancel()
	models, err := a.Models(ctx)
	entry.mu.Lock()
	defer entry.mu.Unlock()
	entry.refreshing = false
	entry.at = time.Now()
	if err != nil {
		if entry.models == nil {
			entry.err = err
		}
		return
	}
	entry.models, entry.err = models, nil
	core.saveAgentModels(a.Name(), entry.at, models)
}

func (core *Core) savedAgentModels() map[string]savedAgentModels {
	saved := map[string]savedAgentModels{}
	if b, err := os.ReadFile(filepath.Join(core.dispatchStateDir(), agentModelsFile)); err == nil {
		_ = json.Unmarshal(b, &saved)
	}
	return saved
}

// agentModelsFileMu keeps two agents' saves from losing each other's list.
var agentModelsFileMu sync.Mutex

func (core *Core) saveAgentModels(agent string, at time.Time, models []adapters.Model) {
	agentModelsFileMu.Lock()
	defer agentModelsFileMu.Unlock()
	saved := core.savedAgentModels()
	saved[agent] = savedAgentModels{At: at, Models: models}
	b, err := json.Marshal(saved)
	if err != nil {
		return
	}
	dir := core.dispatchStateDir()
	if os.MkdirAll(dir, 0o755) == nil {
		_ = os.WriteFile(filepath.Join(dir, agentModelsFile), b, 0o600)
	}
}
