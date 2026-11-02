#!/usr/bin/env python3
"""Small synchronous Due MCP client for Python 3.10+ on macOS/Linux."""
import argparse
from collections import deque
import json
import os
from pathlib import Path
import select
import subprocess
import sys
import time


class ToolError(RuntimeError):
    """A tool returned isError, distinct from a transport or JSON-RPC failure."""


class Client:
    def __init__(self, binary="due", *, on_item=None, on_interaction=None, timeout=300):
        self.on_item = on_item or (lambda item: print(json.dumps(item), flush=True))
        self.on_interaction = on_interaction
        self.timeout = timeout
        self.session_id = None
        self._cursor = self._sequence = 0
        self._buffer = b""
        self._responses, self._interactions = {}, {}
        self._deferred = deque()
        self._recovering = False
        self._result = None
        self._deadline = None
        self.process = subprocess.Popen([str(binary), "mcp"], stdin=subprocess.PIPE,
                                        stdout=subprocess.PIPE, stderr=None, bufsize=0)
        os.set_blocking(self.process.stdin.fileno(), False)

    def __enter__(self):
        try:
            self.rpc("initialize", {"protocolVersion": "2025-11-25", "capabilities": {},
                                    "clientInfo": {"name": "due-python-example", "version": "1"}})
            self._send({"jsonrpc": "2.0", "method": "notifications/initialized"})
            return self
        except BaseException:
            self.close()
            raise

    def __exit__(self, exc_type, exc, traceback):
        try:
            self.close()
        except Exception as error:
            if exc is None:
                raise
            print(f"cleanup: {error}", file=sys.stderr)

    def _remaining(self):
        remaining = (self._deadline or time.monotonic() + self.timeout) - time.monotonic()
        if remaining <= 0:
            raise TimeoutError("Due MCP deadline exceeded")
        return remaining

    def _send(self, message):
        data = json.dumps(message).encode() + b"\n"
        if len(data) > 1024 * 1024:
            raise ValueError("MCP input exceeds Due's 1 MiB frame limit")
        while data:
            if not select.select([], [self.process.stdin], [], self._remaining())[1]:
                raise TimeoutError("Due MCP stdin stalled")
            try:
                data = data[os.write(self.process.stdin.fileno(), data):]
            except BlockingIOError:
                pass

    def _receive(self):
        while b"\n" not in self._buffer:
            if not select.select([self.process.stdout], [], [], self._remaining())[0]:
                raise TimeoutError("Due MCP stdout stalled")
            chunk = os.read(self.process.stdout.fileno(), 65536)
            if not chunk:
                raise EOFError(f"Due MCP closed stdout (exit={self.process.poll()})")
            self._buffer += chunk
            if len(self._buffer) > 16 * 1024 * 1024:
                raise RuntimeError("MCP output frame exceeds example's 16 MiB limit")
        line, self._buffer = self._buffer.split(b"\n", 1)
        message = json.loads(line)
        if "id" in message:
            self._responses[message["id"]] = message
        elif message.get("method") == "notifications/due/session_item":
            self._item(message["params"])

    def rpc(self, method, params):
        previous = self._deadline
        self._deadline = previous or time.monotonic() + self.timeout
        try:
            self._sequence += 1
            ident = self._sequence
            self._send({"jsonrpc": "2.0", "id": ident, "method": method, "params": params})
            while ident not in self._responses:
                self._receive()
            response = self._responses.pop(ident)
            if "error" in response:
                raise RuntimeError(f"{method}: {response['error']}")
            return response["result"]
        finally:
            self._deadline = previous

    def call(self, name, **arguments):
        result = self.rpc("tools/call", {"name": name, "arguments": arguments})
        if result.get("isError"):
            raise ToolError(f"{name}: {result['content']}")
        return result["structuredContent"]

    def _emit(self, item):
        if item["cursor"] <= self._cursor:
            return
        self._cursor = item["cursor"]
        self.on_item(item)
        kind, data = item["kind"], item["data"]
        if kind == "omitted":
            raise RuntimeError(f"Session payload omitted: {data}")
        if kind == "interaction":
            self._interactions[data["request_id"]] = item
        elif kind == "interaction_resolved":
            self._interactions.pop(data["request_id"], None)
        elif kind == "turn_finished":
            self._result = (item["turn_id"], data)
            self._interactions.clear()

    def _item(self, item):
        if self._recovering:
            if len(self._deferred) >= 1024:
                raise RuntimeError("Too many notifications during recovery")
            self._deferred.append(item)
            return
        if item["cursor"] > self._cursor + 1:
            self._recover(item["session_id"])
        self._emit(item)

    def _recover(self, session_id):
        self._recovering = True
        try:
            while True:
                page = self.call("session_read", session_id=session_id, after=self._cursor)
                if page["items_lost"]:
                    raise RuntimeError("Session journal no longer contains the missing items")
                for item in page["items"]:
                    self._emit(item)
                if not page["has_more"]:
                    break
        finally:
            self._recovering = False
        while self._deferred:
            self._item(self._deferred.popleft())

    def open(self, provider, cwd=None, **options):
        if self.session_id is not None:
            raise RuntimeError("This example owns one session; reuse it with turn()")
        opened = self.call("session_open", **dict(options, provider=provider,
                           cwd=str(Path(cwd or ".").resolve()), stream=True))
        self.session_id = opened["session_id"]
        return self.session_id

    def turn(self, prompt):
        if self.session_id is None:
            raise RuntimeError("Open a session first")
        self._result = None
        self._deadline = time.monotonic() + self.timeout
        try:
            started = self.call("turn_start", session_id=self.session_id, prompt=prompt)
            turn_id = started["turn_id"]
            while self._result is None or self._result[0] != turn_id:
                if self._interactions:
                    request_id, item = next(iter(self._interactions.items()))
                    if self.on_interaction is None:
                        raise RuntimeError(f"Native interaction needs a decision: {item['data']}")
                    response = self.on_interaction(item)
                    if not isinstance(response, dict):
                        raise RuntimeError("Interaction callback must return an explicit native response")
                    try:
                        self.call("interaction_reply", session_id=self.session_id, turn_id=turn_id,
                                  request_id=request_id, response=response)
                    except ToolError:
                        # A timeout can finish the turn while its decision is pending.
                        if self._result is None or self._result[0] != turn_id:
                            raise
                    self._interactions.pop(request_id, None)
                else:
                    self._receive()
            return self._result[1]
        finally:
            self._deadline = None

    def close(self):
        if self.process.stdin.closed:
            return
        self._deadline = time.monotonic() + 20
        failure = None
        try:
            if self.session_id and self.process.poll() is None:
                self.call("session_close", session_id=self.session_id)
        except Exception as error:
            failure = error
        finally:
            self.process.stdin.close()  # EOF also asks Due to clean up its provider.
            try:
                # Process exit does not imply its stdout pipe has been drained.
                while True:
                    try:
                        self._receive()
                    except EOFError:
                        break
                code = self.process.wait(timeout=self._remaining())
                if code:
                    failure = failure or RuntimeError(f"Due MCP exited {code}")
            except Exception as error:
                self.process.kill()
                self.process.wait()
                failure = failure or error
            self.process.stdout.close()
            self._deadline = None
        if failure:
            raise failure


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("provider", choices=["claude", "codex"])
    parser.add_argument("--due", default="due", help="Due binary built with --features mcp")
    parser.add_argument("--cwd", default=".")
    parser.add_argument("--model")
    args = parser.parse_args()
    options = {"model": args.model} if args.model else {}
    with Client(args.due) as client:
        client.open(args.provider, args.cwd, timeout="5m", **options)
        for prompt in ["Remember the token copper-8251. Reply READY. Do not use tools.",
                       "What token did I ask you to remember? Reply only that token. Do not use tools."]:
            result = client.turn(prompt)
            print(f"{result['status']}: {result.get('text', '')}", file=sys.stderr)
            if result["status"] != "succeeded":
                return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
