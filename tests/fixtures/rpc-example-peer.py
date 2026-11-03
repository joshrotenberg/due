#!/usr/bin/env python3
"""Immutable, model-free peer for the published Python client example."""
import json
import os
from pathlib import Path
import sys

scenario = os.environ["DUE_EXAMPLE_SCENARIO"]
root = Path.cwd()
cursor = 0


def send(value):
    print(json.dumps(value), flush=True)


def item(kind, data, turn="turn", dropped=0):
    global cursor
    cursor += 1
    send({"jsonrpc": "2.0", "method": "session.item", "params": {
        "session_id": "session", "turn_id": turn, "cursor": cursor,
        "kind": kind, "data": data, "push_dropped": dropped}})


def result(request, value):
    send({"jsonrpc": "2.0", "id": request["id"], "result": value})


def finish():
    item("turn_finished", {"status": "succeeded", "text": "fake answer", "error": None})


try:
    for line in sys.stdin:
        request = json.loads(line)
        with (root / "requests.jsonl").open("a") as log:
            log.write(line)
        method = request["method"]
        if method == "session.open":
            item("session_opened", {"provider": "claude"}, turn=None)
            result(request, {"session_id": "session", "protocol_version": 1})
        elif method == "turn.start":
            item("turn_started", {"prompt": "fixture"})
            result(request, {"turn_id": "turn"})
            if scenario.startswith("omit-"):
                omitted = "turn_finished" if scenario == "omit-terminal" else "interaction"
                item("omitted", {"kind": omitted, "bytes": 4194305, "reason": "fixture omission"})
            elif scenario == "push-drop":
                item("native", {}, dropped=1)
            elif scenario == "cursor-gap":
                cursor += 1
                item("native", {})
            elif scenario == "denial":
                item("interaction", {"request_id": "request", "request": {
                    "provider": "claude", "tool_name": "Bash",
                    "input": {"command": "not-authorized"}}})
            elif scenario == "success":
                finish()
            elif scenario != "deadline":
                raise AssertionError(scenario)
        elif method == "interaction.reply":
            assert request["params"]["response"]["behavior"] == "deny"
            item("interaction_resolved", {"request_id": "request"})
            result(request, {"replied": True})
            finish()
        elif method == "session.close":
            item("session_closed", {"error": None}, turn=None)
            result(request, {"closed": True, "session_error": None})
        else:
            raise AssertionError(method)
finally:
    (root / "peer-exited").touch()
