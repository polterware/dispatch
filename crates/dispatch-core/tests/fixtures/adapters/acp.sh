#!/bin/sh
[ "$1" = "acp" ] || exit 2
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$SENT"
  # The request's own id, wherever serde_json puts it among the top-level keys.
  id=$(printf '%s' "$line" | sed -n 's/^{[^{]*"id":\([0-9][0-9]*\).*/\1/p')
  case "$line" in
  *'"method":"initialize"'*)
    echo '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1,"agentCapabilities":{"loadSession":true}}}' ;;
  *'"method":"session/new"'*)
    echo '{"jsonrpc":"2.0","id":2,"result":{"sessionId":"s-1","models":{"availableModels":[{"modelId":"m-a"},{"modelId":"m-b"}]},"modes":{"availableModes":[{"id":"build"},{"id":"plan"}]},"configOptions":[{"id":"effort","category":"thought_level","options":[{"value":"low"},{"value":"high"}]}]}}' ;;
  *'"method":"session/load"'*)
    echo '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s-0","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"OLD TURN"}}}}'
    echo '{"jsonrpc":"2.0","id":2,"result":{"modes":{"availableModes":[{"id":"plan"}]}}}' ;;
  *'"method":"session/set_'*)
    echo "{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{}}" ;;
  *'"method":"session/prompt"'*)
    echo "$id" > "$SENT.prompt"
    echo '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s-1","update":{"sessionUpdate":"available_commands_update","availableCommands":[]}}}'
    echo '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s-1","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"Looking"}}}}'
    echo '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s-1","update":{"sessionUpdate":"tool_call","toolCallId":"t1","title":"read","kind":"read","rawInput":{"filePath":"/repo/README.md"}}}}'
    echo '{"jsonrpc":"2.0","id":"p-1","method":"session/request_permission","params":{"sessionId":"s-1","toolCall":{"toolCallId":"t2","kind":"edit","title":"Edit README.md"},"options":[{"optionId":"ok","kind":"allow_once"},{"optionId":"no","kind":"reject_once"}]}}' ;;
  *'"id":"p-1"'*)
    prompt=$(cat "$SENT.prompt")
    echo '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s-1","update":{"sessionUpdate":"agent_message_chunk","messageId":"m2","content":{"type":"text","text":"Done."}}}}'
    echo '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s-1","update":{"sessionUpdate":"usage_update","used":5000,"size":200000,"cost":{"amount":0.25,"currency":"USD"}}}}'
    echo "{\"jsonrpc\":\"2.0\",\"id\":$prompt,\"result\":{\"stopReason\":\"${STOP:-end_turn}\",\"usage\":{\"inputTokens\":1000,\"outputTokens\":50,\"cachedReadTokens\":4000,\"thoughtTokens\":10,\"totalTokens\":5060}}}" ;;
  esac
done
