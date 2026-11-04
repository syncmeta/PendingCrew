#!/usr/bin/env python3
"""Headless first-task smoke of the exact candidate helper and real Codex.

Only an owned database, workspace, Codex home, and synthetic loopback responses
are used. This tests the production commands and MCP, not a GUI or live model.
"""
import argparse
import http.server
import json
import os
import pathlib
import plistlib
import sqlite3
import subprocess
import sys
import tempfile
import threading
import time

sys.path.insert(0, str(pathlib.Path(__file__).parent / "fixtures"))
from crewd_recovery_common import Public, birth, same, save, sha, socket_owned, wait_for

ROOT_TITLE = "PCW Tirk Fresh Root"
CHILD_TITLE = "PCW Tirk Fresh Child"
WORKER_TITLE = "PCW Tirk Fresh Worker"
FIRST = "PCW_TIRK_ORIGINAL_FIRST_TASK"
CHILD = "PCW_TIRK_CHILD_FIRST_TASK"
WORKER = "PCW_TIRK_WORKER_FIRST_TASK"
DONE = {"root": "PCW_TIRK_ROOT_FINISHED", "child": "PCW_TIRK_CHILD_FINISHED", "worker": "PCW_TIRK_WORKER_FINISHED"}
TOOLS = {"read_channel", "crew_structure", "start_session"}


def messages(inputs, roles):
    return "\n".join(
        part.get("text", "")
        for item in inputs
        if item.get("type") == "message" and item.get("role") in roles
        for part in item.get("content", [])
    )


class Provider(http.server.BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def do_GET(self):
        self.send_response(200)
        self.end_headers()
        self.wfile.write(b'{"data":[]}')

    def do_POST(self):
        state = self.server.proof
        try:
            assert self.path == "/v1/responses", self.path
            assert not self.headers.get("Authorization"), "unexpected authentication"
            count = int(self.headers["Content-Length"])
            assert 0 < count <= 1_500_000, count
            request = json.loads(self.rfile.read(count))
            with self.server.lock:
                number = state["requestCount"]
                assert number < 6, "unexpected additional provider request"
                state["requestCount"] += 1
                item = self.reply(request, number)
        except Exception as error:
            state["errors"].append(repr(error))
            self.send_error(400, "isolated proof stopped")
            return
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.end_headers()
        rid = f"pcw_{number}"

        def event(kind, **fields):
            payload = json.dumps({"type": kind, **fields})
            self.wfile.write(f"event: {kind}\ndata: {payload}\n\n".encode())
            self.wfile.flush()

        event("response.created", response={"id": rid, "object": "response", "status": "in_progress", "output": []})
        event("response.output_item.added", output_index=0, item=item)
        event("response.output_item.done", output_index=0, item=item)
        event("response.completed", response={"id": rid, "object": "response", "status": "completed",
              "output": [item], "usage": {"input_tokens": 10, "output_tokens": 10, "total_tokens": 20}})

    def reply(self, request, number):
        state = self.server.proof
        inputs = request["input"]
        developer = messages(inputs, {"developer", "system"})
        user = messages(inputs, {"user"})
        if "You are judging one planned coding-agent action" in developer:
            state["approvalReview"] = {"requested": True, "outcome": "unavailable_in_isolated_fixture"}
            raise RuntimeError("native automatic reviewer requested a separate decision; no approval was simulated")
        role = "child" if CHILD in developer or CHILD in user else "worker" if WORKER in developer or WORKER in user else "root"
        offered = {tool["name"] for group in request["tools"] if group.get("name") == "mcp__crew"
                   for tool in group.get("tools", [])}
        state["providerEvents"].append({"role": role, "request": number, "crewTools": sorted(offered),
                                        "atMonotonic": time.monotonic()})
        if role != "worker":
            assert TOOLS <= offered, (role, "captain tool projection missing", sorted(offered))
            assert all("mcp__crew__" + tool in developer for tool in ("crew_structure", "start_session")), role
        else:
            assert not offered & {"crew_structure", "start_session"}, sorted(offered)
        if role == "root":
            outcomes = {row["call_id"]: row["output"] for row in inputs
                        if row.get("type") == "function_call_output"}
            stages = [
                ("read_channel", {"channel": state["firstMessage"]["channelId"], "limit": 10}),
                ("crew_structure", {"action": "create_child", "title": CHILD_TITLE, "brief": CHILD, "runner": "codex"}),
                ("start_session", {"title": WORKER_TITLE, "brief": WORKER, "runner": "codex", "isolation": False}),
            ]
            expected = [FIRST, CHILD_TITLE, WORKER_TITLE]
            for i, (tool, arguments) in enumerate(stages):
                call = f"pcw_root_{i}"
                if call not in outcomes:
                    assert call not in state["issued"], ("tool request repeated", call)
                    state["issued"].append(call)
                    if i == 0:
                        state["originalInFirstRequest"] = FIRST in user or FIRST in developer
                    return {"type": "function_call", "id": f"fc_{number}", "call_id": call, "namespace": "mcp__crew",
                            "name": tool, "arguments": json.dumps(arguments), "status": "completed"}
                assert expected[i] in str(outcomes[call]), (tool, outcomes[call])
                state["toolOutputs"][tool] = outcomes[call]
        assert role not in state["finished"], ("duplicate completion", role)
        state["finished"].append(role)
        return {"type": "message", "id": f"msg_{number}", "role": "assistant", "status": "completed",
                "content": [{"type": "output_text", "text": DONE[role], "annotations": []}]}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--app", type=pathlib.Path, required=True)
    parser.add_argument("--ref", required=True)
    parser.add_argument("--helper-sha256", required=True)
    parser.add_argument("--codex", type=pathlib.Path, required=True)
    parser.add_argument("--codex-sha256", required=True)
    args = parser.parse_args()
    app, sdk = args.app.resolve(strict=True), args.codex.resolve(strict=True)
    assert sys.platform == "darwin" and app.is_relative_to("/private/tmp")
    assert sdk.name == "codex" and sdk.is_file() and sha(sdk) == args.codex_sha256
    helper = app / "Contents/Helpers/crewd"
    info = plistlib.loads((app / "Contents/Info.plist").read_bytes())
    assert info["CFBundleIdentifier"] == "com.pendingname.pendingcrew.native-candidate"
    assert info["PendingCrewSourceRef"] == args.ref and sha(helper) == args.helper_sha256
    base = pathlib.Path(tempfile.mkdtemp(prefix="pcw-tirk-", dir="/private/tmp"))
    print("Evidence:", base, flush=True)
    for name in ("data", "home", "codex", "work", "tmp"):
        (base / name).mkdir(mode=0o700)
    path = base / "data/crewd.sock"
    state = {"requestCount": 0, "providerEvents": [], "toolOutputs": {}, "issued": [], "finished": [], "errors": [],
             "candidateRef": args.ref, "helperSha256": args.helper_sha256, "codexSha256": args.codex_sha256}
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Provider)
    server.proof, server.lock = state, threading.Lock()
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    url = f"http://127.0.0.1:{server.server_port}"
    (base / "codex/config.toml").write_text(f'''model = "gpt-5.1-codex"
model_provider = "pcw_fixture"
chatgpt_base_url = "{url}"
[model_providers.pcw_fixture]
name = "PendingCrew first-task loopback fixture"
base_url = "{url}/v1"
wire_api = "responses"
requires_openai_auth = false
supports_websockets = false
request_max_retries = 0
stream_max_retries = 0
''')
    # Constrain the whole process tree, including SDK and its MCP helper.
    profile = base / "offline.sb"
    quote = lambda value: json.dumps(str(value))
    sdk_metadata = "\n".join(
        f"(allow file-read-metadata (literal {quote(parent)}))"
        for parent in sdk.parents
        if parent.is_relative_to("/Users")
    )
    profile.write_text(f'''(version 1)
(allow default)
(deny network*)
(allow network-bind network-inbound (local unix-socket (literal {quote(path)})))
(allow network-outbound (remote unix-socket (literal {quote(path)})))
(allow network-outbound (remote ip "localhost:{server.server_port}"))
(deny file-read* file-write* (subpath "/Users"))
(allow file-read* (subpath {quote(sdk.parent.parent)}))
{sdk_metadata}
(deny mach-lookup (global-name "com.apple.securityd"))
''')
    env = {"PATH": "/usr/bin:/bin", "HOME": str(base / "home"), "CODEX_HOME": str(base / "codex"),
           "TMPDIR": str(base / "tmp"), "XDG_CACHE_HOME": str(base / "home/cache"),
           "SHELL": "/bin/sh", "NO_PROXY": "127.0.0.1,localhost", "no_proxy": "127.0.0.1,localhost",
           "CODEX_REFRESH_TOKEN_URL_OVERRIDE": url + "/oauth/token"}
    client, daemon = None, None
    owned = []
    with (base / "core.log").open("x") as log:
        try:
            daemon = subprocess.Popen(["/usr/bin/sandbox-exec", "-f", str(profile), str(helper),
                                       "--data-dir", str(base / "data"), "serve", "--disable-claude", "--codex", str(sdk)],
                                      cwd=base / "work", env=env, stdin=subprocess.DEVNULL,
                                      stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
            core = birth(daemon.pid)
            assert core is not None, "owned Core birth unavailable"
            def running():
                assert daemon.poll() is None, "candidate Core exited before readiness"
                return socket_owned(path, core)
            wait_for(running, "candidate Core owned socket")
            client = Public(path, core, required_capabilities=("command.receipts", "session.codex"))
            state["hello"] = client.hello
            def command(operation, params, phase):
                request = client.prepared(operation, params)
                started = time.monotonic()
                client.submit(request)
                result = client.phase(request, [phase])
                state[operation] = {"request": request, "receipt": result, "elapsedSeconds": time.monotonic() - started}
                return result["result"]
            root = command("crew.createFirstMessage", {"crew": {"title": ROOT_TITLE, "workingDirectory": str(base / "work")},
                                                         "text": f"{FIRST}。请创建一个子机组，并在本机组启动一名成员。"}, "committed")
            state["firstMessage"] = root["message"]
            root_id = root["crew"]["id"]
            captain = command("session.start", {"crewId": root_id, "role": "captain", "executor": "codex", "brief":
                               "读取本机组群聊与待办，回应人类需求；没有任务时待命。", "isolation": False}, "launched")
            observed_captain = birth(captain["birth"]["pid"])
            assert same(captain["birth"], observed_captain) and observed_captain["ppid"] == core["pid"]
            owned.append(observed_captain)
            state["captainPhysicalBirth"] = observed_captain
            def completed():
                assert not state["errors"], state["errors"]
                return set(state["finished"]) == set(DONE)
            wait_for(completed, "three actual native initial turns", 120)
            crews = client.rpc("crew.list", {})
            child = next(row for row in crews if row["title"] == CHILD_TITLE)
            assert root_id in child["parentIds"], child
            root_roster = client.rpc("session.list", {"crewId": root_id})
            child_roster = client.rpc("session.list", {"crewId": child["id"]})
            assert len(root_roster) == 2 and len(child_roster) == 1
            sessions = {"root": next(row for row in root_roster if row["id"] == captain["sessionId"]),
                        "worker": next(row for row in root_roster if row["role"] == "worker"),
                        "child": child_roster[0]}
            assert sessions["child"]["role"] == "captain" and sessions["worker"]["title"] == WORKER_TITLE
            for role, row in sessions.items():
                sid = row["id"]
                def history():
                    page = client.rpc("session.codex.history", {"sessionId": sid, "limit": 10, "itemsView": "full"})
                    return page if DONE[role] in json.dumps(page) else None
                page = wait_for(history, f"{role} persisted native history", 20)
                assert page["threadId"] == row["agentSessionId"]
                identity = json.loads((base / "data/exec-registry.json").read_text())[sid]
                observed = birth(identity["pid"])
                assert same(identity, observed) and observed["ppid"] == core["pid"], (role, observed)
                owned.append(observed)
                state.setdefault("sessions", {})[role] = {"session": row, "history": page, "physicalBirth": observed}
            assert len({row["agentSessionId"] for row in sessions.values()}) == 3
            assert state["requestCount"] == 6 and len(state["toolOutputs"]) == 3
            state["completed"] = True
        finally:
            if client is not None:
                client.close()
            try:
                if daemon is not None and daemon.poll() is None:
                    daemon.terminate()
                    assert daemon.wait(timeout=30) == 0, "owned Core stop failed"
                state["coreExit"] = daemon.returncode if daemon is not None else None
                state["observedRunnersGone"] = all(not same(row, birth(row["pid"])) for row in owned) if owned else None
                database = base / "data/crewd.db"
                if state["coreExit"] == 0 and database.is_file():
                    with sqlite3.connect(database.as_uri() + "?immutable=1", uri=True) as db:
                        state["durableCrews"] = db.execute("SELECT title FROM crews ORDER BY title").fetchall()
                        state["durableSessions"] = db.execute("SELECT id, role FROM sessions ORDER BY id").fetchall()
            finally:
                server.shutdown()
                server.server_close()
                save(base / "proof.json", state)
    assert state["completed"] and state["observedRunnersGone"]
    print("PASS exact candidate Core, real native first task and MCP child/worker effects; synthetic loopback only.")


if __name__ == "__main__":
    main()
