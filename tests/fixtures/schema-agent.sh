#!/bin/sh
# Both captured terminal protocols, with configurable structured answer data.
set -eu
printf '%s\0' "$@" > "$DUE_SCHEMA_DIR/argv"
/bin/cat > "$DUE_SCHEMA_DIR/stdin"
schema=''
last_message=''
format='json'
codex_session=''
codex_resume=false
previous=''
while [ "$#" -gt 0 ]; do
    case "$1" in
        resume) codex_resume=true ;;
        --json-schema|--output-schema) shift; schema="$1" ;;
        --json-schema=*|--output-schema=*) schema="${1#*=}" ;;
        --output-last-message|-o) shift; last_message="$1" ;;
        --output-last-message=*) last_message="${1#*=}" ;;
        --output-format) shift; format="$1" ;;
        -) if [ "$codex_resume" = true ]; then codex_session="$previous"; fi ;;
    esac
    previous="$1"
    shift
done
if [ -z "$codex_session" ]; then codex_session='schema-session'; fi
if [ -n "${DUE_SCHEMA_ORIGINAL:-}" ]; then printf 'false\n' > "$DUE_SCHEMA_ORIGINAL"; fi
if [ -n "$schema" ]; then
    printf '%s' "$schema" > "$DUE_SCHEMA_DIR/schema-arg"
    if [ "$DUE_SCHEMA_PROVIDER" = claude ]; then
        printf '%s' "$schema" > "$DUE_SCHEMA_DIR/schema-content"
    else
        /bin/cat "$schema" > "$DUE_SCHEMA_DIR/schema-content"
    fi
fi

if [ "$DUE_SCHEMA_PROVIDER" = claude ]; then
    if [ "$format" = stream-json ]; then
        printf '%s\n' '{"type":"system","subtype":"init","session_id":"schema-session","tools":[]}'
    fi
    if [ "$DUE_SCHEMA_MODE" = failure ] || [ "$DUE_SCHEMA_MODE" = failure_null ]; then
        printf '%s' '{"type":"result","subtype":"error_during_execution","session_id":"schema-session","result":"PARTIAL ANSWER","is_error":true,"errors":["fixture provider failure"],"usage":{"input_tokens":120,"output_tokens":45}'
        if [ "$DUE_SCHEMA_MODE" = failure_null ]; then printf ',"structured_output":null'; fi
        printf '}\n'
        exit 9
    fi
    printf '%s' '{"type":"result","subtype":"success","session_id":"schema-session","result":"A readable answer","is_error":false,"usage":{"input_tokens":120,"output_tokens":45}'
    if [ "$DUE_SCHEMA_MODE" != missing ] && [ "$DUE_SCHEMA_MODE" != malformed ]; then
        printf ',"structured_output":%s' "$DUE_SCHEMA_PAYLOAD"
    fi
    printf '}\n'
else
    printf '{"type":"thread.started","thread_id":"%s"}\n' "$codex_session"
    printf '%s\n' '{"type":"turn.started"}'
    if [ "$DUE_SCHEMA_MODE" = failure ] || [ "$DUE_SCHEMA_MODE" = failure_null ]; then
        if [ -n "$last_message" ]; then
            if [ "$DUE_SCHEMA_MODE" = failure_null ]; then printf 'null\n' > "$last_message";
            else printf 'PARTIAL ANSWER\n' > "$last_message"; fi
        fi
        printf '%s\n' '{"type":"item.completed","item":{"id":"answer","type":"agent_message","text":"PARTIAL ANSWER"}}'
        printf '%s\n' '{"type":"turn.failed","error":{"message":"fixture provider failure"}}'
        exit 9
    fi
    if [ -n "$last_message" ]; then
        case "$DUE_SCHEMA_MODE" in
            missing) /bin/rm -f "$last_message" ;;
            malformed) printf '{broken json\n' > "$last_message" ;;
            *) printf '%s\n' "$DUE_SCHEMA_PAYLOAD" > "$last_message" ;;
        esac
    fi
    printf '%s\n' '{"type":"item.completed","item":{"id":"answer","type":"agent_message","text":"native answer"}}'
    printf '%s\n' '{"type":"turn.completed","usage":{"input_tokens":120,"output_tokens":45}}'
fi
