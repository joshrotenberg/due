# Dogfooding and reporting issues

Features are frozen. Use Due for ordinary bounded work and report reproducible
problems, confusing behavior, or inaccurate documentation. Fix reliability and
installation defects before considering new commands or runner features.

For an opt-in policy covering all delegated work, positive and negative reports,
and controlled comparisons, use the [caller policy](caller-policy.md). Enable it
through [global host instructions](install.md#enable-the-caller-trial-policy).

## Keep enough evidence

Record the Due source commit, resolved executable path, Due/provider versions,
OS/architecture, and host (Codex desktop, CLI, or another plugin client). Note
whether the plugin and binary were both refreshed. A source build can share
`due --version` with another commit.

For work where progress matters, prepare your prompt in `task.txt` and use a
profile such as the checked-in `examples/profile.json`. Choose a fresh trace
filename and save the result:

```sh
due run --request examples/profile.json --stdin --output json \
  --trace --trace-file run.jsonl < task.txt > result.json
run_status=$?
if [ -f run.jsonl ]; then
  due trace summarize run.jsonl --output json > trace-summary.json
fi
```

Preserve `run_status` before running anything else. Intake failures may create no
trace file. CLI argument errors occur before the JSON envelope; retain stderr in
that case. Default traces omit task
content, but results, errors, dry-run plans and `--trace-detail` can contain it.
Trace summaries are observed events, not proof of side effects or complete log
delivery. Check the result's `trace.incomplete` too.

The profile sets no timeout. Watch the trace and artifacts and cancel with
Ctrl-C if needed, or add a deliberate `--timeout` when nobody is monitoring; see
[deadlines and monitoring](cli.md#deadlines-and-monitoring).

Inspect the actual diff, files, artifacts and relevant checks. Failure, timeout,
or cancellation can leave completed edits. Before any follow-up, inspect those
effects; do not retry a mutation simply to obtain cleaner diagnostic evidence.

## File a useful issue

Use the [issue form](https://github.com/joshrotenberg/due/issues/new/choose), or
prepare a report file and submit it explicitly:

```sh
gh issue create --repo joshrotenberg/due --title 'Brief concrete symptom' --body-file report.md
```

Include:

1. What you expected and what actually happened, including the exit code.
2. A minimal command/profile and synthetic prompt that reproduce the problem.
3. Source commit, versions, host and platform information.
4. Relevant normalized error fields and a short trace summary, if available.
5. What side effects you independently observed and any workaround you used.

Search existing issues first and add evidence to a matching one when appropriate.
Review attachments before sending: a private repository is still shared with its
members. Remove secrets, credentials, personal/work content, and unrelated paths.
Prefer a small synthetic reproduction over entire prompts, raw provider logs,
or trace-detail files. Do not include authentication/config files.

Agents should report Due problems while carrying out the user's task when issue
filing is authorized. Otherwise prepare a sanitized draft for the caller. A
provider refusal, permission denial, or unavailable model is not automatically a
Due bug: include the normalized outcome and why you think translation is wrong.
Do not expand permissions or run the task directly with a different policy merely
to make a failure disappear.

## What this trial establishes

Source installation and plugin discovery must work on each machine. Independent
reviews and CI reduce risk; they do not establish behavior under every provider
version or host policy. Keep the wrapper Git pins fixed during the trial so
reports remain comparable. Track feature requests as follow-up ideas without
implementing them during the freeze.

## Caller scripts and agent experience

The [tested Node caller example](../examples/node-caller/README.md) implements
the pattern reported in [#4](https://github.com/joshrotenberg/due/issues/4), with
exclusive run directories, failure records, cancellation forwarding, and a
task-specific review validator. Its offline tests use a fake Due executable;
Node is needed only to run this optional example.

Keep small examples of glue code agents write around Due: prompt-file loading,
stdout/stderr capture, trace paths, exit/signal records, and cancellation handling.
Include the caller's language/runtime and why ordinary invocation or redirection
was insufficient. This separates missing guidance from repeated work that a
future CLI improvement might remove. Collect issues during the freeze before
choosing features.

When a caller launches Due, pass an argument array, close prompt stdin, handle
spawn errors, and wait for process/output closure before consuming final files.
Preserve exit codes and termination signals separately. Forward cancellation to
Due and allow its cleanup to finish; use fresh artifact paths. These are caller
responsibilities today, not an additional Due runner or service.

## Resume and possible midstream corrections

The default interaction remains a bounded request followed by a result. Context
can carry across requests without making ongoing conversation the default.

- **Resume** starts a new request using the provider's saved context, for refinement,
  follow-up work, or a correction after a run. Due already supports `--resume`
  with an explicit native session ID; the session must be persisted and resumable.
- **Midstream correction** would amend an active request when monitoring reveals
  information worth acting on. This is a future option, not current Due behavior
  or an exception to the feature freeze.

These uses overlap, but their lifecycles differ. Any future midstream mechanism
must make acceptance and when the correction takes effect observable in the
trace. Sending a message alone is not evidence that the worker acted on it, and
an amendment cannot undo earlier side effects. Two-way communication would not
by itself require an MCP server or change the one-run, one-agent scope.

During the trial, record cases where a worker needed clarification, monitoring
revealed a useful correction, or a follow-up benefited from saved context. Note
when the information became available, whether a new/resumed request was enough,
and any avoidable interruption or wasted work. Repeated small clarifications or
substantial work after a known correction would justify revisiting steering.
Continue using bounded assignments and explicit follow-ups while collecting that
evidence; inspect effects before resuming interrupted work.
