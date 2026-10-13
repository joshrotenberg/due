# Denied-command trace evidence

Snapshot: 2026-10-13, installed Due 0.1.0, Codex CLI 0.162.0,
macOS 27 arm64. This is a bounded investigation of
[#6](https://github.com/joshrotenberg/due/issues/6), with no runtime,
wrapper, installed binary or permission bypass changes.

The caller used Due's existing `--binary` option with a transparent capture
helper. The helper launches the selected native executable with the same arguments
and stdin, copies every stdout byte both to a fresh private file and Due,
inherits stderr, and preserves the native exit. Offline fixtures checked byte
transport including non-UTF-8 bytes, stderr, nonzero exit and exclusive capture.
This is investigation instrumentation, not a recommended alternate runner.

The two 0.162.0 script turns and inline turn retained the revised
[eight-setting profile](../examples/codex-workspace-profile.json) policy unchanged.
The checked-in profile SHA-256 was `e68d1cf569519177243ba43eefb08e14a7e2022bcd2695bdd5ced0d1c836e903`; saved requests matched all eight
native arguments (cwd/schema/resume were supplied separately). The caller also
verified actual workspace writes for the script controls. Native contexts exposed
workspace-write, approval never, network false and both excluded temp grants;
agent/v2 values and the explicit additional-root list were not exposed.
Forwarding and published-source gate semantics are not a runtime tool manifest.

Each live turn had a successful control and exactly one outside-write attempt.
The caller matched native session/turn, unique tokens, tool calls and actual
outputs, original command IDs where emitted, native JSON and Due trace; it
independently checked the disposable outside canary stayed unchanged.

| Case | Native command completions | Native failed commands | Due failed activities | Evidence |
| --- | ---: | ---: | ---: | --- |
| Fresh, script-based probe, 0.162.0 | 2 | 1 | 1 | Fresh-token ledger; actual PermissionError; exit 13 |
| Resumed, same script probe, 0.162.0 | 2 | 1 | 1 | Same native session, new token; actual denial; exit 13 |
| Resumed, inline Python write, 0.162.0 | 1 | 0 | 0 | Two actual session tool outputs, exit 0 then 1; uncaught PermissionError |

The inline Python case reproduced the missing event. The task-owned session
record contains the exact attempted write and actual exception output, with a
unique turn token; raw native stdout contains the successful command but no
command item for the denied write. Due's 17-record trace correctly represents
what it received. All eight native stdout records parsed, and the Due result
and trace schemas validated with contiguous sequences, a terminal event and no
reported queue drops. Execution succeeded after the expected denial.

The two 0.162.0 script controls had 10 native records each and 18/19 Due trace
records. The exact captured resumed stream also passed an offline replay through
unchanged installed Due, retaining one failed activity. An initial extra control
used another installed Codex executable, 0.162.1; it also emitted both commands.
It remains a separate version-confounded observation, not proof of a native fix.
Neither executable was installed or updated by this batch.

The differing command shapes discriminate the observations, but do not establish
a universal or deterministic trigger. There was no controlled model/effort or
shell-shape trial; settings were omitted and observed identity remains unknown.
This evidence localizes the missing record to the native stdout stream in this
case, not its internal cause. It does not claim that no native permission denial
can ever be traced or that script probes repair the omission. Default traces
remain observations, not a complete command or permission audit. Do not fabricate
failure records from model prose or change Due's result status because an expected
negative control was denied.

Native child lifecycle/usage semantics remain separate work in
[#8](https://github.com/joshrotenberg/due/issues/8). Raw logs, prompts,
local paths, session IDs and private artifact hashes stay local; sanitized aggregate
evidence belongs on the matching issue.
