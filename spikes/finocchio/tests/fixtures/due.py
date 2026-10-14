#!/usr/bin/env python3
"""Synthetic Due JSON CLI, NEVER a provider. Test artifacts stay in granted cwd."""
import json
import os
from pathlib import Path
import signal
import sys
import time

assert sys.argv[1:6] == ["run", "--request", "-", "--output", "json"], sys.argv
assert sys.argv[6] == "--trace-file" and len(sys.argv) == 8
cancelled = False
def cancel(*_):
    global cancelled
    cancelled = True
signal.signal(signal.SIGTERM, cancel)
signal.signal(signal.SIGINT, cancel)
request = json.load(sys.stdin)
assert request["schema_version"] == 1 and request["ephemeral"]
assert set(request) == {"schema_version", "provider", "model", "effort", "cwd", "prompt", "schema", "ephemeral", "native_args"}
cwd = Path(request["cwd"])
context = json.loads(request["prompt"].splitlines()[-1])
task = context.get("task", "approve")
revision = request["prompt"].startswith("Revise")
with open(cwd / "calls", "a") as f:
    f.write(json.dumps({"pid": os.getpid(), "revision": revision, "context": context, "request": request}) + "\n")
with open(sys.argv[7], "x") as trace:
    trace.write(json.dumps({"event": "synthetic-start", "pid": os.getpid()}) + "\n")
    trace.flush()
    if task == "trace-flood":
        trace.write("x" * (2 * 1024 * 1024))
        trace.flush()
    if task in ("barrier", "trace-flood", "slow-cancel", "cancel-flood") or (task == "barrier-revise" and not revision and context.get("revision", 0) == 0):
        (cwd / "ready").write_text(str(os.getpid()))
        while not (cwd / "release").exists() and not cancelled:
            time.sleep(0.005)
    if task == "slow-cancel" and cancelled:
        (cwd / "cancel-seen").write_text("waiting for controlled cleanup")
        while not (cwd / "cleanup-release").exists():
            time.sleep(0.005)
    if task == "flood" or (task == "cancel-flood" and cancelled):
        sys.stdout.write("x" * (2 * 1024 * 1024))
        sys.stdout.flush()
    if task == "pipe-writer":
        pid = os.fork()
        if pid == 0:
            (cwd / "orphan").write_text(str(os.getpid()))
            while not (cwd / "orphan-release").exists():
                time.sleep(0.005)
            os._exit(0)
        task = "native-failure"
    if cancelled:
        (cwd / "cleaned").write_text("cooperative synthetic cleanup")
        status, error, code = "cancelled", {"kind": "cancelled", "message": "synthetic cancellation"}, 130
    elif task == "native-failure" or (task == "failed-revision" and revision):
        status, error, code = "failed", {"kind": "provider", "message": "synthetic native failure"}, 1
    else:
        status, error, code = "succeeded", None, 0
    if revision:
        answer = {"changed": True, "summary": "synthetic mutation"}
        (cwd / "mutation").write_text("one attempted mutation")
    else:
        changes = task in ("endless", "failed-revision") or (task in ("revise", "barrier-revise") and context.get("revision", 0) == 0)
        answer = {"verdict": "changes_requested" if changes else "approved", "findings": ["fix synthetic issue"] if changes else []}
    if task == "invalid-output":
        answer["verdict"] = "maybe"
    if task == "null-output":
        answer = None
    if task == "huge-integer":
        answer = {"big": 123456789012345678901234567890, "null": None, "array": [], "object": {}}
    if task == "protocol":
        print("invalid JSON")
    else:
        print(json.dumps({"schema_version": 1, "provider": request["provider"], "status": 42 if task == "malformed-envelope" else status, "text": "", "structured_output_present": task != "missing-output", "structured_output": None if task == "missing-output" else answer, "answer_schema_validation": "valid" if task == "claimed-validation" else "not_performed", "error": error, "duration_ms": 0, "trace": {"incomplete": False, "dropped_records": 0}}), flush=True)
    trace.write(json.dumps({"event": "synthetic-end", "status": status}) + "\n")
sys.exit(code)
