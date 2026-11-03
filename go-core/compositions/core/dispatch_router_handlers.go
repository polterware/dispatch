package core

import (
	"context"
	"strings"

	"connectrpc.com/connect"

	"github.com/polterware/dispatch/go-core/domains/dispatch"
	dispatchv1 "github.com/polterware/dispatch/go-core/platform/gen/dispatch/v1"
	"github.com/polterware/dispatch/go-core/providers/adapters"
)

func (core *Core) GetRouteDecision(ctx context.Context, request *connect.Request[dispatchv1.GetRouteDecisionRequest]) (*connect.Response[dispatchv1.GetRouteDecisionResponse], error) {
	id := strings.TrimSpace(request.Msg.GetRunId())
	if id == "" {
		return nil, invalid("run_id is required")
	}
	dec, ok, err := core.Store.GetRouteDecision(ctx, id)
	if err != nil {
		return nil, internal(err)
	}
	if !ok {
		return connect.NewResponse(&dispatchv1.GetRouteDecisionResponse{Ok: false, Json: "{}"}), nil
	}
	out := map[string]any{
		"runId": dec.RunID, "policy": dec.PolicyVersion, "route": dec.Route, "priority": dec.Priority,
		"difficulty": dec.Difficulty, "tier": decisionBand(dec.Difficulty), "area": dec.Area,
		"classifier": dec.Classifier, "probs": dec.Probs,
		"candidates": dec.Candidates, "chosen": dec.Chosen,
		"createdAt": formatDispatchTime(dec.CreatedAt),
	}
	// The run keeps its role and how it was picked; the decision log does not.
	if run, err := core.Store.GetRun(ctx, id); err == nil {
		if run.Profile != "" {
			out["role"] = run.Profile
		}
		if run.RolePick != nil {
			out["rolePick"] = run.RolePick
		}
	}
	body, err := jsonBody(out)
	if err != nil {
		return nil, err
	}
	return connect.NewResponse(&dispatchv1.GetRouteDecisionResponse{Ok: true, Json: body}), nil
}

func decisionBand(d *float64) string {
	if d == nil {
		return ""
	}
	return string(dispatch.DifficultyTier(*d))
}

// BuildTiers proposes a tier table from the model catalog over the stored
// table and every model the installed agent CLIs report, and stores it when
// asked, validated like ReplaceTiers. With an agent it ranks only that agent's
// entries and models, and applying replaces only that agent's entries.
func (core *Core) BuildTiers(ctx context.Context, request *connect.Request[dispatchv1.BuildTiersRequest]) (*connect.Response[dispatchv1.BuildTiersResponse], error) {
	table, err := dispatch.LoadTiersFromStore(ctx, core.Store)
	if err != nil {
		return nil, internal(err)
	}
	agent := strings.TrimSpace(request.Msg.GetAgent())
	if agent != "" && !knownAgent(agent) {
		return nil, invalid("unknown agent " + agent)
	}
	var entries, models []dispatch.Entry
	for _, tier := range []adapters.Tier{adapters.TierEasy, adapters.TierNormal, adapters.TierHard} {
		entries = append(entries, table.Tiers[tier]...)
	}
	listed := core.listModels(ctx)
	for _, m := range listed.GetModels() {
		models = append(models, dispatch.Entry{Agent: m.GetAgent(), Model: m.GetModel(), Provider: m.GetProvider()})
	}
	entries = append(entries, models...)
	if agent != "" {
		entries = dispatch.AgentPool(agent, table, models)
	}
	priority := dispatch.PriorityBalanced
	if policy, err := dispatch.LoadAllConfig(dispatch.PolicyConfigPath(core.dispatchStateDir()), ""); err == nil && policy.Priority != "" {
		priority = policy.Priority
	}
	built := dispatch.BuildTiers(entries, dispatch.LoadCatalog(core.dispatchStateDir()), priority)
	out := map[string]any{"priority": priority, "agent": agent, "tiers": built.Tiers, "problems": listed.GetProblems(), "applied": false}
	if request.Msg.GetApply() {
		if agent != "" {
			built.Table = dispatch.MergeAgent(table, built.Table, agent)
		}
		validated, err := built.Table.Validated()
		if err != nil {
			return nil, invalid(err.Error())
		}
		if err := core.Store.ReplaceTiers(ctx, validated.Rows()); err != nil {
			return nil, internal(err)
		}
		out["applied"] = true
	}
	body, err := jsonBody(out)
	if err != nil {
		return nil, err
	}
	return connect.NewResponse(&dispatchv1.BuildTiersResponse{Ok: true, Json: body}), nil
}

func knownAgent(name string) bool {
	for _, a := range adapters.Known() {
		if a.Name() == name {
			return true
		}
	}
	return false
}
