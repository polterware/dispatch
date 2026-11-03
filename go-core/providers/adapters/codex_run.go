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
	"strings"
	"sync"
	"time"

	"github.com/polterware/dispatch/go-core/providers/sandbox"
)

// Run drives one turn over `codex app-server`, the JSON-RPC protocol the Codex
// apps use, because it streams what the agent does as it does it: reply and
// reasoning deltas, command output, and items as they start. Each notification
// goes to the live log as one compact JSON line. The session id is the thread
// id, and `thread/resume` continues it.
func (Codex) Run(ctx context.Context, req Request, model string) (Result, error) {
	if req.Timeout > 0 {
		var cancel context.CancelFunc
		ctx, cancel = context.WithTimeout(ctx, req.Timeout)
		defer cancel()
	}
	runCtx, watch, fatal, stop := failFast(ctx)
	defer stop()
	cmd := exec.CommandContext(runCtx, Bin("codex"), "app-server")
	if len(req.Env) > 0 {
		cmd.Env = req.Env
	} else {
		cmd.Env = sandbox.Environ()
	}
	cmd.Dir = req.Dir
	// OS pipes, not io.Pipe: exec then copies nothing, so Wait never waits on
	// the conversation, and a write to a server that exited fails instead of blocking.
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
	logOut, logErr, closeLog := openLiveLog(req.LogPath)
	defer closeLog()
	cmd.Stdin, cmd.Stdout, cmd.Stderr = inR, outW, io.MultiWriter(&stderr, logErr, watch)

	s := &codexSession{req: req, model: model, in: inW, log: logOut, sessionID: req.ResumeSession}
	done := make(chan struct{})
	go func() {
		defer close(done)
		s.serve(outR)
	}()
	closeChildEnds := func() { inR.Close(); outW.Close() }
	runErr := runAgent(cmd, func(pid int) error {
		closeChildEnds()
		if req.Started != nil {
			return req.Started(pid)
		}
		return nil
	})
	closeChildEnds()
	inW.Close()
	select {
	case <-done:
	case <-time.After(2 * time.Second): // a leftover child still holds the server's stdout
		outR.Close()
		<-done
	}
	outR.Close()

	if errors.Is(runErr, ErrProcessesAlive) {
		return Result{ExitCode: -1}, runErr
	}
	res := Result{ExitCode: cmd.ProcessState.ExitCode(), SessionID: s.sessionID, Output: s.output(), Usage: s.usage}
	if line := fatal(); line != "" {
		return res, fmt.Errorf("codex: stopped early: %s", line)
	}
	if runErr != nil {
		if ctx.Err() != nil {
			return res, ctx.Err()
		}
		return res, fmt.Errorf("codex app-server: %v: %s", runErr, bytes.TrimSpace(stderr.Bytes()))
	}
	if s.err != "" {
		return res, fmt.Errorf("codex: %s", s.err)
	}
	if !s.completed {
		return res, fmt.Errorf("codex app-server ended before the turn completed: %s", bytes.TrimSpace(stderr.Bytes()))
	}
	return res, nil
}

const (
	codexInitID = iota + 1
	codexThreadID
	codexTurnID
)

// codexSession is the client side of one app-server conversation.
type codexSession struct {
	req   Request
	model string
	in    io.WriteCloser
	log   io.Writer

	mu        sync.Mutex
	sessionID string
	turnID    string
	final     string // the final_answer message, else the last one
	last      string
	usage     *Usage
	err       string
	completed bool
}

func (s *codexSession) send(msg map[string]any) {
	msg["jsonrpc"] = "2.0"
	b, _ := json.Marshal(msg)
	s.in.Write(append(b, '\n'))
}

func (s *codexSession) finish() { s.in.Close() }

func (s *codexSession) serve(out io.Reader) {
	defer s.finish()
	s.send(map[string]any{"id": codexInitID, "method": "initialize",
		"params": map[string]any{"clientInfo": map[string]string{"name": "dispatch", "version": "0.1.0"}}})
	s.send(map[string]any{"method": "initialized"})
	method := "thread/start"
	if s.req.ResumeSession != "" {
		method = "thread/resume"
	}
	s.send(map[string]any{"id": codexThreadID, "method": method, "params": codexThreadParams(s.req, s.model)})

	sc := bufio.NewScanner(out)
	sc.Buffer(make([]byte, 1<<20), 32<<20)
	for sc.Scan() {
		if s.handle(sc.Bytes()) {
			s.finish()
		}
	}
	io.Copy(io.Discard, out) // a line over the buffer: keep the server from blocking
}

type codexMessage struct {
	ID     *int            `json:"id,omitempty"`
	Method string          `json:"method,omitempty"`
	Params json.RawMessage `json:"params,omitempty"`
	Result json.RawMessage `json:"result,omitempty"`
	Error  *struct {
		Message string `json:"message"`
	} `json:"error,omitempty"`
}

// handle reads one line from the server and reports whether the turn is over.
func (s *codexSession) handle(line []byte) bool {
	var m codexMessage
	if json.Unmarshal(line, &m) != nil {
		return false
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	if m.ID != nil && m.Method != "" {
		// A request from the server (an approval or a question): runs answer none.
		s.send(map[string]any{"id": *m.ID, "error": map[string]any{"code": -32601, "message": "dispatch runs answer no requests"}})
		return false
	}
	if m.ID != nil {
		if m.Error != nil {
			if *m.ID != codexInitID {
				s.err = m.Error.Message
				return true
			}
			return false
		}
		switch *m.ID {
		case codexThreadID:
			var r struct {
				Thread struct {
					ID string `json:"id"`
				} `json:"thread"`
			}
			json.Unmarshal(m.Result, &r)
			if r.Thread.ID == "" {
				s.err = "the app-server started no thread"
				return true
			}
			s.sessionID = r.Thread.ID
			s.send(map[string]any{"id": codexTurnID, "method": "turn/start", "params": codexTurnParams(s.req, s.sessionID)})
		case codexTurnID:
			var r struct {
				Turn struct {
					ID string `json:"id"`
				} `json:"turn"`
			}
			json.Unmarshal(m.Result, &r)
			if s.turnID == "" {
				s.turnID = r.Turn.ID
			}
		}
		return false
	}
	return s.notify(m.Method, m.Params, line)
}

func (s *codexSession) notify(method string, raw json.RawMessage, line []byte) bool {
	var p struct {
		TurnID string `json:"turnId"`
		Item   struct {
			Type  string `json:"type"`
			Text  string `json:"text"`
			Phase string `json:"phase"`
		} `json:"item"`
		Turn struct {
			ID     string `json:"id"`
			Status string `json:"status"`
			Error  *struct {
				Message string `json:"message"`
			} `json:"error"`
		} `json:"turn"`
		Error struct {
			Message string `json:"message"`
		} `json:"error"`
		WillRetry  bool `json:"willRetry"`
		TokenUsage struct {
			Last *struct {
				Input      int64 `json:"inputTokens"`
				Cached     int64 `json:"cachedInputTokens"`
				CacheWrite int64 `json:"cacheWriteInputTokens"`
				Output     int64 `json:"outputTokens"`
				Reasoning  int64 `json:"reasoningOutputTokens"`
			} `json:"last"`
			Window int64 `json:"modelContextWindow"`
		} `json:"tokenUsage"`
	}
	json.Unmarshal(raw, &p)
	ours := p.TurnID == "" || s.turnID == "" || p.TurnID == s.turnID
	switch method {
	case "turn/started":
		if s.turnID == "" {
			s.turnID = p.Turn.ID
		}
	case "item/completed":
		if p.Item.Type == "agentMessage" && strings.TrimSpace(p.Item.Text) != "" && ours {
			s.last = p.Item.Text
			if p.Item.Phase == "final_answer" {
				s.final = p.Item.Text
			}
		}
	case "thread/tokenUsage/updated":
		// Each update carries the model call that just ended as `last`; their sum
		// is this turn's spend, without the turns a resumed thread had before.
		if x := p.TokenUsage.Last; x != nil && ours {
			if s.usage == nil {
				s.usage = &Usage{}
			}
			s.usage.InputTokens += clampTokens(x.Input - x.Cached - x.CacheWrite)
			s.usage.CacheReadTokens += x.Cached
			s.usage.CacheWriteTokens += x.CacheWrite
			s.usage.OutputTokens += x.Output
			s.usage.ReasoningTokens += x.Reasoning
			// Input counts its cache, and output its reasoning.
			s.usage.ContextTokens, s.usage.ContextWindow = x.Input+x.Output, p.TokenUsage.Window
			writeUsageLine(s.log, s.usage)
		}
	case "error":
		if !p.WillRetry && p.Error.Message != "" {
			s.err = p.Error.Message
		}
	}
	if compact := codexLogLine(method, p.Item.Type, line); compact != nil {
		s.log.Write(compact)
	}
	if method == "turn/completed" && (s.turnID == "" || p.Turn.ID == s.turnID) {
		s.completed = true
		switch {
		case p.Turn.Error != nil && p.Turn.Error.Message != "":
			s.err = p.Turn.Error.Message
		case p.Turn.Status != "" && p.Turn.Status != "completed":
			if s.err == "" {
				s.err = "the turn ended " + p.Turn.Status
			}
		}
		return true
	}
	return false
}

func (s *codexSession) output() string {
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.final != "" {
		return s.final
	}
	return s.last
}

// codexQuiet are notifications that say nothing about the agent's work.
var codexQuiet = map[string]bool{
	"thread/started": true, "thread/status/changed": true, "thread/tokenUsage/updated": true,
	"account/updated": true, "account/rateLimits/updated": true, "remoteControl/status/changed": true,
	"mcpServer/startupStatus/updated": true, "configWarning": true, "warning": true,
	"serverRequest/resolved": true,
}

// codexLogLine is the notification as the transcript reads it: the method and
// its params, without the thread, turn, and timestamp every line repeats. It
// returns nil for a notification the transcript does not show.
func codexLogLine(method, itemType string, line []byte) []byte {
	if method == "" || codexQuiet[method] || itemType == "userMessage" {
		return nil
	}
	var m struct {
		Params map[string]json.RawMessage `json:"params"`
	}
	if json.Unmarshal(line, &m) != nil {
		return nil
	}
	delete(m.Params, "threadId")
	delete(m.Params, "turnId")
	b, err := json.Marshal(map[string]any{"method": method, "params": m.Params})
	if err != nil {
		return nil
	}
	return append(b, '\n')
}

func codexSandbox(req Request) string {
	if req.ReadOnly {
		return "read-only"
	}
	return "workspace-write"
}

func codexThreadParams(req Request, model string) map[string]any {
	p := map[string]any{"sandbox": codexSandbox(req), "approvalPolicy": "never"}
	if req.Dir != "" {
		p["cwd"] = req.Dir
	}
	if model != "" {
		p["model"] = model
	}
	if req.ResumeSession != "" {
		p["threadId"] = req.ResumeSession
	}
	return p
}

func codexTurnParams(req Request, threadID string) map[string]any {
	p := map[string]any{"threadId": threadID, "input": []map[string]any{{"type": "text", "text": req.Prompt}}}
	if req.Effort != "" {
		p["effort"] = req.Effort
	}
	return p
}
