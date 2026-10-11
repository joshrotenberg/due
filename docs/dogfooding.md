# Dogfooding and reporting issues

Features are frozen. Use Due for ordinary bounded work and report reproducible
problems, confusing behavior, or inaccurate documentation. Fix reliability and
installation defects before considering new commands or runner features.

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
