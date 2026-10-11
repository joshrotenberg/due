# A small Node caller

This optional example uses Node.js 22+ built-ins on local macOS/Linux. It starts
one Due process, saves its outputs, and returns its outcome. Install Due and the
chosen native provider separately. Existing provider authentication, permissions,
and user authorization apply. The launcher adds no retries or execution policy.

Prepare an assignment directory with `<id>.request.json` (a Due request/profile)
and `<id>.prompt.txt` (the complete prompt). IDs contain 1–80 ASCII letters,
digits, underscores, or hyphens and must start with a letter or digit.

```sh
node examples/node-caller/run-due.mjs /path/to/assignments review-1 --dry-run
node examples/node-caller/run-due.mjs /path/to/assignments review-1
# Optionally choose a particular installed Due binary:
node examples/node-caller/run-due.mjs /path/to/assignments review-2 --due /path/to/due
```

The assignment directory selects input/output locations; it does **not** change
Due's working directory. Request `cwd` and `schema` paths resolve from the shell's
current directory, following Due's normal rules. Prefer absolute paths when the
launcher may run from different directories. The prompt is sent as bytes through
stdin; Due checks its UTF-8 and task semantics. Args are passed directly without a
shell. `--stdin` replaces any prompt inherited from the request.

Each dry run reserves `<id>.plan/`; each real run reserves `<id>.run/`. Existing
directories fail before spawning, including directories from failed or partial
attempts. Dry-run and execution can use the same ID because they have separate
directories. Choose a new ID for a follow-up and inspect prior effects first.

| File | Meaning |
| --- | --- |
| `plan.json` / `result.json` | Due stdout, saved directly; can be empty or incomplete on failure. |
| `stderr.log` | Due stderr, including diagnostics that can contain task content. |
| `trace.jsonl` | Real-run progress, created by Due; absent if intake fails before tracing. |
| `exit.json` | Actual child `exit_code` and `signal`, launch/stdin errors, first received signal, timestamps, executable, and argv. |

Directories are created with mode 0700 and output files with mode 0600 (subject
to the process umask); Due creates its trace exclusively with mode 0600. These
artifacts may contain private task data. The launcher does not parse, validate,
redact, or summarize them. An input/preflight failure creates no run directory;
a later filesystem failure or forced termination can leave partial artifacts.
Launch/stdin error records contain `code` (or null) and `message`.

`exit.json` preserves a numeric child exit separately from a terminating signal.
The caller returns a nonzero child exit, or 128 plus the signal number for a
signaled child. Launch/stdin failures return 1 if there is no failing child code.
If a parent signal was received but Due exits 0, the caller returns that signal's
conventional status instead of reporting success; the saved child exit stays 0.

SIGINT, SIGTERM, and SIGHUP are forwarded once to Due, including a signal received
during spawn. The launcher waits for child `close`, allowing Due to finish its
cleanup. Cancellation ends at child close; later signals are ignored while the
launcher finalizes that outcome's artifacts. It does not kill process groups or
impose a second deadline. Due owns provider cancellation; there is no universal
hard-kill guarantee. SIGKILL, machine
failure, or an uncooperative child can prevent cleanup or a complete exit record.
Current Due handles all three signals through its cooperative cancellation path;
handled cancellation normally exits 130, independently of the received signal.

Validate a review and summarize a trace as separate caller decisions:

```sh
node examples/node-caller/validate-review.mjs DIR ID EXPECTED_PR
due trace summarize DIR/ID.run/trace.jsonl --output json
```

A successful process exit, a valid structured answer, and the review's verdict
are different checks. The review validator checks the saved exit/result and this
example's specific review-answer contract; it is not a generic JSON Schema or Due
envelope validator. `request_changes` is a valid completed review. Success writes
a fresh `validated.json` with the verdict and validation evidence; existing files
are never replaced. Independent inspection of the diff, tests, and authorized
effects is still required. Neither this launcher nor Due proves a review correct.

The bundled answer schema omits an explicit dialect declaration for native
provider compatibility. Claude Code 2.1.296 rejected a Draft 2020-12 `$schema`
URI during this trial. Due's `--dry-run` checks schema JSON shape, not native
schema support; these fake-provider tests do not establish native compatibility.

The included `pr-101.request.json` uses a schema path relative to this repository
root, so run it from there. Copy/edit the request and prompt for the actual PR:
set request `cwd` to the intended local repository, replace the prompt's
`BASE..HEAD` placeholders, and give the validator the expected PR number
independently of the worker's answer.

Run the example's offline checks from the repository root:

```sh
node --test examples/node-caller/caller.test.mjs
```

The tests use an executable fixture to check argument/stdin transport, exclusive
artifacts, spawn and stdin failures, cancellation, exit/signal preservation, and
review validation. They do not start a provider or spend model calls. The real
launcher performs a provider run unless `--dry-run` is supplied.
