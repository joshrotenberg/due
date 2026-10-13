#!/bin/sh
# Synthetic Claude stream: cancellation must join A while B answers a challenge.
set -eu
prompt=$(/bin/cat)
printf '%s\n' "$$" >> "$DUE_WORKFLOW_TEST_DIR/pids"
printf '%s\n' "$prompt" >> "$DUE_WORKFLOW_TEST_DIR/prompts"
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sibling-fake","tools":[]}'
case "$prompt" in
    barrier-a)
        /bin/sleep 1000 &
        descendant=$!
        printf '%s\n' "$descendant" >> "$DUE_WORKFLOW_TEST_DIR/pids"
        printf '%s\n' "$descendant" > "$DUE_WORKFLOW_TEST_DIR/a-descendant"
        cleanup() {
            kill -TERM "$descendant"
            wait "$descendant" || :
            printf joined > "$DUE_WORKFLOW_TEST_DIR/a-cleaned"
            exit 0
        }
        trap cleanup INT TERM HUP
        printf partial > "$DUE_WORKFLOW_TEST_DIR/a-effect"
        printf 'partial diagnostic\n' >&2
        printf '%s\n' '{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"partial"}]}}'
        printf ready > "$DUE_WORKFLOW_TEST_DIR/a-ready"
        wait "$descendant"
        printf unexpected > "$DUE_WORKFLOW_TEST_DIR/a-post-cancel" ;;
    barrier-b)
        printf ready > "$DUE_WORKFLOW_TEST_DIR/b-ready"
        while [ ! -f "$DUE_WORKFLOW_TEST_DIR/b-release" ]; do
            if [ -f "$DUE_WORKFLOW_TEST_DIR/b-challenge" ]; then
                printf alive > "$DUE_WORKFLOW_TEST_DIR/b-alive"
            fi
            /bin/sleep 0.01
        done ;;
    ordinary)
        printf ready > "$DUE_WORKFLOW_TEST_DIR/third-ready"
        while [ ! -f "$DUE_WORKFLOW_TEST_DIR/go" ]; do /bin/sleep 0.01; done ;;
    *) printf unexpected > "$DUE_WORKFLOW_TEST_DIR/unexpected-launch" ;;
esac
printf '%s\n' '{"type":"result","subtype":"success","session_id":"sibling-fake","is_error":false,"structured_output":{"approved":true}}'
