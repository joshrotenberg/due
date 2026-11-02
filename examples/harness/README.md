# A supervised assignment in JavaScript

`supervision.js` uses the JavaScript runtime embedded in Due. It keeps one
Claude or Codex session through a bounded implement, verify, revise loop.
The caller selects the task, profile and independent verifier; a successful
worker report alone never accepts the assignment.

Save an assignment JSON file, substituting your own paths:

```json
{
  "profile": {"request": "/path/to/worker.json", "cwd": "/path/to/project"},
  "task": "Fix the parser regression and add a test.",
  "verify": {
    "program": "cargo",
    "args": ["test", "--locked"],
    "cwd": "/path/to/project",
    "timeout_ms": 120000
  },
  "max_revisions": 2
}
```

Run it with a fresh evidence directory:

```sh
due script examples/harness/supervision.js \
  --args-file assignment.json --evidence /tmp/due-assignment --timeout 5m
```

`--timeout` bounds the complete workflow. Each turn has a separate five-minute
wait by default; set the optional `turn_timeout_ms` argument to a positive
integer up to `2147483647` when the assignment needs a different turn wait.
The verifier's `timeout_ms` and the native profile's timeout are also independent.

Stdout is one versioned JSON result. On success, its `result` contains
`accepted: true` and the checked attempts. Session items and check summaries go
to stderr through `Due.log`; original RPC bytes and process/command receipts
are also retained in the evidence directory. Failure never returns an accepted
result, including when cleanup fails. Inspect the diff and other task effects
before relying on the result.

The script denies unexpected tool approvals. An optional `permission_check`
command exercises asynchronous permission work; it still ends in denial. Only
normal nonzero verifier exits trigger revision. Timeouts, cancellation, output
overflow and other verifier failures stop the assignment.

The verifier is a trusted caller-selected command running with the caller's
environment and permissions. Native profile sandboxes do not constrain it.
Never derive its program or arguments from model output. No Node installation,
npm package or JavaScript module loader is needed.

See the [harness guide](../../docs/harness.md) for API, cancellation, profile,
evidence and result-contract details. The provider-free acceptance tests run this
example through the actual Due server and fake Claude/Codex binaries.
