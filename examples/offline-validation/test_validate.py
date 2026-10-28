import copy
import json
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

import validate

HERE = Path(__file__).resolve().parent
FIXTURES = HERE / "fixtures"


class ValidationTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(dir=HERE, prefix="test-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.result = json.loads((FIXTURES / "approve.result.json").read_bytes())

    def evaluate(self, result=None, schema=None):
        result_path = self.root / "result.json"
        schema_path = self.root / "schema.json"
        result_path.write_bytes(validate.json_bytes(self.result if result is None else result))
        schema_path.write_bytes(validate.json_bytes(True if schema is None else schema))
        return validate.evaluate(result_path, schema_path)

    def test_saved_fixtures_distinguish_validation_execution_and_verdict(self):
        cases = [
            ("approve", "review", 0, "valid", "succeeded"),
            ("request-changes", "review", 0, "valid", "succeeded"),
            ("wrong-shape", "review", 1, "invalid", "succeeded"),
            ("present-null", "null", 0, "valid", "succeeded"),
            ("present-null", "review", 1, "invalid", "succeeded"),
            ("missing", "null", 1, "missing", "succeeded"),
            ("failed-valid", "review", 0, "valid", "failed"),
            ("malformed", "review", 2, "invalid_input", None),
            ("approve", "unsupported-dialect", 2, "invalid_input", "succeeded"),
            ("approve", "external-ref", 2, "invalid_input", "succeeded"),
        ]
        before = {p: p.read_bytes() for p in FIXTURES.iterdir()}
        for result, schema, code, status, execution in cases:
            with self.subTest(result=result, schema=schema):
                evidence, actual = validate.evaluate(FIXTURES / f"{result}.result.json",
                                                     FIXTURES / f"{schema}.schema.json")
                self.assertEqual(actual, code)
                self.assertEqual(evidence["answer_validation"]["status"], status)
                self.assertEqual((evidence["execution"] or {}).get("status"), execution)
                self.assertEqual(evidence["domain_verdict"], {"evaluated": False})
        for path, raw in before.items():
            self.assertEqual(path.read_bytes(), raw)

    def test_envelope_subset_rejects_ambiguity_and_wrong_types(self):
        mutations = [
            {"schema_version": True}, {"schema_version": 1.0}, {"schema_version": 2},
            {"status": "okay"}, {"structured_output_present": "true"},
            {"structured_output_present": False}, {"status": "failed"},
            {"error": {}}, {"answer_schema_validation": "passed"},
        ]
        candidates = [[], None, {"structured_output": None}]
        candidates += [{**self.result, **change} for change in mutations]
        for field in ("structured_output_present", "structured_output", "error", "status"):
            candidate = copy.deepcopy(self.result)
            del candidate[field]
            candidates.append(candidate)
        for candidate in candidates:
            # None is deliberately passed through a file, not the helper's default.
            path = self.root / "bad.json"
            path.write_bytes(validate.json_bytes(candidate))
            with self.subTest(candidate=candidate):
                self.assertEqual(validate.evaluate(path, FIXTURES / "null.schema.json")[1], 2)

    def test_strict_json_rejects_non_json_or_ambiguous_documents(self):
        for raw in (b'{"x":1,"x":2}', b'NaN', b'Infinity', b'1e999', b'{} {}', b'\xff'):
            with self.subTest(raw=raw), self.assertRaises(validate.InputError):
                validate.strict_json(raw)
        for raw in (b'{', b'{"type":"string","type":"number"}'):
            path = self.root / "bad-schema.json"
            path.write_bytes(raw)
            evidence, code = validate.evaluate(FIXTURES / "approve.result.json", path)
            self.assertEqual(code, 2)
            self.assertEqual(evidence["answer_validation"]["status"], "invalid_input")

    def test_remote_file_relative_and_hidden_refs_never_retrieve(self):
        # Fail loudly on any network attempt, including hidden unused schema branches.
        with patch.object(socket.socket, "connect", side_effect=AssertionError("network attempted")), \
                patch("validate.deny_retrieval", side_effect=AssertionError("retrieval attempted")):
            for ref in ("https://example.invalid/a", "file:///never-read", "other.json#/x", ""):
                for key in ("$ref", "$dynamicRef", "$recursiveRef"):
                    for schema in ({key: ref}, {"$defs": {"unused": {key: ref}}},
                                   {"examples": [{"deep": {key: ref}}]}):
                        with self.subTest(schema=schema):
                            self.assertEqual(self.evaluate(schema=schema)[1], 2)

    def test_local_pointer_anchor_dynamic_ref_and_format_annotation(self):
        schemas = [
            {"$defs": {"answer": {"type": "object"}}, "$ref": "#/$defs/answer"},
            {"$defs": {"answer": {"$anchor": "answer", "type": "object"}}, "$ref": "#answer"},
            {"$defs": {"answer": {"$dynamicAnchor": "answer", "type": "object"}},
             "$dynamicRef": "#answer"},
            {"properties": {"summary": {"type": "string", "format": "email"}}},
        ]
        for schema in schemas:
            with self.subTest(schema=schema):
                self.assertEqual(self.evaluate(schema=schema)[1], 0)
        self.assertEqual(self.evaluate(schema=False)[1], 1)

    def test_bad_schema_or_unresolvable_local_reference_is_input_error(self):
        for schema in ({"type": "banana"}, {"$ref": "#/missing"},
                       {"$id": "https://example.invalid/base"},
                       {"$vocabulary": {}}, {"$defs": {"unused": {"$schema": "old"}}},
                       {"$ref": 3}, {"$ref": "#"}):
            with self.subTest(schema=schema):
                evidence, code = self.evaluate(schema=schema)
                self.assertEqual(code, 2)
                self.assertEqual(evidence["answer_validation"]["status"], "invalid_input")

    def test_cli_exclusive_evidence_never_truncates_inputs_or_existing_paths(self):
        result = self.root / "result.json"
        schema = self.root / "schema.json"
        result.write_bytes((FIXTURES / "failed-valid.result.json").read_bytes())
        schema.write_bytes((FIXTURES / "review.schema.json").read_bytes())
        before = {p: p.read_bytes() for p in (result, schema)}
        out = self.root / "evidence.json"
        def run(output):
            return subprocess.run([sys.executable, "-B", str(HERE / "validate.py"),
                                   str(result), str(schema), "--output", str(output)],
                                  capture_output=True, timeout=10)
        self.assertEqual(run(out).returncode, 0)
        saved = out.read_bytes()
        self.assertEqual(out.stat().st_mode & 0o777, 0o600)
        self.assertEqual(json.loads(saved)["execution"]["error"]["provider_exit_code"], 7)
        self.assertEqual(run(out).returncode, 2)
        link = self.root / "link.json"
        link.symlink_to(result)
        for path in (result, schema, link):
            self.assertEqual(run(path).returncode, 2)
        self.assertEqual(out.read_bytes(), saved)
        for path, raw in before.items():
            self.assertEqual(path.read_bytes(), raw)


if __name__ == "__main__":
    unittest.main()
