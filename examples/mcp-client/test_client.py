"""Provider-free tests of the two-turn example's protocol handling."""
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location("due_mcp_example", Path(__file__).with_name("client.py"))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
Client = module.Client

PEER = r'''#!/usr/bin/env python3
import json
from pathlib import Path
import sys
MODE = "MODE_PLACEHOLDER"
root = Path(__file__).parent
items = []
turn = 0

def send(value):
    print(json.dumps(dict(jsonrpc="2.0", **value)), flush=True)

def item(kind, data, turn_id=None, push=True):
    value = dict(session_id="session", turn_id=turn_id, cursor=len(items)+1, kind=kind, data=data)
    items.append(value)
    if push:
        send(dict(method="notifications/due/session_item", params=dict(value, push_dropped=0)))

def answer(request, data):
    send(dict(id=request["id"], result=dict(structuredContent=data, content=[])))

try:
    for line in sys.stdin:
        request = json.loads(line)
        with (root / "calls.jsonl").open("a") as log:
            log.write(json.dumps(request)+"\n")
        method = request["method"]
        if method == "initialize":
            if MODE == "rpc_error":
                send(dict(id=request["id"], error=dict(code=-32000, message="fixture error")))
            else:
                send(dict(id=request["id"], result=dict(protocolVersion="2025-11-25")))
        elif method == "tools/call":
            name, args = request["params"]["name"], request["params"]["arguments"]
            if name == "session_open":
                assert args["stream"] is True
                item("session_opened", {"provider":args["provider"]})
                answer(request, {"session_id":"session"})
            elif name == "turn_start":
                turn += 1
                tid = f"turn-{turn}"
                if MODE == "tool_error":
                    send(dict(id=request["id"], result=dict(isError=True, content=[{"text":"fixture tool error"}])))
                    continue
                if MODE == "eof":
                    break
                item("turn_started", {"prompt":args["prompt"]}, tid)
                if MODE in ("permission", "stale_reply", "reply_error"):
                    item("interaction", {"request_id":"permission", "request":{"provider":"codex", "method":"item/commandExecution/requestApproval", "params":{"command":"echo fixture"}}}, tid)
                else:
                    item("native", {"provider":"codex", "method":"item/completed", "params":{"item":{"future":{"nested":[1, None, {"keep":True}]}}}}, tid, push=turn != 1)
                    item("turn_finished", {"status":"succeeded", "text":f"answer {turn}", "session_id":"native-session"}, tid)
                # Deliberately arrive after every notification, even terminal.
                answer(request, {"turn_id":tid})
            elif name == "session_read":
                answer(request, {"items":[i for i in items if i["cursor"]>args["after"]], "cursor":len(items), "has_more":False, "items_lost":MODE=="lost"})
            elif name == "interaction_reply":
                assert args["response"] == {"decision":"decline"}
                if MODE in ("stale_reply", "reply_error"):
                    if MODE == "stale_reply":
                        item("turn_finished", {"status":"timed_out", "text":"", "error":{"kind":"timeout"}}, f"turn-{turn}")
                    send(dict(id=request["id"], result=dict(isError=True, content=[{"text":"fixture reply error"}])))
                    continue
                item("interaction_reply", args, f"turn-{turn}")
                item("interaction_resolved", {"request_id":"permission"}, f"turn-{turn}")
                item("turn_finished", {"status":"succeeded", "text":"denied"}, f"turn-{turn}")
                answer(request, {"replied":True})
            elif name == "session_close":
                if MODE == "close_exit":
                    answer(request, {"closed":True})
                    item("session_closed", {})
                    break
                item("session_closed", {})
                answer(request, {"closed":True})
            else:
                raise AssertionError(name)
finally:
    (root / "exited").touch()
'''


class ClientTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.peer = self.root / "due"
        self.items = []

    def fixture(self, mode="normal"):
        self.peer.write_text(PEER.replace("MODE_PLACEHOLDER", mode))
        self.peer.chmod(0o755)
        return Client(self.peer, on_item=self.items.append, timeout=2)

    def calls(self):
        return [json.loads(line) for line in (self.root / "calls.jsonl").read_text().splitlines()]

    def test_two_turns_early_notifications_and_gap_recovery(self):
        with self.fixture() as client:
            self.assertEqual(client.open("codex", self.root), "session")
            self.assertEqual(client.turn("one")["text"], "answer 1")
            self.assertEqual(client.turn("two")["text"], "answer 2")
        self.assertEqual([i["cursor"] for i in self.items], list(range(1, 9)))
        native = next(i for i in self.items if i["kind"] == "native")
        self.assertEqual(native["data"]["params"]["item"]["future"]["nested"], [1, None, {"keep":True}])
        names = [c["params"]["name"] for c in self.calls() if c["method"] == "tools/call"]
        self.assertEqual(names.count("session_open"), 1)
        self.assertEqual(names.count("turn_start"), 2)
        self.assertIn("session_read", names)
        self.assertNotIn("turn_read", names)
        self.assertEqual(names[-1], "session_close")
        self.assertTrue((self.root / "exited").exists())

    def test_close_drains_output_after_process_exit(self):
        with self.fixture("close_exit") as client:
            client.open("codex", self.root)
            client.turn("one")
            original_call = client.call

            def wait_after_close(name, **arguments):
                result = original_call(name, **arguments)
                if name == "session_close":
                    # A finished process can still have unread output in its pipe.
                    client.process.wait(timeout=2)
                return result

            client.call = wait_after_close
        self.assertEqual(self.items[-1]["kind"], "session_closed")
        self.assertTrue((self.root / "exited").exists())

    def test_permission_requires_explicit_callback(self):
        with self.fixture("permission") as client:
            client.on_interaction = lambda item: {"decision":"decline"}
            client.open("codex", self.root)
            self.assertEqual(client.turn("one")["text"], "denied")
        self.assertTrue(any(i["kind"] == "interaction_resolved" for i in self.items))

    def test_terminal_result_survives_a_stale_interaction_reply(self):
        with self.fixture("stale_reply") as client:
            client.on_interaction = lambda item: {"decision":"decline"}
            client.open("codex", self.root)
            result = client.turn("one")
            self.assertEqual(result["status"], "timed_out")
            self.assertEqual(result["error"]["kind"], "timeout")
        with self.assertRaisesRegex(RuntimeError, "fixture reply error"):
            with self.fixture("reply_error") as client:
                client.on_interaction = lambda item: {"decision":"decline"}
                client.open("codex", self.root)
                client.turn("one")

    def test_default_permission_policy_fails_and_closes(self):
        with self.assertRaisesRegex(RuntimeError, "needs a decision"):
            with self.fixture("permission") as client:
                client.open("codex", self.root)
                client.turn("one")
        self.assertFalse(any(c.get("params", {}).get("name") == "interaction_reply" for c in self.calls()))
        self.assertTrue((self.root / "exited").exists())

    def test_rpc_tool_errors_eof_and_unrecoverable_gaps_are_explicit(self):
        for mode, expected in [("rpc_error", "fixture error"), ("tool_error", "fixture tool error"),
                               ("eof", "closed stdout"), ("lost", "no longer contains")]:
            with self.subTest(mode=mode), self.assertRaisesRegex((RuntimeError, EOFError), expected):
                with self.fixture(mode) as client:
                    client.open("claude", self.root)
                    client.turn("one")
            self.assertTrue((self.root / "exited").exists())


if __name__ == "__main__":
    unittest.main()
