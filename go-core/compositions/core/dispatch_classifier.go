package core

import (
	"time"

	"github.com/polterware/dispatch/go-core/domains/dispatch"
	"github.com/polterware/dispatch/go-core/providers/adapters"
	"github.com/polterware/dispatch/go-core/providers/classifier"
)

// cliClassifyTimeout bounds the agent CLI classifier's one read-only call.
const cliClassifyTimeout = 60 * time.Second

// dispatchClassifier is the policy's difficulty classifier, or nil when it is
// off. The CLI classifier runs the easy tier's first installed agent.
func dispatchClassifier(deps dispatch.Deps) dispatch.Classifier {
	model := deps.Policy.ClassifierModel
	switch deps.Policy.Classifier {
	case dispatch.ClassifierJev:
		return classifier.Jev{Model: model}
	case dispatch.ClassifierOpenAI:
		return classifier.OpenAI{Model: model}
	case dispatch.ClassifierCLI:
		for _, e := range deps.Table.Tiers[adapters.TierEasy] {
			for _, a := range deps.Adapters {
				if a.Name() != e.Agent || !deps.IsInstalled(a.Binary()) {
					continue
				}
				if model == "" {
					model = e.Model
				}
				return classifier.NewCLI(a, model, cliClassifyTimeout)
			}
		}
		return classifier.CLI{}
	}
	return nil
}

// classifierCLI is the agent and model the CLI classifier asks, whether or not
// it is the policy's; empty with no easy-tier agent installed.
func classifierCLI(deps dispatch.Deps) classifier.CLI {
	deps.Policy.Classifier = dispatch.ClassifierCLI
	cli, _ := dispatchClassifier(deps).(classifier.CLI)
	return cli
}

// classifierKey is the API key a classifier reads, for those that need one.
func classifierKey(name string) (classifier.Key, bool) {
	switch name {
	case dispatch.ClassifierJev:
		return classifier.JevKey, true
	case dispatch.ClassifierOpenAI:
		return classifier.OpenAIKey, true
	}
	return classifier.Key{}, false
}

// classifierKeyStatus says where the policy's classifier finds its key:
// env, keychain, missing, or none for a classifier that needs no key.
func classifierKeyStatus(name string) string {
	if k, ok := classifierKey(name); ok {
		return k.Status()
	}
	return "none"
}
