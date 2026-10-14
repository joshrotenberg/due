# Caller-owned live fixture (not run by this implementation worker)

The offline acceptance suite does not invoke a real provider. The outer caller
can use `examples/workflow/live-pair.fnl` for **one revision and one review**, with
no replay and a total job cap of two. Both stages are the same reusable modules
used by the direct and review-cycle examples. This is effect verification, not
an A/B trial. Auth, profile choice, permissions and approval remain with the caller.

1. Create a fresh isolated workspace under `target/` (or an explicitly approved
   disposable checkout). A small reproducible seed is `add.py` containing
   `def add(a, b): return a - b`, and `test_add.py` using `unittest` to check
   `add(2, 3) == 5` and `add(-2, 5) == 3`. Run the failing tests and retain output.
   Snapshot the files before allowing mutation.
2. Copy `examples/workflow/live.example.json` to the trial's scratch directory.
   Replace module paths with absolute paths to `live-pair.fnl`, `reviewer.fnl`,
   `reviser.fnl`; select the **exact already-installed frozen Due path**, isolated
   workspace, provider, model and effort. The sample shows Codex read-only and
   workspace-write native flags. For unattended Codex, include caller-approved
   `-c` settings for `approval_policy="never"`, `agents.enabled=false`, disabled
   native multi-agent features, disabled command networking and constrained
   writable roots, as documented by `due skills show run`. For Claude choose appropriate native permission
   controls using that installed Due's documentation. Do not broaden permissions
   to make a failed trial pass. No configuration is installed or changed by this
   template. Model preference tags are caller-defined profile labels.
3. Save this input with the actual task, keeping the workspace grant `trial`:

   ```json
   {
     "workspace": "trial",
     "task": "Fix add.py to add two numbers. Run python3 -m unittest -v. Change only this fixture; do not delegate, commit, push, publish, or use the network.",
     "findings": ["add currently subtracts b; the two supplied tests fail."]
   }
   ```

4. Run `workflow describe /absolute/trial/config.json` and `workflow check` first.
   Inspect metadata and grants. These commands cannot launch either agent.
   Inspect the two selected profiles in the config; describe does not resolve
   providers, query authentication, or guarantee model availability.
5. When the outer caller authorizes the live effects, invoke:

   ```text
   /absolute/target/finocchio/debug/finocchio-spike workflow run /absolute/trial/config.json /absolute/trial/input.json /absolute/trial/fresh-artifacts
   ```

6. Independently inspect the diff and run the fixture tests. Check the revision's
   `changed`/summary against files, review verdict/findings against evidence, and
   receipts against requested profiles and native exits. Inspect traces for
   incompleteness. Unknown observed model stays unknown. If revision failed after
   partial mutation, preserve everything and decide an explicit follow-up; the
   workflow does not retry it. A `changes_requested` review is a successful native
   job and a non-approved domain result, not native failure.

The caller should record the source commit/dirty state, exact Due path/version
and hash where available, platform, requested profiles, receipt outcomes, trace
completeness and independently verified file/test effects. Sanitized source-trial
reporting belongs to that outer caller. No live execution, auth probe, issue,
publication, model comparison or installed-binary modification was done here.

For the full conditional cycle, change entry/modules to `review-cycle.fnl`, set
`max_jobs:3`, and use `max_revisions:1`; this allows review -> revision -> review.
For two instances, set entry `parallel` and configure module keys exactly
`parallel`, `cycle`, `reviewer`, and `reviser` for their corresponding files
(`cycle` maps to `review-cycle.fnl`). Use two distinct caller-approved workspace
grants, two input documents in an array, `max_jobs:6`, and `max_active:2`.
Do not run mutation trials against one shared checkout or count the synthetic
fixture's files as real provider effects.
