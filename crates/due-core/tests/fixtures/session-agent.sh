#!/usr/bin/env bash
# Resident protocol fixture, never runs tools or contacts a model.
# Codex envelopes follow codex-wrapper e91e627/tests/fake-app-server.sh
# (captured CLI 0.149.0). Claude envelopes follow claude-wrapper 79ffb15
# duplex parser/permission tests. IDs, counters and answer text are synthetic.
set -eu

printf '%s\0' "$@" > "$DUE_SESSION_FIXTURE_DIR/argv"
pwd -P > "$DUE_SESSION_FIXTURE_DIR/native-cwd"

provider=claude
partial_messages=0
for arg in "$@"; do
  [ "$arg" != app-server ] || provider=codex
  [ "$arg" != --include-partial-messages ] || partial_messages=1
done
printf '%s\n' "$$" > "$DUE_SESSION_FIXTURE_DIR/pid"
trap 'touch "$DUE_SESSION_FIXTURE_DIR/exited"' EXIT
count=0
active=0
pending=0
startup_permission=0
interrupt_id=
held_ack=
held_ack_error=0
late_mode=
claude_partial_mode=
native="native-$provider"
turn=turn-0

emit() { printf '%s\n' "$1"; }
result() { emit '{"id":'"$1"',"result":'"$2"'}'; }
notify() { emit '{"method":"'"$1"'","params":'"$2"'}'; }

finish() {
  active=0
  pending=0
  local status="$1" text="$2"
  if [ "$provider" = claude ]; then
    local subtype=success failed=false
    if [ "$status" = failed ]; then subtype=error_during_execution; failed=true; fi
    emit '{"type":"result","subtype":"'"$subtype"'","is_error":'"$failed"',"result":"'"$text"'","session_id":"'"$native"'","total_cost_usd":0.01,"usage":{"input_tokens":10,"output_tokens":2}}'
  else
    local items='[]' error=null
    if [ "$status" = completed ]; then
      items='[{"type":"agentMessage","id":"answer-'"$count"'","text":"'"$text"'","phase":"final_answer","memoryCitation":null,"delivery":null}]'
      notify item/completed '{"threadId":"'"$native"'","turnId":"'"$turn"'","item":{"type":"agentMessage","id":"answer-'"$count"'","text":"'"$text"'","phase":"final_answer","memoryCitation":null,"delivery":null}}'
    elif [ "$status" = failed ]; then
      error='{"message":"fixture native failure","codexErrorInfo":null,"additionalDetails":null}'
    fi
    notify thread/tokenUsage/updated '{"threadId":"'"$native"'","turnId":"'"$turn"'","tokenUsage":{"last":{"inputTokens":10,"outputTokens":2,"totalTokens":12},"total":{"inputTokens":20,"outputTokens":4,"totalTokens":24},"modelContextWindow":258400}}'
    notify turn/completed '{"threadId":"'"$native"'","turn":{"id":"'"$turn"'","items":'"$items"',"itemsView":"summary","status":"'"$status"'","error":'"$error"',"startedAt":1790223474,"completedAt":1790223492,"durationMs":18}}'
  fi
}

finish_claude_partial() {
  active=0
  pending=0
  local text=''
  if [ "$claude_partial_mode" = terminal ]; then text="Authoritative terminal answer for turn $count."; fi
  emit '{"type":"assistant","session_id":"'"$native"'","aborted":true,"message":{"id":"message-'"$count"'","role":"assistant","content":[{"type":"thinking","thinking":"thinking must not leak"},{"type":"tool_use","id":"tool-partial","name":"Read","input":{"text":"tool input must not leak"}},{"type":"text","text":"Partial answer for turn '"$count"'.\nStill incomplete."}]}}'
  emit '{"type":"result","subtype":"success","is_error":false,"result":"'"$text"'","terminal_reason":"aborted_streaming","session_id":"'"$native"'","total_cost_usd":0.01,"usage":{"input_tokens":10,"output_tokens":0}}'
}

begin() {
  count=$((count + 1))
  turn="turn-$count"
  active=1
  startup_permission=0
  late_mode=
  claude_partial_mode=
  case "$line" in *'"late-ack"'*) late_mode=success ;; *'"late-error"'*) late_mode=error ;; esac
  case "$line" in *'"startup-ask"'*) startup_permission=1 ;; esac
  if [ "$provider" = claude ]; then
    case "$line" in *'"no-init-hold"'*) return ;; esac
    emit '{"type":"system","subtype":"init","session_id":"'"$native"'","fixtureInit":{"tools":["Read"],"unknown":true}}'
    case "$line" in
      *'"interrupted-partial-empty"'*) claude_partial_mode=empty ;;
      *'"interrupted-partial-terminal"'*) claude_partial_mode=terminal ;;
    esac
    if [ -n "$claude_partial_mode" ]; then
      if [ "$partial_messages" = 1 ]; then
        emit '{"type":"stream_event","session_id":"'"$native"'","event":{"type":"message_start","message":{"id":"message-'"$count"'","role":"assistant","content":[]}}}'
        emit '{"type":"stream_event","session_id":"'"$native"'","event":{"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}}'
        emit '{"type":"stream_event","session_id":"'"$native"'","event":{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"thinking must not leak"}}}'
        emit '{"type":"stream_event","session_id":"'"$native"'","event":{"type":"content_block_stop","index":0}}'
        emit '{"type":"stream_event","session_id":"'"$native"'","event":{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"tool-partial","name":"Read","input":{}}}}'
        emit '{"type":"stream_event","session_id":"'"$native"'","event":{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"text\":\"tool input must not leak\"}"}}}'
        emit '{"type":"stream_event","session_id":"'"$native"'","event":{"type":"content_block_stop","index":1}}'
        emit '{"type":"stream_event","session_id":"'"$native"'","event":{"type":"content_block_start","index":2,"content_block":{"type":"text","text":""}}}'
        emit '{"type":"stream_event","session_id":"'"$native"'","event":{"type":"content_block_delta","index":2,"delta":{"type":"text_delta","text":"Partial answer for turn '"$count"'.\n"}}}'
        emit '{"type":"stream_event","session_id":"'"$native"'","event":{"type":"content_block_delta","index":2,"delta":{"type":"text_delta","text":"Still incomplete."}}}'
      fi
      # Leave the text block open; only an RPC interrupt emits the snapshot/result.
      return
    fi
    emit '{"type":"assistant","session_id":"'"$native"'","message":{"role":"assistant","content":[{"type":"text","text":"working"},{"type":"tool_use","id":"tool-fixture","name":"Read","input":{"file_path":"fixture.txt"}}]},"fixturePayload":{"nested":[{"input":{"path":"a/b","flags":[true,null,3]},"output":{"stdout":"line one\nline two","opaque":{"future":42}}}]}}'
  else
    start_response='{"turn":{"id":"'"$turn"'","items":[],"itemsView":"notLoaded","status":"inProgress","error":null,"startedAt":null,"completedAt":null,"durationMs":null},"fixtureResponse":{"unknown":[1,{"keep":true}]}}'
    case "$line" in
      *'"startup-hold"'*|*'"startup-fail"'*|*'"startup-complete"'*|*'"startup-ask"'*)
        case "$line" in *'"startup-complete"'*) finish completed 'completed before start response' ;; esac
        touch "$DUE_SESSION_FIXTURE_DIR/start-waiting"
        while [ ! -f "$DUE_SESSION_FIXTURE_DIR/release-start" ]; do sleep 0.01; done
        case "$line" in
          *'"startup-fail"'*) emit '{"id":'"$id"',"error":{"code":-32000,"message":"fixture start rejected"}}'; return ;;
        esac
        ;;
      *'"early-overflow"'*|*'"early-release"'*|*'"early-foreign-resolution"'*)
        for request in $(seq 1 33); do
          if [ "$request" = 33 ]; then
            case "$line" in
              *'"early-release"'*) notify serverRequest/resolved '{"threadId":"'"$native"'","turnId":"'"$turn"'","requestId":101}' ;;
              *'"early-foreign-resolution"'*) notify serverRequest/resolved '{"threadId":"other-thread","requestId":101}' ;;
            esac
          fi
          emit '{"method":"item/commandExecution/requestApproval","id":'"$((100 + request))"',"params":{"threadId":"'"$native"'","turnId":"'"$turn"'","command":"fixture-only"}}'
        done
        # Overflow must be rejected while the start response is still withheld.
        case "$line" in
          *'"early-release"'*)
            while [ ! -f "$DUE_SESSION_FIXTURE_DIR/release-start" ]; do sleep 0.01; done
            ;;
          *) return ;;
        esac
        ;;
      *'"stale-approval"'*)
        emit '{"method":"item/commandExecution/requestApproval","id":18,"params":{"threadId":"native-codex","turnId":"turn-1","itemId":"stale-command","command":"stale request from previous turn","unknown":{"keep":[true,null,42]}}}'
        sleep 0.2
        ;;
    esac
    if [ "${DUE_SESSION_EARLY:-0}" != 1 ]; then result "$id" "$start_response"; fi
    notify turn/started '{"threadId":"'"$native"'","turn":{"id":"'"$turn"'","items":[],"itemsView":"notLoaded","status":"inProgress","error":null,"startedAt":1790223474,"completedAt":null,"durationMs":null}}'
    notify item/started '{"threadId":"'"$native"'","turnId":"'"$turn"'","item":{"id":"exec-fixture","type":"commandExecution","command":"fixture-only","fixturePayload":{"nested":[{"input":{"path":"a/b","flags":[true,null,3]},"output":{"stdout":"line one\nline two","opaque":{"future":42}}}]}}}'
  fi
  case "$line" in
    *'"codex-partial-'*)
      notify item/completed '{"threadId":"foreign-thread","turnId":"'"$turn"'","item":{"type":"agentMessage","id":"foreign","phase":"commentary","text":"foreign thread must not leak"}}'
      notify item/completed '{"threadId":"'"$native"'","turnId":"previous-turn","item":{"type":"agentMessage","id":"stale","phase":"commentary","text":"previous turn must not leak"}}'
      notify item/completed '{"threadId":"'"$native"'","turnId":"'"$turn"'","item":{"type":"agentMessage","id":"partial-'"$count"'","phase":"commentary","text":"Partial turn '"$count"'. é"}}'
      notify item/completed '{"threadId":"'"$native"'","turnId":"'"$turn"'","item":{"type":"reasoning","id":"reasoning","summary":["reasoning must not leak"]}}'
      notify item/completed '{"threadId":"foreign-thread","turnId":"'"$turn"'","item":{"type":"agentMessage","id":"foreign-late","phase":"commentary","text":"foreign late text must not leak"}}'
      notify item/completed '{"threadId":"'"$native"'","turnId":"previous-turn","item":{"type":"agentMessage","id":"stale-late","phase":"commentary","text":"stale late text must not leak"}}'
      notify fixture/partial-ready '{"threadId":"'"$native"'","turnId":"'"$turn"'","fixturePartialReady":true}'
      case "$line" in
        *'"codex-partial-early-hold"'*)
          while [ ! -f "$DUE_SESSION_FIXTURE_DIR/release-start" ]; do sleep 0.01; done
          ;;
        *'"codex-partial-failed"'*) finish failed '' ;;
        *'"codex-partial-final"'*) finish completed 'authoritative final answer' ;;
        *'"codex-partial-success"'*)
          active=0
          notify turn/completed '{"threadId":"'"$native"'","turn":{"id":"'"$turn"'","items":[{"type":"agentMessage","id":"only-commentary","phase":"commentary","text":"terminal commentary is not a final answer"}],"status":"completed","error":null}}'
          ;;
        *'"codex-partial-exit"'*)
          while [ ! -f "$DUE_SESSION_FIXTURE_DIR/release-exit" ]; do sleep 0.01; done
          exit 17
          ;;
      esac
      ;;
    *'"native-exit"'*) exit 0 ;;
    *'"native-exit-error"'*) exit 17 ;;
    *'"partial-payload"'*)
      if [ "$provider" = claude ] && [ "$partial_messages" = 1 ]; then
        emit '{"type":"stream_event","session_id":"'"$native"'","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"partial chunk\nsecond line"}},"fixturePartial":{"unknown":[true,null,{"future":42}]}}'
      fi
      finish completed 'partial payload finished'
      ;;
    *'"permission-overflow"'*)
      if [ "$provider" = codex ]; then
        # Tests release this only after observing the native turn/start reply,
        # so this exercises the established turn path instead of early replay.
        while [ ! -f "$DUE_SESSION_FIXTURE_DIR/release-permissions" ]; do sleep 0.01; done
      fi
      for request in $(seq 1 33); do
        if [ "$provider" = claude ]; then
          emit '{"type":"control_request","request_id":"permission-'"$request"'","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{"command":"fixture-only"}}}'
        else
          emit '{"method":"item/commandExecution/requestApproval","id":'"$((100 + request))"',"params":{"threadId":"'"$native"'","turnId":"'"$turn"'","command":"fixture-only"}}'
        fi
      done
      ;;
    *'"hold"'*|*'"late-ack"'*|*'"late-error"'*|*'"startup-hold"'*|*'"startup-complete"'*|*'"stale-approval"'*|*'"early-release"'*) : ;;
    *'"ask"'*|*'"ask-resolved"'*|*'"ask-completed"'*|*'"startup-ask"'*)
      pending=1
      if [ "$provider" = claude ]; then
        emit '{"type":"control_request","request_id":"permission-1","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{"command":"touch never-created"}}}'
      else
        emit '{"method":"item/commandExecution/requestApproval","id":17,"params":{"threadId":"'"$native"'","turnId":"'"$turn"'","itemId":"exec-fixture","environmentId":"local","command":"touch never-created","cwd":"/tmp","availableDecisions":["accept","cancel"]}}'
      fi
      case "$line" in
        *'"ask-completed"'*)
          finish completed 'completed with pending request'
          emit '{"method":"item/commandExecution/requestApproval","id":19,"params":{"threadId":"'"$native"'","turnId":"'"$turn"'","command":"too late"}}'
          ;;
        *'"ask-resolved"'*)
          notify serverRequest/resolved '{"threadId":"'"$native"'","requestId":17}'
          finish completed 'resolved before start'
          ;;
      esac
      ;;
    *'"identity-'*)
      notify item/completed '{"threadId":"'"$native"'","turnId":"'"$turn"'","item":{"type":"agentMessage","id":"accepted","text":"accepted answer","phase":"final_answer"}}'
      notify thread/tokenUsage/updated '{"threadId":"'"$native"'","turnId":"'"$turn"'","tokenUsage":{"last":{"inputTokens":10,"outputTokens":2}}}'
      value=123
      case "$line" in
        *array*) value='[]' ;; *object*) value='{}' ;; *boolean*) value=false ;; *null*) value=null ;;
      esac
      top='"'"$turn"'"'; nested="$value"; thread="$native"
      case "$line" in *-top-*) top="$value"; nested='"'"$turn"'"' ;; esac
      case "$line" in *-foreign-*) thread=foreign-thread ;; esac
      params='{"threadId":"'"$thread"'","turnId":'"$top"',"turn":{"id":'"$nested"'},"fixtureIdentity":true'
      case "$line" in
        *-unknown-*) notify fixture/unknown '{"opaque":{"turnId":123,"turn":{"id":[]}},"future":null}' ;;
        *-usage*) notify thread/tokenUsage/updated "$params"',"tokenUsage":{"last":{"inputTokens":999}}}' ;;
        *-completion*) notify turn/completed '{"threadId":"'"$thread"'","turnId":'"$top"',"fixtureIdentity":true,"turn":{"id":'"$nested"',"status":"completed","items":[]}}' ;;
        *) notify item/completed "$params"',"item":{"type":"agentMessage","id":"bad","text":"untrusted answer","phase":"final_answer"}}' ;;
      esac
      finish completed 'final answer'
      ;;
    *'"conflicting-completion"'*|*'"conflicting-reverse"'*|*'"conflicting-answer"'*|*'"conflicting-usage"'*|*'"matching-completion"'*|*'"nested-completion"'*|*'"stale-completion"'*)
      notify item/completed '{"threadId":"'"$native"'","turnId":"'"$turn"'","item":{"type":"agentMessage","id":"accepted","text":"accepted answer","phase":"final_answer"}}'
      notify thread/tokenUsage/updated '{"threadId":"'"$native"'","turnId":"'"$turn"'","tokenUsage":{"last":{"inputTokens":10,"outputTokens":2,"totalTokens":12}}}'
      top="$turn" nested="$turn"
      case "$line" in
        *'"conflicting-completion"'*|*'"conflicting-answer"'*|*'"conflicting-usage"'*) nested=foreign-turn ;;
        *'"conflicting-reverse"'*) top=foreign-turn ;;
        *'"stale-completion"'*) top=foreign-turn; nested=foreign-turn ;;
      esac
      ids='"turnId":"'"$top"'",'
      case "$line" in *'"nested-completion"'*) ids='' ;; esac
      case "$line" in
        *'"conflicting-answer"'*)
          notify item/completed '{"threadId":"'"$native"'",'"$ids"'"turn":{"id":"'"$nested"'"},"item":{"type":"agentMessage","id":"bad","text":"untrusted answer","phase":"final_answer"}}'
          ;;
        *'"conflicting-usage"'*)
          notify thread/tokenUsage/updated '{"threadId":"'"$native"'",'"$ids"'"turn":{"id":"'"$nested"'"},"tokenUsage":{"last":{"inputTokens":999}}}'
          ;;
        *)
          notify turn/completed '{"threadId":"'"$native"'",'"$ids"'"turn":{"id":"'"$nested"'","items":[],"status":"completed","error":null}}'
          ;;
      esac
      case "$line" in *'"stale-completion"'*) finish completed 'after stale' ;; esac
      ;;
    *'"fail"'*) finish failed 'partial answer' ;;
    *) finish completed "turn $count" ;;
  esac
  if [ "$provider" = codex ] && [ "${DUE_SESSION_EARLY:-0}" = 1 ]; then result "$id" "$start_response"; fi
}

id_re='"id":(-?[0-9]+)'
request_re='"request_id":"([^"]+)"'
method_re='"method":"([^"]+)"'

while IFS= read -r line; do
  printf '%s\n' "$line" >> "$DUE_SESSION_FIXTURE_DIR/input.jsonl"
  if [ "$provider" = claude ]; then
    case "$line" in
      *'"type":"control_request"'*)
        [[ "$line" =~ $request_re ]]
        request_id="${BASH_REMATCH[1]}"
        emit '{"type":"control_response","response":{"subtype":"success","request_id":"'"$request_id"'","response":{}}}'
        if [ "$active" = 1 ]; then
          if [ -n "$claude_partial_mode" ]; then finish_claude_partial
          else finish interrupted 'interrupted'; fi
        fi
        ;;
      *'"type":"control_response"'*)
        if [ "$pending" = 1 ]; then
          case "$line" in *'"behavior":"deny"'*) finish completed 'permission denied' ;; *) finish completed 'permission allowed' ;; esac
        fi
        ;;
      *'"type":"user"'*) begin ;;
    esac
  else
    if [[ "$line" =~ $method_re ]]; then
      method="${BASH_REMATCH[1]}"
      if [[ "$line" =~ $id_re ]]; then id="${BASH_REMATCH[1]}"; else continue; fi
      case "$method" in
        initialize) result "$id" '{"userAgent":"fixture/0.149.0","codexHome":"/tmp/fixture","platformFamily":"unix","platformOs":"macos"}' ;;
        thread/start|thread/resume)
          result "$id" '{"thread":{"id":"'"$native"'","turns":[],"fixtureThread":{"unknown":[1,{"keep":true}]}}}'
          notify fixture/idle '{"threadId":"'"$native"'","future":{"nested":[null,{"keep":true}]}}'
          ;;
        turn/start) begin ;;
        turn/interrupt)
          if [ -n "$late_mode" ]; then
            held_ack="$id"
            held_ack_error=0
            [ "$late_mode" != error ] || held_ack_error=1
            finish interrupted ''
          elif [ "$startup_permission" = 1 ]; then
            interrupt_id="$id"
            notify fixture/interrupt-waiting '{"threadId":"'"$native"'","turnId":"'"$turn"'"}'
          else result "$id" '{}'; finish interrupted ''; fi
          ;;
      esac
    elif [ "$pending" = 1 ]; then
      notify serverRequest/resolved '{"threadId":"'"$native"'","requestId":17}'
      if [ -n "$interrupt_id" ]; then
        result "$interrupt_id" '{}'
        interrupt_id=
        finish interrupted ''
      else
        case "$line" in *'"decision":"cancel"'*|*'"decision":"decline"'*) finish completed 'permission denied' ;; *) finish completed 'permission allowed' ;; esac
      fi
    fi
  fi
  if [ -n "$held_ack" ] && [ -f "$DUE_SESSION_FIXTURE_DIR/release-ack" ]; then
    if [ "$held_ack_error" = 1 ]; then
      emit '{"id":'"$held_ack"',"error":{"code":-32000,"message":"fixture late interrupt rejection"}}'
    else result "$held_ack" '{}'; fi
    held_ack=
  fi

done

# Let lifecycle tests keep the worker alive after its input closes.
if [ -f "$DUE_SESSION_FIXTURE_DIR/hold-close" ]; then
  : > "$DUE_SESSION_FIXTURE_DIR/close-waiting"
  while [ ! -f "$DUE_SESSION_FIXTURE_DIR/release-close" ]; do /bin/sleep 0.01; done
fi
