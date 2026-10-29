#!/bin/sh
# Deterministic Claude fixture for wrapper auth sub-kinds, never a model
# invocation. Diagnostics are synthetic text chosen to match the pinned
# claude-wrapper classifier (src/auth.rs classify_failure).
set -eu

/bin/cat > /dev/null
stream=false
for arg in "$@"; do
    if [ "$arg" = stream-json ]; then stream=true; fi
done
if [ "$stream" = true ]; then
    printf '%s\n' '{"type":"system","subtype":"init","session_id":"limit-session"}'
fi

case "$DUE_LIMIT_MODE" in
    rate_limit)
        printf 'API Error: 429 {"type":"error","error":{"type":"rate_limit_error","message":"Too Many Requests"}}\n' >&2
        exit 2
        ;;
    quota)
        printf 'Usage quota exceeded for this organization\n' >&2
        exit 2
        ;;
    not_logged_in)
        printf 'Not logged in. Please run /login\n' >&2
        exit 3
        ;;
    invalid_key)
        printf 'Invalid API key. Fix external API key\n' >&2
        exit 3
        ;;
    rate_limit_max_turns)
        printf '%s\n' '{"type":"result","subtype":"error_max_turns","is_error":true,"num_turns":2,"session_id":"limit-session","total_cost_usd":0.08,"errors":["Reached maximum number of turns (1)"]}'
        printf 'API Error: 429 Too Many Requests\n' >&2
        exit 1
        ;;
    rate_limit_blocked)
        printf 'API Error: 429 Too Many Requests\n' >&2
        # A nonzero exit after the stop must not replace Due's stop reason.
        trap '/bin/kill "$child" 2>/dev/null || :; wait "$child" 2>/dev/null || :; exit 2' INT TERM
        /bin/sleep 30 &
        child=$!
        printf '%s\n%s\n' "$$" "$child" > "$DUE_LIMIT_DIR/pids"
        wait "$child"
        exit 2
        ;;
esac
exit 64
