#!/usr/bin/env python3
"""Opt-in schema probe via the existing Node caller. --live starts one provider turn."""
import argparse
from datetime import datetime, timezone
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent / "offline-validation"))
from validate import (InputError, answer_validator, envelope_fields, evaluate, json_bytes, sha256,
                      strict_json, write_private)

NODE_CALLER = HERE.parent / "node-caller" / "run-due.mjs"
PROMPT = b'''Return exactly {"summary":"schema smoke"} using the supplied answer schema.
This is a bounded schema transport probe. Do not inspect the repository, use
tools, edit files, access the network, repair anything, retry, publish reports,
or delegate recursively (including native agents, subprocess agents or Due).
If the schema is unsupported, stop and report the failure. Return observations
only to the outer caller, who owns validation and any later live probe.
'''


def check_profile(profile):
    """Narrow example admission rules, not an effective native-policy verifier."""
    allowed = {"schema_version", "provider", "ephemeral", "native_args", "model", "effort"}
    required = {"schema_version", "provider", "ephemeral", "native_args"}
    if not isinstance(profile, dict) or not required <= profile.keys() or profile.keys() - allowed:
        raise InputError("profile needs schema_version/provider/ephemeral/native_args; only model/effort may be added")
    if type(profile["schema_version"]) is not int or profile["schema_version"] != 1:
        raise InputError("profile schema_version must be integer 1")
    if type(profile["ephemeral"]) is not bool:
        raise InputError("profile ephemeral must be explicit boolean")
    if "model" in profile and (not isinstance(profile["model"], str) or not profile["model"].strip()):
        raise InputError("profile model must be a nonempty string when supplied")
    if "effort" in profile and profile["effort"] not in ("low", "medium", "high"):
        raise InputError("profile effort must be low, medium or high when supplied")
    args = profile["native_args"]
    if not isinstance(args, list) or not all(isinstance(arg, str) for arg in args):
        raise InputError("profile native_args must be an array of strings")
    if profile["provider"] == "codex":
        # Accept the frozen workspace profile unchanged, or its read-only variant.
        fixed = {
            'approval_policy="never"', 'agents.enabled=false', 'features.multi_agent_v2=false',
            'sandbox_workspace_write.network_access=false',
            'sandbox_workspace_write.writable_roots=[]',
            'sandbox_workspace_write.exclude_slash_tmp=true',
            'sandbox_workspace_write.exclude_tmpdir_env_var=true',
        }
        values = args[1::2]
        if (len(args) != 16 or args[::2] != ["-c"] * 8 or len(set(values)) != 8
                or not fixed <= set(values)
                or not (set(values) - fixed) <= {'sandbox_mode="read-only"', 'sandbox_mode="workspace-write"'}):
            raise InputError("Codex smoke requires the eight documented native -c restrictions without extra flags")
    elif profile["provider"] == "claude":
        required_options = {
            "--permission-mode": "dontAsk", "--permission-prompts": "none",
            "--tools": "", "--mcp-config": '{"mcpServers":{}}',
        }
        options = {}
        index = 0
        while index < len(args):
            key = args[index]
            if key in options:
                raise InputError("duplicate Claude option")
            if key == "--strict-mcp-config":
                options[key] = True
                index += 1
            elif key in required_options and index + 1 < len(args):
                options[key] = args[index + 1]
                index += 2
            else:
                raise InputError("unapproved Claude smoke option")
        if options != {**required_options, "--strict-mcp-config": True}:
            raise InputError("Claude smoke requires dontAsk, no permission prompts, empty tools and strict empty MCP config")
    else:
        raise InputError("profile provider must be codex or claude")


class Cancelled(Exception):
    pass


class Signals:
    """Forward the first signal once; wait for Node/Due to finish cleanup."""
    def __init__(self):
        self.received = None
        self.child = None
        self.forwarded = False
        self.finalized = False

    def __enter__(self):
        self.previous = {sig: signal.signal(sig, self.handle)
                         for sig in (signal.SIGINT, signal.SIGTERM, signal.SIGHUP)}
        return self

    def handle(self, sig, _frame):
        if not self.finalized and self.received is None:
            self.received = sig
            self.forward()

    def forward(self):
        if self.received and self.child is not None and not self.forwarded:
            self.forwarded = True
            try:
                self.child.send_signal(self.received)
            except ProcessLookupError:
                pass

    def run(self, command, out, phase, env):
        if self.received:
            raise Cancelled()
        # These streams are the Node launcher's diagnostics; Due's own streams are
        # captured separately by Node in probe.plan/ or probe.run/.
        with open_exclusive(out / f"{phase}.launcher.stdout") as stdout, \
                open_exclusive(out / f"{phase}.launcher.stderr") as stderr:
            self.child = subprocess.Popen(command, stdin=subprocess.DEVNULL,
                                          stdout=stdout, stderr=stderr, env=env)
            self.forward()  # Includes a signal received during spawn.
            code = self.child.wait()  # Deliberately no arbitrary deadline.
            self.child = None
        return code if code >= 0 else 128 - code

    def __exit__(self, *_):
        for sig, previous in self.previous.items():
            signal.signal(sig, previous)


def open_exclusive(path):
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    os.fchmod(fd, 0o600)
    return os.fdopen(fd, "wb")


def save_json(path, value):
    write_private(path, json_bytes(value))


def preflight(args):
    profile_raw = args.profile.read_bytes()
    profile = strict_json(profile_raw)
    check_profile(profile)
    workspace = args.workspace.resolve(strict=True)
    if not workspace.is_dir():
        raise InputError("workspace must be an existing authorized directory")
    out = args.artifacts.parent.resolve(strict=True) / args.artifacts.name
    if out.is_relative_to(workspace):
        raise InputError("artifacts must be outside the worker workspace")
    if out.exists() or out.is_symlink():
        raise InputError("artifacts path already exists; choose a fresh path")
    for version in (args.due_version, args.native_version):
        if not version.strip() or "\n" in version:
            raise InputError("record exact single-line version output supplied by the caller")
    binary = shutil.which(args.due)
    node = shutil.which("node")
    if not binary or not node:
        raise InputError("installed Due executable and Node.js 22+ must be available")
    schema_raw = (HERE / "schemas" / f"{args.case}.schema.json").read_bytes()
    answer_validator(strict_json(schema_raw))
    return profile_raw, profile, workspace, out, str(Path(binary).resolve()), node, schema_raw


def run(args):
    profile_raw, profile, workspace, out, binary, node, schema_raw = preflight(args)
    out.mkdir(mode=0o700)  # Exclusive, including across concurrent invocations.
    out.chmod(0o700)
    write_private(out / "profile.original.json", profile_raw)
    write_private(out / "schema.original.json", schema_raw)
    write_private(out / "probe.prompt.txt", PROMPT)
    request = {**profile, "cwd": str(workspace), "schema": str(out / "schema.original.json")}
    save_json(out / "probe.request.json", request)
    save_json(out / "metadata.json", {
        "evidence_version": 1, "case": args.case, "live_requested": args.live,
        "created_at": datetime.now(timezone.utc).isoformat(),
        "due_executable": binary, "due_sha256": sha256(Path(binary).read_bytes()),
        "versions": {"source": "caller_supplied; independently verify against the plan",
                     "due": args.due_version, "native": args.native_version},
        "provider": profile["provider"], "requested_model": profile.get("model"),
        "requested_effort": profile.get("effort"),
        "original_schema_sha256": sha256(schema_raw), "original_profile_sha256": sha256(profile_raw),
        "prompt_sha256": sha256(PROMPT),
        "native_compatibility": "not_established_by_harness",
    })
    temp = out / "tmp"
    temp.mkdir(mode=0o700)
    # Due's schema and output snapshots stay outside the worker's writable roots.
    env = {**os.environ, "TMPDIR": str(temp)}
    command = [node, str(NODE_CALLER), str(out), "probe", "--due", binary]
    summary = {"plan_launcher_exit": None, "run_launcher_exit": None,
               "validation_exit": None, "signal_received": None,
               "native_compatibility": "not_established_by_harness"}
    code = 2
    with Signals() as signals:
        try:
            code = signals.run([*command, "--dry-run"], out, "plan", env)
            summary["plan_launcher_exit"] = code
            if code == 0 and not signals.received:
                plan = strict_json((out / "probe.plan" / "plan.json").read_bytes())
                envelope_fields(plan)
                if (plan.get("status") != "planned" or plan["structured_output_present"]
                        or plan.get("provider") != profile["provider"]
                        or not isinstance(plan.get("invocation"), dict)):
                    raise InputError("plan did not contain the expected provider and planned invocation")
                invocation = plan["invocation"]
                if (not isinstance(invocation.get("binary"), str) or not invocation["binary"]
                        or invocation.get("cwd") != str(workspace)
                        or type(invocation.get("stdin_bytes")) is not int
                        or invocation["stdin_bytes"] != len(PROMPT)
                        or not isinstance(invocation.get("args"), list)
                        or not all(isinstance(arg, str) for arg in invocation["args"])):
                    raise InputError("plan invocation must identify executable, cwd, argv and exact prompt byte count")
                if args.live:
                    code = signals.run(command, out, "run", env)
                    summary["run_launcher_exit"] = code
                    evidence, validation_code = evaluate(out / "probe.run" / "result.json",
                                                         out / "schema.original.json")
                    save_json(out / "answer-validation.json", evidence)
                    summary["validation_exit"] = validation_code
                    if code == 0:
                        code = validation_code
        except Cancelled:
            pass
        except (OSError, InputError, RecursionError) as error:
            summary["harness_error"] = str(error)
            code = code or 2
        signals.finalized = True
        if signals.received:
            summary["signal_received"] = signal.Signals(signals.received).name
            code = code or (128 + signals.received)
        summary["harness_exit"] = code
        save_json(out / "summary.json", summary)
    return code


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--dry-run", action="store_true", help="plan only; no provider launch")
    mode.add_argument("--live", action="store_true", help="explicitly authorize one monitored provider turn after planning")
    parser.add_argument("--profile", type=Path, required=True)
    parser.add_argument("--workspace", type=Path, required=True)
    parser.add_argument("--artifacts", type=Path, required=True, help="fresh directory outside worker workspace")
    parser.add_argument("--case", choices=("minimal", "dialect", "keyword"), required=True)
    parser.add_argument("--due", required=True, help="explicit installed Due executable")
    parser.add_argument("--due-version", required=True, help="exact version output observed by caller")
    parser.add_argument("--native-version", required=True, help="exact chosen provider version output observed by caller")
    args = parser.parse_args()
    try:
        return run(args)
    except (OSError, InputError, RecursionError) as error:
        print(f"smoke: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
