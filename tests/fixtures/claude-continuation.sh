#!/bin/sh
set -eu
/bin/cat >/dev/null
stream=false
verbose=false
previous=
for arg in "$@"; do
    if [ "$previous" = format ] && [ "$arg" = stream-json ]; then stream=true; fi
    if [ "$arg" = --verbose ]; then verbose=true; fi
    if [ "$arg" = --output-format ]; then previous=format; else previous=; fi
done
if [ "$verbose" != true ]; then exit 64; fi
first='{"type":"result","subtype":"success","result":"PHASE_ONE","is_error":false,"result_index":0,"session_id":"first","total_cost_usd":0.01,"usage":{"input_tokens":4,"cache_read_input_tokens":7,"cache_creation_input_tokens":9,"output_tokens":11}}'
second='{"type":"result","subtype":"success","result":"PHASE_TWO","is_error":false,"result_index":1,"session_id":"last","total_cost_usd":0.03,"usage":{"input_tokens":2,"cache_read_input_tokens":3,"cache_creation_input_tokens":5,"output_tokens":13}}'
mode=${DUE_CONTINUATION_MODE:-success}
case "${DUE_CONTINUATION_MODE:-success}" in
    failed) second='{"type":"result","subtype":"error_during_execution","result":"PHASE_TWO","structured_output":{"partial":true},"is_error":true,"session_id":"last","total_cost_usd":0.03,"usage":{"input_tokens":2,"cache_read_input_tokens":3,"cache_creation_input_tokens":5,"output_tokens":13},"errors":["native failure"]}' ;;
    missing) first='{"type":"result","subtype":"success","result":"PHASE_ONE","is_error":false,"usage":{"output_tokens":11}}' ;;
    malformed) first='{"type":"result","subtype":"success","result":42,"is_error":false,"usage":{"output_tokens":11}}' ;;
    failed_after_malformed)
        first='{"type":"result","subtype":"success","result":42,"is_error":false}'
        second='{"type":"result","subtype":"error_during_execution","result":"PHASE_TWO","is_error":true,"errors":["native failure"]}' ;;
    overflow) first='{"type":"result","subtype":"success","result":"PHASE_ONE","is_error":false,"usage":{"output_tokens":18446744073709551615}}' ;;
    adjacent) : ;;
    child) : ;;
    malformed_final)
        first='{"type":"result","subtype":"success","result":"PHASE_ONE","structured_output":{"answer":1},"is_error":false,"session_id":"first","total_cost_usd":0.01,"usage":{"input_tokens":4,"output_tokens":11}}'
        second='{"type":"result","subtype":"success","result":42,"structured_output":{"answer":999},"is_error":false,"session_id":"bad","total_cost_usd":9,"usage":{"input_tokens":99,"output_tokens":99}}' ;;
    cap_after_malformed|budget_after_malformed|auth_after_malformed)
        first='{"type":"result","subtype":"success","result":42,"is_error":false}'
        case "$mode" in
            cap_after_malformed) subtype=error_max_turns; message='turn cap' ;;
            budget_after_malformed) subtype=error_max_budget_usd; message='budget cap' ;;
            auth_after_malformed) subtype=error_during_execution; message='Failed to authenticate: OAuth session expired and could not be refreshed' ;;
        esac
        second=$(printf '{"type":"result","subtype":"%s","result":"PHASE_TWO","is_error":true,"session_id":"last","errors":["%s"]}' "$subtype" "$message") ;;
    exit_without_terminal|exit_with_invalid_terminal) : ;;
esac
if [ "$mode" = invalid_only_structured ]; then
    invalid='{"type":"result","subtype":"success","is_error":false,"result":42,"structured_output":{"kept":"native-json"}}'
    if [ "$stream" = true ]; then printf '%s\n' "$invalid"; else printf '[%s]\n' "$invalid"; fi
    exit 0
fi
if [ "$mode" = exit_without_terminal ]; then
    if [ "$stream" = true ]; then printf '%s\n' '{"type":"system","subtype":"init"}'; else printf '%s\n' '[{"type":"system","subtype":"init"}]'; fi
    printf 'native diagnostic\n' >&2
    exit 9
fi
if [ "$mode" = exit_with_invalid_terminal ]; then
    if [ "$stream" = true ]; then printf '%s\n' '{"type":"result","is_error":false,"result":42}'; else printf '%s\n' '[{"type":"result","is_error":false,"result":42}]'; fi
    printf 'native diagnostic\n' >&2
    exit 9
fi
if [ "$stream" = true ]; then
    printf '%s\n' '{"type":"system","subtype":"init","session_id":"first"}'
    printf '%s\n' "$first"
    if [ "${DUE_CONTINUATION_MODE:-success}" = child ]; then
        printf '%s\n' '{"type":"result","subtype":"error_during_execution","is_error":true,"result":"CHILD","parent_tool_use_id":"child-tool","usage":{"output_tokens":999}}'
    fi
    if [ "$mode" != adjacent ]; then
        printf '%s\n' '{"type":"system","subtype":"init","session_id":"last"}'
        printf '%s\n' '{"type":"assistant","message":{"content":[{"type":"text","text":"PHASE_TWO"}]}}'
    fi
    if [ "$mode" != unfinished ] && [ "$mode" != timeout_continuation ]; then printf '%s\n' "$second"; fi
    if [ "$mode" = bookkeeping ]; then printf '%s\n' '{"type":"system","subtype":"hook_response"}'; fi
else
    printf '[%s,' "$first"
    if [ "${DUE_CONTINUATION_MODE:-success}" = child ]; then
        printf '%s,' '{"type":"result","subtype":"error_during_execution","is_error":true,"result":"CHILD","parent_tool_use_id":"child-tool","usage":{"output_tokens":999}}'
    fi
    if [ "$mode" != adjacent ]; then
        printf '%s,' '{"type":"system","subtype":"init","session_id":"last"}'
        printf '%s,' '{"type":"assistant","message":{"content":[{"type":"text","text":"PHASE_TWO"}]}}'
    fi
    if [ "$mode" = unfinished ]; then
        printf '{}]\n'
    elif [ "$mode" = bookkeeping ]; then
        printf '%s,%s]\n' "$second" '{"type":"system","subtype":"hook_response"}'
    else
        printf '%s]\n' "$second"
    fi
fi
if [ "$mode" = timeout_continuation ]; then sleep 3; fi
if [ "${DUE_CONTINUATION_MODE:-success}" = exit9 ]; then
    printf 'native diagnostic\n' >&2
    exit 9
fi
