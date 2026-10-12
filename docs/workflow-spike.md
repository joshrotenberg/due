# Experimental Rhai workflows

This is the bounded `spike/rhai-workflows` experiment, not a production interface.
Default builds have no workflow command or workflow skill. Wrapper Git pins are
unchanged. Build locally without replacing an installed Due:

```sh
cargo build --locked --features workflow-spike --target-dir target/workflow-spike
./target/workflow-spike/debug/due skills show workflow
./target/workflow-spike/debug/due workflow check examples/workflow/workflow.json --input input.json --output json
```

On hosts where Homebrew rustc precedes Rustup shims, `rustup run 1.90 cargo`
can still find the newer compiler. Ensure the 1.90 toolchain bin directory is
first on PATH (or explicitly set RUSTC/RUSTDOC) when checking MSRV. The toolchain
selection must apply to cargo-fmt and cargo-clippy as well.

Use a separate target directory: a later default build otherwise replaces the
feature-enabled binary at the same path.

The check needs declared profile binaries (or the named provider on PATH) but
never launches them. The example's input path is relative to its manifest.
Only after authorizing actual provider work:

```sh
./target/workflow-spike/debug/due workflow run examples/workflow/workflow.json --input input.json --artifacts ./new-workflow-receipt --output json
```

The complete API, schema subset, receipt format, failure semantics and fixed
limits ship in [the embedded workflow skill](../embedded-skills/workflow/SKILL.md).
The [example](../examples/workflow/review.rhai) prepares a draft, runs two reviews,
synthesizes them and conditionally follows up, for at most five ordinary Due
runs. Change its second import from patterns/v1 to local/review to substitute
[the declared local composition](../examples/workflow/local-review.rhai) without
changing Rust. Profiles remain trusted caller input; this is no OS sandbox and
native permissions remain in force. Cwd, executables and opaque native_args are
not an isolation or reproducibility guarantee.

Implementation boundaries:

- A blocking Rhai VM calls a synchronous join bridge into Tokio. A whole batch is
  validated/reserved first, then runs in concurrency-sized chunks. No detached
  model work or async script handles. Each child is this exact current_exe(),
  ordinary `run --request - --output json --trace-file … --binary …`, with JSON
  request/prompt on stdin. No in-process provider/tracing refactor.
- Check uses optimization None and compiles all declared sources. Runtime modules
  are extracted with Rhai's public clone_functions_only API; their top-level code
  never runs. Imports inside functions use only an explicit static resolver.
  Check is intentionally not a proof of runtime imports, effects or branches.
- jsonschema 0.56.0 has default features disabled; a recursive keyword allowlist
  rejects references and unsupported schema features before compilation. Input,
  child structured answers and final output use this shared experiment-only gate.
- Snapshots retain exact source bytes and resolved JSON before evaluation. Child
  schemas use saved snapshots. Binaries/workspaces are canonical references, not
  copies. There are no hashes or claims of reproducibility. Artifacts use new
  0700 directories and exclusive 0600 files. A provider with the same Unix user
  can still reach them if native permissions allow it.
- Child stdout and stderr capture/draining are capped at 1 MiB each. A hidden, feature-gated child argument caps each child trace at 1 MiB using
  the existing trace-incomplete reporting. The argument is consumed by Due and
  does not reach providers or nested tool processes. No child trace merging.
- Signals stop admission, signal live Due children, await their wrapper cleanup
  and drain output before returning exit 130. VM errors occur between joined
  calls, so no previous child is abandoned. Unresponsive cleanup can wait
  indefinitely; escaped native descendants are not accounted for by this host.

Offline evidence commands (all model fixtures are synthetic native executables):

```sh
cargo test --locked --offline --features workflow-spike --target-dir target/workflow-spike
cargo test --locked --offline --no-default-features --target-dir target/default
```

The [Node caller](../examples/node-caller/README.md) remains the simpler baseline
for callers already using JavaScript. This slice adds installed guidance, reusable
Rhai components, schema gates and joined batch lifecycle; it also adds interpreter
and validator build weight and a deliberately restricted data/module surface.
See [the caller assessment](workflow-spike-assessment.md) for independent probes,
a real cross-provider smoke run, measured costs and the proposed contract-policy
boundary. No authoring-quality A/B or production-readiness claim is made.
