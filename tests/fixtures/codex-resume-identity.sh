#!/bin/sh
# Synthetic native JSONL only. No Codex installation or login state is used.
set -eu
last_message=''
while [ "$#" -gt 0 ]; do
    case "$1" in
        --output-last-message|-o) shift; last_message="$1" ;;
        --output-last-message=*) last_message="${1#*=}" ;;
    esac
    shift
done
/bin/cat >/dev/null
case "$DUE_ID_MODE" in
    auth) printf 'Missing bearer or basic authentication\n' >&2; exit 3 ;;
    exit) printf 'synthetic launch failure\n' >&2; exit 9 ;;
    matching) printf '%s\n' '{"type":"thread.started","thread_id":"Requested-Thread"}' ;;
    case_variant) printf '%s\n' '{"type":"thread.started","thread_id":"REQUESTED-THREAD"}' ;;
    mismatch|structured_mismatch|mismatch_then_exit) printf '%s\n' '{"type":"thread.started","thread_id":"other-thread"}' ;;
    empty) printf '%s\n' '{"type":"thread.started","thread_id":""}' ;;
    missing) printf '%s\n' '{"type":"thread.started"}' ;;
    fallback)
        printf '%s\n' '{"type":"thread.started"}'
        printf '%s\n' '{"type":"future.event","session_id":"unrelated-session"}'
        ;;
    conflict|conflict_failed|conflict_completed_exit)
        printf '%s\n' '{"type":"thread.started","thread_id":"Requested-Thread"}'
        printf '%s\n' '{"type":"future.event","session_id":"other-thread"}'
        ;;
    long_conflict_failed)
        first_id=$(printf '%9000s' '' | tr ' ' A)
        second_id=$(printf '%9000s' '' | tr ' ' B)
        printf '{"type":"thread.started","thread_id":"%s"}\n' "$first_id"
        printf '{"type":"future.event","session_id":"%s"}\n' "$second_id"
        ;;
    repeated_conflict)
        printf '%s\n' '{"type":"thread.started","thread_id":"Requested-Thread"}'
        printf '%s\n' '{"type":"thread.started","thread_id":"other-thread"}'
        ;;
esac
printf '%s\n' '{"type":"turn.started"}'
if [ "$DUE_ID_MODE" = long_conflict_failed ]; then
    printf '%s\n' '{"type":"item.completed","item":{"id":"answer","type":"agent_message","text":"SHORT ANSWER"}}'
    if [ -n "$last_message" ]; then printf 'SHORT ANSWER\n' > "$last_message"; fi
elif [ "$DUE_ID_MODE" = structured_mismatch ]; then
    printf '%s\n' '{"type":"item.completed","item":{"id":"answer","type":"agent_message","text":"{\"answer\":42}"}}'
    if [ -n "$last_message" ]; then printf '{"answer":42}\n' > "$last_message"; fi
else
    printf '%s\n' '{"type":"item.completed","item":{"id":"answer","type":"agent_message","text":"FINAL ANSWER"}}'
    if [ -n "$last_message" ]; then printf 'FINAL ANSWER\n' > "$last_message"; fi
fi
if [ "$DUE_ID_MODE" = conflict_failed ] || [ "$DUE_ID_MODE" = long_conflict_failed ]; then
    printf '%s\n' '{"type":"turn.failed","error":{"message":"synthetic terminal failure"}}'
    exit 9
elif [ "$DUE_ID_MODE" = mismatch_then_exit ]; then
    exit 9
else
    printf '%s\n' '{"type":"turn.completed","usage":{"input_tokens":12,"output_tokens":3}}'
fi
if [ "$DUE_ID_MODE" = conflict_completed_exit ]; then
    exit 9
fi
