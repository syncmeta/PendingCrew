#!/usr/bin/env python3
"""Offline contract for reusing one exact App and timestamping only its copy."""
import contextlib
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import plistlib
import subprocess
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location(
    "reseal", Path(__file__).with_name("reseal-crewd-production-timestamp.py"),
)
reseal = importlib.util.module_from_spec(spec)
spec.loader.exec_module(reseal)


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


class TimestampContract(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.scope = Path(temp.name)
        self.manifest = self.scope / "manifest.json"
        self.manifest.write_text("{}")
        self.original = self.scope / "signed-output"
        self.original.mkdir()
        self.proof = self.original / "sign-only-proof.json"
        self.proof.write_text("{}")
        self.out = self.scope / "timestamped-output"
        self.app = self.original / "PendingCrew.app"
        self.entitlements = {"keychain-access-groups": ["TESTPREFIX.pendingcrew"]}
        self.claims = plistlib.dumps(self.entitlements)
        self.products = [{"path": "Contents/Info.plist", "sha256": "immutable"}]
        self.record = {
            "sourceRef": "a" * 40, "version": "0.2.3", "buildNumber": "1.00001",
            "products": self.products, "manifestSHA256": sha(self.manifest),
            "expectedEntitlementsSHA256": hashlib.sha256(self.claims).hexdigest(),
        }
        self.prepared = (
            self.scope, {"identitySHA1": "c" * 40}, self.app, self.record,
            self.entitlements, self.claims, "exact App certificate", "exact helper certificate", sha(self.proof),
        )

    def test_default_cli_is_read_only_and_pins_reviewed_proof(self):
        args = ["reseal", str(self.manifest), "--expected-proof-sha256", sha(self.proof)]
        with patch.object(sys, "argv", args), patch.object(reseal, "verify_input", return_value=self.prepared) as verify:
            with patch.object(reseal, "execute") as execute, contextlib.redirect_stdout(io.StringIO()) as stdout:
                reseal.main()
        verify.assert_called_once_with(self.manifest, sha(self.proof))
        execute.assert_not_called()
        result = json.loads(stdout.getvalue())
        self.assertFalse(result["timestampRequested"] or result["recompiled"] or result["uploaded"])
        self.assertFalse(self.out.exists())

    def test_mismatched_signing_proof_is_rejected_before_any_security_operation(self):
        tools = Path(reseal.HERE)
        recipe = SimpleNamespace(prepare=lambda _: (self.scope, {"sourceRef": "a" * 40}, self.scope))

        def frozen_bytes(argv, **_):
            return (tools / argv[-1].split("/")[-1]).read_bytes()

        with patch.object(reseal, "module", return_value=recipe), patch.object(reseal.subprocess, "run") as runner:
            with patch.object(reseal.subprocess, "check_output", side_effect=frozen_bytes):
                with self.assertRaisesRegex(ValueError, "proof SHA-256 differs"):
                    reseal.verify_input(self.manifest, "0" * 64)
        runner.assert_not_called()
        self.assertFalse(self.out.exists())

    def run_isolated(self, copied_products):
        commands = []
        saved = {}
        recipe = reseal.module("build-crewd-production-sign-only.py")

        def run(argv, _cwd, _log):
            commands.append(argv)

        modules = {
            "build-crewd-production-sign-only.py": recipe,
            "build-crewd-native-macos.py": SimpleNamespace(
                run=run, save=lambda _, report: saved.update(report),
                regular_inventory=lambda target: self.products if target == self.app else copied_products,
            ),
            "crewd-validation-load-closure.py": SimpleNamespace(audit=lambda _: {
                "images": [{"path": str(reseal.MAIN)}, {"path": str(reseal.HELPER)}],
            }),
            "crewd-macho-seal-equivalence.py": SimpleNamespace(equivalent_files=lambda source, _: source.name),
        }
        with patch.object(reseal, "module", side_effect=modules.__getitem__):
            with patch.object(reseal, "checked_signature") as check, patch.object(reseal, "verify_requirement") as requirement:
                try:
                    reseal.execute(self.manifest, self.prepared, self.out)
                except ValueError as error:
                    return commands, saved, check, requirement, error
        return commands, saved, check, requirement, None

    def test_wrong_private_copy_never_reaches_timestamp(self):
        commands, saved, check, requirement, error = self.run_isolated([])
        self.assertRegex(str(error), "private copy differs")
        self.assertEqual([args[0] for args in commands], ["/usr/bin/ditto"])
        self.assertFalse(saved)
        check.assert_not_called()
        requirement.assert_not_called()

    def test_exact_inputs_sign_inner_then_outer_with_exact_private_claims(self):
        commands, saved, check, requirement, error = self.run_isolated(self.products)
        self.assertIsNone(error)
        staged = self.out / "PendingCrew.app"
        claims = self.out / "expected-distribution-entitlements.plist"
        sign = ["codesign", "--force", "--sign", "c" * 40, "--options", "runtime", "--timestamp"]
        self.assertEqual(commands[1:], [
            sign + [str(staged / reseal.HELPER)], sign + ["--entitlements", str(claims), str(staged)],
        ])
        self.assertEqual(claims.read_bytes(), self.claims)
        self.assertEqual(claims.stat().st_mode & 0o777, 0o600)
        self.assertEqual([call.kwargs for call in check.call_args_list], [{"timestamped": False}, {"timestamped": True}])
        self.assertEqual(requirement.call_count, 2)
        self.assertEqual(saved["originalProofSHA256"], sha(self.proof))
        self.assertTrue(saved["originalInventoryPreserved"] and saved["secureTimestampsVerified"])
        self.assertFalse(any(saved[key] for key in ("recompiled", "uploaded", "notarized", "published", "installed")))

    def test_signed_entitlement_difference_is_a_hard_failure(self):
        with patch.object(reseal.subprocess, "run", return_value=subprocess.CompletedProcess([], 0)):
            with patch.object(reseal.subprocess, "check_output", return_value=plistlib.dumps({})):
                with self.assertRaisesRegex(ValueError, "entitlement claims differ"):
                    reseal.checked_signature(self.app, "exact", self.entitlements, timestamped=False)


if __name__ == "__main__":
    unittest.main()
