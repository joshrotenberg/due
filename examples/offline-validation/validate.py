#!/usr/bin/env python3
"""Optional caller-side validation of saved JSON; never runs Due or a provider."""
import argparse
import hashlib
import importlib.metadata
import json
import math
import os
from pathlib import Path
import sys

from jsonschema import Draft202012Validator
from jsonschema.exceptions import SchemaError
from referencing import Registry
from referencing.exceptions import NoSuchResource, Unresolvable

DIALECT = "https://json-schema.org/draft/2020-12/schema"

# Deliberately a subset, not a copy of Due's exported result contract.
ENVELOPE = {
    "type": "object",
    "required": ["schema_version", "status", "structured_output_present",
                 "structured_output", "answer_schema_validation", "error"],
    "properties": {
        "schema_version": {"type": "integer", "const": 1},
        "status": {"enum": ["planned", "succeeded", "failed", "timed_out", "cancelled"]},
        "structured_output_present": {"type": "boolean"},
        "answer_schema_validation": {
            "enum": ["not_resolved", "not_requested", "not_performed"]},
        "error": {"oneOf": [
            {"type": "null"},
            {"type": "object", "required": ["kind", "message", "provider_exit_code"],
             "properties": {"kind": {"type": "string"}, "message": {"type": "string"},
                            "provider_exit_code": {"type": ["integer", "null"]}}},
        ]},
    },
}


class InputError(ValueError):
    pass


def strict_json(raw):
    """Reject duplicate keys, non-JSON numbers, invalid UTF-8 and trailing data."""
    def pairs(items):
        result = {}
        for key, value in items:
            if key in result:
                raise InputError("duplicate JSON object key")
            result[key] = value
        return result

    def constant(_):
        raise InputError("non-finite JSON number")

    def number(value):
        parsed = float(value)
        if not math.isfinite(parsed):
            raise InputError("JSON number exceeds finite float range")
        return parsed

    try:
        return json.loads(raw.decode("utf-8"), object_pairs_hook=pairs,
                          parse_constant=constant, parse_float=number)
    except (UnicodeError, ValueError) as error:
        raise InputError(f"invalid JSON: {error}") from error


def deny_retrieval(uri):
    # Defense in depth: no urllib, file opener, or implicit network resolver.
    raise NoSuchResource(ref=uri)


def answer_validator(schema):
    """One resource, one dialect, local fragments only, format annotations only."""
    if not isinstance(schema, (dict, bool)):
        raise InputError("answer schema must be an object or boolean")
    pending = [schema]
    while pending:
        node = pending.pop()
        if isinstance(node, list):
            pending.extend(node)
        elif isinstance(node, dict):
            # Walk everything, even unused $defs, annotations and unknown keywords.
            # This intentionally over-rejects data with schema-like reserved keys.
            if "$schema" in node and node["$schema"] != DIALECT:
                raise InputError("unsupported dialect; only Draft 2020-12 is accepted")
            if "$id" in node or "$vocabulary" in node:
                raise InputError("$id and $vocabulary are outside the single-resource policy")
            for keyword in ("$ref", "$dynamicRef", "$recursiveRef"):
                if keyword in node:
                    ref = node[keyword]
                    if not isinstance(ref, str) or not ref.startswith("#"):
                        raise InputError(f"{keyword} must be a local fragment beginning with #")
                    if keyword == "$recursiveRef":
                        raise InputError("$recursiveRef is not a Draft 2020-12 keyword")
            pending.extend(node.values())
    try:
        Draft202012Validator.check_schema(schema)
    except SchemaError as error:
        raise InputError(f"invalid Draft 2020-12 schema: {error.message}") from error
    return Draft202012Validator(schema, registry=Registry(retrieve=deny_retrieval),
                                format_checker=None)


def envelope_fields(result):
    errors = list(Draft202012Validator(ENVELOPE).iter_errors(result))
    if errors:
        raise InputError(f"invalid result envelope subset: {errors[0].message}")
    # jsonschema treats 1.0 as an integer; Due's version field is an integer token.
    if type(result["schema_version"]) is not int:
        raise InputError("schema_version must be integer 1")
    if not result["structured_output_present"] and result["structured_output"] is not None:
        raise InputError("absent structured output must have a null placeholder")
    failed = result["status"] in ("failed", "timed_out", "cancelled")
    if failed != (result["error"] is not None):
        raise InputError("result status and error disagree")


def sha256(raw):
    return hashlib.sha256(raw).hexdigest()


def evaluate(result_path, schema_path):
    """Return (evidence, validation_exit). Never mutate either input."""
    evidence = {
        "evidence_version": 1,
        "validator": {"library": "jsonschema", "version": importlib.metadata.version("jsonschema"),
                      "dialect": DIALECT, "format_policy": "annotation_only",
                      "reference_policy": "single_resource_fragments_only"},
        "envelope_scope": "required answer/execution fields only; not full Due envelope validation",
        "input_sha256": {},
        "execution": None,
        "domain_verdict": {"evaluated": False},
        "answer_validation": {"status": "invalid_input", "valid": None, "errors": []},
    }
    try:
        raw_result = Path(result_path).read_bytes()
        evidence["input_sha256"]["result"] = sha256(raw_result)
        raw_schema = Path(schema_path).read_bytes()
        evidence["input_sha256"]["schema"] = sha256(raw_schema)
        result = strict_json(raw_result)
        envelope_fields(result)
        evidence["execution"] = {
            "status": result["status"], "error": result["error"],
            "due_answer_schema_validation": result["answer_schema_validation"],
            "process_exit": "not_read; inspect the saved exit separately",
        }
        validator = answer_validator(strict_json(raw_schema))
        answer = evidence["answer_validation"]
        if not result["structured_output_present"]:
            answer.update(status="missing", valid=False)
            return evidence, 1
        errors = list(validator.iter_errors(result["structured_output"]))
        answer.update(status="invalid" if errors else "valid", valid=not errors,
                      errors=[{"instance_path": list(error.absolute_path),
                               "schema_path": list(error.absolute_schema_path),
                               "message": error.message} for error in errors])
        return evidence, 1 if errors else 0
    except (OSError, InputError, Unresolvable, RecursionError) as error:
        evidence["answer_validation"]["errors"] = [{"message": str(error)}]
        return evidence, 2


def write_private(path, raw):
    """Exclusive creation also refuses existing symlinks and input/output aliases."""
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, "wb") as stream:
        os.fchmod(stream.fileno(), 0o600)
        stream.write(raw)


def json_bytes(value):
    return (json.dumps(value, indent=2, ensure_ascii=True, allow_nan=False) + "\n").encode("utf-8")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("result", type=Path)
    parser.add_argument("schema", type=Path)
    parser.add_argument("--output", type=Path, required=True,
                        help="fresh evidence file; existing paths are never overwritten")
    args = parser.parse_args()
    evidence, code = evaluate(args.result, args.schema)
    try:
        write_private(args.output, json_bytes(evidence))
    except OSError as error:
        print(f"validate: cannot save evidence: {error}", file=sys.stderr)
        return 2
    return code


if __name__ == "__main__":
    sys.exit(main())
