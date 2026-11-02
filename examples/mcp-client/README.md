# Python MCP session client

A Python 3.10+ standard-library example for macOS/Linux. It launches one `due mcp`
process, opens one resident Claude or Codex session, and sends two prompts through
that session. Full session items are printed as JSONL on stdout as they arrive;
final status and answer summaries go to stderr.

From the Due checkout:

```sh
cargo build --locked --features mcp
python3 examples/mcp-client/client.py codex --due ./target/debug/due --cwd /path/to/project
# Or select Claude and optionally a model:
python3 examples/mcp-client/client.py claude --due ./target/debug/due --model sonnet
```

The default prompts remember a token and ask for it on the second turn. They
request no tools. Authentication and permissions remain the provider's own.
Python is needed only for this example, not for the Due binary.

The reusable part is deliberately small:

```python
from client import Client

with Client("./target/debug/due", on_item=lambda item: print(item)) as session:
    session.open("codex", "/path/to/project", timeout="5m")
    first = session.turn("Review the parser. Do not change files.")
    second = session.turn("Which finding matters most? Do not change files.")
    assert first["status"] == second["status"] == "succeeded"
```

`turn()` returns the normalized Due result. It consumes the custom
`notifications/due/session_item` stream, including notifications that arrive
before tool responses. It does not poll `turn_read`. It recovers cursor gaps
through `session_read`, deduplicates replayed items, and fails explicitly if the
journal has already lost the needed records. This is in-memory recovery within
the current session, not reconnection or durable history. The callback receives
complete records, including `data`, `session_id`, `turn_id`, `kind`, and `cursor`;
replayed records do not have the push-only `push_dropped` field. Keep `on_item`
fast, and write records to your own file if you need a transcript.

An unexpected native interaction fails the example by default and closes the
session. A caller with an explicit policy may supply `on_interaction`, which
receives the full interaction item and returns the provider-native response:

```python
def reject_command(item):
    request = item["data"]["request"]
    if request["provider"] == "codex" and request["method"] == "item/commandExecution/requestApproval":
        return {"decision": "decline"}
    raise RuntimeError("This example has no policy for this interaction")

with Client("./target/debug/due", on_interaction=reject_command) as session:
    session.open("codex", "/path/to/project")
    result = session.turn("Inspect the project and report what you find.")
```

The client never grants permissions automatically. A callback may instead ask a
human or return an already-authorized decision. Different provider request
methods need different response objects. `on_item` observes the full stream;
`on_interaction` handles pending decisions after recovery, so historical resolved
requests are not automatically replayed as actions.

Each turn has a client deadline (`Client(..., timeout=300)` by default), and
`session.open(..., timeout="5m")` sets the separate native turn deadline.
Context exit requests `session_close`, closes stdin, and drains output while
waiting for Due. Cleanup is bounded at 20 seconds; if Due remains stuck the
example kills that process and reports an error. This is not proof that every
native descendant has stopped. Inspect partial changes after failure or
cancellation. The client is sequential, owns one session, and is not a general
MCP SDK. See [the spike guide](../../docs/mcp.md) for protocol and retention limits.

The offline tests launch a fake Due peer and use no models or credentials:

```sh
python3 -B -m unittest discover -s examples/mcp-client -p 'test_*.py' -v
```

## Single-session issue trial

A macOS trial on 2026-11-01 used this client with one resident Codex CLI 0.165.0
session to verify issues [#30](https://github.com/joshrotenberg/due/issues/30),
[#31](https://github.com/joshrotenberg/due/issues/31), and
[#29](https://github.com/joshrotenberg/due/issues/29). The checkout was based on
`a303cc89a6df810243125a6e8c3f2632eb29d32d` with the uncommitted MCP spike.

Four turns shared one native session ID: one per issue, then a corrective
follow-up for a non-executable synthetic fixture. The caller retained the first
attempt, authorized preparation of a private fixture copy, and independently
checked representative results. All three product defects were already fixed;
no product changes or issue closures were needed for this verification.

The caller captured 1,789 consecutive session items, zero reported push drops,
four terminal results, a final session-closed item, and Due exit 0. It used no
`turn_read` polling. By the final journal read, the server had evicted 764 older
items; the caller's JSONL file still held the complete observed stream. Keep a
caller-owned transcript when a session needs more history than the bounded
server journal.

Native configuration was inherited: the opening response reported model
`gpt-6-astra`, `dangerFullAccess`, and approval policy `never`. No policy was
changed and no approval roundtrip occurred. The issue probes used fake providers;
they do not establish live interruption behavior or permission isolation.
