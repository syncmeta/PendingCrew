#!/usr/bin/env python3
"""Replay synthetic app-server observations through the exact ordinary Core.

Only fake_codex is started as a runner. The actual Core writes and reads the
Human-channel notice and projects the same failure on live and history wires.
"""
import argparse
import json
import os
import pathlib
import signal
import subprocess
import sys
import tempfile
import threading

from crewd_recovery_common import Public, birth, physical_observation, same, save, sha, socket_owned, wait_for

NOTICE = "自动审批未完成，动作未执行。"
PRIVATE = "SYNTHETIC_PRIVATE_REVIEW_BODY"
ORDINARY = "Synthetic MCP error retained."
TASK = "PCW_GUARDIAN_SYNTHETIC_FIRST_TASK"
BARRIER = "PCW_GUARDIAN_FOREIGN_PACKETS_CONSUMED"
ERROR = (
    f"Automatic approval review failed: <html>{PRIVATE}</html>\n\n"
    "The action was not executed because automatic approval review could not be completed."
)


def guardian_item():
    return {"id": "owned-guardian-failure", "type": "mcpToolCall", "server": "crew",
            "tool": "crew_structure", "status": "failed",
            "error": {"message": ERROR, "extra": PRIVATE}}


def ordinary_item():
    item = guardian_item()
    return dict(item, id="ordinary-error", error={"message": ORDINARY})


def foreign_item():
    return dict(guardian_item(), id="foreign", server="fixture-external",
                error={"message": ERROR.replace(PRIVATE, "SYNTHETIC_EXTERNAL_REVIEW_BODY")})


def proxy(fake, wire):
    release = pathlib.Path(wire).with_suffix(".release")
    child = subprocess.Popen([fake, *sys.argv[4:]], stdin=subprocess.PIPE, stdout=subprocess.PIPE)
    output_lock = threading.Lock()

    def forward(line):
        with output_lock:
            sys.stdout.buffer.write(line)
            sys.stdout.buffer.flush()

    def emit(frame):
        forward((json.dumps(frame) + "\n").encode())

    def stop(*_):
        child.terminate()
        child.wait(timeout=5)
        sys.exit(0)

    signal.signal(signal.SIGTERM, stop)

    def relay():
        def notify(target, turn, item):
            emit({"method": "item/completed", "params":
                  {"threadId": target, "turnId": turn, "item": item}})

        with open(wire, "a", encoding="utf-8") as log:
            for line in child.stdout:
                forward(line)
                frame = json.loads(line)
                if frame.get("method") != "turn/started":
                    continue
                thread = frame["params"]["threadId"]
                turn = frame["params"]["turn"]["id"]
                item = guardian_item()
                for case, target, current, event_item in [
                    ("foreign_thread", "fixture-other-thread", turn, item),
                    ("old_turn", thread, "fixture-old-turn", item),
                    ("foreign_server", thread, turn, foreign_item()),
                    ("ordinary_error", thread, turn, ordinary_item()),
                    ("actual", thread, turn, item),
                    ("duplicate", thread, turn, item),
                ]:
                    if case == "actual":
                        # The public inspect call must observe the barrier before the qualifying
                        # event is released. The earlier frames use the same ordered pipe.
                        notify(thread, turn, {"id": "barrier", "type": "agentMessage", "text": BARRIER})
                        wait_for(release.is_file, "fixture did not release the qualifying frame", seconds=15)
                    notify(target, current, event_item)
                    log.write(json.dumps({"case": case, "thread": target, "turn": current,
                                          "itemId": event_item["id"]}) + "\n")
                    log.flush()

    relay_thread = threading.Thread(target=relay, daemon=True)
    relay_thread.start()
    try:
        for line in sys.stdin.buffer:
            request = json.loads(line)
            if request.get("method") == "thread/turns/list":
                with open(wire, encoding="utf-8") as log:
                    actual = next(row for line in log if (row := json.loads(line))["case"] == "actual")
                emit({"id": request["id"], "result": {"data": [
                    {"id": actual["turn"], "status": "completed",
                     "items": [guardian_item(), ordinary_item()]}
                ], "nextCursor": None, "backwardsCursor": "stored-anchor"}})
                continue
            child.stdin.write(line)
            child.stdin.flush()
    finally:
        child.stdin.close()
        child.wait(timeout=5)
        relay_thread.join(timeout=5)


def main():
    if not __debug__:
        raise RuntimeError("this fixture requires assertions enabled")
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=pathlib.Path, required=True)
    parser.add_argument("--witness", type=pathlib.Path, required=True)
    parser.add_argument("--ref", required=True)
    parser.add_argument("--fake-codex", type=pathlib.Path, required=True)
    parser.add_argument("--fake-sha256", required=True)
    args = parser.parse_args()
    binary, fake = args.binary.resolve(strict=True), args.fake_codex.resolve(strict=True)
    witness = json.loads(args.witness.read_text())
    assert witness["sourceRef"] == args.ref and witness["freshWorkspaceArtifacts"]
    assert sha(binary) == witness["binarySHA256"]
    assert fake.name == "fake_codex" and sha(fake) == args.fake_sha256
    root = pathlib.Path(tempfile.mkdtemp(prefix="pcw-guardian-wire-", dir="/private/tmp"))
    print("Evidence:", root, flush=True)
    for name in ("bin", "data", "home", "codex", "work"):
        (root / name).mkdir(mode=0o700)
    path = root / "data/crewd.sock"
    frames, calls = root / "notifications.ndjson", root / "fake-input.ndjson"
    script = root / "bin/codex"
    script.write_text("#!/usr/bin/python3\nimport os\nimport sys\n"
                      f"os.execv(sys.executable,[sys.executable,'-B',{str(pathlib.Path(__file__).resolve())!r},"
                      f"'--proxy',{str(fake)!r},{str(frames)!r},*sys.argv[1:]])\n")
    script.chmod(0o700)
    env = {"PATH": "/usr/bin:/bin", "HOME": str(root / "home"), "CODEX_HOME": str(root / "codex"),
           "TMPDIR": str(root), "SHELL": "/usr/bin/false", "LANG": "C.UTF-8"}
    result = {"sourceRef": args.ref, "binarySHA256": witness["binarySHA256"], "fakeSHA256": args.fake_sha256,
              "fixtureSHA256": sha(__file__), "daemonExit": []}
    daemons, clients = [], []

    def start():
        with (root / f"core-{len(daemons)+1}.log").open("x") as log:
            process = subprocess.Popen([str(binary), "--data-dir", str(root / "data"), "serve", "--disable-claude",
                                        "--codex", str(script), "--codex-env", "FAKE_CODEX_LOG=" + str(calls)],
                                       env=env, cwd=root / "work", stdin=subprocess.DEVNULL, stdout=log, stderr=log)
        daemons.append(process)
        core = birth(process.pid)
        assert core and core["ppid"] == os.getpid()
        def ready():
            assert process.poll() is None, "Core exited before its socket was ready"
            return socket_owned(path, core)
        wait_for(ready, "owned daemon socket")
        client = Public(path, core, required_capabilities=("command.receipts", "session.codex"))
        clients.append(client)
        assert client.hello["build"]["gitSha"] == args.ref[:12] and not client.hello["build"]["dirty"]
        return process, client

    def submit(client, method, params, phase):
        request = client.prepared(method, params)
        client.submit(request)
        return request, client.phase(request, [phase])

    def stop(process):
        process.terminate()
        status = process.wait(timeout=25)
        result["daemonExit"].append(status)
        assert status == 0

    def messages(client, channel):
        messages = client.rpc("message.list", {"channelId": channel, "limit": 100})
        assert all(PRIVATE not in row["text"] for row in messages)
        return messages

    def notes(client, channel):
        return [row for row in messages(client, channel) if NOTICE in row["text"]]

    def assert_scrubbed(item):
        assert PRIVATE not in json.dumps(item), "guardian transport body crossed the public wire"
        original = guardian_item()
        assert {key: value for key, value in item.items() if key != "error"} == {
            key: value for key, value in original.items() if key != "error"}
        assert item["error"]["message"].startswith("Automatic approval review failed:")
        assert "The action was not executed" in item["error"]["message"]

    def history(client, sid, expected_turn):
        page = client.rpc("session.codex.history", {"sessionId": sid, "itemsView": "full"})
        assert page["backwardsCursor"] == "stored-anchor" and len(page["items"]) == 1
        turn = page["items"][0]
        assert turn["id"] == expected_turn
        _, ordinary = turn["items"]
        assert ordinary == ordinary_item()
        return page

    try:
        first, client = start()
        client.rpc("events.subscribe", {"topics": ["session"]})
        database = client.identity["dbId"]
        runtime = client.identity["runtimeBindingId"]
        _, created = submit(client, "crew.createFirstMessage",
                            {"crew": {"title": "Synthetic guardian fixture", "workingDirectory": str(root / "work")},
                             "text": "Synthetic original conversation"}, "committed")
        group = created["result"]["crew"]["id"]
        channel = created["result"]["message"]["channelId"]
        request, launched = submit(client, "session.start",
                                   {"crewId": group, "role": "captain", "executor": "codex",
                                    "brief": TASK, "isolation": False}, "launched")
        sid = launched["result"]["sessionId"]
        physical = launched["result"]["birth"]
        assert same(physical, birth(physical["pid"]))
        wait_for(lambda: BARRIER in client.rpc("session.inspect", {"sessionId": sid})["screen"],
                 "Core consumed all three non-qualifying packets")
        assert notes(client, channel) == []
        assert client.rpc("decision.list", {}) == []
        result["nonQualifyingPacketsProducedNoNotice"] = True
        frames.with_suffix(".release").touch(exist_ok=False)
        wait_for(lambda: notes(client, channel), "actual Human-channel notice")
        def finished():
            inspection = client.rpc("session.inspect", {"sessionId": sid})
            return inspection if not inspection["live"]["working"] and NOTICE in inspection["screen"] else None
        inspection = wait_for(finished, "completed synthetic turn and transcript")
        assert PRIVATE not in inspection["screen"] and "runnerRequest" not in inspection
        assert "pendingDecision" not in inspection["live"] and client.rpc("decision.list", {}) == []
        actual = notes(client, channel)
        assert len(actual) == 1 and actual[0]["senderKind"] == "system" and PRIVATE not in actual[0]["text"]
        roster = client.rpc("session.list", {"crewId": group})
        groups = client.rpc("crew.list", {})
        assert len(roster) == 1 and roster[0]["id"] == sid and len(groups) == 2
        injected = [json.loads(line) for line in frames.read_text().splitlines()]
        assert [row["case"] for row in injected] == [
            "foreign_thread", "old_turn", "foreign_server", "ordinary_error", "actual", "duplicate"]
        assert injected[4]["thread"] == roster[0]["agentSessionId"]
        turn = injected[4]["turn"]
        assert {row["turn"] for row in injected[4:]} == {turn}
        live_history = history(client, sid, turn)
        observed = [event["data"] for event in client.events
                    if event.get("event") == "session.runnerItem" and event["data"]["sessionId"] == sid]
        qualified = [row for row in observed if row["item"].get("id") == guardian_item()["id"]]
        assert {row["turnId"] for row in qualified} == {"fixture-old-turn", turn}
        items = [row["item"] for row in observed]
        history_item = live_history["items"][0]["items"][0]
        redacted = {"live": all(PRIVATE not in json.dumps(item) for item in items),
                    "history": PRIVATE not in json.dumps(history_item)}
        result["rawRedaction"] = redacted
        assert all(redacted.values()), f"guardian transport body crossed the public wire: {redacted}"
        for row in qualified:
            assert_scrubbed(row["item"])
        assert_scrubbed(history_item)
        assert ordinary_item() in items
        assert foreign_item() in items
        before = calls.read_bytes()
        native_turns = [json.loads(line) for line in before.splitlines() if b'"turn/start"' in line]
        assert len(native_turns) == 1 and TASK in json.dumps(native_turns[0])
        client.close()
        stop(first)
        assert physical_observation(physical)["groupState"] == "absent"
        second, reopened = start()
        original = reopened.lookup(request)
        assert reopened.identity["dbId"] == database and reopened.identity["runtimeBindingId"] != runtime
        assert original["historical"] and original["phase"] == "launched" and original["result"] == launched["result"]
        assert notes(reopened, channel) == actual and reopened.rpc("decision.list", {}) == []
        assert reopened.rpc("session.live", {"sessionId": sid})["running"] is False
        assert history(reopened, sid, turn) == live_history
        new_calls = [json.loads(line) for line in calls.read_bytes()[len(before):].splitlines()]
        assert not any(row.get("method") in {"thread/resume", "turn/start"} for row in new_calls)
        reopened.close()
        stop(second)
        result.update(status="PASS", notice=actual[0], observedPackets=injected, historicalReceipt=original,
                      rawLiveAndHistoryRedacted=True, ordinaryAndForeignErrorsUnchanged=True,
                      originalRunner=physical_observation(physical), turns=1, pendingApproval=False)
    except Exception as error:
        result.update(status="FAIL", error=repr(error))
        raise
    finally:
        for client in clients:
            client.close()
        try:
            for process in daemons:
                if process.poll() is None:
                    stop(process)
        finally:
            save(root / "proof.json", result)
    print("PASS guardian notice, raw live/history redaction and ordinary error preservation; no action or replay.")


if __name__ == "__main__":
    if len(sys.argv) > 1 and sys.argv[1] == "--proxy":
        proxy(sys.argv[2], sys.argv[3])
    else:
        main()
