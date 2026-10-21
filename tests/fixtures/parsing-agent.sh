#!/bin/sh
# Established top-level event shapes from the existing fixtures. Non-event
# lines, bad JSON and malformed bytes below are synthetic transport faults.
set -eu
/bin/cat > "$DUE_TRACE_DIR/stdin"
printf '%s\n' "$$" > "$DUE_TRACE_DIR/processes"
printf 'SECRET_PROVIDER_STDERR\n' >&2
last_message=''
while [ "$#" -gt 0 ]; do
    case "$1" in
        --output-last-message|-o) shift; last_message="$1" ;;
    esac
    shift
done
mode="$DUE_TRACE_MODE"
if [ "$DUE_TRACE_PROVIDER" = codex ]; then
    case "$mode" in
        ignored) printf '\n  diagnostic SECRET_IGNORED\n[1,2]\nnull\n \r\n' ;;
        utf8_stderr) printf 'invalid stderr: \377\n' >&2 ;;
        utf8) printf '{"type":"turn.started","unused":"\377"}\n' ;;
        replacement) printf '{"type":"turn.started","unused":"\357\277\275"}\n' ;;
        bad_first) printf '{broken\n'; exit 0 ;;
    esac
    printf '  {"type":"thread.started","thread_id":"trace-session"}\r\n'
    if [ "$mode" != no_message ]; then
        printf '%s\n' '{"type":"item.completed","item":{"id":"msg","type":"agent_message","text":"PARTIAL ANSWER"}}'
    fi
    case "$mode" in
        bad_before|bad_before_exit9)
            printf '{"type":"item.completed","item":{"type":"agent_message","text":"SECRET_REJECTED"}\n'
            if [ "$mode" = bad_before_exit9 ]; then exit 9; fi
            exit 0 ;;
        truncated_before) printf '{"type":'; exit 0 ;;
        eof|no_message) exit 0 ;;
        eof9) exit 9 ;;
    esac
    if [ "$mode" = failed ]; then
        printf '%s\n' '{"type":"turn.failed","error":{"message":"native terminal failure"}}'
        exit 9
    fi
    if [ "$mode" != missing_file ]; then printf 'PARTIAL ANSWER\n' > "$last_message"; fi
    printf '%s' '{"type":"turn.completed","usage":{"input_tokens":11,"output_tokens":3}}'
    case "$mode" in
        no_newline) ;;
        crlf) printf '\r\n' ;;
        bad_after|bad_after_exit9) printf '\n{broken}\n' ;;
        truncated_after) printf '\n{"type":' ;;
        *) printf '\n' ;;
    esac
    if [ "$mode" = exit9 ] || [ "$mode" = bad_after_exit9 ]; then exit 9; fi
    exit 0
fi

printf '%s\n' '{"type":"system","subtype":"init","session_id":"trace-session"}'
case "$mode" in
    empty|no_text) ;;
    string) printf '%s\n' '{"type":"assistant","message":{"role":"assistant","content":"PARTIAL ANSWER"}}' ;;
    json_partial) printf '%s\n' '{"type":"assistant","message":{"role":"assistant","content":"{\"answer\":42}"}}' ;;
    *)
        printf '%s\n' '{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"older message"}]}}'
        printf '%s\n' '{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"PARTIAL "},{"type":"thinking","thinking":"SECRET_THINKING"},{"type":"text","text":"ANSWER"}]}}'
        ;;
esac
# Later tool-only, empty text, user tool output, nested messages and deltas
# must not replace the complete text-bearing assistant message.
printf '%s\n' '{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"tool-1","name":"Bash","input":{"command":"SECRET_TOOL_INPUT"}},{"type":"thinking","thinking":"SECRET_THINKING"}]}}'
printf '%s\n' '{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":""}]}}'
printf '%s\n' '{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"tool-1","content":"SECRET_TOOL_OUTPUT"}]}}'
printf '%s\n' '{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"SECRET_DELTA"}}}'
# Synthetic unknown envelope, not a claimed native schema. Nested assistant-like
# data must not become a complete top-level message.
printf '%s\n' '{"type":"future_event","message":{"type":"assistant","content":"SECRET_NESTED"}}'
case "$mode" in
    cancel|timeout)
        /bin/sleep 30 &
        descendant=$!
        printf '%s\n%s\n' "$$" "$descendant" > "$DUE_TRACE_DIR/processes"
        : > "$DUE_TRACE_DIR/ready"
        wait "$descendant"
        ;;
    bad) printf '{broken}\n' ;;
    eof9) exit 9 ;;
    success|success9)
        printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"result":"TERMINAL ANSWER","session_id":"trace-session"}'
        if [ "$mode" = success9 ]; then exit 9; fi
        ;;
    empty_success) printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"result":"","session_id":"trace-session"}' ;;
    failed|failed9|empty_failed)
        text='TERMINAL FAILURE'
        if [ "$mode" = empty_failed ]; then text=''; fi
        printf '{"type":"result","subtype":"error_during_execution","is_error":true,"result":"%s","session_id":"trace-session"}\n' "$text"
        if [ "$mode" = failed9 ]; then exit 9; fi
        ;;
    max_turns|max_budget)
        subtype=error_max_turns
        if [ "$mode" = max_budget ]; then subtype=error_max_budget_usd; fi
        printf '{"type":"result","subtype":"%s","is_error":true,"result":"","session_id":"trace-session"}\n' "$subtype"
        exit 9
        ;;
    structured) printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"result":"","structured_output":{"answer":42},"session_id":"trace-session"}' ;;
    empty) : ;;
esac
