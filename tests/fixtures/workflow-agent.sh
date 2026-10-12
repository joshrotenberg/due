#!/bin/sh
# Synthetic native Claude stream protocol. Each invocation has its own cwd files.
set -eu
if [ "${DUE_WORKFLOW_TRACE_LIMIT+x}" = x ]; then
    printf leaked > "$DUE_WORKFLOW_TEST_DIR/internal-option-leaked"
fi
for arg in "$@"; do
    if [ "$arg" = '--workflow-trace-limit' ]; then
        printf leaked > "$DUE_WORKFLOW_TEST_DIR/internal-option-leaked"
    fi
done
prompt=$(/bin/cat)
printf '%s\n' "$$" >> "$DUE_WORKFLOW_TEST_DIR/pids"
printf '%s\n' "$prompt" >> "$DUE_WORKFLOW_TEST_DIR/prompts"
printf '%s\n' '{"type":"system","subtype":"init","session_id":"workflow-fake","tools":[]}'
case "$prompt" in
    block-request*) printf collision > "$DUE_WORKFLOW_TEST_DIR/artifacts/steps/b/request.json" ;;
    huge-result-bad-exit*) printf collision > "$DUE_WORKFLOW_TEST_DIR/artifacts/steps/a/exit.json" ;;
esac
case "$prompt" in
    slow*)
        printf '%s' "$$" > "$DUE_WORKFLOW_TEST_DIR/slow-pid"
        printf ready > "$DUE_WORKFLOW_TEST_DIR/slow-ready"
        /bin/sleep 1000 &
        descendant=$!
        printf '%s\n' "$descendant" >> "$DUE_WORKFLOW_TEST_DIR/pids"
        wait "$descendant" ;;
    fast*)
        while [ ! -f "$DUE_WORKFLOW_TEST_DIR/slow-ready" ]; do /bin/sleep 0.01; done ;;
    replacement*)
        kill -0 "$(/bin/cat "$DUE_WORKFLOW_TEST_DIR/slow-pid")"
        printf ready > "$DUE_WORKFLOW_TEST_DIR/replacement-ready" ;;
    barrier-*)
        label=${prompt#barrier-}
        printf ready > "$DUE_WORKFLOW_TEST_DIR/$label-ready"
        while [ ! -f "$DUE_WORKFLOW_TEST_DIR/$label-release" ]; do /bin/sleep 0.01; done ;;
    trace-flood*)
        i=0
        while [ "$i" -lt 4000 ]; do
            printf '%s\n' '{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"x"}]}}'
            i=$((i + 1))
            if [ $((i % 100)) -eq 0 ]; then /bin/sleep 0.01; fi
        done ;;
    hold*)
        /bin/sleep 1000 &
        descendant=$!
        printf '%s\n' "$descendant" >> "$DUE_WORKFLOW_TEST_DIR/pids"
        printf '%s\n' ready > "$DUE_WORKFLOW_TEST_DIR/ready"
        wait "$descendant"
        ;;
    first*)
        # Wait for the other arm to reach the protocol boundary; no timing race.
        while [ ! -f "$DUE_WORKFLOW_TEST_DIR/second-ready" ]; do /bin/sleep 0.01; done
        ;;
    second*) printf ready > "$DUE_WORKFLOW_TEST_DIR/second-ready" ;;
esac
case "$prompt" in
    mutate-schema*) printf 'false\n' > "$DUE_WORKFLOW_TEST_DIR/answer.schema.json" ;;
    huge-stderr*) /bin/dd if=/dev/zero bs=1024 count=1100 2>/dev/null | /usr/bin/tr '\000' 'x' >&2 ;;
    huge-result*)
        printf '%s' '{"type":"result","subtype":"success","session_id":"workflow-fake","is_error":false,"result":"'
        /bin/dd if=/dev/zero bs=1024 count=1100 2>/dev/null | /usr/bin/tr '\000' 'x'
        printf '%s\n' '","structured_output":{"approved":true}}'
        exit 0 ;;
esac
case "$prompt" in
    large-answer|deep-answer)
        printf '%s' '{"type":"result","subtype":"success","session_id":"workflow-fake","is_error":false,"structured_output":'
        if [ "$prompt" = large-answer ]; then
            printf '%s' '{"payload":"'
            /bin/dd if=/dev/zero bs=1000 count=400 2>/dev/null | /usr/bin/tr '\000' 'x'
            printf '%s' '"}'
        else
            i=0; while [ "$i" -lt 32 ]; do printf '['; i=$((i + 1)); done
            printf true
            i=0; while [ "$i" -lt 32 ]; do printf ']'; i=$((i + 1)); done
        fi
        printf '%s\n' '}'
        exit 0 ;;
    invalid*) answer='{"approved":"wrong type"}' ;;
    null*) answer='null' ;;
    missing*)
        printf '%s\n' '{"type":"result","subtype":"success","session_id":"workflow-fake","result":"not JSON","is_error":false}'
        exit 0 ;;
    fail*)
        printf '%s\n' '{"type":"result","subtype":"error_during_execution","session_id":"workflow-fake","result":"partial","is_error":true,"errors":["synthetic failure"]}'
        exit 9 ;;
    *) answer='{"approved":true,"needs_more_evidence":false,"findings":[]}' ;;
esac
printf '%s\n' "{\"type\":\"result\",\"subtype\":\"success\",\"session_id\":\"workflow-fake\",\"result\":\"readable\",\"is_error\":false,\"structured_output\":$answer}"
