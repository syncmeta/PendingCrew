"""Unchanged selected original intent/lifecycle methods; runtime is Root-owned."""
import json,os,shutil,socket,stat,time
from pathlib import Path
from common import save,sha,birth,same,wait_for
class Lifecycle:

    def record(self, name, value):
        self.seq += 1
        path = self.out / "observations" / (f"{self.seq:04d}-" + name + ".json")
        save(path, value)
        return path


    def rpc(self, method, params, label=None):
        entry = {"transport": "public", "method": method, "params": params, "attempts": 1}
        self.record((label or method) + "-prepared", entry)
        self.pending.append(entry)
        try:
            result = self.human.rpc(method, params)
            self.record((label or method) + "-response", result)
            self.pending.remove(entry)
            return result
        except BaseException as e:
            self.record((label or method) + "-error", {"error": repr(e), "resubmitted": False})
            raise


    def command(self, op, params, phases=("committed",)):
        original = self.human.prepared(op, params)
        self.commands.append(original)
        self.record(op + "-original", {"request": original, "submitCount": 1})
        reply = self.rpc("command.submit", original, op + "-submit")
        if reply["phase"] not in phases:
            def observe():
                found = self.rpc("command.lookup", {"requestId": original["requestId"], "dbId": original["dbId"]}, op + "-lookup")
                if found and found["phase"] in ["unknown", "suppressed", "rejected"] and found["phase"] not in phases:
                    raise RuntimeError(json.dumps(found))
                return found if found and found["phase"] in phases else None
            reply = wait_for(observe, "original command terminal phase; lookup only")
            self.record(op + "-lookup-terminal", reply)
        assert reply["phase"] in phases and reply["historical"] is False
        assert reply["requestId"] == original["requestId"]
        return original, reply


    def stop(self):
        assert self.human is not None and self.hibernate_request is None
        self.hibernate_request = self.human.prepared("crewd.hibernate", {"deadlineSecs": 0})
        self.commands.append(self.hibernate_request)
        self.record("original-hibernate", {"request": self.hibernate_request, "submitCount": 1})
        # No lookup/resubmit while the owner is physically draining its first intent.
        target = self.human.request("command.submit", self.hibernate_request)
        deadline = time.monotonic() + 35
        receipt = None
        while receipt is None and time.monotonic() < deadline:
            self.human.socket.settimeout(min(1, max(.01, deadline - time.monotonic())))
            try:
                frame = self.human.frame()
            except socket.timeout:
                continue
            if frame.get("id") == target:
                assert "error" not in frame, frame
                receipt = frame["result"]
            else:
                self.human.events.append(frame)
        self.record("hibernate-observed-response", {"response": receipt, "resubmitted": False})
        assert receipt is not None, "unknown original hibernate: never repeat"
        assert receipt["phase"] == "confirmed" and receipt["historical"] is False
        for k in ["requestId", "dbId", "runtimeBindingId"]:
            assert receipt[k] == self.hibernate_request[k]
        assert all(receipt["result"][k] is True for k in ["actorCompleted", "hibernationRecordPersisted", "physicalChildrenExited"])
        self.human.close()
        self.human = None
        assert self.proc.wait(timeout=30) == 0
        assert not self.endpoint.exists() and not same(self.owner, birth(self.proc.pid))
        self.report["phases"].append({"phase": self.phase, "coreBirth": self.owner,
                                      "hibernateUUID": self.hibernate_request["requestId"], "exit": 0,
                                      "confirmed": True, "physicalExit": True, "socketGone": True})
        self.proc = None
        self.archive_cold("phase-" + str(self.phase))


    def archive_cold(self, label):
        assert self.proc is None
        target = self.out / "cold" / label
        special = []
        def ignore(directory, names):
            skipped = []
            for name in names:
                path = Path(directory) / name
                info = path.lstat()
                if not (stat.S_ISREG(info.st_mode) or stat.S_ISDIR(info.st_mode)):
                    special.append({"path": str(path.relative_to(self.data)), "mode": info.st_mode,
                                    "uid": info.st_uid, "device": info.st_dev, "inode": info.st_ino,
                                    "type": "socket" if stat.S_ISSOCK(info.st_mode) else "symlink" if stat.S_ISLNK(info.st_mode) else "other"})
                    skipped.append(name)
            return skipped
        shutil.copytree(self.data, target, symlinks=False, ignore=ignore)
        self.record("cold-archive-" + label, {"path": str(target), "files": {str(f.relative_to(target)): sha(f) for f in target.rglob("*") if f.is_file()},
                                              "specialInodesMetadataOnly": special, "socketCopied": False})

