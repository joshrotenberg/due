# Source-trial backlog readiness

Snapshot: 2026-10-13. Main and installed Due remain frozen. This branch adds
only docs (including an embedded guide), optional caller examples, their tests,
an explicit native profile override, and a prepared local CI patch. No Rust runtime,
Cargo dependency, wrapper pin, plugin payload or release changes are included.
The Codex implementation worker ran only offline checks; the outer caller ran
separate authorized live probes and owns the evidence and publication.

## Freeze-compatible work

- [#5](https://github.com/joshrotenberg/due/issues/5):
  [optional offline answer validator](../examples/offline-validation/README.md),
  with distinct execution/validation/verdict outcomes. Seven tests check null
  versus missing, malformed input, valid approve/request_changes answers, failed
  execution with a valid answer, reference policy and unchanged original bytes.
  This evaluates existing Python libraries before adding any Due command.
- [#7](https://github.com/joshrotenberg/due/issues/7):
  [optional native-schema probe](../examples/native-schema-smoke/README.md),
  explicitly live or plan-only, using the existing Node caller. Offline tests
  use fake Due, never a provider. The caller separately tested both providers
  on three unchanged schemas: four successes and two preserved native rejections.
  The [live table](../examples/native-schema-smoke/README.md#bounded-live-evidence-2026-10-13)
  is schema/version/host evidence, not proof of native constraint enforcement.
- [#8](https://github.com/joshrotenberg/due/issues/8):
  [versioned delegation-control audit](native-control-audit.md). The profile
  now also disables v2, which otherwise overrides `agents.enabled=false` in
  published Codex 0.162.0 source. Tool-invocation enforcement, native-child
  lifecycle and usage attribution remain separate, unverified questions.
- [#6](https://github.com/joshrotenberg/due/issues/6):
  [correlated trace evidence](native-trace-evidence.md). Script controls emit
  failures; an inline Python denial was absent from native stdout before Due
  parsing. No speculative normalization fix is warranted during the freeze.

The default Rust suite, Clippy with warnings denied, formatting, Node caller
checks and Python caller tests were run locally. These checks are distinct from
worker prose, native acceptance, verified workspace effects and hosted CI.
The schema harness gained a stricter plan gate after the initial live snapshot;
all six retained real plans were independently checked against that final gate.
Subsequent malformed-plan handling is tested with a fake native path, not by
spending another model call or retrying a rejected schema.

## Integration and external gates

[#19](https://github.com/joshrotenberg/due/issues/19) and
[#26](https://github.com/joshrotenberg/due/issues/26) have draft implementations in
[PR #27](https://github.com/joshrotenberg/due/pull/27) and
[PR #28](https://github.com/joshrotenberg/due/pull/28). This branch builds on #28;
none of the stack is merged to main or installed. Language/runtime integration
and removing comparison-only implementations remain an unfreeze product decision.

Earlier stack jobs failed before startup with a GitHub account payment/spending
restriction. During this batch newer jobs started successfully: #35's Linux,
macOS and minimum-Rust-version checks passed, and #32's Node matrix passed while
its Rust checks were still running at observation. Use the latest checks for the
reviewed commit; the earlier failure is historical, not proof of a current gate.
Local macOS checks alone do not replace hosted platform evidence.

Provider-free Python CI checks were prepared locally, but GitHub rejected the
initial branch push because the OAuth credential lacked `workflow` scope. The
workflow change is preserved as an unpublished patch; this branch retains the
existing CI configuration. Integrate it with an appropriately authorized
credential before expecting hosted Python coverage. Its explicit dependency
installation uses the package index; the tests themselves use local fixtures
without provider/network calls. The optional requirements file pins caller
validation libraries, not Due runtime dependencies. Verify platform checks on
reviewed commits before release acceptance.

## Post-freeze features and ongoing evidence

[#1](https://github.com/joshrotenberg/due/issues/1) model/effort metadata and
[#2](https://github.com/joshrotenberg/due/issues/2) executable build identity stay
deferred. Save requested settings and binary hashes externally; omitted or
unreported values remain unknown. Native child attribution from #8 and any new
validation command from #5 also require later design/integration decisions.

A/B and dogfooding reports are evidence records; their open state does not mean
each is an unresolved Due defect. Continue sanitized findings on matching issues
as real work resumes. This page establishes no background monitoring, automatic
retry, new execution policy, model ranking or complete permission audit.
