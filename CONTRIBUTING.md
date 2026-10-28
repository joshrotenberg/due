# Contributing during the feature freeze

This is a private source-install trial. Limit changes to defects, documentation,
packaging, and tests that protect existing behavior. The explicitly scoped `due init` command
is the packaging exception: it installs the existing delegate plugin without
starting agent work. Other new commands, configuration layers, automatic retries,
schedulers and MCP servers remain outside the freeze.

Use [installation](docs/install.md) for local setup and
[dogfooding](docs/dogfooding.md) for issue reports. Keep changes small and include
a reproduction or explanation of the behavior they protect.
Use [troubleshooting](docs/troubleshooting.md) for layer-specific recovery and
repairing an existing frozen checkout build without updating the installation.

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo +1.90 check --locked
cargo build --release --locked
```

Tests use fake providers and do not need model credentials. Real provider runs
consume the caller's account resources and can mutate files; use them only for an
authorized, bounded task in a disposable workspace and retain the result/trace.
One Due invocation owns one foreground run. The caller owns orchestration and
independent verification.

The optional [Node caller example](examples/node-caller/README.md) has separate
offline lifecycle and answer-validation tests. Run them with Node 22 or later:

```sh
node --test examples/node-caller/caller.test.mjs
```

These tests launch a fake Due executable and make no provider calls. Node is not
a dependency of the Due binary or its plugin.

The optional [offline validator](examples/offline-validation/README.md) and
[native schema smoke](examples/native-schema-smoke/README.md) have provider-free
Python suites. In a caller-chosen environment with the pinned optional
[requirements](examples/offline-validation/requirements.txt) already available:

```sh
python3 -B -m unittest discover -s examples/offline-validation -p 'test_*.py' -v
python3 -B -m unittest discover -s examples/native-schema-smoke -p 'test_*.py' -v
node --check examples/native-schema-smoke/fake-due.fixture.mjs
```

The smoke suite uses the existing Node launcher and a fake Due executable; it
does not run native models. CI installs the optional Python dependencies in its
own job, retaining the default Rust/MSRV and Node matrices. These examples add
no runtime dependency, command, feature or workflow engine.

The [native control audit](docs/native-control-audit.md) and
[trace evidence](docs/native-trace-evidence.md) retain historical, versioned
caller observations. Extracting them onto this stable branch does not revalidate
them on current provider versions or authorize updating an installed frozen build.

Keep Cargo.lock committed and preserve the exact wrapper Git revisions. Review
dependency updates separately. The crate has `publish = false`; source readiness
does not authorize a crates.io release or wrapper release.

Runtime help, README/reference docs, exported schemas and embedded skills should
agree. Changes to embedded guides or schemas require rebuilding the binary.
The plugin entry skill should remain short and route into installed guidance.
Keep plugin and crate versions aligned; source trial reports also need the Git
commit because the version alone does not distinguish local builds.

CI covers macOS/Linux and checks Rust 1.90. Any untested platform or native CLI
behavior should be identified in the report rather than inferred from a green
fake-provider test.
