#!/bin/sh
# A fake ACP agent for the end-to-end tests: it answers the handshake, offers
# two models, and answers a prompt. A prompt with WRITE writes e2e.txt in the
# folder it runs in, naming this process so no two runs write the same file;
# one with SLOW then waits a minute, to be cancelled or cut off; one with FAIL
# ends with an error. It loads its one session again for a follow-up. FAKE_ACP_LOG
# names a file that gets every line it reads.
while IFS= read -r line; do
  [ -n "$FAKE_ACP_LOG" ] && printf '%s\n' "$line" >> "$FAKE_ACP_LOG"
  id=$(printf '%s' "$line" | sed -n 's/^{[^{]*"id":\([0-9][0-9]*\).*/\1/p')
  case "$line" in
  *'"method":"initialize"'*)
    echo "{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{\"protocolVersion\":1,\"agentCapabilities\":{\"loadSession\":true}}}" ;;
  *'"method":"session/new"'*)
    echo "{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{\"sessionId\":\"fake-1\",\"models\":{\"currentModelId\":\"fake-fast\",\"availableModels\":[{\"modelId\":\"fake-fast\"},{\"modelId\":\"fake-deep\"}]},\"modes\":{\"availableModes\":[{\"id\":\"build\"},{\"id\":\"plan\"}]}}}" ;;
  *'"method":"session/load"'*)
    echo "{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{\"models\":{\"currentModelId\":\"fake-fast\",\"availableModels\":[{\"modelId\":\"fake-fast\"},{\"modelId\":\"fake-deep\"}]},\"modes\":{\"availableModes\":[{\"id\":\"build\"},{\"id\":\"plan\"}]}}}" ;;
  *'"method":"session/set_'*)
    echo "{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{}}" ;;
  *'"method":"session/prompt"'*)
    case "$line" in *WRITE*) printf 'written by the fake agent (process %s)\n' "$$" > e2e.txt ;; esac
    case "$line" in *SLOW*) sleep 60 ;; esac
    case "$line" in
    *FAIL*)
      echo "{\"jsonrpc\":\"2.0\",\"id\":$id,\"error\":{\"code\":-32603,\"message\":\"the fake agent failed\"}}" ;;
    *)
      echo '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"fake-1","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"Fake agent done."}}}}'
      echo "{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{\"stopReason\":\"end_turn\",\"usage\":{\"inputTokens\":100,\"outputTokens\":10,\"totalTokens\":110}}}" ;;
    esac ;;
  esac
done
