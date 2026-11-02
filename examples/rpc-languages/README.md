# Embedded RPC client comparison

This private experiment embeds Lua 5.4 (`mlua =0.12.2`) and QuickJS
(`rquickjs =0.14.0`) in the same small Rust runner. It connects to the existing
`due rpc` process. It adds no command, dependency or behavior to default Due
builds, and does not select the eventual embedding language.

```text
Lua workflow -> Lua RPC client --\
                                 Rust raw-frame bridge -> due rpc -> Claude/Codex
JS workflow  -> JS RPC client ---/

Any external client -------------------------------> due rpc
```

Rust owns process startup, byte capture, bounded I/O and child settlement.
With `--async`, it also owns bounded verifier processes and their cleanup.
The language libraries own IDs, response routing, item delivery, permissions,
turn waiting and session convenience methods. The existing Rust RPC server
continues to own provider-specific session behavior. Neither embedded client
is required to use the raw protocol from another language or process.

## Run the same conversation

Build Due with RPC support, then build the separate runner:

```sh
cargo build --locked --release --features rpc
cargo build --locked --release --manifest-path examples/rpc-languages/Cargo.toml
```

Run either workflow, giving the evidence directory a fresh name each time:

```sh
examples/rpc-languages/target/release/due-rpc-languages \
  js examples/rpc-languages/workflows/conversation.js \
  --due "$PWD/target/release/due" --cwd "$PWD" \
  --evidence /tmp/due-embedded-js-example \
  --args '{"profile":{"provider":"codex","cwd":"/path/to/project"}}'

examples/rpc-languages/target/release/due-rpc-languages \
  lua examples/rpc-languages/workflows/conversation.lua \
  --due "$PWD/target/release/due" --cwd "$PWD" \
  --evidence /tmp/due-embedded-lua-example \
  --args '{"profile":{"provider":"claude","cwd":"/path/to/project"}}'
```

Both workflows send a marker, recall it in a second turn, consume session items
and explicitly deny unexpected permission requests. They use native credentials
and settings, and real runs consume the caller's model allowance. Change the
provider independently of the language. JSON args accept `profile`, `marker`,
`first_prompt`, `second_prompt` and optional `turn_timeout_ms`; profile is passed to `session.open` with
streaming enabled. There is no TOML or new request configuration format.

The profile object may instead reference an existing Due JSON file, for example
`{"profile":{"request":"/caller-owned/worker.json","cwd":"/path/to/project"}}`.
Rust loads and validates it once at open; explicit fields override file defaults.
`open` returns the provider and `resolved_options`, so permission handlers can use
the resolved provider even when it was supplied only by the file. Both embedded
clients use the [same profile rules](../../docs/rpc.md#reusing-json-profiles) as raw
RPC and the Node client; they do not parse configuration files themselves.

## Client and bridge boundary

Both clients expose `open`, `start`, `wait`, `turn`, `interrupt`, `reply`,
`close`, and lower-level `request`, `await` and `poll` operations. Lua uses
`DueClient.new(options)` and colon methods; JS uses `new DueClient(options)`.
`request` registers a string ID before sending, and `await` pumps responses and
items until that ID resolves. `interrupt` and `reply` return request IDs; await
their acknowledgements, then await the terminal turn result. A failed model
turn is result data; an RPC error throws with its `code`, `message` and `data`.

The bridge has the same synchronous operations in each language:

| Operation | Behavior |
| --- | --- |
| `send(line)` | Send one raw JSON frame; host appends newline |
| `recv(timeout_ms)` | Raw frame string; null/nil on timeout; error on EOF |
| `now_ms()` | Monotonic elapsed milliseconds |
| `log(text)` | Diagnostic text on stderr |
| `args` | Original caller JSON string |

Without `--async`, the client uses an explicit cooperative event pump. Callbacks must finish
synchronously. They can enqueue requests, interrupts and permission replies,
but recursive blocking calls are rejected. Returning null/nil from
`onInteraction` defers the decision; the caller can keep polling and later call
`reply(item, response)`. There is no automatic permission approval. A decision
for a resolved or finished turn is rejected locally.

The Rust reader continues while guest code runs, but guest callbacks run only
when the client pumps. In this synchronous mode, JavaScript Promise use is rejected, including discarded
rejections. Lua callback yields are rejected. Detached caller-created Lua
coroutines are not supervised; this experiment is for trusted scripts.

Items that precede open/start responses are supported. Identity changes,
malformed frames, cursor gaps, push loss, omitted items and session failure
fail explicitly. Valid old cursor duplicates are ignored. These small clients
do not yet recover gaps using `session.read`; the raw RPC method remains
available to clients that implement replay. They retain bounded request,
interaction and terminal state rather than a whole conversation transcript.

## Async supervision

Pass `--async` to compare JavaScript Promises with managed Lua coroutines on
the same Rust command runner. The [JavaScript](workflows/supervision.js) and
[Lua](workflows/supervision.lua) examples implement the same bounded assignment:
ask the worker, run an independent caller-selected check, and send failed check
output back for a limited number of revisions in the same session.

```sh
examples/rpc-languages/target/release/due-rpc-languages \
  js examples/rpc-languages/workflows/supervision.js --async \
  --due "$PWD/target/release/due" --cwd /path/to/project \
  --evidence /tmp/due-supervision-js \
  --args '{"profile":{"request":"/path/to/worker.json","cwd":"/path/to/project"},"task":"Fix the parser regression and add a test.","verify":{"program":"cargo","args":["test","--locked"],"timeout_ms":120000},"max_revisions":2}'
```

Replace `js` and the workflow path with `lua` and `workflows/supervision.lua`
to run the Lua version. A small JavaScript workflow looks like this:

```js
Due.run(async scope => {
  const worker = await scope.open({request: '/path/to/worker.json'}, {
    onItem(item) { bridge.log(JSON.stringify(item)); },
    async onInteraction(item, signal) {
      await scope.sleep(100, signal); // Other items continue arriving.
      signal.throwIfAborted();
      return {decision: 'decline'}; // Codex approval shape; choose for your provider.
    },
  });
  const turn = await worker.turn('Investigate the parser regression.');
  if (turn.status !== 'succeeded') throw Error('Worker failed');
  const check = await scope.exec({program: 'cargo', args: ['test', '--locked']});
  if (check.error != null || check.status !== 'succeeded') throw Error('Check failed');
});
```

Lua uses `Due.run(function(scope) ... end)` and colon methods; its managed
coroutine suspends inside `scope:exec`, `scope:sleep` and worker methods. It does
not need an `await` keyword. JSON array arguments use `json.array({...})`.

| Async API | Contract |
| --- | --- |
| `scope.open(profile, callbacks)` | Open the scope's single resident session; return a worker with `opened` details |
| `worker.start`, `wait`, `turn`, `interrupt`, `reply`, `close` | Awaitable versions of the existing validated client operations |
| `scope.exec(spec, signal?)` | Run a caller-owned program, wait for settlement, return its result |
| `scope.sleep(ms, signal?)` | Yield while frames and other managed work continue |
| `scope.signal`, `scope.cancel(reason)` | Sticky scope cancellation; catching it does not make the workflow succeed |

`Due.run` owns one root workflow. It closes the session and joins managed
permission callbacks and commands before returning. Returning with an unfinished
turn or unjoined work fails. `onItem` remains synchronous; Promise returns or
Lua yields are errors. Lua's async mode disables direct coroutine scheduling and
the bare synchronous client to keep managed work within this scope. JavaScript
also checks unhandled rejections and unsettled Promises.

An interaction callback receives its own cancellation signal. Pass that signal
to its awaited commands and sleeps: interaction retirement, a terminal turn or
session closure cancels that work and suppresses a late decision. A distinct
callback failure remains a failure even after retirement. A reply already sent
must still consume its acknowledgement; only a known stale-reply error after
matching retirement is ignored. A failed native session cancels the root scope,
including verification between turns. An ordinary failed turn or nonzero command
exit is result data that the caller must check.

The command specification accepts only `program`, string-array `args`, optional
`cwd`, `timeout_ms` and `max_output_bytes`. Working directories resolve against
the runner's `--cwd`, not the native session's profile. Commands inherit the
runner environment, receive closed stdin and use no implicit shell. Defaults are
30 seconds and 64 KiB combined stdout/stderr; maxima are five minutes and 1 MiB.
At most eight command results may be pending. Output overflow or invalid UTF-8
is an explicit failure; invalid text is unavailable rather than silently replaced.
Results include `status`, `exit_code`, `signal`, `stdout`, `stderr` and `error`.

The Rust owner cancels and joins commands even when guest cleanup fails. Unix
commands have their own process groups; cleanup signals the group before reaping
its leader and drains both pipes with deadlines. Shutdown has a shared seven-second
command cleanup budget. Descendants that escape that group are not supervised;
non-Unix builds only control the direct child and are not covered by this trial.

These are trusted caller scripts. Native profile permissions and sandboxes do
not constrain host verifier commands. The caller chooses programs, arguments
and acceptance criteria; never construct commands from worker output. The example
denies unexpected approvals and retries only normal verifier test failures, not
timeouts, cancellation or infrastructure failures. `permission_check` optionally
runs a caller command before denial to exercise an awaited interaction.

## JSON and evidence

JavaScript uses native objects, arrays and null. Its decoded `Number` view
rounds integers beyond the safe range. Outbound serialization rejects unsafe
integers and other non-JSON values instead of silently rewriting them.

Lua preserves signed 64-bit integers, but needs `json.null` to distinguish null
from an absent table key and `json.array({})` to distinguish an empty array from
an object. `json.object({})` explicitly marks an object; unmarked tables require
string keys. The adapter rejects mixed/sparse arrays, cycles, non-finite numbers
and unsupported values. Unsigned integers above `i64::MAX` can still become
floating-point values on decode; Lua is not a universally lossless JSON view.

Both engines share these private, exclusive-create evidence files:

- `stdout.bin`: original Due stdout bytes captured before framing or decoding.
- `receipt.json`: child PID, exit status, settlement, forced kill and transport
  error, plus actual retained byte count.
- `workflow.json`: language, workflow outcome, error and elapsed time.
- `exec.jsonl` in async mode: resolved command specifications, spawned PIDs and
  bounded command results. Its 64 MiB cap and write failures fail the workflow.

The capture retains a fixed 64 MiB prefix; overflow fails the run. Frames are
limited to 8 MiB, outbound requests to 1 MiB, and the receive queue to 256 frames
and 8 MiB. Queue overflow fails rather than silently dropping items. This is raw
Due transport evidence, not a claim that Due exposes every native provider byte.
Decoded logs are conveniences and must not replace the raw capture.

Each interpreter has a 32 MiB heap limit. `--timeout-ms` defaults to 300000 for
the entire workflow, including waits. SIGINT/SIGTERM/SIGHUP and deadline errors
are sticky, including when caught by a script. Cleanup closes stdin and awaits
the Due child, with bounded escalation and a receipt. Graceful child wait,
forced-kill wait and stdout settlement each have a two-second bound. These
controls are cooperative: Lua library C calls are not universally preemptible
by an instruction hook. This runner is not an untrusted-code sandbox and does
not guarantee settlement of arbitrary detached descendants.

## Verify without models

```sh
cargo fmt --manifest-path examples/rpc-languages/Cargo.toml -- --check
cargo clippy --locked --manifest-path examples/rpc-languages/Cargo.toml --all-targets -- -D warnings
cargo test --locked --manifest-path examples/rpc-languages/Cargo.toml
rustup run 1.90 cargo check --locked --manifest-path examples/rpc-languages/Cargo.toml

# Exercise the actual Due RPC server against both fake native backends:
DUE_RPC_BINARY="$PWD/target/release/due" cargo test --locked \
  --manifest-path examples/rpc-languages/Cargo.toml --test native -- --ignored
```

For separate interpreter builds, use `--no-default-features --features lua`
or `--no-default-features --features js` with the nested manifest. The common
fixture suite exercises both clients against identical wire sequences. Tests
require Python 3 on Unix; no model authentication is used.

See the [comparison report](../../docs/rpc-languages.md) for the observed
results and remaining decision points.
