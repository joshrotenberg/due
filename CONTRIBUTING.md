# Contributing

Due is open for development. The supported direction is one-shot execution,
resident JSON-RPC sessions, and a JavaScript harness that composes bounded
assignments above that protocol. Preserve existing `due run` behavior and versioned
contracts. Rust owns provider/process lifecycle and limits; caller scripts own
assignment composition and independent acceptance checks. Do not move a policy
loop into the RPC server.

Normal builds ship RPC and the JavaScript harness. The `rpc` and `harness`
features also permit explicit reduced builds; test the feature combinations
when changing gated code. Lua remains in the separate historical comparison.
The MCP experiment stays on `codex/due-mcp-spike` and is not part of this direction.

The earlier source-trial requirements are retired. Development does not require
Due-only delegation, reporting every successful call, or periodic A/B runs.
Follow the current task's authorized tools, provider choices and reporting scope.

Use [installation](docs/install.md) for local setup and
[dogfooding](docs/dogfooding.md) for issue reports. Keep changes small and include
a reproduction or explanation of the behavior they protect.
Use [troubleshooting](docs/troubleshooting.md) for layer-specific recovery and
repairing an existing checkout build without updating the installation.

```sh
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
cargo +1.90 check --locked
cargo build --release --locked
```

Tests use fake providers and do not need model credentials. Real provider runs
consume the caller's account resources and can mutate files; use them only for an
authorized, bounded task in a disposable workspace and retain the result/trace.
Each `due run` invocation owns one foreground turn; each RPC or script process
owns its resident session. The caller owns orchestration and independent verification.

The optional [JavaScript session comparison](examples/session-harness/README.md)
uses one scoped API over CLI resume and resident RPC. Its offline tests use
fake Due peers and do not invoke models:

```sh
node --test examples/session-harness/*.test.mjs
```

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
them on current provider versions or replace an installed build automatically.

Keep Cargo.lock committed and preserve the exact wrapper Git revisions. Review
dependency updates separately. The crate has `publish = false`; source readiness
does not authorize a crates.io release or wrapper release.

Runtime help, README/reference docs, exported schemas and embedded skills should
agree. Changes to embedded guides or schemas require rebuilding the binary.
The plugin entry skill should remain short and route into installed guidance.
Keep plugin and crate versions aligned. Source reports also need the Git commit
because the version alone does not distinguish local builds.

CI covers macOS/Linux and checks Rust 1.90. Any untested platform or native CLI
behavior should be identified in the report rather than inferred from a green
fake-provider test.

The [historical Lua/JavaScript comparison](examples/rpc-languages/README.md) has
its own lockfile and fixtures. It shares the production Rust/JavaScript runtime;
validate its nested manifest separately when changing that boundary. New product
examples and guidance should target `due script`, not the comparison runner.

Help uses progressive disclosure: `-h` is the concise command/option reference;
`--help` adds workflow examples and contracts. Keep command help in `src/help.rs`
and its option-specific details aligned with embedded guides. `src/skills.rs`
provides the installed catalog; keep its names/descriptions consistent with each
guide's frontmatter. Feature-gate guides and help routes with their commands.
The plugin entry skill explains purpose/scope and routes into installed discovery;
mode-specific procedures belong in the binary. Check examples offline with fake
providers, and rebuild after any embedded text or plugin package change.
