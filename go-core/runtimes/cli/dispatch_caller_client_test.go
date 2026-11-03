package main

import (
	"context"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"connectrpc.com/connect"
	"github.com/polterware/dispatch/go-core/domains/dispatch"
	dispatchv1 "github.com/polterware/dispatch/go-core/platform/gen/dispatch/v1"
	"github.com/polterware/dispatch/go-core/platform/gen/dispatch/v1/dispatchv1connect"
)

type callerTestService struct {
	dispatchv1connect.UnimplementedDispatchServiceHandler
	start *dispatchv1.StartRunRequest
	list  *dispatchv1.ListRunsRequest
}

func (s *callerTestService) StartRun(_ context.Context, req *connect.Request[dispatchv1.StartRunRequest]) (*connect.Response[dispatchv1.StartRunResponse], error) {
	s.start = req.Msg
	return connect.NewResponse(&dispatchv1.StartRunResponse{Run: &dispatchv1.DispatchRun{Id: "run", ConversationId: req.Msg.Caller.ConversationId, CallerAgent: req.Msg.Caller.Agent}}), nil
}
func (s *callerTestService) ListRuns(_ context.Context, req *connect.Request[dispatchv1.ListRunsRequest]) (*connect.Response[dispatchv1.ListRunsResponse], error) {
	s.list = req.Msg
	return connect.NewResponse(&dispatchv1.ListRunsResponse{Runs: []*dispatchv1.DispatchRun{{Id: "run", ConversationId: req.Msg.ConversationId, CallerAgent: "codex"}}}), nil
}

type callerTestTransport struct{ handler http.Handler }

func (t callerTestTransport) RoundTrip(req *http.Request) (*http.Response, error) {
	recorder := httptest.NewRecorder()
	t.handler.ServeHTTP(recorder, req)
	return recorder.Result(), nil
}

func TestDispatchClientForwardsCallerAndConversation(t *testing.T) {
	service := &callerTestService{}
	_, handler := dispatchv1connect.NewDispatchServiceHandler(service)
	client := &connectDispatchClient{svc: dispatchv1connect.NewDispatchServiceClient(&http.Client{Transport: callerTestTransport{handler}}, "http://dispatch.test")}
	caller := dispatch.Caller{Agent: "codex", SessionID: "chat", ConversationID: "codex:chat", Label: "Codex"}
	run, err := client.StartRun(context.Background(), dispatchStartRequest{ProjectID: "workspace", Prompt: "Read the code", Caller: caller})
	if err != nil {
		t.Fatal(err)
	}
	got := service.start.GetCaller()
	if got.Agent != caller.Agent || got.SessionId != caller.SessionID || got.ConversationId != caller.ConversationID || got.Label != caller.Label {
		t.Fatalf("caller=%+v", got)
	}
	if run.ConversationID != caller.ConversationID || run.CallerAgent != caller.Agent || run.SessionID != "" {
		t.Fatalf("run=%+v", run)
	}
	runs, err := client.ListRuns(context.Background(), dispatchListRunsRequest{ProjectID: "workspace", ConversationID: caller.ConversationID, Limit: 2, Offset: 1})
	if err != nil {
		t.Fatal(err)
	}
	if service.list.ConversationId != caller.ConversationID || service.list.Limit != 2 || service.list.Offset != 1 || len(runs) != 1 || runs[0].ConversationID != caller.ConversationID {
		t.Fatalf("request=%+v runs=%+v", service.list, runs)
	}
}

func TestDispatchSummaryShowsShortConversation(t *testing.T) {
	run := dispatch.Run{ID: "run", ConversationID: "codex:0123456789abcdef", CallerAgent: "codex"}
	summary := summarizeDispatchRun(run)
	if summary.ConversationID != run.ConversationID || summary.CallerAgent != "codex" {
		t.Fatalf("summary=%+v", summary)
	}
	text := summary.Text()
	if !strings.Contains(text, conversationSuffix(run.ConversationID)) || strings.Contains(text, run.ConversationID) {
		t.Fatalf("text=%q", text)
	}
	if conversationSuffix("") != "" {
		t.Fatal("empty conversation has a suffix")
	}
}
