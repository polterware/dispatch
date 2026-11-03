#!/bin/sh
[ "$1" = "app-server" ] || exit 2
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$SENT"
  case "$line" in
  *'"method":"initialize"'*) echo '{"id":1,"result":{}}' ;;
  *'"method":"thread/'*)
    echo '{"id":2,"result":{"thread":{"id":"th-1"}}}'
    echo '{"method":"mcpServer/startupStatus/updated","params":{"name":"x","status":"ready"}}' ;;
  *'"method":"turn/start"'*)
    echo '{"id":3,"result":{"turn":{"id":"tu-1","status":"inProgress"}}}'
    echo '{"method":"turn/started","params":{"threadId":"th-1","turn":{"id":"tu-1"}}}'
    echo '{"method":"item/started","params":{"threadId":"th-1","turnId":"tu-1","item":{"type":"userMessage","id":"u1"}}}'
    echo '{"method":"item/agentMessage/delta","params":{"threadId":"th-1","turnId":"tu-1","itemId":"m1","delta":"Hel"}}'
    echo '{"method":"item/completed","params":{"threadId":"th-1","turnId":"tu-1","item":{"type":"agentMessage","id":"m1","text":"Hello","phase":"commentary"}}}'
    echo '{"method":"item/started","params":{"threadId":"th-1","turnId":"tu-1","item":{"type":"commandExecution","id":"c1","command":"ls","status":"inProgress"}}}'
    echo '{"method":"thread/tokenUsage/updated","params":{"threadId":"th-1","turnId":"tu-1","tokenUsage":{"total":{"inputTokens":900},"last":{"inputTokens":1000,"cachedInputTokens":800,"outputTokens":50,"reasoningOutputTokens":20}}}}'
    echo '{"method":"item/completed","params":{"threadId":"th-1","turnId":"tu-1","item":{"type":"agentMessage","id":"m2","text":"Done.","phase":"final_answer"}}}'
    echo '{"method":"thread/tokenUsage/updated","params":{"threadId":"th-1","turnId":"tu-1","tokenUsage":{"last":{"inputTokens":500,"cachedInputTokens":100,"outputTokens":10},"modelContextWindow":272000}}}'
    echo "{\"method\":\"turn/completed\",\"params\":{\"threadId\":\"th-1\",\"turn\":{\"id\":\"tu-1\",\"status\":\"${STATUS:-completed}\"}}}" ;;
  esac
done
