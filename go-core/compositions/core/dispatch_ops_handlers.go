package core

import (
	"context"
	"database/sql"
	"encoding/json"
	"errors"
	"fmt"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"slices"
	"strings"
	"time"

	"connectrpc.com/connect"

	"github.com/polterware/dispatch/go-core/domains/dispatch"
	dispatchv1 "github.com/polterware/dispatch/go-core/platform/gen/dispatch/v1"
	"github.com/polterware/dispatch/go-core/providers/adapters"
	"github.com/polterware/dispatch/go-core/providers/sandbox"
	"github.com/polterware/dispatch/go-core/providers/setup"
	"github.com/polterware/dispatch/go-core/providers/skill"
)

// dispatchDepsIn is dispatchDeps with the global and project routing policy
// loaded. An empty dir loads the global policy only.
func (core *Core) dispatchDepsIn(ctx context.Context, dir string) (dispatch.Deps, error) {
	deps, err := core.dispatchDeps(ctx)
	if err != nil {
		return deps, err
	}
	project := ""
	if dir != "" {
		project = filepath.Join(dir, ".dispatch", "config.json")
	}
	policy, err := dispatch.LoadAllConfig(dispatch.PolicyConfigPath(core.dispatchStateDir()), project)
	if err != nil {
		return deps, err
	}
	deps.Policy = policy
	deps.Classifier = dispatchClassifier(deps)
	if deps.Profiles, err = core.dispatchProfiles(dir); err != nil {
		return deps, err
	}
	return deps, nil
}

func jsonBody(v any) (string, error) {
	b, err := json.Marshal(v)
	if err != nil {
		return "", internal(err)
	}
	return string(b), nil
}

func dispatchOpError(err error) error {
	if errors.Is(err, dispatch.ErrUnknownRun) || errors.Is(err, sql.ErrNoRows) {
		return connect.NewError(connect.CodeNotFound, err)
	}
	return invalid(sandbox.Redact(err.Error()))
}

func requiredRunID(id string) (string, error) {
	id = strings.TrimSpace(id)
	if id == "" {
		return "", invalid("run id is required")
	}
	return id, nil
}

func (core *Core) Route(ctx context.Context, request *connect.Request[dispatchv1.RouteRequest]) (*connect.Response[dispatchv1.RouteResponse], error) {
	m := request.Msg
	dir := strings.TrimSpace(m.GetDir())
	deps, err := core.dispatchDepsIn(ctx, dir)
	if err != nil {
		return nil, internal(err)
	}
	hints := core.runHints(ctx, m.GetProjectId(), m.GetCardSlug(), dir)
	opts := dispatch.Options{
		Tier:     adapters.Tier(strings.TrimSpace(m.GetTier())),
		ReadOnly: m.GetReadOnly(),
		Dir:      dir,
		Prompt:   strings.TrimSpace(m.GetPrompt()),
		Area:     runArea(hints),
		CardText: hints.CardText,
		Filters: dispatch.Filters{
			Provider: strings.TrimSpace(m.GetProvider()),
			Model:    strings.TrimSpace(m.GetModel()),
			Effort:   strings.TrimSpace(m.GetEffort()),
			Agent:    strings.TrimSpace(m.GetAgent()),
			Priority: strings.TrimSpace(m.GetPriority()),
			Relax:    m.GetRelax(),
		},
	}
	if name := strings.TrimSpace(m.GetProfile()); name != "" {
		if err := core.applyDispatchProfile(&opts, name); err != nil {
			return nil, invalid(err.Error())
		}
	}
	report, err := deps.Route(ctx, opts)
	if err != nil {
		return nil, invalid(err.Error())
	}
	body, err := jsonBody(report)
	if err != nil {
		return nil, err
	}
	return connect.NewResponse(&dispatchv1.RouteResponse{Ok: len(report.Candidates) > 0, Json: body}), nil
}

// applyDispatchProfile resolves a named role for opts.Dir; none runs with no
// role and keeps the classifier from picking one.
func (core *Core) applyDispatchProfile(opts *dispatch.Options, name string) error {
	if name == "none" {
		opts.NoProfile = true
		return nil
	}
	profiles, err := core.dispatchProfiles(opts.Dir)
	if err != nil {
		return err
	}
	p, ok := profiles[name]
	if !ok {
		return fmt.Errorf("unknown role %q (have: %s)", name, strings.Join(dispatch.ProfileNames(profiles), ", "))
	}
	return dispatch.ApplyProfile(opts, name, p)
}

func (core *Core) dispatchProfiles(dir string) (map[string]dispatch.Profile, error) {
	project := ""
	if dir != "" {
		project = dispatch.ProjectProfilesPath(dir)
	}
	return dispatch.LoadAllProfiles(dispatch.GlobalProfilesPath(core.dispatchStateDir()), project)
}

func (core *Core) Classify(ctx context.Context, request *connect.Request[dispatchv1.ClassifyRequest]) (*connect.Response[dispatchv1.ClassifyResponse], error) {
	m := request.Msg
	prompt := strings.TrimSpace(m.GetPrompt())
	if prompt == "" {
		return nil, invalid("prompt is required")
	}
	out := map[string]any{}
	{
		deps, err := core.dispatchDepsIn(ctx, strings.TrimSpace(m.GetDir()))
		if err != nil {
			return nil, internal(err)
		}
		if deps.Classifier == nil {
			return nil, invalid(dispatch.ErrAutoOff.Error())
		}
		out["keyStatus"] = classifierKeyStatus(deps.Policy.Classifier)
		task := dispatch.TaskText(core.runHints(ctx, m.GetProjectId(), m.GetCardSlug(), strings.TrimSpace(m.GetDir())).CardText, prompt)
		cctx, cancel := context.WithTimeout(ctx, 90*time.Second)
		defer cancel()
		var roles map[string]string
		if deps.Policy.Roles {
			roles = dispatch.PickRoles(deps.Profiles)
		}
		diff, err := deps.Classifier.Classify(cctx, dispatch.Task{Text: task, Roles: roles})
		if err != nil {
			return nil, invalid(fmt.Sprintf("classifier %s failed: %s", deps.Classifier.Name(), sandbox.Redact(err.Error())))
		}
		view := map[string]any{
			"provider": deps.Classifier.Name(), "model": diff.Model, "tier": diff.Tier, "d": diff.D,
			"latencyMs": diff.Latency.Milliseconds(),
		}
		if diff.HasProbs {
			view["probs"] = diff.Probs[:]
		}
		if diff.Confidence != nil {
			view["confidence"] = *diff.Confidence
		}
		out["difficulty"] = view
		// role is the pick the classifier's role answer makes: absent when the
		// pick is off or no role is pickable, general when none fits.
		if roles != nil {
			pick := map[string]any{"role": dispatch.GeneralRole, "picked": false}
			if diff.Role != nil {
				role := dispatch.PickRole(*diff.Role, roles)
				pick = map[string]any{"role": role, "picked": role != dispatch.GeneralRole, "probs": diff.Role.Probs, "answer": diff.Role.Role}
				if diff.Role.Confidence != nil {
					pick["confidence"] = *diff.Role.Confidence
				}
			} else {
				pick["unsupported"] = true
			}
			out["role"] = pick
		}
	}
	body, err := jsonBody(out)
	if err != nil {
		return nil, err
	}
	return connect.NewResponse(&dispatchv1.ClassifyResponse{Ok: true, Json: body}), nil
}

func (core *Core) ListModelCatalog(ctx context.Context, _ *connect.Request[dispatchv1.ListModelCatalogRequest]) (*connect.Response[dispatchv1.ListModelCatalogResponse], error) {
	table, err := dispatch.LoadTiersFromStore(ctx, core.Store)
	if err != nil {
		return nil, internal(err)
	}
	dir := core.dispatchStateDir()
	// Every model the installed agent CLIs report is listed; Auto routes to the
	// ones outside the tier table only with discover on.
	var installed []dispatch.Entry
	for _, m := range core.listModels(ctx).GetModels() {
		installed = append(installed, dispatch.Entry{Agent: m.GetAgent(), Model: m.GetModel(), Provider: m.GetProvider()})
	}
	priority, discover := dispatch.PriorityBalanced, false
	if policy, err := dispatch.LoadAllConfig(dispatch.PolicyConfigPath(dir), ""); err == nil {
		if policy.Priority != "" {
			priority = policy.Priority
		}
		discover = policy.Discover
	}
	report := dispatch.ReportCatalog(dispatch.LoadCatalog(dir), table, installed)
	report.Priority, report.Discover = priority, discover
	body, err := jsonBody(report)
	if err != nil {
		return nil, err
	}
	return connect.NewResponse(&dispatchv1.ListModelCatalogResponse{Ok: true, Json: body}), nil
}

// ListModelBenchmarks reads the catalog's benchmarks for every model the installed agent CLIs report.
func (core *Core) ListModelBenchmarks(ctx context.Context, _ *connect.Request[dispatchv1.ListModelBenchmarksRequest]) (*connect.Response[dispatchv1.ListModelBenchmarksResponse], error) {
	var installed []dispatch.Entry
	for _, m := range core.listModels(ctx).GetModels() {
		installed = append(installed, dispatch.Entry{Agent: m.GetAgent(), Model: m.GetModel(), Provider: m.GetProvider()})
	}
	body, err := jsonBody(dispatch.BenchmarkModels(dispatch.LoadCatalog(core.dispatchStateDir()), installed))
	if err != nil {
		return nil, err
	}
	return connect.NewResponse(&dispatchv1.ListModelBenchmarksResponse{Ok: true, Json: body}), nil
}

func (core *Core) RefreshModelCatalog(ctx context.Context, _ *connect.Request[dispatchv1.RefreshModelCatalogRequest]) (*connect.Response[dispatchv1.RefreshModelCatalogResponse], error) {
	out := map[string]any{"source": dispatch.ModelsDevURL}
	n, err := dispatch.RefreshCatalog(ctx, &http.Client{Timeout: 30 * time.Second}, core.dispatchStateDir(), time.Now())
	if err != nil {
		out["error"] = err.Error()
	} else {
		out["models"] = n
	}
	body, jerr := jsonBody(out)
	if jerr != nil {
		return nil, jerr
	}
	return connect.NewResponse(&dispatchv1.RefreshModelCatalogResponse{Ok: err == nil, Json: body}), nil
}

func (core *Core) MergeRun(ctx context.Context, request *connect.Request[dispatchv1.MergeRunRequest]) (*connect.Response[dispatchv1.MergeRunResponse], error) {
	id, err := requiredRunID(request.Msg.GetId())
	if err != nil {
		return nil, err
	}
	deps, err := core.dispatchDepsIn(ctx, "")
	if err != nil {
		return nil, internal(err)
	}
	mu := core.dispatchRunMu(id)
	mu.Lock()
	defer mu.Unlock()
	res, err := deps.Merge(ctx, id, request.Msg.KeepWorktree)
	if err != nil {
		return nil, dispatchOpError(err)
	}
	body, err := jsonBody(res)
	if err != nil {
		return nil, err
	}
	return connect.NewResponse(&dispatchv1.MergeRunResponse{Ok: res.OK, Json: body}), nil
}

func (core *Core) CleanRun(ctx context.Context, request *connect.Request[dispatchv1.CleanRunRequest]) (*connect.Response[dispatchv1.CleanRunResponse], error) {
	id, err := requiredRunID(request.Msg.GetId())
	if err != nil {
		return nil, err
	}
	deps, err := core.dispatchDepsIn(ctx, "")
	if err != nil {
		return nil, internal(err)
	}
	mu := core.dispatchRunMu(id)
	mu.Lock()
	defer mu.Unlock()
	rec, err := deps.Clean(ctx, id, request.Msg.GetForce())
	if err != nil {
		return nil, dispatchOpError(err)
	}
	body, err := jsonBody(map[string]any{"id": rec.ID, "removed": rec.Worktree})
	if err != nil {
		return nil, err
	}
	return connect.NewResponse(&dispatchv1.CleanRunResponse{Ok: true, Json: body}), nil
}

func (core *Core) RecoverRun(ctx context.Context, request *connect.Request[dispatchv1.RecoverRunRequest]) (*connect.Response[dispatchv1.RecoverRunResponse], error) {
	id, err := requiredRunID(request.Msg.GetId())
	if err != nil {
		return nil, err
	}
	deps, err := core.dispatchDepsIn(ctx, "")
	if err != nil {
		return nil, internal(err)
	}
	mu := core.dispatchRunMu(id)
	mu.Lock()
	defer mu.Unlock()
	rec, stat, err := deps.Recover(ctx, id)
	if err != nil {
		return nil, dispatchOpError(err)
	}
	_ = saveRunWorktree(ctx, core.Store, rec)
	body, err := jsonBody(map[string]any{"id": rec.ID, "status": rec.Status, "diffStat": stat})
	if err != nil {
		return nil, err
	}
	return connect.NewResponse(&dispatchv1.RecoverRunResponse{Ok: true, Json: body}), nil
}

func (core *Core) Config(ctx context.Context, request *connect.Request[dispatchv1.ConfigRequest]) (*connect.Response[dispatchv1.ConfigResponse], error) {
	m := request.Msg
	dir := strings.TrimSpace(m.GetDir())
	if m.GetProject() && dir == "" {
		return nil, invalid("a project directory is required for project config")
	}
	global := dispatch.PolicyConfigPath(core.dispatchStateDir())
	projectPath := ""
	if dir != "" {
		projectPath = filepath.Join(dir, ".dispatch", "config.json")
	}
	path := global
	if m.GetProject() {
		path = projectPath
	}
	action, key, values := m.GetAction(), strings.TrimSpace(m.GetKey()), m.GetValues()

	// key-set and key-clear keep a classifier's API key in the macOS Keychain;
	// the reply says only where the key is now found.
	if action == "key-set" || action == "key-clear" {
		k, ok := classifierKey(key)
		if !ok {
			return nil, invalid("unknown classifier key " + key + " (want jev|openai)")
		}
		var err error
		if action == "key-set" {
			if len(values) != 1 {
				return nil, invalid("key-set takes the key as its one value")
			}
			err = k.Save(values[0])
		} else {
			err = k.Clear()
		}
		if err != nil {
			return nil, invalid(err.Error())
		}
		body, err := jsonBody(map[string]any{"key": key, "status": k.Status()})
		if err != nil {
			return nil, err
		}
		return connect.NewResponse(&dispatchv1.ConfigResponse{Ok: true, Json: body}), nil
	}

	// agent-add and agent-remove keep the ACP agents the user added, in the
	// global config only: a repository must not add a program to run.
	if action == "agent-add" || action == "agent-remove" {
		if m.GetProject() {
			return nil, invalid("agents are added to the global config only")
		}
		f, err := dispatch.LoadConfig(global)
		if err != nil {
			return nil, invalid(err.Error())
		}
		if action == "agent-add" {
			if err := adapters.ValidCustomAgent(key, values); err != nil {
				return nil, invalid(err.Error())
			}
			if f.ACPAgents == nil {
				f.ACPAgents = map[string][]string{}
			}
			f.ACPAgents[key] = values
		} else {
			if _, ok := f.ACPAgents[key]; !ok {
				return nil, invalid("no added agent " + key)
			}
			delete(f.ACPAgents, key)
		}
		if err := dispatch.WriteConfigKey(global, "acp_agents", f.ACPAgents); err != nil {
			return nil, internal(err)
		}
		adapters.SetCustomAgents(f.ACPAgents)
		body, err := jsonBody(map[string]any{"path": global, "key": "acp_agents", "value": f.ACPAgents})
		if err != nil {
			return nil, err
		}
		return connect.NewResponse(&dispatchv1.ConfigResponse{Ok: true, Json: body}), nil
	}

	if action == "get" {
		var f dispatch.Policy
		var err error
		if m.GetProject() {
			f, err = dispatch.LoadConfig(path)
		} else {
			f, err = dispatch.LoadAllConfig(global, projectPath)
		}
		if err != nil {
			return nil, invalid(err.Error())
		}
		var v any = f
		switch key {
		case "":
		case "priority":
			v = f.Priority
		case "discover":
			v = f.Discover
		case "roles":
			v = f.Roles
		case "allow":
			v = f.Allow
		case "deny":
			v = f.Deny
		case "classifier":
			v = f.Classifier
		case "classifier_model":
			v = f.ClassifierModel
		case "acp_agents":
			v = f.ACPAgents
		case "classifier_keys":
			v = map[string]string{
				dispatch.ClassifierJev:    classifierKeyStatus(dispatch.ClassifierJev),
				dispatch.ClassifierOpenAI: classifierKeyStatus(dispatch.ClassifierOpenAI),
			}
		case "classifier_cli":
			deps, err := core.dispatchDepsIn(ctx, dir)
			if err != nil {
				return nil, internal(err)
			}
			cli := classifierCLI(deps)
			v = map[string]string{"agent": cli.Agent, "model": cli.Model}
		default:
			return nil, invalid("unknown config key " + key)
		}
		body, err := jsonBody(v)
		if err != nil {
			return nil, err
		}
		return connect.NewResponse(&dispatchv1.ConfigResponse{Ok: true, Json: body}), nil
	}

	f, err := dispatch.LoadConfig(path)
	if err != nil {
		return nil, invalid(err.Error())
	}
	var writeKey string
	var value any
	switch {
	case action == "set" && key == "priority" && len(values) == 1:
		f.Priority, writeKey, value = values[0], "priority", values[0]
	case action == "set" && key == "discover" && len(values) == 1 && (values[0] == "true" || values[0] == "false"):
		f.Discover, writeKey, value = values[0] == "true", "discover", values[0] == "true"
	case action == "set" && key == "roles" && len(values) == 1 && (values[0] == "true" || values[0] == "false"):
		f.Roles, writeKey, value = values[0] == "true", "roles", values[0] == "true"
	case action == "set" && key == "classifier" && len(values) == 1:
		name := strings.TrimSpace(values[0])
		if name == "off" {
			name = dispatch.ClassifierOff
		}
		f.Classifier, writeKey, value = name, "classifier", name
	case action == "set" && key == "classifier_model" && len(values) == 1:
		f.ClassifierModel, writeKey, value = strings.TrimSpace(values[0]), "classifier_model", strings.TrimSpace(values[0])
	case strings.HasPrefix(action, "allow-") || strings.HasPrefix(action, "deny-"):
		side, op, _ := strings.Cut(action, "-")
		rules := &f.Allow
		if side == "deny" {
			rules = &f.Deny
		}
		var dst *[]string
		switch key {
		case "providers":
			dst = &rules.Providers
		case "models":
			dst = &rules.Models
		case "agents":
			dst = &rules.Agents
		default:
			return nil, invalid("rule dimension must be providers, models, or agents")
		}
		for _, v := range values {
			switch op {
			case "add":
				if !slices.Contains(*dst, v) {
					*dst = append(*dst, v)
				}
			case "rm":
				*dst = slices.DeleteFunc(*dst, func(x string) bool { return x == v })
			default:
				return nil, invalid("unknown config action " + action)
			}
		}
		writeKey, value = side, *rules
	default:
		return nil, invalid("invalid config request")
	}
	if err := dispatch.ValidateConfig(f); err != nil {
		return nil, invalid(err.Error())
	}
	if err := dispatch.WriteConfigKey(path, writeKey, value); err != nil {
		return nil, internal(err)
	}
	body, err := jsonBody(map[string]any{"path": path, "key": writeKey, "value": value})
	if err != nil {
		return nil, err
	}
	return connect.NewResponse(&dispatchv1.ConfigResponse{Ok: true, Json: body}), nil
}

func (core *Core) Subagents(ctx context.Context, request *connect.Request[dispatchv1.SubagentsRequest]) (*connect.Response[dispatchv1.SubagentsResponse], error) {
	dir := strings.TrimSpace(request.Msg.GetDir())
	state := core.dispatchStateDir()
	user := dispatch.GlobalProfilesPath(state)
	respond := func(ok bool, v any) (*connect.Response[dispatchv1.SubagentsResponse], error) {
		body, err := jsonBody(v)
		if err != nil {
			return nil, err
		}
		return connect.NewResponse(&dispatchv1.SubagentsResponse{Ok: ok, Json: body}), nil
	}
	switch request.Msg.GetAction() {
	case "save", "remove":
		name := strings.TrimSpace(request.Msg.GetName())
		if !dispatch.ValidProfileName(name) {
			return nil, invalid(fmt.Sprintf("invalid role name %q (lowercase letters, digits, and dashes, starting with a letter)", name))
		}
		var p dispatch.Profile
		if request.Msg.GetAction() == "save" {
			if err := json.Unmarshal([]byte(request.Msg.GetProfile()), &p); err != nil {
				return nil, invalid("invalid role: " + err.Error())
			}
			known := map[string]bool{}
			for _, a := range adapters.Known() {
				known[a.Name()] = true
			}
			if problems := dispatch.ValidateProfile(name, p, known); len(problems) > 0 {
				return nil, invalid(strings.Join(problems, "; "))
			}
		}
		if err := dispatch.SaveProfile(user, name, p, request.Msg.GetAction() == "remove"); err != nil {
			return nil, invalid(err.Error())
		}
		return respond(true, map[string]any{"wrote": user, "name": name})
	case "list", "check":
	default:
		return nil, invalid("unknown roles action")
	}
	profiles, err := core.dispatchProfiles(dir)
	if err != nil {
		return nil, invalid(err.Error())
	}
	if request.Msg.GetAction() == "list" {
		// user names the roles in the user file, the ones save and remove change;
		// the rest come from the repository's .dispatch/roles.json or the built-in set.
		own, err := dispatch.LoadProfiles(user)
		if err != nil {
			return nil, invalid(err.Error())
		}
		// source names where each effective role comes from: the
		// repository, the user file, or the built-in set; builtin lists the
		// names the install ships, so removing a user copy restores one.
		var fromRepo map[string]dispatch.Profile
		if dir != "" {
			if fromRepo, err = dispatch.LoadProfiles(dispatch.ProjectProfilesPath(dir)); err != nil {
				return nil, invalid(err.Error())
			}
		}
		source := map[string]string{}
		for name := range profiles {
			_, inRepo := fromRepo[name]
			_, inUser := own[name]
			switch {
			case inRepo:
				source[name] = "repository"
			case inUser:
				source[name] = "user"
			default:
				source[name] = "builtin"
			}
		}
		projectConfig := ""
		if dir != "" {
			projectConfig = filepath.Join(dir, ".dispatch", "config.json")
		}
		pick := dispatch.DefaultPolicy().Roles
		if policy, err := dispatch.LoadAllConfig(dispatch.PolicyConfigPath(state), projectConfig); err == nil {
			pick = policy.Roles
		}
		return respond(true, map[string]any{
			"roles": profiles, "user": dispatch.ProfileNames(own), "path": user,
			"source": source, "builtin": dispatch.ProfileNames(dispatch.BuiltinProfiles()), "pick": pick,
		})
	}

	deps, err := core.dispatchDepsIn(ctx, dir)
	if err != nil {
		return nil, internal(err)
	}
	known := map[string]adapters.Adapter{}
	names := map[string]bool{}
	for _, a := range adapters.Known() {
		known[a.Name()], names[a.Name()] = a, true
	}
	problems := []string{}
	for _, name := range dispatch.ProfileNames(profiles) {
		p := profiles[name]
		problems = append(problems, dispatch.ValidateProfile(name, p, names)...)
		if p.Tier != "" && len(deps.Table.Tiers[adapters.Tier(p.Tier)]) == 0 {
			problems = append(problems, fmt.Sprintf("%s: tier %q has no entries in the tier table", name, p.Tier))
		}
		for _, agent := range append(slices.Clone(p.Prefer), p.Avoid...) {
			if a, ok := known[agent]; ok {
				if _, err := exec.LookPath(a.Binary()); err != nil {
					problems = append(problems, fmt.Sprintf("%s: %s is not installed", name, agent))
				}
			}
		}
	}
	return respond(len(problems) == 0, map[string]any{"roles": len(profiles), "problems": problems})
}

// Skill installs the dispatch skill for the agent CLIs, or refreshes the
// copies already installed, like `polterware skill install|refresh`.
func (core *Core) Skill(ctx context.Context, request *connect.Request[dispatchv1.SkillRequest]) (*connect.Response[dispatchv1.SkillResponse], error) {
	home, err := os.UserHomeDir()
	if err != nil {
		return nil, internal(err)
	}
	m := request.Msg
	out := map[string]any{}
	switch m.GetAction() {
	case "refresh":
		paths, err := skill.Refresh(home)
		if err != nil {
			return nil, internal(err)
		}
		out["refreshed"] = paths
	case "install":
		agent := strings.TrimSpace(m.GetAgent())
		targets := skill.Targets(home)
		names := make([]string, 0, len(targets))
		for name := range targets {
			if agent == "" || agent == name {
				names = append(names, name)
			}
		}
		if len(names) == 0 {
			return nil, invalid(fmt.Sprintf("unknown agent %q", agent))
		}
		slices.Sort(names)
		installed, skipped, seen := []string{}, []string{}, map[string]bool{}
		for _, name := range names {
			dir := targets[name].Dir
			if seen[dir] {
				continue
			}
			seen[dir] = true
			path, err := skill.Install(dir, m.GetForce())
			if err != nil {
				skipped = append(skipped, fmt.Sprintf("%s: %v", name, err))
				continue
			}
			installed = append(installed, path)
		}
		out["installed"], out["skipped"] = installed, skipped
	default:
		return nil, invalid("unknown skill action: use install or refresh")
	}
	body, err := jsonBody(out)
	if err != nil {
		return nil, err
	}
	return connect.NewResponse(&dispatchv1.SkillResponse{Ok: true, Json: body}), nil
}

// GetAgentSetup reports how the agent CLIs reach dispatch: the command on the
// PATH, each agent's skill, the Claude Code SessionStart hook, and the
// OpenCode plugin.
func (core *Core) GetAgentSetup(ctx context.Context, request *connect.Request[dispatchv1.GetAgentSetupRequest]) (*connect.Response[dispatchv1.GetAgentSetupResponse], error) {
	home, binary, err := setupPaths()
	if err != nil {
		return nil, internal(err)
	}
	body, err := jsonBody(setup.Read(home, binary, core.dispatchStateDir()))
	if err != nil {
		return nil, err
	}
	return connect.NewResponse(&dispatchv1.GetAgentSetupResponse{Json: body}), nil
}

// ApplyAgentSetup links the command, installs the skill, switches the hook,
// or installs, removes, or switches the OpenCode plugin, pointing each at this
// core's binary, and answers the setup after it.
func (core *Core) ApplyAgentSetup(ctx context.Context, request *connect.Request[dispatchv1.ApplyAgentSetupRequest]) (*connect.Response[dispatchv1.ApplyAgentSetupResponse], error) {
	home, binary, err := setupPaths()
	if err != nil {
		return nil, internal(err)
	}
	state := core.dispatchStateDir()
	switch request.Msg.GetAction() {
	case "link_cli":
		_, err = setup.LinkCLI(home, binary)
	case "install_skill":
		_, err = setup.InstallSkill(home, request.Msg.GetAgent())
	case "hook_on", "hook_off":
		err = setup.SetHook(home, binary, state, request.Msg.GetAction() == "hook_on")
	case "install_opencode":
		_, err = setup.InstallOpenCodePlugin(home, setup.OpenCodeBinary(home, binary), state)
	case "remove_opencode":
		err = setup.RemoveOpenCodePlugin(home, state)
	case "opencode_on", "opencode_off":
		err = setup.SetOpenCodeDelegation(state, request.Msg.GetAction() == "opencode_on")
	default:
		return nil, invalid("unknown setup action")
	}
	if err != nil {
		return nil, invalid(err.Error())
	}
	body, err := jsonBody(setup.Read(home, binary, state))
	if err != nil {
		return nil, err
	}
	return connect.NewResponse(&dispatchv1.ApplyAgentSetupResponse{Json: body}), nil
}

// setupPaths is the home folder and this core's own binary, the one the
// command, the skill, and the hook point agents at.
func setupPaths() (string, string, error) {
	home, err := os.UserHomeDir()
	if err != nil {
		return "", "", err
	}
	binary, err := os.Executable()
	if err != nil {
		return "", "", err
	}
	if resolved, err := filepath.EvalSymlinks(binary); err == nil {
		binary = resolved
	}
	return home, binary, nil
}
