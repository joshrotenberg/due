# Using Due and reporting issues

Use Due for ordinary development work: one-shot tasks, saved-context follow-ups,
or supervised resident assignments. Report concrete defects, confusing behavior
and repeated friction. Feature development is open. There is no mandatory
Due-only delegation, success-reporting or A/B workflow; see [caller scope and
verification](caller-policy.md).

## Keep enough evidence

Record the Due source commit, resolved executable path, Due/provider versions,
OS/architecture and host. Note whether the binary and plugin were both refreshed.
Two source builds can share a version, so a source commit is useful when
reproducing a problem.

For a one-shot task, save the JSON result and a fresh trace:

```sh
due run --request examples/profile.json --stdin --output json \
  --trace --trace-file run.jsonl < task.txt > result.json
run_status=$?
if [ -f run.jsonl ]; then
  due trace summarize run.jsonl --output json > trace-summary.json
fi
```

Preserve `run_status` before running another command. Intake failures may create
no trace; CLI syntax errors precede the result envelope. Default traces omit task
content, but result/error text, plans and detailed traces can contain it. Check
`trace.incomplete` and reported loss; missing terminal records do not prove a
worker outcome.

For a supervised assignment, preserve the script, caller arguments and result
privately, along with its fresh evidence directory:

```sh
due script workflow.js --args-file assignment.json \
  --cwd /path/to/project --evidence /path/to/new-evidence > result.json
script_status=$?
```

The [harness guide](harness.md) describes raw Due stdout, process and command
receipts, JSON results and limits. Distinguish a failed native turn, a failed
independent check, lost observation, script error and cleanup failure. Evidence
paths in a failed result do not guarantee that every file was created.

Inspect actual diffs, files, artifacts and authorized external effects. Failure,
timeout and cancellation can leave completed edits. Do not repeat a mutation
solely to obtain cleaner diagnostics. A worker's narrative, success status or
activity stream is not independent acceptance evidence.

## File a useful issue

Use the [issue form](https://github.com/joshrotenberg/due/issues/new/choose), or
prepare a report and submit it within the task's authorization:

```sh
gh issue create --repo joshrotenberg/due --title 'Brief concrete symptom' --body-file report.md
```

Include:

1. Expected and actual behavior, including exit status.
2. A minimal command/profile, script or RPC sequence with synthetic inputs.
3. Source commit, versions, host and platform information.
4. Relevant result/error fields or a short sanitized receipt/trace summary.
5. Independently observed effects and any workaround.

Search existing issues first. Review attachments before posting: private
repositories are still shared with their members. Remove credentials, private
source/task content, unrelated paths and raw configuration. Prefer a synthetic
reproduction over raw transcripts or full provider logs. If issue filing is not
authorized, provide a sanitized draft.

A provider refusal, permission denial, unavailable model or native CLI failure
is not automatically a Due defect. Identify the layer supported by the evidence.
Do not broaden permissions or substitute a different policy to hide a failure.

## What to learn from normal use

Source installation, relocated binaries, plugin discovery and existing request
profiles should work without a checkout at runtime. Fake-provider tests and CI
cover process/protocol behavior, but do not prove every native version, permission
configuration or model behavior. Keep wrapper revisions fixed within a report;
review updates separately.

Capture small examples of repeated caller work: constructing profiles, handling
interaction shapes, inspecting streamed items, writing verifiers, choosing turn
boundaries and interpreting evidence. Explain why a one-shot call, resume, the
supported harness or raw RPC was insufficient. This distinguishes a documentation
gap from an API or implementation change worth making.

One-shot callers still own process launch, stdin closure, result capture and
cancellation forwarding. Harness callers use the Rust-owned bridge for those
responsibilities, await every managed operation and inspect turn/check results.
The [Node caller](../examples/node-caller/README.md) and
[external session client](../examples/session-harness/README.md) remain examples
for independently implemented clients; they are not dependencies of Due.

## Follow-up and redirection

`due run --resume ID` starts a new invocation with persisted native context.
A resident RPC/script worker keeps one native session across turns. To redirect
an active turn, interrupt its exact Due turn ID, await terminal settlement,
inspect partial effects, then submit the correction as a new prompt. There is
no general in-flight prompt amendment or durable reconnection after process loss.

When a workflow needed clarification or redirection, note when that information
became available, which interface was used, and whether interruption/resume/new
turn behavior was sufficient. A sent message alone does not prove the worker
acted on it, and no correction undoes earlier effects. Use these concrete cases
to guide further development without requiring a report for every routine task.
