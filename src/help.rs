//! Installed help text, split into orientation and detailed working examples.

pub const SHORT: &str = "Use --help for examples and detailed guidance.";

pub fn overview() -> String {
    let mut text = String::from(
        "GETTING STARTED\n  Delegate one bounded task to an installed, authenticated provider:\n    due run --provider claude -p 'Explain this repository without changing files.'\n    due run --provider codex -p 'Review the current diff.' --output json\n\nCHOOSE A WORKFLOW\n  run       One foreground turn; --resume ID continues saved native context.\n",
    );
    #[cfg(feature = "harness")]
    text.push_str("  script    A JavaScript caller supervises several turns, items and checks.\n");
    #[cfg(feature = "rpc")]
    text.push_str("  rpc       Build a client in any language over resident JSON-RPC stdio.\n");
    text.push_str(
        "\nPROFILES\n  Reuse JSON defaults, then supply the assignment separately:\n    printf '%s\\n' '{\"schema_version\":1,\"provider\":\"claude\"}' > worker.json\n    due run --request worker.json -p 'Review the parser.' --dry-run --output json\n    due skills show profiles\n\nDISCOVER OFFLINE\n    due skills list --output json        List guides in this installed build\n    due skills show overview             Choose a workflow and get started\n    due skills show use-cases            Decide whether Due fits; pick a pattern\n    due skills show run                  Prepare and verify one-shot assignments\n    due skills show contracts            Interpret results and structured answers\n    due skills show observe              Monitor and summarize progress\n",
    );
    #[cfg(feature = "rpc")]
    text.push_str("    due skills show rpc                  Implement a raw RPC client\n");
    #[cfg(feature = "harness")]
    text.push_str(
        "    due skills show supervise            Write a supervised JavaScript workflow\n",
    );
    text.push_str(
        "    due schema request                  Export the request JSON Schema\n    due build-info --output json        Report this binary's revision and features\n    due run --help                      Examples and options for a command\n\nSETUP\n    due init --dry-run                   Preview local plugin setup\n    due init                            Register the bundled plugin in both hosts\n  Due must be on PATH. Install and authenticate Claude Code or Codex separately.\n  Guides, schemas and help work without a provider or source checkout.\n\nRESULTS\n  Due owns execution and process cleanup; the caller owns the assignment, native\n  permission policy, independent acceptance checks and authorized follow-ups.\n  A successful worker turn does not establish that its changes were accepted.\n  Run answers/results use stdout; progress and diagnostics use stderr.\n  Read a command's --help for its output, timeout and exit-code contract.",
    );
    text
}

pub const RUN: &str = r#"GETTING STARTED
  One invocation executes one foreground provider turn:
    due run --provider claude -p 'Explain this repository without changing files.'
    printf '%s\n' 'Review the current diff.' | due run --provider codex --stdin --output json

PROFILES AND RESUME
  Save reusable defaults in worker.json:
    {"schema_version":1,"provider":"claude","ephemeral":false}
  Then supply a task, preview it, or continue a persisted native session:
    due run --request worker.json -p 'Review the parser.' --dry-run --output json
    due run --request worker.json -p 'Review the parser.' --output json > result.json
    due run --provider claude --resume NATIVE_SESSION_ID -p 'Explain the main finding.'
  Substitute the returned result.session_id for NATIVE_SESSION_ID. Resume needs
  the same provider and its saved state. Ephemeral sessions cannot be resumed.

  Explicit execution flags override profile values; omitted flags inherit.
  --no-ephemeral overrides profile true. Native options after -- replace the
  entire native_args list; --clear-native-args clears it. A bare -- inherits it.
  cwd/schema paths resolve from the caller's directory, not the profile file.
  --request - reads JSON from stdin and cannot be combined with --stdin.
  See `due skills show profiles` for examples and resident-session differences.

OUTPUT AND STRUCTURED ANSWERS
    due schema result > due-result.schema.json
    due run --provider codex -p 'Summarize the module.' --schema answer.schema.json --output json
  --output json selects Due's result envelope. --schema FILE requests the model's
  structured answer using a caller-supplied, provider-supported JSON Schema.
  They serve different purposes. Due does not independently validate that answer.
  Stdout is the answer or one JSON result; stderr is diagnostics/progress.
  Inspect result.status, result.error and structured_output_present, then verify
  task artifacts independently. Execution success is separate from acceptance.

PROGRESS AND DEADLINES
    due run --request worker.json -p 'Review the parser.' --output json --trace --trace-file new-run.jsonl
    due trace summarize new-run.jsonl --output json
  --trace is readable progress; --trace=json emits JSONL on stderr.
  --trace-file alone saves JSONL without printing progress. Use a new file.
  --trace-detail opts into content-bearing activity labels and messages.
  There is no default execution deadline. Choose --timeout 10m when appropriate;
  activity does not reset the deadline, and bounded cleanup can extend beyond it.
  Ctrl-C requests cancellation and cleanup. Cancellation does not roll back edits.

PLANNING AND NATIVE OPTIONS
    due run --provider claude -p 'Review only.' --dry-run --output json -- --max-turns 3
  --dry-run validates/prepares the merged invocation without starting a provider.
  It does not prove authentication, model support or effective native permissions.
  Plans omit prompt text, but may include native option values and schema content.
  Provider settings and permissions apply. Due does not retry automatically.

EXIT CODES
  0 success/planned; 1 execution failure; 2 invalid input; 124 timeout;
  127 missing executable; 130 cancellation. After argument parsing, JSON mode
  returns an envelope on failure too. CLI syntax errors use stderr and exit 2.

LEARN MORE
  due skills show run         Assignment scope, execution and verification
  due skills show profiles    Configuration, precedence and paths
  due skills show contracts   Result fields, schemas and typed failures
  due skills show observe     Progress, trace delivery and cancellation"#;

#[cfg(feature = "rpc")]
pub const RPC: &str = r#"GETTING STARTED
  Launch due rpc as a child process. Write one compact UTF-8 JSON object per
  newline to stdin; continuously read responses and notifications from stdout.
  Stderr carries diagnostics. No initialization handshake is required.

  Send session.open and check result.protocol_version == 1:
    {"jsonrpc":"2.0","id":1,"method":"session.open","params":{"provider":"claude","stream":true}}
  Substitute its result.session_id for SESSION, then start a turn:
    {"jsonrpc":"2.0","id":2,"method":"turn.start","params":{"session_id":"SESSION","prompt":"Explain the parser without editing files."}}
  Substitute result.turn_id for TURN and read until state is finished:
    {"jsonrpc":"2.0","id":3,"method":"turn.read","params":{"session_id":"SESSION","turn_id":"TURN","after":0,"wait_ms":10000}}
  Advance after to the returned cursor; use a unique request ID for each call.
  On completion inspect result.result.status, not just JSON-RPC success.

OPERATIONS
  session.open       Open one Claude/Codex session; optional JSON request profile
  turn.start         Start one turn and return its Due turn ID
  turn.read          Poll/long-poll turn events, pending input and terminal result
  session.read       Page session items across turns using after and limit
  turn.interrupt    Request interruption, then await the terminal result
  interaction.reply Send an explicit native permission/interaction response
  session.close     Await native cleanup and release the Due handle

STREAMING AND LIFETIME
  With stream:true, session.item notifications interleave with responses.
  A turn_finished item's data is the normalized turn result. Keep reading while
  waiting on controls, replying to permissions and closing. Replies may arrive
  out of order; correlate them by ID. Reconcile cursor gaps using session.read.
  Each process hosts one session and one active turn. Native session/thread IDs
  differ from Due handles. After a finished turn, reuse the handle only while
  the session remains usable. A profile timeout ends the native session:
  timed_out is followed by session_failed, and further turns are rejected.
  Before exiting, read any needed items and send:
    {"jsonrpc":"2.0","id":4,"method":"session.close","params":{"session_id":"SESSION"}}
  Continue through stdout EOF and await child exit. Closing stdin also requests
  shutdown. An unexpected transport exit is not evidence of task acceptance.

ERRORS AND PARAMETERS
  Named parameters only; batches are unsupported. A valid envelope without id
  is a notification and gets no response. Parse/envelope errors -32700/-32600
  use id:null even if an invalid object supplied an ID. Other errors retain ID.
  Use requests when acknowledgement matters.
  Common errors: -32602 invalid params; -32000 session operation failure;
  -32001 too many in-flight requests. Failed native turns are result data.
  session.read limit is 1..256 (default 64); turn.read wait_ms is 0..10000
  (default 0). Retained items/events and output queues are bounded.
  Authentication and native permissions are inherited; no automatic approvals.

LEARN MORE
  due skills show rpc         Full wire contract, parameters and permission shapes
  due skills show profiles    Reuse JSON worker configuration
  due schema result           Normalized turn result fields
  For the embedded JavaScript client, discover available commands with due -h."#;

#[cfg(feature = "harness")]
pub const SCRIPT: &str = r#"GETTING STARTED
  Save this as echo.js for a provider-free runtime check:
    Due.run(async scope => {
      await scope.sleep(1);
      return {message: Due.args.message};
    });
  Run it with a new evidence directory whose parent already exists:
    due script echo.js --args '{"message":"hello"}' --evidence ./echo-evidence

SUPERVISE A WORKER
  Save this as review.js:
    Due.run(async scope => {
      const worker = await scope.open(Due.args.profile, {
        onItem(item) { Due.log(JSON.stringify(item)); }
      });
      const turn = await worker.turn(Due.args.task);
      if (turn.status !== 'succeeded') throw Error('Worker execution failed');
      return {text: turn.text};
    });
  Run an authorized read-only assignment:
    due script review.js --evidence ./review-evidence --args '{"profile":{"provider":"claude"},"task":"Explain this repository without changing files."}'
  One scope owns one resident worker. Start another turn on that worker to
  continue. Unexpected permission interactions fail unless a caller supplies
  an explicit response. See the supervise guide for narrow approval examples,
  async permission checks, interrupts and an independent verify/revise loop.

INPUTS AND PATHS
  --args JSON and --args-file FILE|- are alternatives; the default input is {}.
  Scripts and JSON input are UTF-8, at most 1 MiB each. Due.args is parsed JSON.
  --cwd selects the RPC process and default verifier directory. Script, argument,
  evidence and --cwd paths resolve from the invocation directory. Profile request
  paths resolve from the RPC directory, not the profile file's directory.
  The evidence directory must be new, with an existing parent.

OUTPUT AND DEADLINES
  Stdout is one version-1 script result after owned work and cleanup settle.
  Due.log(text) writes stderr. The workflow's JSON return value becomes result;
  failure returns result:null and error text. A successful script does not invent
  an acceptance verdict: inspect turn/check outcomes and encode it explicitly.
  --timeout defaults to 5m for workflow execution. Cleanup is additional and
  bounded. worker.turn/worker.wait also default to a separate five-minute wait;
  pass a deliberate timeout in milliseconds when a turn needs longer.
  A wait timeout fails the whole scope even when caught. A profile turn timeout
  also fails the native session and workflow; the handle accepts no more turns.
  Exit 0: script success; 2: invalid input; 1: setup/workflow/cancellation/cleanup
  failure. CLI syntax errors use stderr and exit 2 before the JSON envelope.

RUNTIME AND EVIDENCE
  JavaScript is embedded; no Node, npm, imports or browser APIs are provided.
  scope.exec runs a caller-selected command without an implicit shell. Scripts
  and host commands are trusted caller code; provider sandboxes do not constrain
  them. Await managed work and pass cancellation signals into checks.
  Evidence includes original RPC stdout, process/command receipts and workflow
  settlement. Streams and command output can contain private task content.

LEARN MORE
  due skills show supervise   Script API, permissions, verification and limits
  due skills show profiles    Shared JSON configuration and paths
  due skills show rpc         Underlying wire protocol for independent clients
  due schema script-result    Exact script output envelope"#;

pub const SKILLS: &str = r#"GETTING STARTED
  due skills                          List installed guides (same as list)
  due skills list --output json       Discover names and descriptions as JSON
  due skills show overview             Choose a workflow and find examples
  due skills show profiles             Configure reusable JSON worker defaults
  due skills show contracts --output json

GUIDES
  overview    Getting started, mode selection, setup and troubleshooting
  use-cases   Task fit and patterns for delegation, review, extraction and embedding
  run         Scope, execute and verify one foreground assignment
  profiles    JSON configuration, overrides, native options and path resolution
  contracts   Structured answers and one-shot result/error semantics
  observe     Progress traces, cancellation and offline summaries
  Other guides depend on build features; use the installed catalog to discover
  resident RPC and JavaScript supervision guidance when available.

OUTPUT
  list --output json returns schema_version, cli_version and skills metadata.
  show NAME --output json returns the complete document in skill.content.
  Text mode prints the catalog or complete SKILL.md. Discovery is offline and
  read-only; it needs no provider, source checkout or network.
  Unknown names exit 2; JSON mode returns a versioned error document.
  Help and guides describe this binary's supported commands and contracts."#;

pub const INIT: &str = r#"GETTING STARTED
  due init --dry-run                    Preview the default setup for both hosts
  due init --host claude --dry-run --output json
  due init --host claude                Install/update only Claude Code's plugin
  due init --host codex                 Install/update only Codex's plugin

WHAT SETUP DOES
  The binary embeds a small delegate skill and portable/native manifests.
  Apply refreshes the managed package and registers it through each selected
  host CLI. Existing matching marketplace registrations are retained.
  Default package root: $XDG_DATA_HOME/due, or ~/.local/share/due when unset.
  --install-dir DIR overrides the package root, not the host's user profile.
  Keep that directory in place and due on the host's PATH. Setup does not install
  Due or provider binaries, manage credentials, or start a model turn.

PREVIEW AND RECOVERY
  --dry-run reads local metadata and resolves selected host executables; it
  writes nothing and starts no host process. Select only installed hosts.
  Apply can retain earlier successful steps after a later step fails. Inspect
  failed_step/error and existing effects, then deliberately rerun the same setup.
  Nonempty unmanaged directories, conflicting marketplace roots, unexpected
  files and symlinked package entries are rejected rather than overwritten.
  Rebuild/install Due before refreshing the plugin. Reload plugins or start a
  new host session afterward; native registration alone does not verify discovery.

OUTPUT AND EXIT CODES
  Text is a setup summary; --output json returns one versioned setup report.
  0: planned/installed; 1: not ready or partial install; 130: cancelled;
  2: argument error. Setup starts no model work.

LEARN MORE
  due skills show overview             Setup, discovery and troubleshooting
  due -h                               Check commands in this installed build"#;

pub const SCHEMA: &str = r#"EXAMPLES
  due schema request > worker.schema.json
  due schema result > result.schema.json
  due schema trace > trace.schema.json
  due schema trace-summary > summary.schema.json

CONTRACTS
  request        Version-1 JSON task/profile accepted by due run and resident opens
  result         Normalized Due run/turn result, including typed failures
  trace          A due run progress JSONL record
  trace-summary  Offline trace summary or parsing-error envelope
  Additional contracts depend on build features; see the available values above.

OUTPUT
  Stdout contains the selected JSON Schema, offline and without a provider.
  request describes input shape; --dry-run checks merged execution semantics.
  --schema on due run is different: it takes the caller's desired answer schema,
  not one of these Due envelope schemas. Due does not independently validate
  the generated answer against that caller schema.

LEARN MORE
  due skills show profiles
  due skills show contracts
  due skills show observe"#;

pub const BUILD_INFO: &str = r#"EXAMPLES
  due build-info
  due build-info --output json > due-build.json

WHAT IT REPORTS
  The package version, the source revision and working tree state recorded
  when this binary was built, and the enabled Due feature flags (for example
  harness and rpc). Values are fixed at compile time. Nothing is read from a
  checkout, the environment or the network when the command runs, so the
  current directory never changes the answer. due --version is unchanged.

UNKNOWN SOURCE METADATA
  Builds from a source archive, without a git executable, or from a directory
  that is not its own Git work tree root succeed and report revision null
  (text: unknown) with tree "unknown". source.status explains why:
  recorded, no_git_metadata, git_unavailable, not_repository_root, git_error.
  tree is clean, dirty or unknown. Dirty includes untracked, non-ignored files.

LIMITS
  This is a descriptive snapshot, not a reproducible-build attestation or
  signature. It contains no build paths, timestamps or environment values.
  A binary built with uncommitted changes reports the commit it started from
  and tree "dirty"; it cannot say what those changes were. Source state is
  rechecked on every Cargo build of the package, which therefore recompiles
  Due's own targets each time (dependencies stay cached).

OUTPUT AND EXIT CODES
  Text is a short summary. JSON is one object with schema_version 1, package
  (name, version), source (status, revision, tree) and features (sorted names).
  0: written; 1: stdout write failure; 2: argument error.

LEARN MORE
  due --version                       Package version only
  due -h                              Commands in this installed build"#;

pub const TRACE: &str = r#"EXAMPLES
  due trace summarize run.jsonl
  due trace summarize run.jsonl --output json > summary.json
  due schema trace-summary > summary.schema.json

READ A SAVED RUN
  Summaries are offline and read-only. They group runs and report observed
  outcomes, activity counts, sequence gaps and missing/conflicting boundaries.
  Missing or conflicting terminal records leave execution status unknown.
  A complete trace is not a complete native command or permission audit.

OUTPUT AND EXIT CODES
  Text is a readable summary. JSON is a versioned summary/error envelope.
  Exit 0 means the file was read and summarized, not that its recorded run
  succeeded. I/O failures exit 1; invalid/oversized records and argument errors
  exit 2.
  Preserve the original run's exit status/result when summarizing afterward.

LEARN MORE
  due skills show observe
  due schema trace
  due run --help"#;
