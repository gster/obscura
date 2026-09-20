from __future__ import annotations

import hashlib
import json
import subprocess
import sys
import tempfile
import unittest
from contextlib import contextmanager
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

    @contextmanager
    def source_repository(self):
        with tempfile.TemporaryDirectory() as temporary:
            repository = Path(temporary)
            (repository / "runtime").mkdir()
            (repository / ".github" / "workflows").mkdir(parents=True)
            (repository / "tools" / "unblocked").mkdir(parents=True)
            (repository / "Cargo.lock").write_bytes(b"historical root lock\n")
            (repository / "runtime" / "Cargo.lock").write_bytes(
                b"historical runtime lock\n"
            )
            toolchain = '[toolchain]\nchannel = "1.98.1"\n'
            (repository / "rust-toolchain.toml").write_text(toolchain, encoding="utf-8")
            (repository / "runtime" / "rust-toolchain.toml").write_text(
                toolchain, encoding="utf-8"
            )
            baseline = json.loads(
                (TOOL_ROOT / "baseline.json").read_text(encoding="utf-8")
            )
            benchmark_slug = baseline["benchmark"]["repository"].removeprefix(
                "https://github.com/"
            )
            ci = (
                f"repository: {benchmark_slug}\n"
                f"ref: {baseline['benchmark']['revision']}\n"
                "toolchain: 1.98.1\n"
            )
            (repository / ".github" / "workflows" / "ci.yml").write_text(
                ci, encoding="utf-8"
            )
            (repository / ".github" / "workflows" / "release.yml").write_text(
                "toolchain: 1.98.1\n", encoding="utf-8"
            )
            baseline["source"]["revision"] = "0" * 40
            for relative_path, key in (
                ("Cargo.lock", "cargo_lock_sha256"),
                ("runtime/Cargo.lock", "runtime_cargo_lock_sha256"),
            ):
                baseline["source"][key] = hashlib.sha256(
                    (repository / relative_path).read_bytes()
                ).hexdigest()
            subprocess.run(["git", "init", "-q"], cwd=repository, check=True)
            subprocess.run(
                ["git", "config", "user.email", "tests@example.invalid"],
                cwd=repository,
                check=True,
            )
            subprocess.run(
                ["git", "config", "user.name", "Validation Tests"],
                cwd=repository,
                check=True,
            )
            subprocess.run(["git", "add", "."], cwd=repository, check=True)
            subprocess.run(
                ["git", "commit", "-qm", "historical baseline"],
                cwd=repository,
                check=True,
            )
            revision = subprocess.run(
                ["git", "rev-parse", "HEAD"],
                cwd=repository,
                check=True,
                capture_output=True,
                text=True,
            ).stdout.strip()
            baseline["source"]["revision"] = revision
            manifest = repository / "tools" / "unblocked" / "baseline.json"
            manifest.write_text(json.dumps(baseline), encoding="utf-8")
            yield repository, manifest, baseline

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

    def test_baseline_rejects_a_tampered_source_lock_digest(self) -> None:
        result = self.validate_mutation(
            "baseline",
            "baseline.json",
            lambda value: value["source"].update(cargo_lock_sha256="0" * 64),
        )

        self.assertEqual(result.returncode, 1)
        self.assertIn("source Cargo.lock digest does not match", result.stderr)

    def test_baseline_reads_historical_blobs_after_worktree_locks_change(self) -> None:
        with self.source_repository() as (repository, manifest, _):
            (repository / "Cargo.lock").write_bytes(b"mutated working tree lock\n")
            (repository / "runtime" / "Cargo.lock").write_bytes(
                b"mutated working tree runtime lock\n"
            )
            result = self.run_validator("baseline", str(manifest))

        self.assertEqual(result.returncode, 0, result.stderr)

    def test_baseline_rejects_a_tampered_historical_digest(self) -> None:
        with self.source_repository() as (repository, manifest, baseline):
            baseline["source"]["cargo_lock_sha256"] = "0" * 64
            manifest.write_text(json.dumps(baseline), encoding="utf-8")
            result = self.run_validator("baseline", str(manifest))

        self.assertEqual(result.returncode, 1)
        self.assertIn("source Cargo.lock digest does not match", result.stderr)

    def test_baseline_rejects_an_unknown_source_revision(self) -> None:
        with self.source_repository() as (repository, manifest, baseline):
            baseline["source"]["revision"] = "f" * 40
            manifest.write_text(json.dumps(baseline), encoding="utf-8")
            result = self.run_validator("baseline", str(manifest))

        self.assertEqual(result.returncode, 1)
        self.assertIn("source revision is unavailable", result.stderr)

    def test_baseline_rejects_a_missing_historical_lock_blob(self) -> None:
        with self.source_repository() as (repository, manifest, baseline):
            (repository / "runtime" / "Cargo.lock").unlink()
            subprocess.run(["git", "add", "-u"], cwd=repository, check=True)
            subprocess.run(
                ["git", "commit", "-qm", "remove historical runtime lock"],
                cwd=repository,
                check=True,
            )
            baseline["source"]["revision"] = subprocess.run(
                ["git", "rev-parse", "HEAD"],
                cwd=repository,
                check=True,
                capture_output=True,
                text=True,
            ).stdout.strip()
            manifest.write_text(json.dumps(baseline), encoding="utf-8")
            result = self.run_validator("baseline", str(manifest))

        self.assertEqual(result.returncode, 1)
        self.assertIn("source blob is unavailable", result.stderr)

    def test_baseline_rejects_a_current_toolchain_manifest_with_wrong_version(self) -> None:
        with self.source_repository() as (repository, manifest, _):
            (repository / "rust-toolchain.toml").write_text(
                '[toolchain]\nchannel = "1.97.0"\n', encoding="utf-8"
            )
            result = self.run_validator("baseline", str(manifest))

        self.assertEqual(result.returncode, 1)
        self.assertIn("root toolchain manifest does not pin Rust 1.98.1", result.stderr)

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
