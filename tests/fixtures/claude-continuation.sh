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
case "${DUE_CONTINUATION_MODE:-success}" in
    failed) second='{"type":"result","subtype":"error_during_execution","result":"PHASE_TWO","structured_output":{"partial":true},"is_error":true,"session_id":"last","total_cost_usd":0.03,"usage":{"input_tokens":2,"cache_read_input_tokens":3,"cache_creation_input_tokens":5,"output_tokens":13},"errors":["native failure"]}' ;;
    missing) first='{"type":"result","subtype":"success","result":"PHASE_ONE","is_error":false,"usage":{"output_tokens":11}}' ;;
    malformed) first='{"type":"result","subtype":"success","result":42,"is_error":false,"usage":{"output_tokens":11}}' ;;
    overflow) first='{"type":"result","subtype":"success","result":"PHASE_ONE","is_error":false,"usage":{"output_tokens":18446744073709551615}}' ;;
    adjacent) : ;;
    child) : ;;
esac
if [ "$stream" = true ]; then
    printf '%s\n' '{"type":"system","subtype":"init","session_id":"first"}'
    printf '%s\n' "$first"
    if [ "${DUE_CONTINUATION_MODE:-success}" = child ]; then
        printf '%s\n' '{"type":"result","subtype":"error_during_execution","is_error":true,"result":"CHILD","parent_tool_use_id":"child-tool","usage":{"output_tokens":999}}'
    fi
    if [ "${DUE_CONTINUATION_MODE:-success}" != adjacent ]; then
        printf '%s\n' '{"type":"system","subtype":"init","session_id":"last"}'
        printf '%s\n' '{"type":"assistant","message":{"content":[{"type":"text","text":"PHASE_TWO"}]}}'
    fi
    printf '%s\n' "$second"
else
    printf '[%s,' "$first"
    if [ "${DUE_CONTINUATION_MODE:-success}" = child ]; then
        printf '%s,' '{"type":"result","subtype":"error_during_execution","is_error":true,"result":"CHILD","parent_tool_use_id":"child-tool","usage":{"output_tokens":999}}'
    fi
    if [ "${DUE_CONTINUATION_MODE:-success}" != adjacent ]; then
        printf '%s,' '{"type":"system","subtype":"init","session_id":"last"}'
        printf '%s,' '{"type":"assistant","message":{"content":[{"type":"text","text":"PHASE_TWO"}]}}'
    fi
    printf '%s]\n' "$second"
fi
if [ "${DUE_CONTINUATION_MODE:-success}" = exit9 ]; then
    printf 'native diagnostic\n' >&2
    exit 9
fi
