#!/usr/bin/env python3
"""Offline provenance fixture. No provider, tool, or network calls.

Claude message.model follows claude-wrapper 79ffb15/tests/fake-claude.sh.
Codex thread/turn shapes follow codex-wrapper e91e627 app_server.rs.
Model IDs, unknown fields, counters, and failures below are synthetic.
"""
import json
import sys


def emit(value):
    print(json.dumps(value), flush=True)


def messages(prompt):
    if "unknown" in prompt:
        return []
    models = ["held-native"] if "hold" in prompt else ["native-a", "native-b", "native-a"]
    return [
        {"type": "assistant", "session_id": "native-id", "message": {
            "role": "assistant", "model": model, "future": 42,
            "content": [{"type": "text", "text": "partial"}]}}
        for model in models
    ]


def terminal(prompt):
    failed = "fail" in prompt
    return {"type": "result", "subtype": "error_during_execution" if failed else "success",
            "is_error": failed, "session_id": "native-id", "result": "partial" if failed else "",
            "usage": {"input_tokens": 10, "output_tokens": 2},
            "modelUsage": {"old-session-model": {"costUSD": 1}}, "effort": "high"}


args = sys.argv[1:]
codex = "app-server" in args or "exec" in args
resident = "app-server" in args or "--input-format" in args
if not resident:
    prompt = sys.stdin.read()
    if codex:
        path = args[args.index("--output-last-message") + 1]
        with open(path, "w") as output:
            output.write("partial" if "fail" in prompt else "")
        emit({"type": "thread.started", "thread_id": "native-id", "model": "configured"})
        emit({"type": "item.completed", "item": {"type": "agent_message", "text": "partial",
                                                  "model": "unvalidated"}})
        emit({"type": "turn.failed", "error": {"message": "native failure"}} if "fail" in prompt
             else {"type": "turn.completed", "usage": {"input_tokens": 10, "output_tokens": 2}})
    else:
        records = messages(prompt) + [terminal(prompt)]
        if "stream-json" in args:
            for record in records:
                emit(record)
        else:
            emit(records)
    sys.exit(9 if "fail" in prompt else 0)

count = 0
held = False
for line in sys.stdin:
    request = json.loads(line)
    if not codex:
        if request.get("type") == "control_request":
            emit({"type": "control_response", "response": {
                "subtype": "success", "request_id": request["request_id"], "response": {}}})
            if held and request.get("request", {}).get("subtype") == "interrupt":
                # Settle only after the explicit native interrupt, never a timer.
                held = False
                result = terminal("unknown")
                result["terminal_reason"] = "aborted_streaming"
                emit(result)
        elif request.get("type") == "user":
            emit({"type": "system", "subtype": "init", "session_id": "native-id", "model": "configured"})
            held = "hold" in line
            for record in messages(line):
                emit(record)
            if not held:
                emit(terminal(line))
        continue
    method = request.get("method")
    if method == "initialize":
        emit({"id": request["id"], "result": {"userAgent": "fixture/0.149.0"}})
    elif method in ("thread/start", "thread/resume"):
        emit({"id": request["id"], "result": {"thread": {"id": "native-id"},
                                               "model": "configured", "reasoningEffort": "high"}})
    elif method == "turn/start":
        count += 1
        turn_id = "turn-" + str(count)
        emit({"id": request["id"], "result": {"turn": {"id": turn_id, "status": "inProgress"}}})
        failed = "fail" in line
        turn = {"id": turn_id, "status": "failed" if failed else "completed",
                "model": "unvalidated", "effort": "high", "items": [{"type": "agentMessage",
                "phase": "final_answer", "text": "partial" if failed else ""}]}
        if failed:
            turn["error"] = {"message": "native failure"}
        emit({"method": "turn/completed", "params": {"threadId": "native-id", "turn": turn}})
