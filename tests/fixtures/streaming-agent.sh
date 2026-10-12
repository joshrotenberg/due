#!/bin/sh
# Deterministic live-provider fixture. Claude follows the wrapper's
# system/assistant/tool_use/user/tool_result/result stream shapes. Codex follows
# thread.started/turn.started/item.started/item.completed/turn.completed.
set -eu
printf '%s\0' "$@" > "$DUE_TRACE_DIR/argv"
/bin/cat > "$DUE_TRACE_DIR/stdin"
printf '%s\n' "$$" > "$DUE_TRACE_DIR/processes"
printf 'SECRET_PROVIDER_STDERR\n' >&2
last_message=''
while [ "$#" -gt 0 ]; do
    case "$1" in
        --output-last-message|-o) shift; last_message="$1" ;;
        --output-last-message=*) last_message="${1#*=}" ;;
    esac
    shift
done

if [ "$DUE_TRACE_PROVIDER" = claude ]; then
    printf '{"type":"system","subtype":"init","session_id":"%s","cwd":"SECRET_CWD","tools":["Bash"],"unknown":"SECRET_UNKNOWN"}\n' "$DUE_TRACE_SESSION"
    printf '%s\n' '{"type":"stream_event","event":{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"tool-test-1","name":"Bash","input":{"command":"SECRET_TOOL_INPUT"}}}}'
    printf '%s\n' '{"type":"stream_event","event":{"type":"content_block_stop","index":0}}'
    printf '{"type":"assistant","session_id":"%s","message":{"id":"msg-tools","role":"assistant","content":[{"type":"tool_use","id":"tool-test-1","name":"Bash","input":{"command":"SECRET_TOOL_INPUT"}},{"type":"text","text":"SECRET_ASSISTANT_TEXT"}]}}\n' "$DUE_TRACE_SESSION"
    printf '%s\n' '{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"SECRET_DELTA"}}}'
    printf '%s\n' '{"type":"future_event","payload":"SECRET_UNKNOWN"}'
else
    printf '{"type":"thread.started","thread_id":"%s","unknown":"SECRET_UNKNOWN"}\n' "$DUE_TRACE_SESSION"
    if [ "$DUE_TRACE_MODE" = diagnostic ] || [ "$DUE_TRACE_MODE" = diagnostic_failed ]; then
        # A nonfatal startup diagnostic with no item.started, then one
        # reported through every item lifecycle shape.
        printf '%s\n' '{"type":"item.completed","item":{"id":"item_0","type":"error","message":"SECRET_DIAGNOSTIC_MESSAGE"}}'
        printf '%s\n' '{"type":"item.started","item":{"id":"item_1","type":"error","message":"SECRET_DIAGNOSTIC_MESSAGE"}}'
        printf '%s\n' '{"type":"item.updated","item":{"id":"item_1","type":"error","message":"SECRET_DIAGNOSTIC_MESSAGE"}}'
        printf '%s\n' '{"type":"item.completed","item":{"id":"item_1","type":"error","message":"SECRET_DIAGNOSTIC_MESSAGE"}}'
    fi
    printf '%s\n' '{"type":"turn.started"}'
    printf '%s\n' '{"type":"item.started","item":{"id":"tool-test-1","type":"command_execution","command":"SECRET_TOOL_INPUT","status":"in_progress"}}'
    printf '%s\n' '{"type":"item.updated","item":{"id":"tool-test-1","type":"command_execution","aggregated_output":"SECRET_DELTA"}}'
    printf '%s\n' '{"type":"future_event","payload":"SECRET_UNKNOWN"}'
fi

case "$DUE_TRACE_MODE" in
    gated)
        : > "$DUE_TRACE_DIR/ready"
        while [ ! -e "$DUE_TRACE_DIR/release" ]; do /bin/sleep 0.05; done
        ;;
    blocked|flood)
        if [ "$DUE_TRACE_MODE" = flood ]; then
            i=0
            while [ "$i" -lt 10000 ]; do
                if [ "$DUE_TRACE_PROVIDER" = claude ]; then
                    printf '%s\n' '{"type":"assistant","message":{"id":"flood-message","role":"assistant","content":[{"type":"text","text":"SECRET_ASSISTANT_TEXT"}]}}'
                else
                    printf '%s\n' '{"type":"item.completed","item":{"id":"flood-message","type":"agent_message","text":"SECRET_ASSISTANT_TEXT"}}'
                fi
                i=$((i + 1))
            done
        fi
        /bin/sleep 30 &
        descendant=$!
        printf '%s\n%s\n' "$$" "$descendant" > "$DUE_TRACE_DIR/processes"
        : > "$DUE_TRACE_DIR/ready"
        wait "$descendant"
        exit 0
        ;;
esac

if [ "$DUE_TRACE_MODE" = diverse ]; then
    if [ "$DUE_TRACE_PROVIDER" = claude ]; then
        for activity_spec in activity-file:Edit activity-mcp:mcp__fixture__lookup activity-web:WebSearch activity-plan:TodoWrite activity-tool:MysteryFutureTool; do
            activity_id="${activity_spec%%:*}"
            activity_tool="${activity_spec#*:}"
            printf '{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"%s","name":"%s","input":{"content":"SECRET_ACTIVITY_INPUT"}}]}}\n' "$activity_id" "$activity_tool"
            printf '{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"%s","content":"SECRET_ACTIVITY_OUTPUT","is_error":false}]}}\n' "$activity_id"
        done
    else
        for activity_kind in file_change mcp_tool_call web_search todo_list status_update future_native_item; do
            printf '{"type":"item.started","item":{"id":"activity-%s","type":"%s","status":"in_progress","arguments":"SECRET_ACTIVITY_INPUT","tool":"lookup"}}\n' "$activity_kind" "$activity_kind"
            activity_status=completed
            if [ "$activity_kind" = future_native_item ]; then activity_status=future_status; fi
            printf '{"type":"item.completed","item":{"id":"activity-%s","type":"%s","status":"%s","payload":"SECRET_ACTIVITY_OUTPUT","tool":"lookup","items":[{"text":"SECRET_PLAN_TEXT","completed":false}],"changes":[{"path":"SECRET_FILE_PATH","kind":"update"}]}}\n' "$activity_kind" "$activity_kind" "$activity_status"
        done
        printf '%s\n' '{"type":"item.started","item":{"id":"activity-cancelled","type":"command_execution","command":"SECRET_COMMAND","status":"in_progress"}}'
        printf '%s\n' '{"type":"item.completed","item":{"id":"activity-cancelled","type":"command_execution","command":"SECRET_COMMAND","status":"cancelled"}}'
        printf '%s\n' '{"type":"item.completed","item":{"id":"reasoning-private","type":"reasoning","text":"SECRET_REASONING_TEXT"}}'
    fi
fi

if [ "$DUE_TRACE_PROVIDER" = claude ]; then
    tool_error=false
    if [ "$DUE_TRACE_MODE" = tool_failure ]; then tool_error=true; fi
    printf '{"type":"user","session_id":"%s","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"tool-test-1","content":"SECRET_TOOL_OUTPUT","is_error":%s}]}}\n' "$DUE_TRACE_SESSION" "$tool_error"
    if [ "$DUE_TRACE_MODE" = semantic ] || [ "$DUE_TRACE_MODE" = nonzero ]; then
        printf '{"type":"result","subtype":"error_during_execution","result":"PARTIAL ANSWER","session_id":"%s","is_error":true,"total_cost_usd":0.0123,"errors":["SECRET_PROVIDER_ERROR"]}\n' "$DUE_TRACE_SESSION"
    else
        printf '{"type":"assistant","session_id":"%s","message":{"id":"msg-final","role":"assistant","content":[{"type":"text","text":"FINAL TRACED ANSWER"}]}}\n' "$DUE_TRACE_SESSION"
        printf '{"type":"result","subtype":"success","result":"FINAL TRACED ANSWER","session_id":"%s","is_error":false,"total_cost_usd":0.0123,"usage":{"input_tokens":120,"output_tokens":45}}\n' "$DUE_TRACE_SESSION"
    fi
else
    if [ "$DUE_TRACE_MODE" = tool_failure ]; then
        printf '%s\n' '{"type":"item.completed","item":{"id":"tool-test-1","type":"command_execution","command":"SECRET_TOOL_INPUT","status":"failed","exit_code":1,"aggregated_output":"SECRET_TOOL_OUTPUT"}}'
    else
        printf '%s\n' '{"type":"item.completed","item":{"id":"tool-test-1","type":"command_execution","command":"SECRET_TOOL_INPUT","status":"completed","exit_code":0,"aggregated_output":"SECRET_TOOL_OUTPUT"}}'
    fi
    if [ "$DUE_TRACE_MODE" = semantic ] || [ "$DUE_TRACE_MODE" = nonzero ] || [ "$DUE_TRACE_MODE" = diagnostic_failed ]; then
        if [ -n "$last_message" ]; then printf 'PARTIAL ANSWER\n' > "$last_message"; fi
        printf '%s\n' '{"type":"item.completed","item":{"id":"message-final","type":"agent_message","text":"PARTIAL ANSWER"}}'
        printf '%s\n' '{"type":"turn.failed","error":{"message":"SECRET_PROVIDER_ERROR"}}'
    else
        if [ -n "$last_message" ]; then printf 'FINAL TRACED ANSWER\n' > "$last_message"; fi
        printf '%s\n' '{"type":"item.completed","item":{"id":"message-final","type":"agent_message","text":"FINAL TRACED ANSWER"}}'
        printf '%s\n' '{"type":"turn.completed","usage":{"input_tokens":120,"output_tokens":45}}'
    fi
fi
if [ "$DUE_TRACE_MODE" = nonzero ]; then exit 9; fi
