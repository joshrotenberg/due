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
    if [ "$DUE_TRACE_MODE" = semantic ] || [ "$DUE_TRACE_MODE" = nonzero ]; then
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
