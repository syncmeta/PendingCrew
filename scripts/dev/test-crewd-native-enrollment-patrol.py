#!/usr/bin/env python3
"""Compile production Apple enrollment/transport against a warm macOS product cache and run a private Core.

Pass a normal Core witness, the exact source ref, and an existing arm64 macOS
Build/Products/<configuration> directory containing Swift package modules and
aggregate objects. No Xcode build, account, Keychain vault or real device is used.
"""
import argparse
import json
import pathlib
import re
import shutil
import socket
import subprocess
import sys
import tempfile
import uuid

sys.dont_write_bytecode = True
sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent / "fixtures"))
from crewd_recovery_common import Public, birth, save, sha, socket_owned, wait_for


SWIFT = (
    "Sources/Mac/Crewd/CrewdClient.swift", "Sources/Mac/Crewd/CrewdLocalIPC.swift",
    *(f"Sources/CrewdRemote/{name}.swift" for name in (
        "CrewdRemoteProtocol", "CrewdRemoteVault", "CrewdRemoteCertificate", "CrewdRemotePairing",
        "CrewdRemoteTransport", "CrewdEnrollment", "CrewdEnrollmentWire", "CrewdEnrollmentVault",
        "CrewdEnrollmentTransport", "CrewdDeliveryValidation",
    )),
)
OBJECTS = ("X509", "SwiftASN1", "Crypto", "CCryptoBoringSSL", "CryptoBoringWrapper", "CryptoExtras", "_CryptoExtras", "_CertificateInternals")


def compile_peer(repo, output, ref, products):
    source = output / "source"
    source.mkdir(mode=0o700)
    modules = output / "modules"
    modules.mkdir(mode=0o700)
    files = []
    checks = {}
    for entry in SWIFT:
        target = source / pathlib.Path(entry).name
        target.write_bytes(subprocess.check_output(["/usr/bin/git", "show", f"{ref}:{entry}"], cwd=repo))
        checks[entry] = sha(target)
        files.append(target)
    fixture = source / "fixture.swift"
    shutil.copyfile(repo / "scripts/dev/fixtures/crewd-native-enrollment-patrol.swift", fixture)
    checks["fixture.swift"] = sha(fixture)
    files.append(fixture)

    original_cache = {}
    objects = []
    for module in OBJECTS:
        original = products / f"{module}.o"
        original_cache[original] = sha(original)
        target = modules / original.name
        shutil.copyfile(original, target)
        assert sha(target) == original_cache[original]
        objects.append(target)
        if module == "CCryptoBoringSSL":
            continue
        original = products / f"{module}.swiftmodule" / "arm64-apple-macos.swiftmodule"
        original_cache[original] = sha(original)
        target = modules / original.parent.name / original.name
        target.parent.mkdir()
        shutil.copyfile(original, target)
        assert sha(target) == original_cache[original]

    peer = output / "native-peer"
    arguments = ["/usr/bin/xcrun", "swiftc", "-target", "arm64-apple-macos14.0", "-parse-as-library",
                 "-module-cache-path", str(output / "module-cache"), "-I", str(modules), "-o", str(peer),
                 *map(str, files), *map(str, objects)]
    with (output / "compile.log").open("xb") as log:
        subprocess.run(arguments, check=True, stdout=log, stderr=log, timeout=120)
    assert all(sha(path) == digest for path, digest in original_cache.items()), "warm dependency cache changed"
    save(output / "client-inputs.json", {"ref": ref, "swift": checks, "cache": {str(p): d for p, d in original_cache.items()}, "binarySHA256": sha(peer)})
    return peer


def run_core(output, binary, expected, ref, peer):
    data, work, home = [output / name for name in ("data", "work", "home")]
    for path in (data, work, home):
        path.mkdir(mode=0o700)
    endpoint = data / "crewd.sock"
    with socket.socket() as reserved:
        reserved.bind(("127.0.0.1", 0))
        port = reserved.getsockname()[1]
    profile = output / "fixture.sb"
    quoted_endpoint = json.dumps(str(endpoint))
    profile.write_text(f'''(version 1)
(allow default)
(deny network*)
(allow network-bind network-inbound (local unix-socket (literal {quoted_endpoint})))
(allow network-outbound (remote unix-socket (literal {quoted_endpoint})))
(allow network-bind network-inbound (local ip "localhost:{port}"))
(allow network-outbound (remote ip "localhost:{port}"))
(deny file-read* file-write* (subpath "/Users"))
''')
    env = {"PATH": "/usr/bin:/bin:/usr/sbin:/sbin", "HOME": str(home), "TMPDIR": str(output),
           "SHELL": "/usr/bin/false", "PENDINGCREW_ISOLATED_CREDENTIALS": "memory"}
    sandbox = ["/usr/bin/sandbox-exec", "-f", str(profile)]
    with (output / "core.log").open("xb") as log:
        core = subprocess.Popen([*sandbox, str(binary), "--data-dir", str(data), "serve", "--disable-claude",
                                 "--codex", "/usr/bin/false"], env=env, cwd=work,
                                stdin=subprocess.DEVNULL, stdout=log, stderr=log)
    client = None
    try:
        owned = birth(core.pid)
        assert owned is not None

        def ready():
            assert core.poll() is None, "Core exited before owned socket opened"
            return socket_owned(endpoint, owned)

        wait_for(ready, "owned Core socket")
        client = Public(endpoint, owned, required_capabilities=("command.receipts", "work.patrolStart"))
        assert client.hello["build"]["gitSha"] == ref[:12] and not client.hello["build"]["dirty"]
        before = client.rpc("remote.status", {})
        assert not before["listening"] and not before["configuration"]["enabled"]
        client.rpc("remote.configure", {"requestId": str(uuid.uuid4()), "expectedRevision": before["configuration"]["revision"],
                                       "enabled": True, "listenHost": "127.0.0.1", "port": port})
        assert client.rpc("remote.status", {})["listening"]
        with (output / "native-peer.log").open("xb") as log:
            subprocess.run([*sandbox, str(peer), str(data)], env=env, cwd=work, stdin=subprocess.DEVNULL,
                           stdout=log, stderr=log, timeout=90, check=True)
        native = json.loads((output / "native-result.json").read_text())
        assert native["verifiedOnFreshTransport"] and native["liveAndFreshTransportRevoked"]
        assert client.rpc("session.list", {"crewId": native["crewId"]}) == []
    finally:
        if client is not None:
            client.close()
        if core.poll() is None:
            core.terminate()
        assert core.wait(timeout=25) == 0, "owned Core did not stop normally"
    assert sha(binary) == expected
    save(output / "result.json", {"ref": ref, "coreSHA256": expected, "native": native, "coreExited": True,
                                  "credentials": "process-local vaults", "transport": "macOS native loopback", "providerProcesses": 0})


def main():
    if not __debug__:
        raise RuntimeError("this isolated fixture requires assertions enabled")
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=pathlib.Path, required=True)
    parser.add_argument("--witness", type=pathlib.Path, required=True)
    parser.add_argument("--ref", required=True)
    parser.add_argument("--products", type=pathlib.Path, required=True)
    args = parser.parse_args()
    assert sys.platform == "darwin" and re.fullmatch(r"[0-9a-f]{40}", args.ref)
    repo = pathlib.Path(__file__).resolve().parents[2]
    binary = args.binary.resolve(strict=True)
    witness = json.loads(args.witness.read_text())
    assert binary.is_relative_to("/private/tmp") and witness["sourceRef"] == args.ref and witness["freshWorkspaceArtifacts"]
    assert sha(binary) == witness["binarySHA256"]
    output = pathlib.Path(tempfile.mkdtemp(prefix="pcw-enrollment-patrol.", dir="/private/tmp"))
    print(f"Evidence: {output}", flush=True)
    peer = compile_peer(repo, output, args.ref, args.products.resolve(strict=True))
    run_core(output, binary, witness["binarySHA256"], args.ref, peer)
    print("PASS production short-link enrollment, original manual patrol receipt after reconnect, and grant revocation", flush=True)


if __name__ == "__main__":
    main()
