# Contributing during the feature freeze

This is a private source-install trial. Limit changes to defects, documentation,
packaging, and tests that protect existing behavior. New commands, configuration
layers, automatic retries, schedulers and MCP servers are outside the freeze.

Use [installation](docs/install.md) for local setup and
[dogfooding](docs/dogfooding.md) for issue reports. Keep changes small and include
a reproduction or explanation of the behavior they protect.

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
