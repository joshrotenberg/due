#!/bin/sh
# Synthetic provider output only. Payload is emitted one line at a time.
set -eu
printf '%s\n' "$$" > "$DUE_BOUNDS_PID"
last_message=''
stream=false
while [ "$#" -gt 0 ]; do
    case "$1" in
        --output-last-message|-o) shift; last_message="$1" ;;
        --output-last-message=*) last_message="${1#*=}" ;;
        stream-json) stream=true ;;
    esac
    shift
done
/bin/cat >/dev/null
case "$DUE_BOUNDS_MODE" in
    stdout_limit)
        if [ "$DUE_BOUNDS_PROVIDER" = codex ]; then
            printf '%s\n' '{"type":"thread.started","thread_id":"observed"}'
            printf '%s\n' '{"type":"item.completed","item":{"type":"agent_message","text":"partial"}}'
        elif [ "$stream" = true ]; then
            printf '%s\n' '{"type":"system","subtype":"init","session_id":"observed"}'
        fi
        awk 'BEGIN { for (j=0;j<32000;j++) payload=payload "x"; for (i=0;i<2200;i++) print "{\"type\":\"noise\",\"value\":\"" payload "\"}" }'
        ;;
    stderr_limit)
        if [ "$DUE_BOUNDS_PROVIDER" = codex ]; then
            printf '%s\n' '{"type":"thread.started","thread_id":"observed"}'
            printf '%s\n' '{"type":"turn.completed"}'
            printf 'answer\n' > "$last_message"
        else
            printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"result":"answer","session_id":"observed"}'
        fi
        awk 'BEGIN { for (j=0;j<32000;j++) payload=payload "e"; for (i=0;i<2200;i++) print payload }' >&2
        ;;
    final_exact|final_over|final_symlink|final_fifo|failed_over|diagnostic)
        if [ "$DUE_BOUNDS_PROVIDER" = codex ]; then
            printf '%s\n' '{"type":"thread.started","thread_id":"observed"}'
            printf '%s\n' '{"type":"item.completed","item":{"type":"agent_message","text":"partial"}}'
            case "$DUE_BOUNDS_MODE" in
                final_exact) dd if=/dev/zero bs=1048576 count=4 2>/dev/null | tr '\000' x > "$last_message" ;;
                final_over|failed_over) dd if=/dev/zero bs=1048576 count=4 2>/dev/null | tr '\000' x > "$last_message"; printf x >> "$last_message" ;;
                final_symlink) ln -s /etc/passwd "$last_message" ;;
                final_fifo) mkfifo "$last_message" ;;
                *) printf 'answer\n' > "$last_message" ;;
            esac
            if [ "$DUE_BOUNDS_MODE" = failed_over ]; then
                printf '%s\n' '{"type":"turn.failed","error":{"message":"native failure"}}'
                exit 9
            fi
            printf '%s\n' '{"type":"turn.completed"}'
        else
            printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"result":"answer","session_id":"observed"}'
        fi
        if [ "$DUE_BOUNDS_MODE" = diagnostic ]; then
            awk 'BEGIN { for (i=0;i<9000;i++) printf "é"; printf "\n" }' >&2
            exit 9
        fi
        ;;
esac
