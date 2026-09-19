#!/usr/bin/env python3
"""Validate committed Obscura qualification manifests."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
from pathlib import Path


SHA256 = re.compile(r"^[0-9a-f]{64}$")
GIT_SHA = re.compile(r"^[0-9a-f]{40}$")
RESULT_STATUSES = {"passed", "failed", "skipped", "not-run"}
PLATFORMS = {"macos-arm64", "linux-x86_64"}
ROOT_COMMANDS = {
    "root-nextest-render",
    "root-build-render",
    "root-build-render-stealth",
    "root-build-no-render",
    "root-build-no-render-stealth",
    "obstacle-course",
}
CLIENT_CATEGORIES = {
    "connect",
    "context-page",
    "locator",
    "frame",
    "network",
    "file-artifact",
    "close-cancel",
}
CLIENT_PRIORITIES = {"required", "deferred", "unsupported"}
CLIENT_EXECUTION_PATHS = {"cdp", "driver", "host"}
CDP_CAPABILITY_STATUSES = {"SUPPORTED", "LIMITED", "VERIFIED_NOOP", "UNSUPPORTED"}
CDP_IMPLEMENTATION_STATES = {"implemented", "compatibility-ack", "fixed-value", "rejected"}
CDP_VERIFICATION_STATES = {"verified", "partial", "not-run"}
PLAYWRIGHT_SMOKE_METHODS = {
    "Browser.getVersion",
    "Browser.getWindowForTarget",
    "Browser.setWindowBounds",
    "Browser.setDownloadBehavior",
    "DOM.getContentQuads",
    "DOM.getDocument",
    "DOM.scrollIntoViewIfNeeded",
    "Emulation.setDeviceMetricsOverride",
    "Emulation.setEmulatedMedia",
    "Emulation.setFocusEmulationEnabled",
    "Input.dispatchMouseEvent",
    "Input.insertText",
    "Log.auditMethodDoesNotExist",
    "Log.enable",
    "Network.enable",
    "Network.getResponseBody",
    "Page.addScriptToEvaluateOnNewDocument",
    "Page.createIsolatedWorld",
    "Page.enable",
    "Page.getFrameTree",
    "Page.getLayoutMetrics",
    "Page.navigate",
    "Page.captureScreenshot",
    "Page.setLifecycleEventsEnabled",
    "Runtime.callFunctionOn",
    "Runtime.enable",
    "Runtime.evaluate",
    "Runtime.releaseObject",
    "Runtime.runIfWaitingForDebugger",
    "Target.attachToBrowserTarget",
    "Target.attachToTarget",
    "Target.createBrowserContext",
    "Target.createTarget",
    "Target.disposeBrowserContext",
    "Target.getTargetInfo",
    "Target.setAutoAttach",
}


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ValueError(message)


def load_json(path: Path) -> dict:
    value = json.loads(path.read_text(encoding="utf-8"))
    require(isinstance(value, dict), f"{path} must contain a JSON object")
    return value


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def locked_package_version(lock_text: str, package: str) -> str | None:
    for block in re.split(r"(?=^\[\[package\]\]$)", lock_text, flags=re.MULTILINE):
        name = re.search(r'^name\s*=\s*"([^"]+)"\s*$', block, re.MULTILINE)
        version = re.search(r'^version\s*=\s*"([^"]+)"\s*$', block, re.MULTILINE)
        if name is not None and name.group(1) == package and version is not None:
            return version.group(1)
    return None


def validate_result(result: object, location: str) -> None:
    require(isinstance(result, dict), f"{location} must be an object")
    require(result.get("status") in RESULT_STATUSES, f"{location} has invalid status")
    require(isinstance(result.get("command"), str), f"{location} needs a command")
    require(bool(result["command"].strip()), f"{location} command cannot be empty")
    require(isinstance(result.get("summary"), str), f"{location} needs a summary")
    if result["status"] == "not-run":
        require(bool(result.get("reason")), f"{location} not-run result needs a reason")


def validate_baseline(path: Path) -> None:
    value = load_json(path)
    repository = path.resolve().parents[2]
    require(value.get("schema_version") == 1, "unsupported baseline schema_version")
    require(value.get("task") == "OB-001", "baseline task must be OB-001")

    source = value.get("source")
    require(isinstance(source, dict), "baseline source must be an object")
    require(bool(GIT_SHA.fullmatch(str(source.get("revision", "")))), "invalid source revision")
    for key in ("cargo_lock_sha256", "runtime_cargo_lock_sha256"):
        require(bool(SHA256.fullmatch(str(source.get(key, "")))), f"invalid {key}")
    require(
        source["cargo_lock_sha256"] == sha256(repository / "Cargo.lock"),
        "Cargo.lock digest does not match the repository",
    )
    require(
        source["runtime_cargo_lock_sha256"] == sha256(repository / "runtime" / "Cargo.lock"),
        "runtime/Cargo.lock digest does not match the repository",
    )

    benchmark = value.get("benchmark")
    require(isinstance(benchmark, dict), "baseline benchmark must be an object")
    require(bool(GIT_SHA.fullmatch(str(benchmark.get("revision", "")))), "invalid benchmark revision")
    benchmark_repository = str(benchmark.get("repository", ""))
    require(
        benchmark_repository.startswith("https://github.com/") and not benchmark_repository.endswith("/"),
        "benchmark repository must be a canonical GitHub URL",
    )
    benchmark_slug = benchmark_repository.removeprefix("https://github.com/")
    ci_text = (repository / ".github" / "workflows" / "ci.yml").read_text(encoding="utf-8")
    require(
        f"repository: {benchmark_slug}" in ci_text,
        "benchmark repository does not match CI",
    )
    require(
        f"ref: {benchmark['revision']}" in ci_text,
        "benchmark revision does not match CI",
    )

    toolchains = value.get("toolchains")
    require(isinstance(toolchains, dict), "baseline toolchains must be an object")
    for workspace in ("root", "runtime"):
        toolchain = toolchains.get(workspace)
        require(isinstance(toolchain, dict), f"missing {workspace} toolchain")
        require(toolchain.get("rust") == "1.98.1", f"{workspace} Rust must be pinned to 1.98.1")
        manifest = repository / str(toolchain.get("manifest", ""))
        require(manifest.is_file(), f"missing {workspace} toolchain manifest: {manifest}")
        manifest_text = manifest.read_text(encoding="utf-8")
        require(
            re.search(r'^channel\s*=\s*"1\.98\.1"\s*$', manifest_text, re.MULTILINE) is not None,
            f"{workspace} toolchain manifest does not pin Rust 1.98.1",
        )
    release_text = (repository / ".github" / "workflows" / "release.yml").read_text(encoding="utf-8")
    require("toolchain: 1.98.1" in ci_text, "CI does not pin Rust 1.98.1")
    require("toolchain: 1.98.1" in release_text, "release workflow does not pin Rust 1.98.1")

    configurations = value.get("configurations")
    require(isinstance(configurations, list), "baseline configurations must be a list")
    by_platform = {entry.get("id"): entry for entry in configurations if isinstance(entry, dict)}
    require(set(by_platform) == PLATFORMS, "baseline must cover macos-arm64 and linux-x86_64")
    for platform_id, configuration in by_platform.items():
        environment = configuration.get("environment")
        require(isinstance(environment, dict), f"{platform_id} needs environment metadata")
        for key in ("os", "architecture", "ca", "fonts"):
            require(bool(environment.get(key)), f"{platform_id} environment needs {key}")
        results = configuration.get("results")
        require(isinstance(results, list), f"{platform_id} results must be a list")
        by_id = {entry.get("id"): entry for entry in results if isinstance(entry, dict)}
        require(ROOT_COMMANDS <= set(by_id), f"{platform_id} is missing required root commands")
        require("runtime-nextest" in by_id, f"{platform_id} is missing runtime-nextest")
        for result_id, result in by_id.items():
            validate_result(result, f"{platform_id}.{result_id}")


def validate_client_scope(path: Path) -> None:
    value = load_json(path)
    repository = path.resolve().parents[2]
    require(value.get("schema_version") == 1, "unsupported client scope schema_version")
    require(value.get("task") == "OB-025", "client scope task must be OB-025")

    client = value.get("client")
    require(isinstance(client, dict), "client metadata must be an object")
    require(client.get("package") == "playwright", "official package must be playwright")
    client_version = str(client.get("version", ""))
    require(bool(re.fullmatch(r"\d+\.\d+\.\d+", client_version)), "client version must be exact")
    require(client.get("unmodified") is True, "official client must remain unmodified")
    require(client.get("connection") == "BrowserType.connect_over_cdp", "connection must use connect_over_cdp")
    require(client.get("dependency_scope") == "development-only", "client must remain development-only")
    lockfile = repository / str(client.get("lockfile", ""))
    require(lockfile.is_file(), f"missing client lockfile: {lockfile}")
    require(
        bool(SHA256.fullmatch(str(client.get("lockfile_sha256", "")))),
        "invalid client lockfile_sha256",
    )
    require(
        client["lockfile_sha256"] == sha256(lockfile),
        "client lockfile digest does not match",
    )
    lock_text = lockfile.read_text(encoding="utf-8")
    require(
        locked_package_version(lock_text, "playwright") == client_version,
        "client version does not match its lockfile",
    )
    reference_browser = client.get("reference_browser")
    require(isinstance(reference_browser, dict), "reference_browser must be an object")
    require(reference_browser.get("name") == "chromium", "reference browser must be Chromium")
    require(bool(str(reference_browser.get("revision", "")).strip()), "reference browser revision is required")
    require(bool(str(reference_browser.get("version", "")).strip()), "reference browser version is required")

    boundaries = value.get("boundaries")
    require(isinstance(boundaries, dict), "client boundaries must be an object")
    require(boundaries.get("launch") == "host", "Obscura launch must remain a host responsibility")
    require(boundaries.get("connect") == "cdp", "Playwright must connect over CDP")
    require(boundaries.get("api_request_context") == "driver-network", "APIRequestContext boundary must be explicit")
    require(boundaries.get("route_fetch") == "driver-network", "route.fetch boundary must be explicit")

    capabilities = value.get("capabilities")
    require(isinstance(capabilities, list), "client capabilities must be a list")
    require(bool(capabilities), "client capabilities cannot be empty")
    categories = set()
    api_names = set()
    for index, capability in enumerate(capabilities):
        location = f"capabilities[{index}]"
        require(isinstance(capability, dict), f"{location} must be an object")
        category = capability.get("category")
        api = capability.get("api")
        require(category in CLIENT_CATEGORIES, f"{location} has invalid category")
        require(isinstance(api, str) and bool(api.strip()), f"{location} needs an API name")
        require(api not in api_names, f"duplicate API scope entry: {api}")
        require(capability.get("priority") in CLIENT_PRIORITIES, f"{location} has invalid priority")
        require(capability.get("execution_path") in CLIENT_EXECUTION_PATHS, f"{location} has invalid execution path")
        require(bool(capability.get("acceptance")), f"{location} needs acceptance criteria")
        require(bool(capability.get("boundary")), f"{location} needs a boundary")
        categories.add(category)
        api_names.add(api)
    require(categories == CLIENT_CATEGORIES, "client scope must cover every migration category")
    require("BrowserType.connect_over_cdp" in api_names, "client scope must include connect_over_cdp")
    require("APIRequestContext" in api_names, "client scope must include APIRequestContext")
    require("Route.fetch" in api_names, "client scope must include route.fetch")


def validate_automation_profile(path: Path) -> None:
    value = load_json(path)
    require(value.get("schema_version") == 1, "unsupported automation profile schema_version")
    require(value.get("task") == "OB-027", "automation profile task must be OB-027")

    client = value.get("client")
    require(isinstance(client, dict), "automation profile client must be an object")
    require(client.get("package") == "playwright", "automation profile client must be playwright")
    require(client.get("version") == "1.60.0", "automation profile client version must be pinned")
    require(
        client.get("connection") == "BrowserType.connect_over_cdp",
        "automation profile must use connect_over_cdp",
    )

    coverage = value.get("coverage")
    require(isinstance(coverage, dict), "automation profile coverage must be an object")
    require(
        coverage.get("kind") == "observed-official-client-slice",
        "automation profile must declare its observed slice",
    )
    require(bool(coverage.get("source")), "automation profile coverage needs a source")
    require(
        coverage.get("unlisted") == "not-qualified",
        "unlisted automation methods must remain not-qualified",
    )
    require(bool(coverage.get("completion")), "automation profile coverage needs completion state")

    methods = value.get("methods")
    require(isinstance(methods, list) and methods, "automation profile methods cannot be empty")
    names = set()
    statuses = set()
    for index, method in enumerate(methods):
        location = f"methods[{index}]"
        require(isinstance(method, dict), f"{location} must be an object")
        name = method.get("method")
        require(
            isinstance(name, str) and re.fullmatch(r"[A-Za-z]+\.[A-Za-z]+", name) is not None,
            f"{location} needs a qualified method name",
        )
        require(name not in names, f"duplicate automation profile method: {name}")
        status = method.get("capability")
        require(status in CDP_CAPABILITY_STATUSES, f"{location} has invalid capability")
        require(
            method.get("implementation") in CDP_IMPLEMENTATION_STATES,
            f"{location} has invalid implementation state",
        )
        require(
            method.get("verification") in CDP_VERIFICATION_STATES,
            f"{location} has invalid verification state",
        )
        for field in ("params", "result", "events", "scope", "errors", "evidence"):
            require(
                isinstance(method.get(field), str) and bool(method[field].strip()),
                f"{location} needs {field}",
            )
        names.add(name)
        statuses.add(status)

    require(
        {"LIMITED", "VERIFIED_NOOP", "UNSUPPORTED"} <= statuses,
        "automation profile must distinguish limited, verified no-op, and unsupported methods",
    )
    require(
        PLAYWRIGHT_SMOKE_METHODS <= names,
        "automation profile is missing official smoke methods",
    )

    negative = value.get("negative_contract")
    require(isinstance(negative, dict), "automation profile needs negative_contract")
    require(
        negative.get("observed_unknown_method_probe") == "error",
        "the observed unknown-method probe must error",
    )
    require(
        negative.get("exact_initializer_outside_allowlist") == "error",
        "exact initializers outside their allowlist must error",
    )
    require(
        negative.get("other_invalid_params") == "not-qualified",
        "unvalidated invalid params must remain not-qualified",
    )
    require(
        negative.get("unlisted") == "not-qualified",
        "unlisted negative behavior must remain not-qualified",
    )


def validate_protocol_log(profile_path: Path, log_path: Path) -> int:
    validate_automation_profile(profile_path)
    profile = load_json(profile_path)
    profiled = {method["method"] for method in profile["methods"]}
    observed = set()
    marker = "pw:protocol SEND"
    for line_number, line in enumerate(log_path.read_text(encoding="utf-8").splitlines(), 1):
        if marker not in line:
            continue
        payload_start = line.find("{", line.find(marker) + len(marker))
        require(payload_start >= 0, f"protocol log line {line_number} has no JSON payload")
        try:
            payload, _ = json.JSONDecoder().raw_decode(line[payload_start:])
        except json.JSONDecodeError as error:
            raise ValueError(
                f"protocol log line {line_number} has invalid JSON: {error.msg}"
            ) from error
        require(isinstance(payload, dict), f"protocol log line {line_number} payload must be an object")
        method = payload.get("method")
        require(
            isinstance(method, str) and re.fullmatch(r"[A-Za-z]+\.[A-Za-z]+", method) is not None,
            f"protocol log line {line_number} needs a qualified method",
        )
        observed.add(method)

    require(bool(observed), "protocol log contains no Playwright SEND entries")
    missing_from_log = sorted(PLAYWRIGHT_SMOKE_METHODS - observed)
    require(
        not missing_from_log,
        "protocol log is missing required smoke methods: " + ", ".join(missing_from_log),
    )
    unprofiled = sorted(observed - profiled)
    require(
        not unprofiled,
        "protocol log contains unprofiled methods: " + ", ".join(unprofiled),
    )
    return len(observed)


def main() -> None:
    parser = argparse.ArgumentParser()
    subparsers = parser.add_subparsers(dest="command", required=True)
    baseline = subparsers.add_parser("baseline")
    baseline.add_argument("path", type=Path)
    client = subparsers.add_parser("client")
    client.add_argument("path", type=Path)
    profile = subparsers.add_parser("profile")
    profile.add_argument("path", type=Path)
    protocol_log = subparsers.add_parser("protocol-log")
    protocol_log.add_argument("profile", type=Path)
    protocol_log.add_argument("log", type=Path)
    args = parser.parse_args()

    try:
        if args.command == "baseline":
            validate_baseline(args.path)
            message = "OB-001 baseline manifest is valid"
        elif args.command == "client":
            validate_client_scope(args.path)
            message = "OB-025 client scope is valid"
        elif args.command == "profile":
            validate_automation_profile(args.path)
            message = "OB-027 automation profile is valid"
        else:
            count = validate_protocol_log(args.profile, args.log)
            message = f"OB-027 protocol log matches automation profile ({count} methods)"
    except (OSError, json.JSONDecodeError, ValueError) as error:
        parser.exit(1, f"error: {error}\n")
    print(message)


if __name__ == "__main__":
    main()
