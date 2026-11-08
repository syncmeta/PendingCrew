#!/usr/bin/env python3
"""Compile production Apple enrollment/transport against a warm macOS product cache and run a private Core.

Pass a normal Core witness, the exact source ref, and an existing arm64 macOS
Build/Products/<configuration> directory containing Swift package modules and
aggregate objects. No Xcode build, account, Keychain vault or real device is used.
"""
import argparse
import hashlib
import json
import os
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
    "Sources/CrewdRemote/CrewdAdminAskGroupsWire.swift",
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


def verify_app_build_inputs(repo, ref, products, library, app_witness, module_map):
    """Fail before compiling or starting Core unless the whole build matches."""
    witness = json.loads(app_witness.read_text())
    assert witness["exit"] == 0 and witness["allSourceInputsUnchanged"] is True
    assert witness["sourceCommit"] == ref
    assert sha(library) == witness["librarySHA256"]
    assert subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=repo, text=True).strip() == ref
    assert not subprocess.check_output(["git", "status", "--porcelain", "--untracked-files=all"], cwd=repo, text=True).strip()
    source_inputs = app_witness.parent / "source-inputs.json"
    assert sha(source_inputs) == witness["sourceInputSHA256"]
    entries = json.loads(source_inputs.read_text())
    assert entries == json.loads((app_witness.parent / "source-inputs-after.json").read_text())
    tracked = sorted(set(subprocess.check_output(["git", "ls-files", "-z"], cwd=repo).decode().split("\0")) - {""})
    assert isinstance(entries, list) and all(isinstance(e, dict) and set(e) == {"path", "sha256"} for e in entries)
    assert [entry["path"] for entry in entries] == tracked, "App source inventory is not the complete unique tracked input"
    for entry in entries:
        path = repo / entry["path"]
        digest = hashlib.sha256(os.fsencode(path.readlink())).hexdigest() if path.is_symlink() else sha(path)
        assert digest == entry["sha256"], entry["path"]
    assert products.resolve(strict=True) == pathlib.Path(witness["products"]).resolve(strict=True)
    assert module_map.resolve(strict=True) == pathlib.Path(witness["moduleMap"]).resolve(strict=True)
    assert sha(module_map) == witness["moduleMapSHA256"]
    modules = {p.name: sha(p) for p in (products / "PendingCrew.swiftmodule").glob("*") if p.is_file()}
    assert modules and modules == witness["appModules"], "App compiler modules differ from the successful build"
    return witness


def compile_app_peer(repo, output, ref, products, library, app_witness, module_map):
    """Use the real ordinary App module; keep the original TLS assertions too."""
    sealed_witness = sha(app_witness)
    witness = verify_app_build_inputs(repo, ref, products, library, app_witness, module_map)

    original = repo / "scripts/dev/fixtures/crewd-native-enrollment-patrol.swift"
    extension = repo / "scripts/dev/fixtures/crewd-native-window-tls.swift"
    raw = original.read_text()
    alias = "// CrewdRemoteTransport's production dependency on the workspace is this typealias.\nenum CrewdWorkspace {\n    typealias Connector = @MainActor (CrewdClient) async throws -> Void\n}\n"
    assert raw.count(alias) == 1
    raw = raw.replace(alias, "@testable import PendingCrew\n")
    marker = '        stage("original enrollment recovered")'
    assert raw.count(marker) == 1
    raw = raw.replace(marker, marker + '\n        try await actualWindows(pairing:pairing,recordID:prepared.id,directory:directory,grantID:grant.id)')
    marker = "    private static func stage(_ value: String) {"
    assert raw.count(marker) == 1
    raw = raw.replace(marker, extension.read_text() + "\n" + marker)
    marker = '"verifiedOnFreshTransport": .bool(true),'
    assert raw.count(marker) == 1
    raw = raw.replace(marker, '"actualAppTwoWindowLocalTLSLocal": .bool(true), ' + marker)
    fixture = output / "fixture.swift"
    fixture.write_text(raw)
    peer = output / "native-peer"
    arguments = ["/usr/bin/xcrun", "swiftc", "-swift-version", "5", "-parse-as-library", "-I", str(products),
                 "-module-cache-path", str(output / "module-cache"), "-Xcc", "-fmodule-map-file=" + str(module_map),
                 str(fixture), str(library), "-Xlinker", "-rpath", "-Xlinker", str(library.parent),
                 "-Xlinker", "-rpath", "-Xlinker", str(library.parent.parent / "Frameworks"), "-o", str(peer)]
    with (output / "compile.log").open("xb") as log:
        subprocess.run(arguments, check=True, stdout=log, stderr=log, timeout=120)
    assert sha(app_witness) == sealed_witness, "App build witness changed while compiling"
    verify_app_build_inputs(repo, ref, products, library, app_witness, module_map)
    save(output / "client-inputs.json", {
        "ref": ref, "appBuildWitnessSHA256": sha(app_witness), "appLibrarySHA256": sha(library),
        "originalFixtureSHA256": sha(original), "windowExtensionSHA256": sha(extension),
        "renderedFixtureSHA256": sha(fixture), "moduleMapSHA256": sha(module_map),
        "binarySHA256": sha(peer), "command": arguments,
        "appModules": witness["appModules"],
    })
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
    parser.add_argument("--app-library", type=pathlib.Path, help="Optional real ordinary Debug App library with testability enabled")
    parser.add_argument("--app-build-witness", type=pathlib.Path, help="Exact App build result and sibling source-inputs.json")
    parser.add_argument("--module-map", type=pathlib.Path, help="The same build's CTOML module map")
    args = parser.parse_args()
    app_mode = args.app_library is not None
    if any(v is not None for v in (args.app_library, args.app_build_witness, args.module_map)) and not all(v is not None for v in (args.app_library, args.app_build_witness, args.module_map)):
        parser.error("--app-library, --app-build-witness and --module-map must be supplied together")
    assert sys.platform == "darwin" and re.fullmatch(r"[0-9a-f]{40}", args.ref)
    repo = pathlib.Path(__file__).resolve().parents[2]
    binary = args.binary.resolve(strict=True)
    witness = json.loads(args.witness.read_text())
    assert binary.is_relative_to("/private/tmp") and witness["sourceRef"] == args.ref and witness["freshWorkspaceArtifacts"]
    assert sha(binary) == witness["binarySHA256"]
    output = pathlib.Path(tempfile.mkdtemp(prefix="pcw-enrollment-patrol.", dir="/private/tmp"))
    print(f"Evidence: {output}", flush=True)
    if app_mode:
        sealed_app_witness = sha(args.app_build_witness)
        peer = compile_app_peer(repo, output, args.ref, args.products.resolve(strict=True),
                                args.app_library.resolve(strict=True), args.app_build_witness.resolve(strict=True), args.module_map.resolve(strict=True))
    else:
        peer = compile_peer(repo, output, args.ref, args.products.resolve(strict=True))
    run_core(output, binary, witness["binarySHA256"], args.ref, peer)
    if app_mode:
        assert sha(args.app_build_witness) == sealed_app_witness, "App build witness changed during TLS runtime"
        verify_app_build_inputs(repo, args.ref, args.products.resolve(strict=True),
                                args.app_library.resolve(strict=True), args.app_build_witness.resolve(strict=True), args.module_map.resolve(strict=True))
        result = json.loads((output / "result.json").read_text())
        assert result["native"]["actualAppTwoWindowLocalTLSLocal"]
        print("PASS actual ordinary App two-window Local/mTLS/Local with original Unknown and drafts", flush=True)
    print("PASS production short-link enrollment, original manual patrol receipt after reconnect, and grant revocation", flush=True)


if __name__ == "__main__":
    main()
