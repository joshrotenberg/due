# JavaScript composition example

Build with `cargo build --features workflow-quickjs-spike --locked`.
Copy this directory into protected caller-owned storage. Supply two explicitly
authorized, isolated Git workspaces named workspace-a and workspace-b (or edit
those cwd values). Keep profile/schema/artifact paths outside worker write grants.
Review the complete native settings for your installed Codex before a real run.
Replace input.json with a concrete bounded task and independently verify effects.
The profile is persistent and avoids fresh-only --sandbox syntax.

Check with `due workflow check workflow.json --input input.json`; this initializes
no modules and runs no agents. Run only under explicit task authorization using
`due workflow run workflow.json --input input.json --artifacts /new/private/path`.
Use the spike executable explicitly; do not replace an installed frozen binary.
The pipeline function is reused twice, with one custom review stage. Rust owns
shared capacity, receipts, cancellation, and joins. Built-in JS array mapping,
flattening, and sorting handle synthesis input. This source example does not
claim a real provider run was performed by the spike acceptance tests.
