# Native delegation controls: bounded source audit

Snapshot: 2026-10-13, installed Due 0.1.0, PATH Codex CLI 0.162.0,
Claude Code 2.1.296, macOS 27 arm64. In that historical investigation, main,
installed Due, wrappers and Rust runtime were unchanged. This stable extraction
retains that versioned evidence; it is not a new live test or a current effective
tool inventory. This is guidance and profile evidence, not
native child lifecycle or accounting acceptance.

## Codex: configuration is not a feature inventory

The [official configuration reference](https://learn.chatgpt.com/docs/config-file/config-reference)
describes both `agents.enabled` and `features.multi_agent`. Running
`codex features list -c agents.enabled=false` left `multi_agent` true;
`codex features list -c features.multi_agent=false` made that feature false.
Those observations are feature values, not an inventory of model-visible tools.
They do **not** demonstrate that `agents.enabled=false` is ineffective.

The public [0.162.0 configuration source](https://github.com/openai/codex/blob/rust-v0.162.0/codex-rs/core/src/config/mod.rs#L1604)
resolves an enabled `features.multi_agent_v2` first, then resolves
`agents.enabled=false` to the disabled version. If neither overrides, model
metadata can select the version before the `multi_agent` feature fallback.
The [versioned schema](https://github.com/openai/codex/blob/rust-v0.162.0/codex-rs/core/config.schema.json#L60)
explicitly documents the v2 precedence. The
[tool registration gate](https://github.com/openai/codex/blob/rust-v0.162.0/codex-rs/core/src/tools/spec_plan.rs#L719)
returns false for the disabled version, and
[collaboration registration](https://github.com/openai/codex/blob/rust-v0.162.0/codex-rs/core/src/tools/spec_plan.rs#L1374)
uses that gate.

For that source version, request both overrides:

```text
-c agents.enabled=false -c features.multi_agent_v2=false
```

The [workspace profile](../examples/codex-workspace-profile.json) now includes
both, alongside the original sandbox/approval/network/root controls. Native
arguments after `--` replace the **entire** profile list; edit a copied profile
or provide every required argument rather than append a lone override.
`features.multi_agent=false` alone is not a substitute: the source resolves
model metadata and v2 overrides before that feature fallback.

This statically verifies the intended registration path in the published tag.
A matching version string does not prove an installed executable is byte-for-byte
that source. No native child was launched, and no runtime tool manifest or
invocation-enforcement test is claimed. Per-run native policy, inherited managed
requirements, connected tools, and ordinary subprocess permissions still apply.
Disabling built-in delegation is not a general subprocess security boundary.

## Claude

Use the installed CLI's `--disallowedTools Agent` together with the task's narrow
available-tool and permission configuration. In this batch read-only reviews
used Read/Glob/Grep and no shell, edit or Agent tools. The complete native tool
catalog was not independently captured; successful reviews and no observed
children do not establish universal enforcement or descendant accounting.

## Remaining #8 work

Native child identifiers, parent links, failure/cancellation events and usage
semantics remain unverified. No real native-child fixture collection was performed:
this source trial requires all delegated model work to use Due, and workers must
not recursively delegate. Capture sanitized existing event fixtures when available;
leave missing relationships and accounting explicit rather than inventing a tree.
Keep any future deliberate native-child experiment separately authorized.

See [#8](https://github.com/joshrotenberg/due/issues/8) and
[the native comparison](native-comparison.md). This audit adds no queue,
scheduler, server, child control API, model defaults or accounting guarantees.
