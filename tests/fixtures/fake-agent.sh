#!/bin/sh
# Deterministic protocol fixture, never a model invocation.
# Shapes follow claude-wrapper/tests/fake-claude.sh and
# codex-wrapper/crates/codex-wrapper/tests/fake-codex.sh. The Codex fixture
# uses the observed thread.started/item.completed/turn.completed vocabulary.
set -eu

printf '%s\0' "$@" > "$DUE_TEST_DIR/argv"
/bin/pwd > "$DUE_TEST_DIR/cwd"
/bin/cat > "$DUE_TEST_DIR/stdin"

last_message=''
schema=''
codex_thread='codex-test-thread'
codex_resuming=false
previous=''
while [ "$#" -gt 0 ]; do
    case "$1" in
        resume) codex_resuming=true ;;
        -) if [ "$codex_resuming" = true ]; then codex_thread="$previous"; fi ;;
        --output-schema) shift; schema="$1" ;;
        --output-last-message|-o)
            shift
            last_message="$1"
            ;;
        --output-last-message=*) last_message="${1#*=}" ;;
    esac
    previous="$1"
    shift
done
if [ -n "$schema" ]; then /bin/cat "$schema" > "$DUE_TEST_DIR/schema"; fi

# Incomplete Codex turns: received messages are evidence, not a final answer.
case "$DUE_TEST_MODE" in
    codex_partial_*|codex_no_message_*)
        printf '%s\n' '{"type":"thread.started","thread_id":"codex-test-thread"}'
        printf '%s\n' '{"type":"turn.started"}'
        case "$DUE_TEST_MODE" in
            codex_partial_*)
                printf '%s\n' '{"type":"item.completed","item":{"id":"earlier","type":"agent_message","text":"EARLIER MESSAGE"}}'
                partial_text='"SYNTHETIC PARTIAL ANSWER"'
                printf '{"type":"item.completed","item":{"id":"partial","type":"agent_message","text":%s}}\n' "${DUE_TEST_PARTIAL_TEXT:-$partial_text}"
                ;;
        esac
        # This later tool text must never be substituted for assistant text.
        printf '%s\n' '{"type":"item.completed","item":{"id":"partial-ready","type":"command_execution","command":"SYNTHETIC COMMAND","aggregated_output":"SYNTHETIC TOOL OUTPUT","status":"completed","exit_code":0}}'
        case "$DUE_TEST_MODE" in
            *_exit) exit 9 ;;
            *_eof) exit 0 ;;
        esac
        ;;
esac

case "$DUE_TEST_MODE" in
    success_stderr) printf 'fixture progress diagnostic\n' >&2 ;;
    auth)
        printf 'Not authenticated. Run `claude login`.\n' >&2
        exit 3
        ;;
    codex_auth)
        printf 'Missing bearer or basic authentication\n' >&2
        exit 3
        ;;
    codex_config)
        printf 'Error loading config.toml: fixture invalid configuration\n' >&2
        exit 3
        ;;
    codex_session)
        printf 'no rollout found for thread id fixture-missing-session\n' >&2
        exit 3
        ;;
    max_turns)
        # Captured shape from claude-wrapper/src/error.rs MAX_TURNS_STDOUT.
        printf '%s\n' '{"type":"result","subtype":"error_max_turns","is_error":true,"num_turns":2,"session_id":"cap-session","total_cost_usd":0.08,"terminal_reason":"max_turns","errors":["Reached maximum number of turns (1)"]}'
        exit 1
        ;;
    max_budget)
        # Captured shape from claude-wrapper/src/error.rs MAX_BUDGET_STDOUT.
        printf '%s\n' '{"type":"result","subtype":"error_max_budget_usd","is_error":true,"errors":["Reached maximum budget ($0.01)"],"num_turns":1,"total_cost_usd":0.1273986,"session_id":"cap-session"}'
        exit 1
        ;;
    interruptible|codex_partial_stop|codex_no_message_stop)
        # Native CLIs use SIGINT to clean up tool sessions. A successful exit
        # during that cleanup must not erase Due's latched stop reason.
        trap 'printf "SIGINT\n" > "$DUE_TEST_DIR/interrupt"; /bin/kill "$child" 2>/dev/null || :; wait "$child" 2>/dev/null || :; exit 0' INT
        /bin/sleep 30 &
        child=$!
        printf '%s\n%s\n' "$$" "$child" > "$DUE_TEST_DIR/pids"
        wait "$child"
        exit 0
        ;;
    blocked)
        /bin/sleep 30 &
        child=$!
        printf '%s\n%s\n' "$$" "$child" > "$DUE_TEST_DIR/pids"
        wait "$child"
        exit 0
        ;;
    nonzero)
        printf 'fixture provider failure\n' >&2
        exit 7
        ;;
    malformed)
        printf '{this is not JSON}\n'
        exit 0
        ;;
    nonzero_terminal)
        printf 'fixture failure diagnostic\n' >&2
        if [ "$DUE_TEST_PROVIDER" = claude ]; then
            printf '%s\n' '{"type":"result","subtype":"error_during_execution","result":"PARTIAL ANSWER","session_id":"claude-test-session","is_error":true,"total_cost_usd":0.5}'
        else
            printf '%s\n' '{"type":"thread.started","thread_id":"codex-test-thread"}'
            printf '%s\n' '{"type":"item.completed","item":{"id":"item_0","type":"agent_message","text":"PARTIAL ANSWER"}}'
            printf '%s\n' '{"type":"turn.failed","error":{"message":"fixture terminal failure"}}'
        fi
        exit 9
        ;;
esac

if [ "$DUE_TEST_PROVIDER" = claude ]; then
    case "$DUE_TEST_MODE" in
        missing_terminal) printf '{}\n' ;;
        verbose_array)
            # --verbose --output-format json wraps system/assistant/result
            # records in an array; inner record shapes match the wrapper fixture.
            printf '%s\n' '[{"type":"system","subtype":"init","session_id":"claude-test-session","tools":[],"mcp_servers":[]},{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"intermediate text"}]},"session_id":"claude-test-session"},{"type":"result","subtype":"success","result":"FINAL ANSWER","session_id":"claude-test-session","total_cost_usd":0.0123,"num_turns":1,"is_error":false}]'
            ;;
        structured_empty_result)
            printf '%s\n' '{"type":"result","subtype":"success","result":"","structured_output":{"answer":42,"ok":true},"session_id":"claude-test-session","is_error":false}'
            ;;
        wrong_type) printf '%s\n' '{"type":17,"subtype":"success","result":"WRONG","is_error":false}' ;;
        wrong_subtype_type) printf '%s\n' '{"type":"result","subtype":17,"result":"WRONG","is_error":false}' ;;
        wrong_error_type) printf '%s\n' '{"type":"result","subtype":"success","result":"WRONG","is_error":"false"}' ;;
        wrong_result_type) printf '%s\n' '{"type":"result","subtype":"success","result":17,"is_error":false}' ;;
        wrong_session_type) printf '%s\n' '{"type":"result","subtype":"success","result":"WRONG","session_id":17,"is_error":false}' ;;
        semantic_failure)
            printf '%s\n' '{"type":"result","subtype":"error_during_execution","result":"fixture semantic failure","session_id":"claude-test-session","is_error":true}'
            ;;
        *)
            printf '%s\n' '{"type":"result","subtype":"success","result":"FINAL ANSWER","session_id":"claude-test-session","total_cost_usd":0.0123,"duration_ms":10,"num_turns":1,"is_error":false,"usage":{"input_tokens":120,"output_tokens":45}}'
            ;;
    esac
else
    printf '{"type":"thread.started","thread_id":"%s"}\n' "$codex_thread"
    printf '%s\n' '{"type":"turn.started"}'
    printf '%s\n' '{"type":"item.completed","item":{"id":"item_0","type":"agent_message","text":"Interim commentary should not be the final answer."}}'
    case "$DUE_TEST_MODE" in
        semantic_failure)
            printf '%s\n' '{"type":"turn.failed","error":{"message":"fixture semantic failure"}}'
            ;;
        missing_terminal) ;;
        *)
            if [ -n "$last_message" ]; then
                if [ "$DUE_TEST_MODE" = missing_final ]; then
                    /bin/rm -f "$last_message"
                elif [ "$DUE_TEST_MODE" = structured ]; then
                    printf '%s\n' "$DUE_TEST_ANSWER" > "$last_message"
                else
                    printf 'FINAL ANSWER\n' > "$last_message"
                fi
            fi
            printf '%s\n' '{"type":"item.completed","item":{"id":"item_1","type":"agent_message","text":"FINAL ANSWER"}}'
            printf '%s\n' '{"type":"turn.completed","usage":{"input_tokens":120,"cached_input_tokens":0,"cache_write_input_tokens":0,"output_tokens":45,"reasoning_output_tokens":0}}'
            ;;
    esac
fi
