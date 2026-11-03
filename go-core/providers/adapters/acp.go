package adapters

import (
	"bufio"
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"slices"
	"strings"
	"sync"
	"time"

	"github.com/polterware/dispatch/go-core/providers/sandbox"
)

// acpSpec is one agent CLI that speaks the Agent Client Protocol: JSON-RPC
// 2.0 over stdio, the protocol editors host coding agents with. It streams
// what the agent does as it does it, so every ACP agent runs live.
type acpSpec struct {
	name string
	// command is the argument list that starts the ACP server; readOnly is
	// true for a run that must not change files.
	command func(readOnly bool) []string
	// readMode is the session mode (or "mode" config option value) a
	// read-only run must select; empty leaves the mode alone. Either way, a
	// read-only run rejects every permission request to edit, delete, move,
	// or execute.
	readMode string
	caps     Capabilities
	flags    []probeFlag
	// listModels lists the models from a CLI subcommand; nil reads them
	// from the agent's own session/new response.
	listModels func(ctx context.Context) ([]Model, error)
}

// acpAdapter runs an acpSpec. Quota has no ACP source; adapters with an
// official one wrap it (Antigravity).
type acpAdapter struct{ spec acpSpec }

func (a acpAdapter) Name() string               { return a.spec.name }
func (a acpAdapter) Binary() string             { return Bin(a.spec.name) }
func (a acpAdapter) Capabilities() Capabilities { return a.spec.caps }
func (a acpAdapter) Quota(context.Context) (Quota, error) {
	return Quota{}, ErrNoQuota
}

func (a acpAdapter) Probe(ctx context.Context) ProbeReport {
	return probeBinary(ctx, a.spec.name, a.Binary(), a.spec.caps, a.spec.flags)
}

func (a acpAdapter) Resume(ctx context.Context, req Request, model string) (Result, error) {
	if req.ResumeSession == "" {
		return Result{}, fmt.Errorf("%s: resume requires a session", a.spec.name)
	}
	return a.Run(ctx, req, model)
}

var acpModelCache = struct {
	sync.Mutex
	at     map[string]time.Time
	models map[string][]Model
}{at: map[string]time.Time{}, models: map[string][]Model{}}

// Models lists what the CLI offers, without Claude models outside Claude's
// own CLI. An agent with no list subcommand is asked over ACP, which starts
// it, so the answer is kept for ten minutes.
func (a acpAdapter) Models(ctx context.Context) ([]Model, error) {
	if a.spec.listModels != nil {
		return a.spec.listModels(ctx)
	}
	acpModelCache.Lock()
	if at, ok := acpModelCache.at[a.spec.name]; ok && time.Since(at) < 10*time.Minute {
		models := acpModelCache.models[a.spec.name]
		acpModelCache.Unlock()
		return models, nil
	}
	acpModelCache.Unlock()
	ctx, cancel := context.WithTimeout(ctx, 30*time.Second)
	defer cancel()
	ids, err := a.listSessionModels(ctx)
	if err != nil {
		return nil, err
	}
	var models []Model
	for _, id := range ids {
		if AllowedModel(a.spec.name, id) {
			models = append(models, Model{ID: id})
		}
	}
	acpModelCache.Lock()
	acpModelCache.at[a.spec.name], acpModelCache.models[a.spec.name] = time.Now(), models
	acpModelCache.Unlock()
	return models, nil
}

// listSessionModels opens a session in a scratch folder and reads the models
// its response offers; it sends no prompt.
func (a acpAdapter) listSessionModels(ctx context.Context) ([]string, error) {
	dir, err := os.MkdirTemp("", "dispatch-acp-")
	if err != nil {
		return nil, err
	}
	defer os.RemoveAll(dir)
	s := &acpSession{spec: a.spec, req: Request{Dir: dir, ReadOnly: true}, log: io.Discard, listOnly: true}
	if _, err := s.run(ctx, a.Binary()); err != nil && len(s.offered) == 0 {
		return nil, err
	}
	return s.offered, nil
}

func (a acpAdapter) Run(ctx context.Context, req Request, model string) (Result, error) {
	if !AllowedModel(a.spec.name, model) {
		return Result{}, fmt.Errorf("%s: claude models are not routed through this CLI", a.spec.name)
	}
	if req.Dir != "" {
		// ACP wants an absolute cwd.
		if abs, err := filepath.Abs(req.Dir); err == nil {
			req.Dir = abs
		}
	}
	if req.Timeout > 0 {
		var cancel context.CancelFunc
		ctx, cancel = context.WithTimeout(ctx, req.Timeout)
		defer cancel()
	}
	logOut, logErr, closeLog := openLiveLog(req.LogPath)
	defer closeLog()
	s := &acpSession{spec: a.spec, req: req, model: model, log: logOut, logErr: logErr, sessionID: req.ResumeSession}
	return s.run(ctx, a.Binary())
}

const (
	acpInitID = iota + 1
	acpSessionID
	acpSetupID // each setup call takes the next id from here
)

// acpSession is the client side of one ACP conversation: initialize, open or
// resume a session, select the model, effort, and mode, and send one prompt.
type acpSession struct {
	spec     acpSpec
	req      Request
	model    string
	log      io.Writer
	logErr   io.Writer
	listOnly bool // stop after session/new, keeping the models it offers

	in     io.WriteCloser
	mu     sync.Mutex
	nextID int
	setup  []acpCall
	// replaying is true while session/load replays the earlier turns.
	replaying bool
	sessionID string
	promptID  int
	offered   []string

	messages  []string // messages before the last tool call
	current   strings.Builder
	messageID string
	usage     *Usage
	context   *Usage // context and cost from usage_update, before the prompt's totals
	stop      string
	err       string
	completed bool
}

type acpCall struct {
	method string
	params map[string]any
	// optional calls may fail (an agent that has no such option) without
	// failing the run.
	optional bool
}

// run starts the agent and holds the conversation until the prompt ends.
func (s *acpSession) run(ctx context.Context, binary string) (Result, error) {
	runCtx, watch, fatal, stop := failFast(ctx)
	defer stop()
	// done ends the agent once the turn is over: some ACP servers keep
	// running after their stdin closes.
	agentCtx, endAgent := context.WithCancel(runCtx)
	defer endAgent()
	cmd := exec.CommandContext(agentCtx, binary, s.spec.command(s.req.ReadOnly)...)
	if len(s.req.Env) > 0 {
		cmd.Env = s.req.Env
	} else {
		cmd.Env = sandbox.Environ()
	}
	cmd.Dir = s.req.Dir
	inR, inW, err := os.Pipe()
	if err != nil {
		return Result{}, err
	}
	outR, outW, err := os.Pipe()
	if err != nil {
		inR.Close()
		inW.Close()
		return Result{}, err
	}
	var stderr bytes.Buffer
	logErr := s.logErr
	if logErr == nil {
		logErr = io.Discard
	}
	cmd.Stdin, cmd.Stdout, cmd.Stderr = inR, outW, io.MultiWriter(&stderr, logErr, watch)
	s.in = inW

	turnOver := make(chan struct{})
	done := make(chan struct{})
	go func() {
		defer close(done)
		s.serve(outR, turnOver)
	}()
	go func() {
		select {
		case <-turnOver:
			select {
			case <-time.After(3 * time.Second):
				endAgent()
			case <-done:
			}
		case <-done:
		}
	}()
	closeChildEnds := func() { inR.Close(); outW.Close() }
	runErr := runAgent(cmd, func(pid int) error {
		closeChildEnds()
		if s.req.Started != nil {
			return s.req.Started(pid)
		}
		return nil
	})
	closeChildEnds()
	inW.Close()
	select {
	case <-done:
	case <-time.After(2 * time.Second): // a leftover child still holds the agent's stdout
		outR.Close()
		<-done
	}
	outR.Close()

	name := s.spec.name
	if errors.Is(runErr, ErrProcessesAlive) {
		return Result{ExitCode: -1}, runErr
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	res := Result{ExitCode: cmd.ProcessState.ExitCode(), SessionID: s.sessionID, Output: s.output(), Usage: s.finalUsage()}
	if line := fatal(); line != "" {
		return res, fmt.Errorf("%s: stopped early: %s", name, line)
	}
	if (s.completed || s.listOnly) && runErr != nil && agentCtx.Err() != nil && runCtx.Err() == nil {
		runErr, res.ExitCode = nil, 0 // the agent was ended after its turn
	}
	switch {
	case s.err != "":
		return res, fmt.Errorf("%s: %s", name, s.err)
	case runErr != nil && ctx.Err() != nil:
		return res, ctx.Err()
	case runErr != nil:
		return res, fmt.Errorf("%s acp: %v: %s", name, runErr, bytes.TrimSpace(stderr.Bytes()))
	case s.listOnly:
		return res, nil
	case !s.completed:
		return res, fmt.Errorf("%s acp ended before the prompt completed: %s", name, bytes.TrimSpace(stderr.Bytes()))
	}
	switch s.stop {
	case "refusal":
		return res, fmt.Errorf("%s: the agent refused the prompt", name)
	case "cancelled":
		return res, fmt.Errorf("%s: the prompt was cancelled", name)
	}
	return res, nil
}

func (s *acpSession) send(msg map[string]any) {
	msg["jsonrpc"] = "2.0"
	b, _ := json.Marshal(msg)
	s.in.Write(append(b, '\n'))
}

func (s *acpSession) call(method string, params map[string]any) int {
	s.nextID++
	s.send(map[string]any{"id": s.nextID, "method": method, "params": params})
	return s.nextID
}

func (s *acpSession) serve(out io.Reader, turnOver chan<- struct{}) {
	s.mu.Lock()
	s.nextID = acpInitID - 1
	s.call("initialize", map[string]any{
		"protocolVersion":    1,
		"clientCapabilities": map[string]any{"fs": map[string]bool{"readTextFile": false, "writeTextFile": false}, "terminal": false},
		"clientInfo":         map[string]string{"name": "dispatch", "version": "0.1.0"},
	})
	s.mu.Unlock()
	over := false
	sc := bufio.NewScanner(out)
	sc.Buffer(make([]byte, 1<<20), 32<<20)
	for sc.Scan() {
		if !over && s.handle(sc.Bytes()) {
			over = true
			s.in.Close()
			close(turnOver)
		}
	}
	io.Copy(io.Discard, out) // a line over the buffer: keep the agent from blocking
}

// acpMessage is one JSON-RPC line. Agents may number their own requests with
// strings (Antigravity does), so an id is kept raw and echoed back as sent.
type acpMessage struct {
	ID     json.RawMessage `json:"id,omitempty"`
	Method string          `json:"method,omitempty"`
	Params json.RawMessage `json:"params,omitempty"`
	Result json.RawMessage `json:"result,omitempty"`
	Error  *struct {
		Message string `json:"message"`
	} `json:"error,omitempty"`
}

// handle reads one line from the agent and reports whether the run is over.
func (s *acpSession) handle(line []byte) bool {
	var m acpMessage
	if json.Unmarshal(line, &m) != nil {
		return false
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	hasID := len(m.ID) > 0 && string(m.ID) != "null"
	switch {
	case hasID && m.Method != "":
		s.answer(m.ID, m.Method, m.Params)
		return false
	case hasID:
		var id int
		if json.Unmarshal(m.ID, &id) != nil {
			return false // not one of ours
		}
		return s.response(id, m)
	case m.Method == "session/update":
		s.update(m.Params)
	}
	return false
}

// answer replies to a request from the agent. Dispatch offers no file system
// or terminal, so only permission requests get a yes or no.
func (s *acpSession) answer(id json.RawMessage, method string, raw json.RawMessage) {
	if method != "session/request_permission" {
		s.send(map[string]any{"id": id, "error": map[string]any{"code": -32601, "message": "dispatch runs answer no " + method + " requests"}})
		return
	}
	var p struct {
		ToolCall struct {
			Kind  string `json:"kind"`
			Title string `json:"title"`
		} `json:"toolCall"`
		Options []struct {
			OptionID string `json:"optionId"`
			Kind     string `json:"kind"`
		} `json:"options"`
	}
	json.Unmarshal(raw, &p)
	allow := !s.req.ReadOnly || acpReadKinds[p.ToolCall.Kind]
	pick := ""
	for _, want := range acpPermissionOrder(allow) {
		for _, o := range p.Options {
			if o.Kind == want && pick == "" {
				pick = o.OptionID
			}
		}
	}
	outcome := map[string]any{"outcome": "cancelled"}
	if pick != "" {
		outcome = map[string]any{"outcome": "selected", "optionId": pick}
	}
	if !allow {
		fmt.Fprintf(s.log, "[note] read-only run: refused %s\n", strings.TrimSpace(p.ToolCall.Kind+" "+p.ToolCall.Title))
	}
	s.send(map[string]any{"id": id, "result": map[string]any{"outcome": outcome}})
}

// acpReadKinds are the tool kinds a read-only run allows.
var acpReadKinds = map[string]bool{"read": true, "search": true, "fetch": true, "think": true}

func acpPermissionOrder(allow bool) []string {
	if allow {
		return []string{"allow_once", "allow_always"}
	}
	return []string{"reject_once", "reject_always"}
}

func (s *acpSession) response(id int, m acpMessage) bool {
	errText := ""
	if m.Error != nil {
		errText = m.Error.Message
		if errText == "" {
			errText = "request failed"
		}
	}
	switch {
	case id == acpInitID:
		if errText != "" {
			s.err = "initialize: " + errText
			return true
		}
		return s.openSession(m.Result)
	case id == acpSessionID:
		if errText != "" {
			s.err = errText
			return true
		}
		s.replaying = false
		return s.configure(m.Result)
	case id == s.promptID:
		if errText != "" {
			s.err = errText
			return true
		}
		s.finishPrompt(m.Result)
		return true
	case id >= acpSetupID:
		i := id - acpSetupID
		if errText != "" && i < len(s.setup) && !s.setup[i].optional {
			s.err = s.setup[i].method + ": " + errText
			return true
		}
		return s.nextSetup(i + 1)
	}
	return false
}

// openSession starts a new session, or continues one: session/resume when the
// agent offers it, else session/load, whose replay of earlier turns stays out
// of the log.
func (s *acpSession) openSession(raw json.RawMessage) bool {
	var r struct {
		AgentCapabilities struct {
			LoadSession         bool `json:"loadSession"`
			SessionCapabilities struct {
				Resume *json.RawMessage `json:"resume"`
			} `json:"sessionCapabilities"`
		} `json:"agentCapabilities"`
	}
	json.Unmarshal(raw, &r)
	params := map[string]any{"cwd": s.req.Dir, "mcpServers": []any{}}
	method := "session/new"
	if s.req.ResumeSession != "" {
		params["sessionId"] = s.req.ResumeSession
		switch {
		case r.AgentCapabilities.SessionCapabilities.Resume != nil:
			method = "session/resume"
		case r.AgentCapabilities.LoadSession:
			method, s.replaying = "session/load", true
		default:
			s.err = "the agent cannot resume a session"
			return true
		}
	}
	s.nextID = acpSessionID - 1
	s.call(method, params)
	return false
}

type acpOption struct {
	ID           string `json:"id"`
	Category     string `json:"category"`
	CurrentValue any    `json:"currentValue"`
	Options      []struct {
		Value any `json:"value"`
		// Grouped options nest their values.
		Options []struct {
			Value any `json:"value"`
		} `json:"options"`
	} `json:"options"`
}

func (o acpOption) values() []string {
	var out []string
	for _, v := range o.Options {
		if s, ok := v.Value.(string); ok {
			out = append(out, s)
		}
		for _, g := range v.Options {
			if s, ok := g.Value.(string); ok {
				out = append(out, s)
			}
		}
	}
	return out
}

// configure reads the session's models, modes, and options, and queues the
// calls that select the run's model, effort, and read-only mode.
func (s *acpSession) configure(raw json.RawMessage) bool {
	var r struct {
		SessionID string `json:"sessionId"`
		Models    *struct {
			Available []struct {
				ModelID string `json:"modelId"`
			} `json:"availableModels"`
		} `json:"models"`
		Modes *struct {
			Available []struct {
				ID string `json:"id"`
			} `json:"availableModes"`
		} `json:"modes"`
		ConfigOptions []acpOption `json:"configOptions"`
	}
	json.Unmarshal(raw, &r)
	if r.SessionID != "" {
		s.sessionID = r.SessionID
	}
	if s.sessionID == "" {
		s.err = "the agent opened no session"
		return true
	}
	option := func(id, category string) *acpOption {
		for i, o := range r.ConfigOptions {
			if o.ID == id || (category != "" && o.Category == category) {
				return &r.ConfigOptions[i]
			}
		}
		return nil
	}
	var models []string
	if r.Models != nil {
		for _, m := range r.Models.Available {
			models = append(models, m.ModelID)
		}
	}
	modelOption := option("model", "model")
	if len(models) == 0 && modelOption != nil {
		models = modelOption.values()
	}
	if s.listOnly {
		s.offered = models
		return true
	}
	set := func(o *acpOption, value string, optional bool) {
		s.setup = append(s.setup, acpCall{method: "session/set_config_option", optional: optional,
			params: map[string]any{"sessionId": s.sessionID, "configId": o.ID, "value": value}})
	}
	if s.model != "" {
		switch {
		case r.Models != nil && slices.Contains(models, s.model):
			s.setup = append(s.setup, acpCall{method: "session/set_model", params: map[string]any{"sessionId": s.sessionID, "modelId": s.model}})
		case modelOption != nil && slices.Contains(modelOption.values(), s.model):
			set(modelOption, s.model, false)
		case len(models) > 0:
			s.err = fmt.Sprintf("model %q is not offered (%d models listed)", s.model, len(models))
			return true
		default: // the agent lists no models: ask for it anyway
			s.setup = append(s.setup, acpCall{method: "session/set_model", params: map[string]any{"sessionId": s.sessionID, "modelId": s.model}})
		}
	}
	if effort := option("effort", "thought_level"); s.req.Effort != "" && effort != nil && slices.Contains(effort.values(), s.req.Effort) {
		set(effort, s.req.Effort, true)
	}
	if s.req.ReadOnly && s.spec.readMode != "" {
		mode := ""
		if r.Modes != nil {
			for _, m := range r.Modes.Available {
				if m.ID == s.spec.readMode || strings.HasSuffix(m.ID, "#"+s.spec.readMode) {
					mode = m.ID
				}
			}
		}
		// The mode is required: an agent that edits without asking (OpenCode's
		// build agent does) is read-only only in it.
		switch o := option("mode", "mode"); {
		case mode != "":
			s.setup = append(s.setup, acpCall{method: "session/set_mode", params: map[string]any{"sessionId": s.sessionID, "modeId": mode}})
		case o != nil && slices.Contains(o.values(), s.spec.readMode):
			set(o, s.spec.readMode, false)
		default:
			s.err = fmt.Sprintf("the agent offers no %s mode for a read-only run", s.spec.readMode)
			return true
		}
	}
	return s.nextSetup(0)
}

// nextSetup sends setup call i, or the prompt once they are all answered.
func (s *acpSession) nextSetup(i int) bool {
	if i < len(s.setup) {
		s.nextID = acpSetupID + i - 1
		s.call(s.setup[i].method, s.setup[i].params)
		return false
	}
	s.nextID = acpSetupID + len(s.setup)
	s.promptID = s.call("session/prompt", map[string]any{
		"sessionId": s.sessionID,
		"prompt":    []map[string]any{{"type": "text", "text": s.req.Prompt}},
	})
	return false
}

// acpQuiet are updates that say nothing about the agent's work.
var acpQuiet = map[string]bool{
	"available_commands_update": true, "session_info_update": true, "current_mode_update": true,
	"config_option_update": true, "user_message_chunk": true, "usage_update": true,
}

// update reads one session/update and writes it to the live log as
// {"method":"session/update","params":{"update":…}}, without the session id.
func (s *acpSession) update(raw json.RawMessage) {
	if s.replaying {
		return
	}
	var p struct {
		Update json.RawMessage `json:"update"`
	}
	json.Unmarshal(raw, &p)
	var u struct {
		Kind      string `json:"sessionUpdate"`
		MessageID string `json:"messageId"`
		Content   struct {
			Text string `json:"text"`
		} `json:"content"`
		Used int64 `json:"used"`
		Size int64 `json:"size"`
		Cost *struct {
			Amount   float64 `json:"amount"`
			Currency string  `json:"currency"`
		} `json:"cost"`
	}
	if json.Unmarshal(p.Update, &u) != nil {
		return
	}
	switch u.Kind {
	case "agent_message_chunk":
		if u.MessageID != "" && u.MessageID != s.messageID {
			s.closeMessage()
			s.messageID = u.MessageID
		}
		s.current.WriteString(u.Content.Text)
	case "tool_call":
		s.closeMessage()
	case "usage_update":
		if s.context == nil {
			s.context = &Usage{}
		}
		s.context.ContextTokens, s.context.ContextWindow = u.Used, u.Size
		// A session's cost is cumulative, so a resumed one's includes the
		// turns before it; a zero is a subscription that bills no tokens,
		// which the catalog prices instead.
		if u.Cost != nil && strings.EqualFold(u.Cost.Currency, "USD") && u.Cost.Amount > 0 && s.req.ResumeSession == "" {
			cost := u.Cost.Amount
			s.context.CostUSD = &cost
		}
		live := *s.context
		if s.usage != nil {
			live = *s.usage
			live.ContextTokens, live.ContextWindow = s.context.ContextTokens, s.context.ContextWindow
		}
		writeUsageLine(s.log, &live)
	}
	if acpQuiet[u.Kind] {
		return
	}
	b, err := json.Marshal(map[string]any{"method": "session/update", "params": map[string]any{"update": p.Update}})
	if err == nil {
		s.log.Write(append(b, '\n'))
	}
}

func (s *acpSession) closeMessage() {
	if text := strings.TrimSpace(s.current.String()); text != "" {
		s.messages = append(s.messages, text)
	}
	s.current.Reset()
}

// finishPrompt reads the prompt's stop reason and the tokens it spent.
func (s *acpSession) finishPrompt(raw json.RawMessage) {
	var r struct {
		StopReason string    `json:"stopReason"`
		Usage      *acpUsage `json:"usage"`
		Meta       struct {
			Usage *acpUsage `json:"usage"`
		} `json:"_meta"`
	}
	json.Unmarshal(raw, &r)
	s.completed, s.stop = true, r.StopReason
	if u := r.Usage; u != nil || r.Meta.Usage != nil {
		if u == nil {
			u = r.Meta.Usage
		}
		s.usage = u.normalize()
		if s.context != nil {
			s.usage.ContextTokens, s.usage.ContextWindow = s.context.ContextTokens, s.context.ContextWindow
		}
		writeUsageLine(s.log, s.finalUsage())
	}
}

// output is the reply: what the agent wrote after its last tool call, else
// its last message.
func (s *acpSession) output() string {
	if text := strings.TrimSpace(s.current.String()); text != "" {
		return text
	}
	if len(s.messages) > 0 {
		return s.messages[len(s.messages)-1]
	}
	return ""
}

// finalUsage is the prompt's tokens with the session's context and cost; nil
// when the agent reported no tokens.
func (s *acpSession) finalUsage() *Usage {
	if s.usage == nil {
		return nil
	}
	u := *s.usage
	if s.context != nil && s.context.CostUSD != nil {
		u.CostUSD = s.context.CostUSD
	}
	return &u
}

// acpUsage is PromptResponse.usage. Agents disagree on whether input counts
// its cache (Antigravity's does, OpenCode's does not) and whether output
// counts the thinking, so the total says which.
type acpUsage struct {
	Input      int64 `json:"inputTokens"`
	Output     int64 `json:"outputTokens"`
	CacheRead  int64 `json:"cachedReadTokens"`
	CacheWrite int64 `json:"cachedWriteTokens"`
	Thought    int64 `json:"thoughtTokens"`
	Total      int64 `json:"totalTokens"`
}

func (a acpUsage) normalize() *Usage {
	in, cache := a.Input, a.CacheRead+a.CacheWrite
	if cache > 0 && a.Total > 0 && a.Total < in+a.Output+cache+a.Thought {
		in = clampTokens(in - cache) // the total counts the cache once, inside input
	}
	out := a.Output
	if a.Thought > 0 && a.Total >= in+cache+out+a.Thought {
		out += a.Thought // counted apart from output
	}
	return &Usage{InputTokens: in, CacheReadTokens: a.CacheRead, CacheWriteTokens: a.CacheWrite, OutputTokens: out, ReasoningTokens: a.Thought}
}
