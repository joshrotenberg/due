#!/usr/bin/env python3
"""Deterministic NDJSON peer for both embedded clients; never calls a model."""

import json
import os
import sys
import time

CASE = os.environ.get("RPC_TEST_CASE", "conversation")
cursor = 0
turn_number = 0
pending = {}
held = []


def send(value):
    sys.stdout.write(json.dumps(value, separators=(",", ":"), ensure_ascii=False) + "\n")
    sys.stdout.flush()


def result(request, value):
    send({"jsonrpc": "2.0", "id": request["id"], "result": value})


def item(kind, data=None, turn=None, skip=0):
    global cursor
    cursor += 1 + skip
    send({"jsonrpc": "2.0", "method": "session.item", "params": {
        "session_id": "s1", "turn_id": turn, "cursor": cursor,
        "kind": kind, "data": data or {}, "push_dropped": 0,
    }})


def finish(turn, status="succeeded"):
    value = {"schema_version": 1, "status": status, "run_id": turn, "text": "marker-731",
             "session_id": "native-s1", "provider": "codex"}
    if status == "failed":
        value["error"] = {"message": "model turn failed"}
    pending[turn] = value
    item("turn_finished", value, turn)


for line in sys.stdin:
    request = json.loads(line)
    method = request["method"]
    params = request["params"]
    if method == "session.open":
        # Model-free startup pressure before the callback can possibly run.
        open_delay_ms = int(os.environ.get("RPC_TEST_OPEN_DELAY_MS", "0"))
        if open_delay_ms:
            sys.stderr.write("FIXTURE_OPEN_DELAY\n")
            sys.stderr.flush()
            time.sleep(open_delay_ms / 1000)
        if CASE == "bad_json":
            sys.stdout.buffer.write(b"{this is not JSON}\n")
            sys.stdout.buffer.flush()
            continue
        if CASE == "bad_utf8":
            sys.stdout.buffer.write(b'{"bad":"\xff"}\n')
            sys.stdout.buffer.flush()
            continue
        item("session_opened", {"provider": "codex"})
        if CASE == "invalid_id":
            send({"jsonrpc": "2.0", "id": "not-requested", "result": {}})
            continue
        result(request, {"session_id": "s1", "native_session_id": "native-s1",
                         "protocol_version": 2 if CASE == "invalid_version" else 1,
                         "stream": {"enabled": True, "method": "session.item"},
                         "capabilities": {"interrupt": True, "interaction_reply": True}})
        if os.environ.get("RPC_TEST_READY"):
            with open(os.environ["RPC_TEST_READY"], "x", encoding="utf-8") as ready:
                ready.write(str(os.getpid()))
        if CASE == "session_failed":
            item("session_failed", {"message": "fixture session failed"})
        elif CASE == "gap":
            item("native", {"text": "lost predecessor"}, skip=1)
        elif CASE == "omitted":
            item("omitted", {"kind": "native", "bytes": 9999999, "reason": "fixture"})
    elif method == "turn.start":
        turn_number += 1
        turn = "t" + str(turn_number)
        item("turn_started", {"prompt": params["prompt"]}, turn)
        if CASE == "shapes":
            item("native", {"null": None, "array": [], "object": {},
                            "large": 9007199254740993, "unsigned": 18446744073709551615, "text": "multi-byte café native"}, turn)
        else:
            item("native", {"text": "marker-731", "unknown": {"keep": [1, None, {}]}}, turn)
        if CASE in ("permission", "interrupt") and turn_number == 1:
            pending[turn] = None
            item("interaction", {"request_id": "permission-1",
                                  "request": {"method": "tool/approval", "tool_name": "fixture"}}, turn)
        else:
            finish(turn, "failed" if CASE == "failed_turn" and turn_number == 1 else "succeeded")
        # Intentionally after all items, including the terminal result.
        result(request, {"turn_id": turn})
    elif method == "interaction.reply":
        if params["request_id"] != "permission-1" or params["response"] != {"decision": "decline"}:
            raise AssertionError("fixture requires an explicit decline")
        item("interaction_resolved", {"request_id": "permission-1"}, params["turn_id"])
        result(request, {"accepted": True})
        finish(params["turn_id"])
    elif method == "turn.interrupt":
        item("interaction_resolved", {"request_id": "permission-1"}, params["turn_id"])
        result(request, {"interrupted": True})
        finish(params["turn_id"], "failed")
    elif method == "turn.read":
        value = pending.get(params["turn_id"])
        result(request, {"state": "finished" if value else "running", "result": value,
                         "cursor": 0, "events": [], "interactions": []})
    elif method == "test.echo":
        if CASE == "out_of_order":
            held.append(request)
            if len(held) == 2:
                for earlier in reversed(held):
                    result(earlier, earlier["params"])
                held.clear()
        else:
            result(request, params)
    elif method == "test.error":
        send({"jsonrpc": "2.0", "id": request["id"], "error": {
            "code": -32602, "message": "fixture invalid parameter", "data": {"field": "prompt"}}})
    elif method == "session.close":
        item("session_closed", {"native_session_id": "native-s1"})
        result(request, {"session_id": "s1", "native_session_id": "native-s1"})
    else:
        raise AssertionError("unexpected fixture method: " + method)

# Proves stdin was closed and the owned process reached its normal exit path.
sys.stderr.write("FIXTURE_EOF\n")
