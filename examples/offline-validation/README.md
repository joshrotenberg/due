# Optional offline answer validation

This Python caller example evaluates backlog #5 without changing Due. It reads a
saved Due JSON result and a local answer schema and writes separate JSON evidence.
It never runs Due or a provider, inspects a repository, repairs an answer, or
retries. It does not decide whether a review is correct or approve its verdict.

Use an existing Python 3.11+ environment with `jsonschema` and its `referencing`
dependency. The offline checks here used Python **3.13.0**, `jsonschema`
**4.26.0**, and `referencing` **0.37.0**. These are optional caller dependencies,
not Due dependencies. Nothing installs them automatically during example execution. The optional
[requirements file](requirements.txt) pins the tested caller dependencies for
CI or an explicitly chosen caller environment. Evidence records the
actual `jsonschema` version; other versions need their own offline checks.

From the repository root, with a fresh output filename:

```sh
python3 -B examples/offline-validation/validate.py \
  examples/offline-validation/fixtures/request-changes.result.json \
  examples/offline-validation/fixtures/review.schema.json \
  --output /path/to/private-artifacts/validation.json
validation_status=$?
```

The output's parent directory must already exist and should be private (0700).
The evidence file is created exclusively with mode 0600. Existing files,
symlinks, and input/output aliases are refused. Input bytes are read once and
never rewritten; the evidence includes their SHA-256 hashes. A filesystem error
can leave a partial evidence file, which also cannot be reused. Errors and
validation messages can contain answer data: keep evidence private.

## Three separate decisions

| Validator exit | Meaning |
| --- | --- |
| 0 | A present structured answer satisfies the local schema. |
| 1 | Answer violates the schema, or structured output is absent. |
| 2 | Malformed input, unsupported schema policy, unresolved reference, read/write error, or CLI usage error. |

`answer_validation` records `valid`, `invalid`, `missing`, or `invalid_input`.
An invalid-input result has `valid: null`, not a validation failure assertion.
Except for usage/output-write errors, the JSON evidence is saved on failures too.

`execution` copies the checked result's `status`, `error`, and Due's
`answer_schema_validation` metadata without changing them. The validator does
**not** read or infer a process exit code. Keep the original Due/Node `exit.json`
alongside the result and inspect it separately. A failed run with an independently
valid answer produces validator exit **0** while execution stays **failed**.
The caller must still treat that execution as failed. A valid `request_changes`
answer also produces exit **0**: `domain_verdict.evaluated` stays false, and the
verdict remains in the original answer for the caller's separate decision.

Presence comes from `structured_output_present`, never Python truthiness.
`true` with `structured_output: null` validates the JSON null against the schema;
`false` with the same null placeholder is missing even when the schema accepts
null. A missing presence flag, omitted output key, or false flag paired with a
non-null value is malformed input.

## Schema and envelope policy

- **Draft 2020-12 only.** An absent `$schema` means that dialect. An explicit
  declaration, including in a nested object, must exactly equal
  `https://json-schema.org/draft/2020-12/schema`. Boolean schemas are supported.
- **One local resource.** `$ref` and `$dynamicRef` must start with `#`; JSON
  pointers and anchors within that document are allowed. Empty references,
  relative filenames, absolute files, and network URLs are rejected. `$id`,
  `$vocabulary`, and the older `$recursiveRef` are rejected. No schemas are loaded
  from adjacent files, caches, or the network; the resolver's retrieval callback
  always fails. The known dialect URI selects built-in validation; it is not
  fetched.
- The reference/dialect scan visits **every object and array**, including unused
  definitions, unknown keywords, examples, and constants. A remote reference
  hidden there is rejected before validation. This deliberately rejects some
  otherwise valid schemas containing schema-like data in annotations/constants.
  An unresolved local reference encountered during validation is an input error.
- **Formats are annotations only.** No `FormatChecker` is enabled: `format:
  "email"` does not assert that a string is an email. Standard Draft 2020-12
  assertions are checked; unrecognized extension keywords carry no assertion.
  This example does not establish native support for any keyword.
- JSON must be UTF-8 with no duplicate object keys, trailing data, NaN, Infinity,
  or numbers outside Python's finite floating-point range. This small example is
  for trusted, bounded inputs; it supplies no CPU/memory isolation for hostile
  schemas or regular expressions.

Envelope validation is intentionally limited. It requires version 1, a known
execution status, an output key plus boolean presence flag, recognized Due
answer-validation metadata, and an error that is null or has string
`kind`/`message` and nullable integer `provider_exit_code`. Failure statuses
require an error; other statuses require null. Extra fields are allowed.
It does **not** validate the complete provider, session, usage, trace, invocation,
or error-kind contracts. It neither exports nor silently substitutes a Due
schema. For full envelope checking, callers may separately use the schema
exported by their installed `due schema result` under an appropriate validator.

## Offline checks

```sh
python3 -B -m unittest discover -s examples/offline-validation -p 'test_*.py' -v
```

Fixtures cover approve/request_changes, wrong answer shape, malformed JSON,
present-null/missing, failed execution with a valid answer, an unsupported
dialect, and a hidden remote reference. Additional checks cover strict envelope
fields, local references, formats, blocked retrieval, and unchanged input and
existing output bytes. Temporary test files stay in this example directory and
are removed after the test; no provider, network, or dependency installation is
used. See the [native smoke example](../native-schema-smoke/README.md) for the
separate, explicitly invoked provider-compatibility question.
