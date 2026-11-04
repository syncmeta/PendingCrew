#!/usr/bin/env python3
"""Timestamp one exact private Developer ID App copy without rebuilding or uploading it."""
import argparse
from datetime import datetime, timezone
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import plistlib
import re
import subprocess
import sys

sys.dont_write_bytecode = True
HERE = Path(__file__).resolve().parent
HELPER = Path("Contents/Helpers/crewd")
MAIN = Path("Contents/MacOS/PendingCrew")
SEAL = Path("Contents/_CodeSignature/CodeResources")


def module(filename):
    spec = importlib.util.spec_from_file_location(filename, HERE / filename)
    value = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(value)
    return value


def require(condition, reason):
    if not condition:
        raise ValueError(reason)


def read(path):
    return json.loads(path.read_text())


def checked_signature(app, requirement, expected, timestamped):
    subprocess.run(
        ["/usr/bin/codesign", "--verify", "--deep", "--strict", str(app)],
        check=True, capture_output=True,
    )
    verify_requirement(app, requirement)
    actual = subprocess.check_output(
        ["/usr/bin/codesign", "--display", "--entitlements", "-", "--xml", str(app)],
        stderr=subprocess.DEVNULL,
    )
    require(plistlib.loads(actual) == expected, "actual App entitlement claims differ")
    for target in (app, app / HELPER):
        display = subprocess.run(
            ["/usr/bin/codesign", "--display", "--verbose=4", str(target)],
            capture_output=True, text=True, check=True,
        ).stderr
        require(any(line.startswith("Timestamp=") for line in display.splitlines()) == timestamped,
                "secure timestamp state differs")
        require("runtime)" in display and "Authority=Developer ID Application:" in display,
                "Developer ID hardened signature required")


def verify_requirement(target, requirement):
    subprocess.run(
        ["/usr/bin/codesign", "--verify", "--strict", "--test-requirement", "=" + requirement, str(target)],
        check=True, capture_output=True,
    )


def verify_input(manifest, proof_hash):
    recipe = module("build-crewd-production-sign-only.py")
    scope, selection, repo = recipe.prepare(manifest)
    for name in ("build-crewd-native-macos.py", "write-codex-first-provenance.py",
                 "crewd-validation-load-closure.py", "crewd-macho-seal-equivalence.py"):
        frozen = subprocess.check_output(["git", "show", selection["sourceRef"] + ":scripts/dev/" + name], cwd=repo)
        require(frozen == (HERE / name).read_bytes(), "qualification tool differs from frozen source: " + name)
    builder = module("build-crewd-native-macos.py")
    proof = module("write-codex-first-provenance.py")
    closure = module("crewd-validation-load-closure.py")
    seal = module("crewd-macho-seal-equivalence.py")
    original = scope / "signed-output"
    record_file = original / "sign-only-proof.json"
    record_bytes = record_file.read_bytes()
    require(hashlib.sha256(record_bytes).hexdigest() == proof_hash, "original sign-only proof SHA-256 differs")
    record = json.loads(record_bytes)
    app = original / "PendingCrew.xcarchive/Products/Applications/PendingCrew.app"
    require(app.resolve(strict=True) == app, "original private App path changed")
    require(record["sourceRef"] == selection["sourceRef"]
            and record["actualHelperCompiledRef"] == selection["backendCompiledRef"]
            and record["manifestSHA256"] == recipe.sha(manifest)
            and record["timestampMode"] == "none"
            and record["signingAction"] is True
            and record["staticAppleRequirementVerified"] is True
            and record["actualEntitlementsMatchExpected"] is True
            and all(record[k] is False for k in ("AppLaunched", "installed", "notarized", "published")),
            "original sign-only proof is not a matching private local-signing result")
    snapshot = original / "source-inputs"
    links = builder.committed_symlink_inputs(repo, selection["sourceRef"])
    source_rows = builder.regular_inventory(snapshot, links)
    require([r for r in source_rows if r["path"] != "Config/Local.xcconfig"]
            == [r for r in record["fullCommittedInputs"] if r["path"] != "Config/Local.xcconfig"]
            and recipe.sha(snapshot / "Config/Local.xcconfig") == record["explicitLocalOverrideSHA256"],
            "original committed input inventory changed")
    os.environ["PENDINGCREW_BUILD_BACKEND_PROFILE"] = selection["backendProfile"]
    rust_rows = [r for r in source_rows if r["path"].startswith("crewd/")]
    normal, _ = proof.verify_witness(
        original / "backend-witness.json", original / "crewd.pre-sign",
        proof.digest(rust_rows), proof.requested_configuration(snapshot / "crewd"), require_fresh=True,
    )
    require(normal["sourceRef"] == selection["backendCompiledRef"]
            and normal["binarySHA256"] == record["helperOriginalSHA256"], "exact backend witness differs")
    expected_file = original / "expected-distribution-entitlements.plist"
    expected_bytes = expected_file.read_bytes()
    expected = plistlib.loads(expected_bytes)
    profile = app / "Contents/embedded.provisionprofile"
    require(expected == record["expectedEntitlements"]
            and hashlib.sha256(expected_bytes).hexdigest() == record["expectedEntitlementsSHA256"]
            and recipe.sha(profile) == record["profileSHA256"] == selection["profileSHA256"],
            "original profile or exact entitlement claims changed")
    approved_profile = plistlib.loads(subprocess.run(
        ["/usr/bin/security", "cms", "-D", "-i", str(profile)], check=True, capture_output=True,
    ).stdout)
    expiry = approved_profile.get("ExpirationDate")
    require(approved_profile.get("UUID", "").lower() == selection["profileUUID"]
            and isinstance(expiry, datetime)
            and expiry.replace(tzinfo=expiry.tzinfo or timezone.utc) > datetime.now(timezone.utc),
            "reviewed profile UUID or expiry differs")
    template = plistlib.loads((snapshot / "Resources/PendingCrew.entitlements").read_bytes())
    require(recipe.distribution_entitlements(template, approved_profile, selection["team"], "com.pendingname.pendingcrew")
            == expected, "signed App entitlement claims differ from approved source expansion")
    require(recipe.sha(original / "settings.json") == record["buildSettingsSHA256"], "original build settings changed")
    recipe.qualify_product_info(plistlib.loads((app / "Contents/Info.plist").read_bytes()),
                                record["sourceRef"], record["buildNumber"], record["version"])
    require(builder.regular_inventory(app) == record["products"], "original signed App inventory changed")
    loaded = closure.audit(app)
    require(loaded == record["loadClosure"] and {row["path"] for row in loaded["images"]}
            == {str(HELPER), str(MAIN)}, "unexpected original loaded code closure")
    leaf = 'certificate leaf = H"' + selection["identitySHA1"].lower() + '"'
    exact_requirement = "(" + record["staticAppleRequirement"] + ") and " + leaf
    helper_requirement = "anchor apple generic and " + leaf
    checked_signature(app, exact_requirement, expected, timestamped=False)
    verify_requirement(app / HELPER, helper_requirement)
    reference = original / "seal-equivalence.json"
    require(recipe.sha(reference) == record["sealEquivalenceSHA256"]
            and read(reference) == {
                "GUI": seal.equivalent_files(original / "gui.archive-image", app / MAIN),
                "backend": seal.equivalent_files(original / "crewd.pre-sign", app / HELPER),
            }, "original GUI/Core sealing evidence differs")
    return scope, selection, app, record, expected, expected_bytes, exact_requirement, helper_requirement, proof_hash


def execute(manifest, prepared, out):
    scope, selection, app, record, expected, expected_bytes, exact_requirement, helper_requirement, proof_hash = prepared
    recipe = module("build-crewd-production-sign-only.py")
    builder = module("build-crewd-native-macos.py")
    closure = module("crewd-validation-load-closure.py")
    seal = module("crewd-macho-seal-equivalence.py")
    out.mkdir(mode=0o700)
    staged = out / "PendingCrew.app"
    expected_file = out / "expected-distribution-entitlements.plist"
    fd = os.open(expected_file, os.O_CREAT | os.O_EXCL | os.O_WRONLY, mode=0o600)
    with os.fdopen(fd, "wb") as handle:
        handle.write(expected_bytes)
    original = scope / "signed-output"
    builder.run(["/usr/bin/ditto", "--rsrc", "--extattr", "--acl", str(app), str(staged)], scope, out / "copy.log")
    require(builder.regular_inventory(staged) == record["products"]
            and builder.regular_inventory(app) == record["products"], "private copy differs from exact original App")
    checked_signature(staged, exact_requirement, expected, timestamped=False)
    verify_requirement(staged / HELPER, helper_requirement)
    require(recipe.sha(manifest) == record["manifestSHA256"]
            and recipe.sha(original / "sign-only-proof.json") == proof_hash
            and recipe.sha(expected_file) == record["expectedEntitlementsSHA256"], "pinned inputs changed before timestamp")
    builder.run(recipe.production_sign_argv(selection["identitySHA1"], staged / HELPER, "secure"),
                scope, out / "helper-timestamp.log")
    builder.run(recipe.production_sign_argv(selection["identitySHA1"], staged, "secure", expected_file),
                scope, out / "app-timestamp.log")
    checked_signature(staged, exact_requirement, expected, timestamped=True)
    verify_requirement(staged / HELPER, helper_requirement)
    equivalent = {
        "GUI": seal.equivalent_files(original / "gui.archive-image", staged / MAIN),
        "backend": seal.equivalent_files(original / "crewd.pre-sign", staged / HELPER),
    }
    products = builder.regular_inventory(staged)
    allowed = {str(MAIN), str(HELPER), str(SEAL)}
    require([r for r in products if r["path"] not in allowed]
            == [r for r in record["products"] if r["path"] not in allowed],
            "a non-signature App resource changed")
    loaded = closure.audit(staged)
    require({r["path"] for r in loaded["images"]} == {str(MAIN), str(HELPER)}, "timestamped code closure differs")
    require(builder.regular_inventory(app) == record["products"]
            and recipe.sha(manifest) == record["manifestSHA256"]
            and recipe.sha(original / "sign-only-proof.json") == proof_hash, "original App or pinned inputs changed")
    builder.save(out / "timestamp-proof.json", {
        "sourceRef": record["sourceRef"], "version": record["version"], "buildNumber": record["buildNumber"],
        "app": str(staged), "originalApp": str(app),
        "originalProofSHA256": proof_hash,
        "originalInventoryPreserved": True, "products": products, "sealEquivalence": equivalent,
        "loadClosure": loaded, "developerIdVerified": True, "secureTimestampsVerified": True,
        "recompiled": False, "uploaded": False, "notarized": False, "published": False, "installed": False,
    })


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("manifest", type=Path)
    parser.add_argument("--expected-proof-sha256", required=True, help="SHA-256 of the reviewed local sign-only proof")
    parser.add_argument("--execute", action="store_true", help="re-sign the private copy and request Apple timestamps")
    args = parser.parse_args()
    require(re.fullmatch(r"[0-9a-f]{64}", args.expected_proof_sha256), "exact lower-case proof SHA-256 required")
    prepared = verify_input(args.manifest, args.expected_proof_sha256)
    scope, _, app, record, *_ = prepared
    out = scope / "timestamped-output"
    require(not os.path.lexists(out), "fresh private output required")
    if not args.execute:
        print(json.dumps({"sourceRef": record["sourceRef"], "buildNumber": record["buildNumber"],
                          "originalApp": str(app), "originalProofSHA256": args.expected_proof_sha256,
                          "output": str(out), "verifiedOriginal": True,
                          "timestampRequested": False, "uploaded": False, "recompiled": False}, indent=2))
        return
    execute(args.manifest, prepared, out)
    print(f"Private timestamped App and proof: {out}")


if __name__ == "__main__":
    try:
        main()
    except subprocess.CalledProcessError as error:
        raise SystemExit(f"FAIL: local {Path(error.cmd[0]).name} check returned exit {error.returncode}") from None
    except (KeyError, ValueError, OSError) as error:
        raise SystemExit(f"FAIL: {error}") from None
