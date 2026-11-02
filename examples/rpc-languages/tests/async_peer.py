#!/usr/bin/env python3
"""Async supervision peer: real local verifier processes, no model/provider calls."""
import json
import os
from pathlib import Path
import sys
import threading
import time

case = os.environ.get("RPC_ASYNC_CASE", "verification")
root = Path(os.environ["RPC_ASYNC_DIR"])
lock = threading.RLock()
cursor = 0
turn_number = 0
closed = False
stopping = threading.Event()
active = None


def send(value):
    with lock:
        sys.stdout.write(json.dumps(value, separators=(",", ":")) + "\n")
        sys.stdout.flush()


def item(kind, data, turn=None):
    global cursor
    with lock:
        if closed:
            return
        cursor += 1
        send({"jsonrpc": "2.0", "method": "session.item", "params": {
            "session_id": "async-session", "turn_id": turn, "cursor": cursor,
            "kind": kind, "data": data, "push_dropped": 0}})


def result(request, value):
    send({"jsonrpc": "2.0", "id": request["id"], "result": value})


def finish(turn, status="succeeded"):
    global active
    item("turn_finished", {"schema_version": 1, "provider": "codex",
        "status": status, "run_id": turn, "session_id": "native-async",
        "text": "All tests passed.", "error": None if status == "succeeded" else {
            "kind": "cancelled", "message": "fixture interrupted"}}, turn)
    active = None


def wait_for_checker(turn, failure=False):
    for _ in range(1000):
        if stopping.wait(0.005):
            return
        if (root / "checker.pid").exists():
            break
    else:
        return
    with lock:
        if closed:
            return
        if failure:
            item("session_failed", {"message": "native failed during verification"})
        else:
            if case in ("permission_retired", "permission_retired_error"):
                item("interaction_resolved", {"request_id": "permission-1"}, turn)
            finish(turn)


def emit_progress():
    while not stopping.wait(0.01):
        item("native", {"method": "fixture/progress", "unknown": [None, {"kept": True}]}, active)


progress = threading.Thread(target=emit_progress, daemon=True)
progress.start()
try:
    for line in sys.stdin:
        request = json.loads(line)
        with (root / "requests.jsonl").open("a") as capture:
            capture.write(line)
        method, params = request["method"], request["params"]
        if method == "session.open":
            item("session_opened", {"provider": "codex"})
            result(request, {"session_id": "async-session", "provider": "codex",
                "native_session_id": "native-async", "protocol_version": 1,
                "stream": {"enabled": True, "method": "session.item"},
                "capabilities": {"interrupt": True, "interaction_reply": True}})
        elif method == "turn.start":
            turn_number += 1
            active = turn = f"turn-{turn_number}"
            item("turn_started", {"prompt": params["prompt"]}, turn)
            if case.startswith("permission") or case == "callback_error":
                item("interaction", {"request_id": "permission-1",
                    "request": {"method": "item/commandExecution/requestApproval"}}, turn)
                if case in ("permission_retired", "permission_terminal", "permission_retired_error"):
                    threading.Thread(target=wait_for_checker, args=(turn,), daemon=True).start()
            elif case == "active_turn":
                pass
            else:
                (root / "artifact.txt").write_text("bad" if turn_number == 1 else "good")
                finish(turn)
                if case == "verification_failure":
                    threading.Thread(target=wait_for_checker, args=(turn, True), daemon=True).start()
            # Exercise terminal items arriving before turn.start responses.
            result(request, {"turn_id": turn})
        elif method == "interaction.reply":
            if case in ("permission_retired", "permission_terminal", "permission_retired_error"):
                raise AssertionError("late permission reply was sent")
            assert params["request_id"] == "permission-1"
            assert params["response"] == {"decision": "decline"}
            item("interaction_resolved", {"request_id": "permission-1"}, params["turn_id"])
            finish(params["turn_id"])
            if case in ("permission_reply_error", "permission_reply_error_substring", "permission_reply_stale"):
                message = "unknown or resolved interaction"
                if case == "permission_reply_error":
                    message = "explicit reply delivery failed"
                elif case == "permission_reply_error_substring":
                    message = "explicit reply delivery failed: unknown or resolved interaction"
                send({"jsonrpc": "2.0", "id": request["id"], "error": {
                    "code": -32000, "message": message}})
            else:
                result(request, {"replied": True})
        elif method == "turn.interrupt":
            finish(params["turn_id"], "cancelled")
            result(request, {"interrupt_requested": True, "turn_id": params["turn_id"]})
        elif method == "session.close":
            with lock:
                if active is not None:
                    finish(active, "cancelled")
                item("session_closed", {"native_session_id": "native-async", "error": None})
                closed = True
            result(request, {"closed": True, "native_session_id": "native-async"})
        else:
            raise AssertionError(f"unexpected method {method}")
finally:
    stopping.set()
    progress.join()
    (root / "peer-exited").write_text("settled")
