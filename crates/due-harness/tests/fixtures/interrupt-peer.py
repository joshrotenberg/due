#!/usr/bin/env python3
"""Scripted Due RPC peer for interrupt ordering. No models, threads, timers or sleeps.

The turn.start prompt selects how that turn behaves, so message ordering is fixed by the
script and not by scheduling:

  quick    terminal item is sent before the turn.start response; never interrupted upstream
  race     turn.interrupt: terminal item, then "turn is already finished" error
  genuine  turn.interrupt: terminal item, then an unrelated error
  failed   turn.interrupt: terminal item, session_failed item, then "turn is already finished"
  early    first turn.interrupt: "turn is already finished" error with no terminal item;
           later interrupts succeed normally
  other    turn.interrupt: cancelled terminal item, then a normal acknowledgement

Every request is appended to requests.jsonl in the isolated working directory.
"""
import json
from pathlib import Path
import sys

log = Path.cwd() / "requests.jsonl"
cursor = 0
turn_number = 0
modes = {}
attempts = {}
unfinished = set()


def send(value):
    sys.stdout.write(json.dumps(value, separators=(",", ":")) + "\n")
    sys.stdout.flush()


def item(kind, data, turn=None):
    global cursor
    cursor += 1
    send({"jsonrpc": "2.0", "method": "session.item", "params": {
        "session_id": "interrupt-session", "turn_id": turn, "cursor": cursor,
        "kind": kind, "data": data, "push_dropped": 0}})


def result(request, value):
    send({"jsonrpc": "2.0", "id": request["id"], "result": value})


def reject(request, message, code=-32000):
    send({"jsonrpc": "2.0", "id": request["id"], "error": {"code": code, "message": message}})


def finish(turn, status="succeeded"):
    unfinished.discard(turn)
    item("turn_finished", {"schema_version": 1, "provider": "codex", "status": status,
        "run_id": turn, "session_id": "native-interrupt", "text": f"{turn} {status}",
        "error": None if status == "succeeded" else {
            "kind": "cancelled", "message": "fixture interrupted"}}, turn)


for line in sys.stdin:
    request = json.loads(line)
    with log.open("a") as capture:
        capture.write(line)
    method, params = request["method"], request["params"]
    if method == "session.open":
        item("session_opened", {"provider": "codex"})
        result(request, {"session_id": "interrupt-session", "provider": "codex",
            "native_session_id": "native-interrupt", "protocol_version": 1,
            "stream": {"enabled": True, "method": "session.item"},
            "capabilities": {"interrupt": True, "interaction_reply": True}})
    elif method == "turn.start":
        turn_number += 1
        turn = f"turn-{turn_number}"
        modes[turn] = params["prompt"]
        unfinished.add(turn)
        item("turn_started", {"prompt": params["prompt"]}, turn)
        if modes[turn] == "quick":
            finish(turn)
        result(request, {"turn_id": turn})
    elif method == "turn.interrupt":
        turn = params["turn_id"]
        attempts[turn] = attempts.get(turn, 0) + 1
        mode = modes[turn]
        if mode in ("race", "race-native", "race-idle"):
            finish(turn)
            reject(request, {
                "race": "turn is already finished",
                "race-native": "Unknown or finished Due turn to interrupt",
                "race-idle": "No active turn",
            }[mode])
        elif mode == "genuine":
            finish(turn)
            reject(request, "interrupt delivery failed")
        elif mode == "wrong-code":
            finish(turn)
            reject(request, "No active turn", code=-32603)
        elif mode == "failed":
            finish(turn)
            item("session_failed", {"message": "native failed"})
            reject(request, "turn is already finished")
        elif mode == "early" and attempts[turn] == 1:
            reject(request, "turn is already finished")
        else:
            finish(turn, "cancelled")
            result(request, {"interrupt_requested": True, "turn_id": turn})
    elif method == "session.close":
        for turn in sorted(unfinished):
            finish(turn, "cancelled")
        item("session_closed", {"native_session_id": "native-interrupt", "error": None})
        result(request, {"closed": True, "native_session_id": "native-interrupt"})
    else:
        raise AssertionError(f"unexpected method {method}")
