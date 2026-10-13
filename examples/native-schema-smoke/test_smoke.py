import base64
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import unittest

import smoke

HERE = Path(__file__).resolve().parent


class SmokeTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(dir=HERE, prefix="test-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.workspace = self.root / "worker with spaces"
        self.workspace.mkdir()
        self.artifacts = self.root / "private artifacts"
        self.profile_path = self.root / "profile.json"
        self.profile_bytes = (HERE.parent / "codex-workspace-profile.json").read_bytes()
        self.profile_path.write_bytes(self.profile_bytes)
        self.binary = self.root / "fake due.mjs"
        shutil.copyfile(HERE / "fake-due.fixture.mjs", self.binary)
        self.binary.chmod(0o700)

    def command(self, *extra):
        return [sys.executable, "-B", str(HERE / "smoke.py"),
                "--profile", str(self.profile_path), "--workspace", str(self.workspace),
                "--artifacts", str(self.artifacts), "--case", "minimal", "--due", str(self.binary),
                "--due-version", "offline Due fixture 1", "--native-version", "no native provider (fixture)", *extra]

    def run_smoke(self, *extra, mode="success"):
        return subprocess.run(self.command(*extra), capture_output=True, timeout=15,
                              env={**os.environ, "FAKE_SMOKE_MODE": mode})

    def read(self, name):
        return json.loads((self.artifacts / name).read_bytes())

    def test_explicit_mode_required_and_dry_run_only_plans(self):
        self.assertEqual(self.run_smoke().returncode, 2)
        self.assertFalse(self.artifacts.exists())
        result = self.run_smoke("--dry-run")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.read("probe.plan/plan.json")["status"], "planned")
        self.assertEqual(self.read("summary.json")["run_launcher_exit"], None)
        self.assertFalse((self.artifacts / "probe.run").exists())
        self.assertFalse((self.artifacts / "run.transport.json").exists())

    def test_schema_prompt_request_transport_hashes_and_private_exclusive_artifacts(self):
        for case in ("minimal", "dialect", "keyword"):
            with self.subTest(case=case):
                self.artifacts = self.root / case
                result = self.run_smoke("--live", "--case", case)
                self.assertEqual(result.returncode, 0, result.stderr)
                original_schema = (HERE / "schemas" / f"{case}.schema.json").read_bytes()
                self.assertEqual((self.artifacts / "schema.original.json").read_bytes(), original_schema)
                self.assertEqual((self.artifacts / "profile.original.json").read_bytes(), self.profile_bytes)
                self.assertEqual(self.read("metadata.json")["original_schema_sha256"], smoke.sha256(original_schema))
                for phase in ("plan", "run"):
                    captured = self.read(f"{phase}.transport.json")
                    self.assertEqual(base64.b64decode(captured["schema"]), original_schema)
                    self.assertEqual(base64.b64decode(captured["prompt"]), smoke.PROMPT)
                    request = json.loads(base64.b64decode(captured["request"]))
                    self.assertEqual(request["native_args"], json.loads(self.profile_bytes)["native_args"])
                    self.assertNotIn("timeout", request)
                    self.assertNotIn("resume", request)
                    self.assertEqual(request["cwd"], str(self.workspace))
                    self.assertEqual(captured["tmpdir"], str(self.artifacts / "tmp"))
                    self.assertEqual(captured["args"][:6], ["run", "--request", str(self.artifacts / "probe.request.json"), "--stdin", "--output", "json"])
                    self.assertEqual("--dry-run" in captured["args"], phase == "plan")
                    self.assertEqual("--trace-file" in captured["args"], phase == "run")
                self.assertEqual(self.read("answer-validation.json")["answer_validation"]["valid"], True)
                self.assertEqual(self.read("summary.json")["native_compatibility"], "not_established_by_harness")
                paths = [self.artifacts, *self.artifacts.rglob("*")]
                for path in paths:
                    self.assertEqual(path.stat().st_mode & 0o777, 0o700 if path.is_dir() else 0o600, str(path))
                before = {p: p.read_bytes() for p in paths if p.is_file()}
                self.assertEqual(self.run_smoke("--live", "--case", case).returncode, 2)
                for path, raw in before.items():
                    self.assertEqual(path.read_bytes(), raw)
        self.assertEqual(self.profile_path.read_bytes(), self.profile_bytes)

    def test_missing_unsafe_and_ambiguous_profiles_rejected_before_any_launch(self):
        safe = json.loads(self.profile_bytes)
        profiles = [{}, {**safe, "native_args": []},
                    {**safe, "native_args": [*safe["native_args"], "--dangerously-bypass-approvals-and-sandbox"]},
                    {**safe, "native_args": ["--skip-git-repo-check", *safe["native_args"]]},
                    {**safe, "resume": "old-session"}, {**safe, "timeout": "1s"},
                    {**safe, "cwd": str(self.workspace)}, {**safe, "schema_version": True},
                    {**safe, "provider": "unknown"}]
        for setting in ('agents.enabled=false', 'features.multi_agent_v2=false',
                        'sandbox_workspace_write.network_access=false',
                        'sandbox_workspace_write.exclude_tmpdir_env_var=true'):
            profiles.append({**safe, "native_args": [arg for arg in safe["native_args"] if arg != setting]})
        for profile in profiles:
            with self.subTest(profile=profile):
                self.profile_path.write_bytes(smoke.json_bytes(profile))
                self.assertEqual(self.run_smoke("--live").returncode, 2)
                self.assertFalse(self.artifacts.exists())
        self.profile_path.write_bytes(b'{"provider":"codex","provider":"claude"}')
        self.assertEqual(self.run_smoke("--live").returncode, 2)
        self.profile_path.unlink()
        self.assertEqual(self.run_smoke("--live").returncode, 2)
        self.assertFalse(self.artifacts.exists())

    def test_claude_tool_less_profile_and_rejection_of_added_grants(self):
        profile = {"schema_version": 1, "provider": "claude", "ephemeral": True,
                   "native_args": ["--permission-mode", "dontAsk", "--permission-prompts", "none",
                                   "--tools", "", "--strict-mcp-config", "--mcp-config", '{"mcpServers":{}}']}
        self.profile_path.write_bytes(smoke.json_bytes(profile))
        result = self.run_smoke("--dry-run")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.artifacts = self.root / "rejected"
        for args in ([*profile["native_args"], "--allowedTools", "Bash"],
                     [*profile["native_args"], "--permission-mode", "bypassPermissions"],
                     [arg if arg != "" else "Agent" for arg in profile["native_args"]]):
            self.profile_path.write_bytes(smoke.json_bytes({**profile, "native_args": args}))
            self.assertEqual(self.run_smoke("--live").returncode, 2)
            self.assertFalse(self.artifacts.exists())

    def test_deep_profile_is_rejected_before_reserving_artifacts(self):
        profile_bytes = b'[' * 10000 + b'null' + b']' * 10000
        self.profile_path.write_bytes(profile_bytes)
        with self.assertRaises(RecursionError):
            smoke.strict_json(profile_bytes)
        result = self.run_smoke("--live")
        self.assertEqual(result.returncode, 2, result.stderr)
        self.assertIn(b"smoke:", result.stderr)
        self.assertNotIn(b"Traceback", result.stderr)
        self.assertFalse(self.artifacts.exists())
        self.assertEqual(self.profile_path.read_bytes(), profile_bytes)

    def test_artifacts_inside_worker_or_existing_symlink_are_rejected(self):
        self.artifacts = self.workspace / "unsafe"
        self.assertEqual(self.run_smoke("--live").returncode, 2)
        self.assertFalse(self.artifacts.exists())
        self.artifacts = self.root / "link"
        self.artifacts.symlink_to(self.root / "missing", target_is_directory=True)
        self.assertEqual(self.run_smoke("--live").returncode, 2)
        self.assertFalse((self.root / "missing").exists())

    def test_execution_and_validation_have_independent_exit_accounting_without_retries(self):
        for mode, execution, validation, overall in (
                ("failed-valid", 7, 0, 7), ("reject", 7, 1, 7),
                ("wrong-answer", 0, 1, 1), ("malformed", 0, 2, 2), ("self-signal", 143, 2, 143)):
            with self.subTest(mode=mode):
                self.artifacts = self.root / mode
                result = self.run_smoke("--live", mode=mode)
                self.assertEqual(result.returncode, overall, result.stderr)
                summary = self.read("summary.json")
                self.assertEqual(summary["run_launcher_exit"], execution)
                self.assertEqual(summary["validation_exit"], validation)
                self.assertEqual(summary["harness_exit"], overall)
                exit_record = self.read("probe.run/exit.json")
                if mode == "self-signal":
                    self.assertIsNone(exit_record["exit_code"])
                    self.assertEqual(exit_record["signal"], "SIGTERM")
                else:
                    self.assertEqual(exit_record["exit_code"], execution)
                self.assertEqual(len(list(self.artifacts.glob("*.run"))), 1)

    def test_failed_or_malformed_plan_never_proceeds_to_live(self):
        for mode in ("plan-error", "plan-malformed", "plan-wrong-shape"):
            self.artifacts = self.root / mode
            result = self.run_smoke("--live", mode=mode)
            self.assertEqual(result.returncode, 2, result.stderr)
            self.assertIsNone(self.read("summary.json")["run_launcher_exit"])
            self.assertFalse((self.artifacts / "run.transport.json").exists())

    def test_deep_plan_records_harness_error_and_preserves_inputs_without_live_launch(self):
        schema_path = HERE / "schemas" / "minimal.schema.json"
        schema_bytes = schema_path.read_bytes()
        for option in ("--dry-run", "--live"):
            with self.subTest(option=option):
                self.artifacts = self.root / option.removeprefix("--")
                result = self.run_smoke(option, mode="plan-too-deep")
                self.assertEqual(result.returncode, 2, result.stderr)
                self.assertNotIn(b"Traceback", result.stderr)
                plan_bytes = (self.artifacts / "probe.plan/plan.json").read_bytes()
                with self.assertRaises(RecursionError):
                    smoke.strict_json(plan_bytes)
                summary = self.read("summary.json")
                self.assertIn("recursion", summary["harness_error"].lower())
                self.assertEqual(summary["harness_exit"], 2)
                self.assertEqual(summary["plan_launcher_exit"], 0)
                self.assertIsNone(summary["run_launcher_exit"])
                self.assertIsNone(summary["validation_exit"])
                self.assertIsNone(summary["signal_received"])
                exit_record = self.read("probe.plan/exit.json")
                self.assertEqual(exit_record["exit_code"], 0)
                self.assertIsNone(exit_record["signal"])
                self.assertFalse((self.artifacts / "probe.run").exists())
                self.assertFalse((self.artifacts / "run.transport.json").exists())
                captured = self.read("plan.transport.json")
                self.assertIn("--dry-run", captured["args"])
                self.assertEqual(base64.b64decode(captured["schema"]), schema_bytes)
                self.assertEqual(base64.b64decode(captured["prompt"]), smoke.PROMPT)
                self.assertEqual(base64.b64decode(captured["request"]),
                                 (self.artifacts / "probe.request.json").read_bytes())
                self.assertEqual((self.artifacts / "schema.original.json").read_bytes(), schema_bytes)
                self.assertEqual((self.artifacts / "profile.original.json").read_bytes(), self.profile_bytes)
                self.assertEqual((self.artifacts / "probe.prompt.txt").read_bytes(), smoke.PROMPT)
                self.assertEqual(self.profile_path.read_bytes(), self.profile_bytes)
                self.assertEqual(schema_path.read_bytes(), schema_bytes)

    def test_native_success_can_ignore_keyword_and_launch_failure_stays_a_failure(self):
        result = self.run_smoke("--live", "--case", "keyword", mode="keyword-ignored")
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertEqual(self.read("summary.json")["run_launcher_exit"], 0)
        self.assertEqual(self.read("answer-validation.json")["answer_validation"]["status"], "invalid")
        self.assertIn("not", self.read("answer-validation.json")["answer_validation"]["errors"][0]["schema_path"])
        self.artifacts = self.root / "launch-failure"
        result = self.run_smoke("--live", mode="remove-after-plan")
        self.assertEqual(result.returncode, 1, result.stderr)
        record = self.read("probe.run/exit.json")
        self.assertIsNone(record["exit_code"])
        self.assertEqual(record["launch_error"]["code"], "ENOENT")
        self.assertEqual(self.read("summary.json")["validation_exit"], 2)
        self.assertFalse((self.artifacts / "run.transport.json").exists())

    def test_cancellation_waits_for_cleanup_and_never_starts_another_phase(self):
        def wait_for(path):
            deadline = time.monotonic() + 5
            while time.monotonic() < deadline:
                if path.exists():
                    return
                time.sleep(0.01)
            self.fail(f"fixture did not create {path.name}")

        for mode, phase, expected in (("signal", "run", 130), ("plan-signal", "plan", 143)):
            with self.subTest(mode=mode):
                self.artifacts = self.root / mode
                process = subprocess.Popen(self.command("--live"), stdout=subprocess.PIPE,
                                           stderr=subprocess.PIPE, start_new_session=True,
                                           env={**os.environ, "FAKE_SMOKE_MODE": mode})
                try:
                    wait_for(self.artifacts / f"{phase}.ready.json")
                    process.send_signal(signal.SIGTERM)
                    wait_for(self.artifacts / f"{phase}.cleanup-started.json")
                    process.send_signal(signal.SIGTERM)
                    _, stderr = process.communicate(timeout=10)
                    self.assertEqual(process.returncode, expected, stderr)
                    self.assertTrue((self.artifacts / f"{phase}.cleanup-finished.json").exists())
                    self.assertEqual(self.read("summary.json")["signal_received"], "SIGTERM")
                    self.assertEqual(self.read(f"probe.{phase}/exit.json")["signal_received"], "SIGTERM")
                    if phase == "plan":
                        self.assertFalse((self.artifacts / "probe.run").exists())
                finally:
                    # Test-only guard for a broken fixture. The production harness
                    # has no deadline, process-group kill or automatic cleanup.
                    if process.poll() is None:
                        os.killpg(process.pid, signal.SIGKILL)
                    process.communicate()


if __name__ == "__main__":
    unittest.main()
