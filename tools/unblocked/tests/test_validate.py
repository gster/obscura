from __future__ import annotations

import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[3]
TOOL_ROOT = ROOT / "tools" / "unblocked"


class ManifestValidationTests(unittest.TestCase):
    def run_validator(self, *arguments: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [sys.executable, str(TOOL_ROOT / "validate.py"), *arguments],
            cwd=ROOT,
            text=True,
            capture_output=True,
            check=False,
        )

    def validate_mutation(self, command: str, filename: str, mutate) -> subprocess.CompletedProcess[str]:
        value = json.loads((TOOL_ROOT / filename).read_text(encoding="utf-8"))
        mutate(value)
        with tempfile.NamedTemporaryFile(
            mode="w",
            suffix=".json",
            dir=TOOL_ROOT,
            encoding="utf-8",
        ) as manifest:
            json.dump(value, manifest)
            manifest.flush()
            return self.run_validator(command, manifest.name)

    def protocol_log(self, methods: list[str], extra_lines: list[str] | None = None):
        lines = list(extra_lines or [])
        lines.extend(
            f'2026-09-19T00:00:00Z pw:protocol SEND ► {json.dumps({"id": index, "method": method, "params": {"raw": "complete-value"}})}'
            for index, method in enumerate(methods, 1)
        )
        return tempfile.NamedTemporaryFile(
            mode="w+",
            suffix=".log",
            encoding="utf-8",
            delete=False,
        ), "\n".join(lines) + "\n"

    def test_committed_baseline_is_complete_and_reproducible(self) -> None:
        result = self.run_validator(
            "baseline",
            str(TOOL_ROOT / "baseline.json"),
        )

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout, "OB-001 baseline manifest is valid\n")

    def test_official_client_scope_covers_the_migration_boundary(self) -> None:
        result = self.run_validator(
            "client",
            str(TOOL_ROOT / "client-scope.json"),
        )

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout, "OB-025 client scope is valid\n")

    def test_automation_profile_separates_capability_from_evidence(self) -> None:
        result = self.run_validator(
            "profile",
            str(TOOL_ROOT / "automation-cdp-profile.json"),
        )

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout, "OB-027 automation profile is valid\n")

    def test_baseline_rejects_a_lock_digest_that_does_not_match_the_repository(self) -> None:
        result = self.validate_mutation(
            "baseline",
            "baseline.json",
            lambda value: value["source"].update(cargo_lock_sha256="0" * 64),
        )

        self.assertEqual(result.returncode, 1)
        self.assertIn("Cargo.lock digest does not match", result.stderr)

    def test_baseline_rejects_a_benchmark_revision_that_does_not_match_ci(self) -> None:
        result = self.validate_mutation(
            "baseline",
            "baseline.json",
            lambda value: value["benchmark"].update(revision="0" * 40),
        )

        self.assertEqual(result.returncode, 1)
        self.assertIn("benchmark revision does not match CI", result.stderr)

    def test_client_scope_rejects_an_incomplete_api_category_inventory(self) -> None:
        def remove_frames(value: dict) -> None:
            value["capabilities"] = [
                item for item in value["capabilities"] if item["category"] != "frame"
            ]

        result = self.validate_mutation(
            "client",
            "client-scope.json",
            remove_frames,
        )

        self.assertEqual(result.returncode, 1)
        self.assertIn("every migration category", result.stderr)

    def test_client_scope_rejects_a_lock_digest_that_does_not_match(self) -> None:
        result = self.validate_mutation(
            "client",
            "client-scope.json",
            lambda value: value["client"].update(lockfile_sha256="0" * 64),
        )

        self.assertEqual(result.returncode, 1)
        self.assertIn("client lockfile digest does not match", result.stderr)

    def test_client_scope_rejects_a_version_that_does_not_match_the_lock(self) -> None:
        result = self.validate_mutation(
            "client",
            "client-scope.json",
            lambda value: value["client"].update(version="9.9.9"),
        )

        self.assertEqual(result.returncode, 1)
        self.assertIn("client version does not match its lockfile", result.stderr)

    def test_automation_profile_rejects_duplicate_methods(self) -> None:
        def duplicate_method(value: dict) -> None:
            value["methods"].append(dict(value["methods"][0]))

        result = self.validate_mutation(
            "profile",
            "automation-cdp-profile.json",
            duplicate_method,
        )

        self.assertEqual(result.returncode, 1)
        self.assertIn("duplicate automation profile method", result.stderr)

    def test_automation_profile_rejects_missing_negative_contract(self) -> None:
        result = self.validate_mutation(
            "profile",
            "automation-cdp-profile.json",
            lambda value: value["negative_contract"].update(other_invalid_params="error"),
        )

        self.assertEqual(result.returncode, 1)
        self.assertIn("unvalidated invalid params must remain not-qualified", result.stderr)

    def test_automation_profile_rejects_global_claims_for_unlisted_methods(self) -> None:
        result = self.validate_mutation(
            "profile",
            "automation-cdp-profile.json",
            lambda value: value["coverage"].update(unlisted="supported"),
        )

        self.assertEqual(result.returncode, 1)
        self.assertIn("unlisted automation methods must remain not-qualified", result.stderr)

    def test_protocol_log_methods_are_reconciled_without_changing_raw_payloads(self) -> None:
        profile = json.loads(
            (TOOL_ROOT / "automation-cdp-profile.json").read_text(encoding="utf-8")
        )
        methods = [method["method"] for method in profile["methods"]]
        log, text = self.protocol_log(methods, ["unrelated complete log line"])
        try:
            log.write(text)
            log.close()
            result = self.run_validator(
                "protocol-log",
                str(TOOL_ROOT / "automation-cdp-profile.json"),
                log.name,
            )
            self.assertEqual(Path(log.name).read_text(encoding="utf-8"), text)
        finally:
            Path(log.name).unlink(missing_ok=True)

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("protocol log matches automation profile", result.stdout)

    def test_protocol_log_rejects_a_truncated_smoke_inventory(self) -> None:
        log, text = self.protocol_log(["Browser.getVersion"])
        try:
            log.write(text)
            log.close()
            result = self.run_validator(
                "protocol-log",
                str(TOOL_ROOT / "automation-cdp-profile.json"),
                log.name,
            )
        finally:
            Path(log.name).unlink(missing_ok=True)

        self.assertEqual(result.returncode, 1)
        self.assertIn("protocol log is missing required smoke methods", result.stderr)

    def test_protocol_log_rejects_an_unprofiled_method(self) -> None:
        profile = json.loads(
            (TOOL_ROOT / "automation-cdp-profile.json").read_text(encoding="utf-8")
        )
        methods = [method["method"] for method in profile["methods"]]
        methods.append("Page.newUnqualifiedMethod")
        log, text = self.protocol_log(methods)
        try:
            log.write(text)
            log.close()
            result = self.run_validator(
                "protocol-log",
                str(TOOL_ROOT / "automation-cdp-profile.json"),
                log.name,
            )
        finally:
            Path(log.name).unlink(missing_ok=True)

        self.assertEqual(result.returncode, 1)
        self.assertIn("protocol log contains unprofiled methods", result.stderr)


if __name__ == "__main__":
    unittest.main()
