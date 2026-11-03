---
name: rpc
description: Write a raw stdio JSON-RPC client for a resident Claude Code or Codex session, with the method map, streaming items, interaction replies, retention limits, lifecycle and error semantics.
---

# Write a raw RPC client

`due rpc` hosts one resident Claude Code or Codex session per process and speaks
newline-delimited JSON-RPC over stdin and stdout. Any language that can spawn a
child process and read lines can drive it. This guide is the complete wire
reference for the installed build; no source checkout is needed. If `due -h` does
not list `rpc`, this build does not include it.

Use `due run` for one turn. In harness-enabled builds, `due script` and the
`supervise` guide provide an embedded JavaScript client. Use raw RPC when your own program must own the
loop. Your client owns request IDs, stream consumption, recovery, permission
decisions, verification and cleanup. Due never approves a request for you, adds no
authorization, and does not verify what the worker did.

Check it without a provider (a bad method, then EOF):

```sh
printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"nope"}' | due rpc
# {"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"Method not found"}}
```

## Framing and envelope

- Spawn `due rpc`. Write one compact UTF-8 JSON object per line to its stdin.
  Read stdout continuously; it carries responses and `session.item`
  notifications interleaved. stderr holds diagnostics only.
- Each object has `"jsonrpc":"2.0"`, a `method`, and named `params` (an object;
  omitted means `{}`). Positional params and batch arrays are rejected.
- `id` is a string, an integer or `null`. A request with an `id` (even `null`)
  gets a response unless the connection fails. A valid envelope with no `id` is a
  notification and never gets a reply, even for dispatch errors. An invalid
  envelope can receive a null-id error. Use requests when the outcome matters.
- Responses may arrive out of order and `session.item` notifications may arrive
  before or after the response that caused them. Use unique IDs and correlate by
  ID, never by position.
- Input lines are limited to 1 MiB. At most 16 requests are in flight; a 17th
  receives error `-32001` and may be retried later.
- Malformed JSON in a valid UTF-8 line gets `-32700` (id `null`) and the
  connection stays usable. Invalid UTF-8 or an oversized line ends the
  connection. There is no initialization handshake and no tool discovery.

## Method map

All methods take a params object and reject unknown fields.

| Method | Params | Result |
| --- | --- | --- |
| `session.open` | `provider` or `request` supplying it; optional `model`, `effort`, `cwd`, `binary`, `resume`, `ephemeral`, `timeout`, `stream`, `native_args` | `session_id`, `protocol_version` (1), `provider`, `native_session_id` (nullable), `capabilities`, `stream` (`{"enabled":bool,"method":"session.item"}`), `resolved_options` |
| `turn.start` | `session_id`, `prompt` | `{turn_id, session_id}` immediately |
| `turn.read` | `session_id`, `turn_id`; optional `after` (default 0), `wait_ms` (0 to 10000, default 0) | `turn_id`, `native_session_id`, `state`, `cursor`, `events`, `events_lost`, `dropped_events`, `interactions`, `result`, `session_error` |
| `session.read` | `session_id`; optional `after` (default 0), `limit` (1 to 256, default 64) | `session_id`, `items`, `cursor`, `latest_cursor`, `has_more`, `items_lost`, `dropped_items` |
| `turn.interrupt` | `session_id`, `turn_id` | `{interrupt_requested:true, turn_id}` |
| `interaction.reply` | `session_id`, `turn_id`, `request_id`, `response` | `{replied:true, request_id}` |
| `session.close` | `session_id` | `{closed:true, native_session_id, session_error}` |

Notes:

- `session.open` options and profiles are described in `due skills show profiles`.
  Defaults: `ephemeral` false (conflicts with `resume`), `stream` false, no
  timeout. `timeout` (for example `"2m"`) is a per-turn deadline; `effort` is
  `low`, `medium` or `high`; `binary` selects a local provider executable;
  `cwd` and `request` paths resolve from Due's starting directory. Settings are
  fixed for the open session. Resident `native_args` accept only an audited set
  (see `profiles`). There are no answer schemas and no trace settings.
- `capabilities` is `{multi_turn:true, interrupt:true, interaction_reply:true,
  steer:false, session_read:true, session_stream:<stream>}`. There is no steering
  of a running turn.
- Check `protocol_version` is 1 before sending work; close if it is not.
- Due session handles (`session_id`, `turn_id`) are not provider IDs. The native
  Claude session or Codex thread ID is `native_session_id`; save it from
  `turn.read`, the `session_id` field of a `turn_finished` result, or `session.close` to `resume` later in a
  new process. It can be null until Claude's first turn. Resume needs the same
  provider and persisted storage; ephemeral sessions cannot be resumed.
- Open fails if another session is open (`close the current session before
  opening another`). Open one session, use it, close it.

## Errors

Dispatched method failures carry `error: {code, message}` and the request's `id`.
Parse and envelope errors (`-32700`, `-32600`) always carry `id: null`, even when
the invalid object supplied an ID. Clients cannot correlate these by request ID.
They may fail explicitly on these codes; the server can remain usable. Other
null-id replies can belong to valid requests that explicitly used `id: null`.

| Code | Meaning |
| --- | --- |
| `-32700` | Parse error |
| `-32600` | Invalid request: not an object, wrong `jsonrpc`, bad `method` or `id`, or a batch |
| `-32601` | Unknown method |
| `-32602` | Invalid params: not an object, unknown or mistyped field, bad provider or effort, unreadable or invalid profile, profile with `prompt` or `schema` |
| `-32000` | Operation failed: unknown session or turn handle, empty prompt, a turn already running, invalid session state, stale interaction, cursor errors, provider startup or cleanup failure |
| `-32001` | Too many in-flight requests |

Some `session.open` checks run after parameter/profile resolution: unsupported
resident `native_args`, an invalid merged `timeout`, or conflicting session
options currently return `-32000`, even before a provider starts. The code alone
does not establish that retrying is useful; retain the diagnostic message.

An ordinary failed model turn is a successful RPC call. The turn's `result` has
`status` `failed`, `timed_out` or `cancelled` and an `error`. Always inspect
`result.status`, not only the absence of an RPC error. Nothing is retried.
An expired profile `timeout` also fails the session: `session_failed` follows the
`timed_out` result and further turns are rejected. Close the handle; a later
session can start fresh or explicitly resume non-ephemeral history that persisted.

## A conversation

```jsonl
{"jsonrpc":"2.0","id":1,"method":"session.open","params":{"provider":"codex","cwd":"/path/to/project","timeout":"5m","stream":true}}
{"jsonrpc":"2.0","id":2,"method":"turn.start","params":{"session_id":"SESSION","prompt":"Review the parser. Do not change files."}}
{"jsonrpc":"2.0","id":3,"method":"session.close","params":{"session_id":"SESSION"}}
```

1. Register your item handler before `session.open`. Startup items, including a
   failed open's `session_failed`, can arrive before the open response. A failed
   open's provisional ID is not a usable handle; retry only after it settles.
2. Open with `"stream": true` to receive pushes, or poll (below).
3. `turn.start` returns the Due `turn_id` at once. One turn may run at a time;
   a second `turn.start` while one runs is rejected.
4. Read items until `turn_finished` for that turn. Its `data` is the normalized
   result (the same vocabulary as `due run`; see `due skills show contracts`),
   with `run_id` equal to the Due turn ID. A `turn_finished` item can arrive
   before the `turn.start` response, so buffer items by `turn_id`.
5. After a settled turn, send another `turn.start` on the same handle.
6. `session.close`, keep reading stdout through its response, close stdin, then
   wait for the child to exit (0 on clean shutdown; an error is written to stderr
   with a nonzero status).

## Streaming and items

A pushed item is a notification with method `session.item`; `session.read` returns
the same objects:

```json
{"jsonrpc":"2.0","method":"session.item","params":{"session_id":"S","turn_id":"T","cursor":8,"kind":"native","data":{"provider":"codex","method":"item/completed","params":{}},"push_dropped":0}}
```

`cursor` increases by one per item across the whole session. `turn_id` is null
for startup, idle and unattributed items. `push_dropped` is the number of pushed
items dropped so far for slow consumption; a jump in it or a gap in `cursor`
means you must recover with `session.read`.

| `kind` | `data` |
| --- | --- |
| `session_opened` | The open result |
| `turn_started` | `{prompt}` |
| `turn_finished` | The normalized result for the turn |
| `native` | Native payload: Claude `{provider:"claude", event}`; Codex `{provider:"codex", method, params}` |
| `native_response` | Codex thread or turn response in `response`; native RPC failures in `error` |
| `native_request` | Codex native requests; Claude permission requests denied outside the pending flow |
| `native_summary`, `native_snapshot` | Claude initialization projection, later full snapshot, or a subscriber-loss marker |
| `interaction` | `{request_id, request}`, a pending decision (below) |
| `interaction_reply`, `interaction_resolved` | Your reply echoed; `{request_id}` once resolved |
| `interrupt_pending` | Codex interruption requested before the native turn ID arrived |
| `session_failed` | `{message}`: the session is unusable; close it |
| `session_closed` | `{error, session_error, native_session_id}` |
| `omitted` | `{kind, bytes, reason}`: an item over 4 MiB was replaced |

Treat unknown kinds as observations and ignore them. Payloads can hold model text,
tool input, paths and other sensitive data; handle them accordingly.

Recovery and polling:

```jsonl
{"jsonrpc":"2.0","id":5,"method":"session.read","params":{"session_id":"S","after":18,"limit":64}}
{"jsonrpc":"2.0","id":6,"method":"turn.read","params":{"session_id":"S","turn_id":"T","after":0,"wait_ms":10000}}
```

Advance `after` to the returned `cursor` and repeat while `has_more` is true.
`turn.read` has its own turn-local cursor; do not mix the two. `state` is
`running`, `awaiting_input` or `finished`; a read returns at once when input is
pending or the turn is finished, otherwise waits up to `wait_ms`. `turn.read`
keeps only the latest turn; asking for an earlier turn is an error. Polling works
without `stream: true`.

## Interactions (permission and question requests)

Pending decisions arrive as `interaction` items and need an explicit client
response. Native requests outside an active turn or beyond capacity can be
denied or rejected automatically; see the limits below. The item is:

```json
{"session_id":"S","turn_id":"T","cursor":12,"kind":"interaction","data":{"request_id":"REQ","request":{"provider":"claude","method":"can_use_tool","tool_name":"Bash","input":{"command":"python3 -B -m unittest -v"},"request":{}}}}
```

- `data.request_id` is the outer ID. Pass it as `request_id` to `interaction.reply`,
  along with the item's `session_id` and `turn_id`.
- `data.request` is the provider-specific body.

Claude (`data.request_id` is the native request ID):

```json
{"provider":"claude","method":"can_use_tool","tool_name":"Bash","input":{"command":"python3 -B -m unittest -v"},"request":{ }}
```

`data.request.tool_name` and `data.request.input` hold the tool and its arguments.
`data.request.request` is the raw native request body. Reply with
`{"behavior":"allow"}` (optionally `"updated_input":{...}`, an object) or
`{"behavior":"deny","message":"..."}`. Other fields are rejected.

Codex (`data.request_id` is `codex:` plus the JSON native ID, such as `codex:17`):

```json
{"provider":"codex","id":17,"method":"item/commandExecution/requestApproval","params":{ }}
```

`method` and `params` are the native server request; shapes vary by method and
version, so inspect a real item (`session.read`) rather than assuming fields.
Reply with a JSON object in the native response shape. Approval requests
commonly take `{"decision":"accept"}`, `{"decision":"decline"}` or
`{"decision":"cancel"}`; question-style requests differ.

Reply:

```json
{"jsonrpc":"2.0","id":9,"method":"interaction.reply","params":{"session_id":"S","turn_id":"T","request_id":"REQ","response":{"behavior":"deny","message":"Not authorized by this caller."}}}
```

Success means the reply was accepted for native delivery, not that the native action then succeeded.
Unknown, already resolved and finished-turn IDs are rejected with `-32000`. A
reply racing with resolution or turn end is expected; treat that error as a race
only after you saw that exact `interaction_resolved` or the turn's
`turn_finished`. Pending requests are capped at 32: Claude denies the excess and
continues; Codex rejects it and fails the session. Actions the native settings
already allow never produce an interaction.

Approve narrowly. Match the exact tool and exact arguments the assignment
authorizes and deny everything else. Never blanket-approve:

```python
AUTHORIZED = "python3 -B -m unittest -v"

def decide(item):
    request = item["data"]["request"]            # provider-specific body
    if request.get("provider") == "claude":
        if (request.get("tool_name") == "Bash"
                and request.get("input", {}).get("command") == AUTHORIZED):
            return {"behavior": "allow"}
        return {"behavior": "deny", "message": "Not authorized by this caller."}
    # Codex: decline by default. Allow only after checking request["method"]
    # and request["params"] against your own exact policy.
    if request.get("method", "").endswith("requestApproval"):
        return {"decision": "decline"}
    raise RuntimeError("No authorized response for this interaction kind")
```

## Interruption

`turn.interrupt` with the exact `turn_id` requests a stop and returns after the
provider acknowledges. Then keep reading until that turn's `turn_finished`
(usually `cancelled`) before starting another turn. Interrupting a finished turn
is an error. For Codex, an interrupt sent before its native turn ID exists is held
and an `interrupt_pending` item records intent only; one interruption can be
pending, and the control deadline is 30 seconds. After a timeout the stop may
already have been delivered. Interruption does not undo edits or prove every
native subprocess stopped.

## Retention and limits

Bounded in memory; nothing is durable. Read what you need before closing.

| Store | Limit |
| --- | --- |
| Session journal (`session.read`) | 1024 items, 8 MiB; an item over 4 MiB becomes `omitted` |
| Push queue | 256 items, 8 MiB; drops oldest, reported in `push_dropped` |
| `turn.read` event window | Latest turn only; 256 events, 1 MiB; `events_lost` and `dropped_events` report gaps |
| Terminal result and pending interactions | Kept separately, so a missed push does not lose the result |
| Output to you | Responses and their aggregate backlog limited to 8 MiB, at most 16 queued; a blocked write for 5 seconds ends the connection |

Do not stop reading stdout. If your client stalls, Due drops the connection and
starts cleanup. The error `-32001` is recoverable only while output capacity
remains.

## Lifecycle and shutdown

- Close with `session.close`. It awaits provider shutdown, cancels any unfinished
  turn, and returns the last `native_session_id` and the first recorded
  `session_error` (null if none). A cleanup failure returns `-32000`, with the
  same identifiers in the `session_closed` item. After close settles the handle is
  gone, even on failure.
- EOF on stdin, SIGINT, SIGTERM, SIGHUP, bad framing or failed output starts
  cleanup and rejects new work. Shutdown waits for provider cleanup and is not
  bounded to a few seconds. Avoid SIGKILL, which can leave provider processes.
- `session_failed` means the session is unusable (a native deadline, a provider
  crash, a protocol violation). Close its handle before opening another. Closing
  successfully does not turn a failed turn into a success.
- A session lives only in this process. Reconnecting after process loss is not
  provided; resume the native session in a new process instead.
- Native permissions are inherited, and the worker's tool processes are the
  provider's. Verify diffs, artifacts and any external effect yourself.

## Minimal client

A Python 3 client example for one turn that approves only one exact command.
Adapt the paths and provider; add your own verification afterwards:

```python
import json, queue, subprocess, threading, time

class Due:
    """Single-caller example; a reader thread continuously drains RPC stdout."""
    def __init__(self):
        self.proc = subprocess.Popen(
            ["due", "rpc"], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            text=True, encoding="utf-8", bufsize=1)
        self.items = queue.Queue(maxsize=256)
        self.waiting, self.lock, self.n, self.failure = {}, threading.Lock(), 0, None
        self.reader = threading.Thread(target=self._read, daemon=True)
        self.reader.start()

    def _read(self):
        failure = EOFError("Due stdout reached EOF")
        try:
            for line in self.proc.stdout:
                message = json.loads(line)
                if message.get("method") == "session.item":
                    self.items.put_nowait(message["params"])
                else:
                    with self.lock:
                        waiter = self.waiting.pop(message.get("id"), None)
                    if waiter is not None:
                        waiter.put(message)
        except Exception as error:
            failure = error  # Malformed output or a full item queue fails explicitly.
        finally:
            with self.lock:
                self.failure = failure
                for waiter in self.waiting.values():
                    waiter.put({"error": {"message": str(failure)}})
                self.waiting.clear()
            # Do not silently discard a queue overflow. The terminal sentinel
            # carries the failure, and the caller closes stdin in its finally.
            if self.items.full():
                while True:
                    try:
                        self.items.get_nowait()
                    except queue.Empty:
                        break
            self.items.put_nowait(None)

    def call(self, method, **params):
        with self.lock:
            if self.failure is not None:
                raise RuntimeError(str(self.failure))
            self.n += 1
            ident, waiter = self.n, queue.Queue()
            self.waiting[ident] = waiter
        try:
            frame = {"jsonrpc": "2.0", "id": ident, "method": method, "params": params}
            self.proc.stdin.write(json.dumps(frame, separators=(",", ":")) + "\n")
            self.proc.stdin.flush()
            message = waiter.get(timeout=75)  # Example request-ack budget, not a turn deadline.
            if "error" in message:
                raise RuntimeError(message["error"])
            return message["result"]
        finally:
            with self.lock:
                self.waiting.pop(ident, None)

    def finish(self):
        try:
            self.proc.stdin.close()  # EOF requests cleanup even after an exception.
        except BrokenPipeError:
            pass
        try:
            status = self.proc.wait(timeout=90)
        except subprocess.TimeoutExpired as error:
            self.proc.kill()
            self.proc.wait()
            raise RuntimeError("Forced Due termination; native settlement unconfirmed") from error
        self.reader.join(timeout=2)
        if self.reader.is_alive() or status != 0:
            raise RuntimeError(f"RPC process/reader failed to settle cleanly: exit {status}")
        if self.failure is not None and not isinstance(self.failure, EOFError):
            raise RuntimeError(str(self.failure))

AUTHORIZED = "python3 -B -m unittest -v"

def decide(item):
    request = item["data"]["request"]
    if request.get("provider") == "claude":
        if (request.get("tool_name") == "Bash"
                and request.get("input", {}).get("command") == AUTHORIZED):
            return {"behavior": "allow"}
        return {"behavior": "deny", "message": "Not authorized by this caller."}
    if request.get("method", "").endswith("requestApproval"):
        return {"decision": "decline"}
    raise RuntimeError("No authorized response for this interaction kind")

TURN_WAIT_SECONDS = 600  # Caller-selected turn budget, separate from native timeout.
due = Due()
try:
    opened = due.call("session.open", provider="claude", cwd="/path/to/project",
                      timeout="10m", stream=True)
    if opened["protocol_version"] != 1:
        raise RuntimeError("Unsupported protocol version")
    sid = opened["session_id"]
    turn = due.call("turn.start", session_id=sid,
                    prompt="Run the unit tests and report failures. Do not edit files.")["turn_id"]
    result, cursor = None, 0
    deadline = time.monotonic() + TURN_WAIT_SECONDS
    while result is None:
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            raise RuntimeError("Caller turn wait deadline exceeded")
        try:
            item = due.items.get(timeout=remaining)
        except queue.Empty as error:
            raise RuntimeError("Caller turn wait deadline exceeded") from error
        if item is None:
            raise RuntimeError(str(due.failure))
        if (item["cursor"] != cursor + 1 or item.get("push_dropped", 0) > 0
                or item["kind"] == "omitted"):
            raise RuntimeError("Incomplete session stream; explicit recovery required")
        cursor = item["cursor"]
        if item["kind"] == "session_failed":
            raise RuntimeError(item["data"])
        if item["turn_id"] != turn:
            continue
        if item["kind"] == "interaction":
            due.call("interaction.reply", session_id=sid, turn_id=turn,
                     request_id=item["data"]["request_id"], response=decide(item))
        elif item["kind"] == "turn_finished":
            result = item["data"]
    print(result["status"], result.get("text"))
    closed = due.call("session.close", session_id=sid)
    if result["status"] != "succeeded" or closed["session_error"] is not None:
        raise RuntimeError("Worker execution or session failed; inspect the result")
finally:
    due.finish()
```

The reader is bounded to 256 queued items and fails on overflow. A production
client should also budget queued bytes and select request/assignment deadlines. The example
fails explicitly on an omitted item, dropped push or cursor gap instead of
silently continuing with incomplete observation. It uses a 75-second request-ack
budget, a separate 600-second caller turn budget and a 90-second shutdown budget;
choose budgets appropriate to your caller. Progress does not reset the turn
deadline. A forced Due kill leaves native settlement
unconfirmed. A stale interaction reply fails this example closed; a more capable
client may reconcile an exact known retirement as described above.

Check `result["status"]` and your own evidence before treating the work as done.
For the result fields see `due skills show contracts`; for the same loop with
embedded JavaScript see `due skills show supervise` when listed by `due skills`.
