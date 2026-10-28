# Native schema smoke: Claude Code 2.1.312 and Codex CLI 0.163.0

Snapshot: 2026-10-28. Tracks [#7](https://github.com/joshrotenberg/due/issues/7).
Evidence and testing of the existing optional caller example; no Due runtime,
wrapper pin, installed CLI, schema or permission change.

## Dependency on #87

The harness, schemas and Codex profile exist only in unmerged
[#87](https://github.com/joshrotenberg/due/pull/87). They were used unchanged
from a detached worktree at its exact head
[`437aea5`](https://github.com/joshrotenberg/due/tree/437aea507f4f3ae3a24e4e0071558a2e2e4a845f/examples/native-schema-smoke).
If #87 changes the harness, schemas or profile after that commit, this page
describes the commit above, not the merged files.

| Input | SHA-256 |
| --- | --- |
| `examples/native-schema-smoke/smoke.py` | `65e676e787a0b3bbb25c47fe3cc152482dd3d30dda302be57390f631ac64cd03` |
| `examples/node-caller/run-due.mjs` | `d431f8a2842aac301f78d04cf62bc42ca8709741ff66d680c55e7c326795d228` |
| `schemas/minimal.schema.json` | `bee2fc28dc3135fa71dee92b4b6b0cd58632f4aa15d6b7ccaa6969c042f0910a` |
| `schemas/dialect.schema.json` | `31cc33b6b6e1fd6b0e395d92b3d31c17d528556dc8b55f1f72a2bc0110ff3187` |
| `schemas/keyword.schema.json` | `771324f38d2745d0dd516a0aa109b11582d0146b3b681e4003dd32eae26a14c7` |
| `examples/codex-workspace-profile.json` (Codex profile, unchanged) | `e68d1cf569519177243ba43eefb08e14a7e2022bcd2695bdd5ced0d1c836e903` |
| Caller-owned Claude profile, exact option set from the harness README | `cee13599a688f37a2284598f15b84a80876e2e7ef7f1f353ea405b98734b648d` |

The Claude profile hash depends on whitespace; its content is the JSON block in
the harness README, written verbatim.

## Environment

| Item | Value |
| --- | --- |
| Due | `due 0.1.0`, installed `~/.cargo/bin/due`, a symlink to the main checkout's `target/release/due` |
| Due executable SHA-256 | `e345866de1ddebb40e51b147982778a275d367ba8ccd2088e63e5ca4864763d2` |
| Due source | Main `3a5ad50`; binary built in that clean checkout 75 s after the commit, embeds rustc `b940084d7` (1.99.0). A rebuild of the same source in another directory was not byte-identical, so the source match is inferred, not reproduced. |
| Claude Code | `2.1.312 (Claude Code)`, PATH `~/.local/bin/claude` |
| Codex CLI | `codex-cli 0.163.0`, PATH `~/.local/bin/codex` |
| Host | macOS 27.0.1 (26A434), arm64 |
| Caller tooling | Python 3.13.0, `jsonschema` 4.26.0, Node.js 26.12.0 |
| Model / effort | Not requested; observed identity unknown |

Both providers used existing authentication and settings. No credential,
configuration or permission file was edited. The workspace was a fresh
disposable Git repository with one committed file; the artifact parent was a
separate mode-0700 directory outside it.

## Procedure

1. Offline harness checks at `437aea5`: 11 smoke tests, 7 offline-validator
   tests and the fixture `node --check` passed.
2. For each provider and case: one `--dry-run` plan with a fresh artifact path,
   then one `--live` turn with another fresh path. Six plans and six live turns
   in total, run sequentially by the caller. No retries.
3. Independent verification with a separate caller script, outside the harness:
   - saved `schema.original.json` and the request's schema file hash-match the
     `437aea5` schema;
   - saved request `native_args` equal the profile's `native_args`;
   - `result.json` validates against `schemas/run-result.schema.json` and every
     trace record against `schemas/trace.schema.json` from main `3a5ad50`;
   - trace `seq` values are contiguous, all records share the result's `run_id`,
     the last record is `run_finished`, `trace.incomplete` is false and
     `dropped_records` is 0;
   - every present `structured_output` validates against the original schema
     with `jsonschema` Draft 2020-12;
   - native diagnostics of each failure were read to check that they name the
     dialect or keyword.
4. The workspace had no tracked or untracked changes after all six turns.

## Results

All six plans exited 0 without launching a provider. Live outcomes:

| Case | Claude Code 2.1.312 | Codex CLI 0.163.0 |
| --- | --- | --- |
| `minimal` | Exit 0, `succeeded`; answer valid | Exit 0, `succeeded`; answer valid |
| `dialect` (explicit Draft 2020-12 `$schema`) | Exit 1, `failed`, kind `provider`; native diagnostic: `--json-schema` is not a valid JSON Schema, no schema with key or ref for the 2020-12 URI | Exit 0, `succeeded`; answer valid |
| `keyword` (`not` on `summary`) | Exit 0, `succeeded`; answer valid | Exit 1, `failed`, kind `configuration`; native API 400 `invalid_json_schema`: `schema.properties.summary.not` is not permitted when strict is true |

For all six live turns: snapshots and native arguments matched, result and trace
records validated with zero schema errors, sequences were contiguous with a
terminal `run_finished`, and no records were dropped. Trace lengths were 10
records for each success, 4 for the Claude rejection and 9 for the Codex
rejection. The harness's own `answer-validation.json` agreed with the
independent check in every case; `native_compatibility` stayed
`not_established_by_harness`.

Codex turns reported no activities. Each successful Claude turn reported one
completed `StructuredOutput` tool call, which is the native mechanism behind
`--json-schema`; no other tool activity was observed. Trace observations are
not a complete tool or permission audit.

## Comparison with 2026-10-13

The [historical trial](https://github.com/joshrotenberg/due/blob/437aea507f4f3ae3a24e4e0071558a2e2e4a845f/examples/native-schema-smoke/README.md#bounded-live-evidence-2026-10-13)
used Claude Code 2.1.296 and Codex CLI 0.162.0. The outcome pattern is the same
on Claude Code 2.1.312 and Codex CLI 0.163.0: both accept the minimal contract,
Claude rejects the explicit Draft 2020-12 declaration, and Codex rejects `not`
under strict structured output.

## Limitations

- One turn per cell on one machine. No frequency, model or effort claim.
- A valid answer shows the schema was accepted and the observed answer satisfies
  it. It does not show that the provider enforced `not` or any other keyword.
- The rejections are attributed by their native diagnostics, which name the
  dialect URI and the `not` keyword. They are not evidence about other keywords
  or dialect URIs.
- The intake layer (Due plan), native schema acceptance and independent answer
  validation are reported separately above. A plan success says nothing about
  native acceptance.
- Raw artifacts, prompts, results, traces, native argv and local paths remain
  private on the machine that ran them.
