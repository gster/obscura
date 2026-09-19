#!/usr/bin/env python3
"""Record and compare a small, deterministic CDP exchange."""

from __future__ import annotations

import argparse
import copy
import json
import re
import time
from pathlib import Path
from typing import Any, Iterable


_ID_KEYS = {
    "backendNodeId",
    "browserContextId",
    "connectionId",
    "executionContextId",
    "frameId",
    "loaderId",
    "nodeId",
    "objectId",
    "requestId",
    "sessionId",
    "scriptId",
    "targetId",
    "uniqueId",
}
_ID_KEY_ALIASES = {
    "parentFrameId": "frameId",
    "openerFrameId": "frameId",
}
_PARENT_ID_KEYS = {
    "context": "executionContextId",
    "executionContext": "executionContextId",
    "frame": "frameId",
    "request": "requestId",
    "targetInfo": "targetId",
}
_ABSOLUTE_TIME_KEYS = {"requestTime", "timestamp", "wallTime"}
_PHASE_TIME_KEYS = {
    "connectEnd",
    "connectStart",
    "dnsEnd",
    "dnsStart",
    "proxyEnd",
    "proxyStart",
    "pushEnd",
    "pushStart",
    "receiveHeadersEnd",
    "receiveHeadersStart",
    "sendEnd",
    "sendStart",
    "sslEnd",
    "sslStart",
    "workerFetchStart",
    "workerReady",
    "workerRespondWithSettled",
    "workerStart",
}
_RAW_SUBTREE_KEYS = {"body", "expression", "headers", "postData", "value"}
_URL_KEYS = {"baseURL", "documentURL", "origin", "securityOrigin", "url"}
_PLAYWRIGHT_WORLD = re.compile(r"(__playwright_utility_world_[^@]+)@[0-9a-f]+")


class Normalizer:
    """Replace only run-specific identifiers and timing with stable tokens."""

    def __init__(self, fixture_origin: str):
        self.fixture_origin = fixture_origin.rstrip("/")
        self._aliases: dict[str, dict[str, str]] = {}

    def _alias(self, kind: str, value: Any) -> Any:
        if not isinstance(value, (str, int)):
            return value
        raw = str(value)
        aliases = self._aliases.setdefault(kind, {})
        if raw not in aliases:
            aliases[raw] = f"<{kind}:{len(aliases) + 1}>"
        return aliases[raw]

    def _url(self, value: str) -> str:
        if self.fixture_origin and value.startswith(self.fixture_origin):
            return "<fixture>" + value[len(self.fixture_origin) :]
        return value

    def normalize(self, value: Any, *, parent_key: str | None = None) -> Any:
        if isinstance(value, dict):
            normalized = {}
            for key, item in value.items():
                key_text = str(key)
                if key_text in _RAW_SUBTREE_KEYS:
                    normalized[key_text] = copy.deepcopy(item)
                elif key_text in _ABSOLUTE_TIME_KEYS:
                    normalized[key_text] = "<protocol-time>"
                elif key_text in _PHASE_TIME_KEYS:
                    if type(item) in (int, float) and item < 0:
                        normalized[key_text] = item
                    else:
                        normalized[key_text] = "<protocol-time>"
                elif key_text in _ID_KEYS:
                    normalized[key_text] = self._alias(key_text, item)
                elif key_text in _ID_KEY_ALIASES:
                    normalized[key_text] = self._alias(_ID_KEY_ALIASES[key_text], item)
                elif key_text == "id" and parent_key in _PARENT_ID_KEYS:
                    normalized[key_text] = self._alias(_PARENT_ID_KEYS[parent_key], item)
                else:
                    normalized[key_text] = self.normalize(item, parent_key=key_text)
            return normalized
        if isinstance(value, list):
            return [self.normalize(item, parent_key=parent_key) for item in value]
        if isinstance(value, str):
            if parent_key in _URL_KEYS and value.startswith(("http://", "https://")):
                return self._url(value)
            if parent_key == "name":
                return _PLAYWRIGHT_WORLD.sub(r"\1@<id>", value)
            return value
        return value


class Trace:
    def __init__(self, mode: str, fixture_origin: str):
        self.mode = mode
        self.fixture_origin = fixture_origin
        self.entries: list[dict[str, Any]] = []
        self.started = time.monotonic()
        self._next_call = 1
        self._active_call: int | None = None
        self._last_call_by_session: dict[str, int] = {}

    def _append(self, entry: dict[str, Any]) -> None:
        entry = {
            "sequence": len(self.entries) + 1,
            "atMs": round((time.monotonic() - self.started) * 1000, 3),
            **entry,
        }
        self.entries.append(entry)

    def observe(self, session: Any, events: Iterable[str], *, session_name: str) -> None:
        for event_name in events:
            def handler(params: Any, name: str = event_name) -> None:
                entry = {
                    "kind": "event",
                    "session": session_name,
                    "method": name,
                    "params": copy.deepcopy(params),
                }
                if session_name in self._last_call_by_session:
                    entry["afterCallId"] = self._last_call_by_session[session_name]
                if self._active_call is not None:
                    entry["receivedDuringCallId"] = self._active_call
                self._append(entry)

            session.on(event_name, handler)

    def send(
        self,
        session: Any,
        method: str,
        params: dict[str, Any] | None = None,
        *,
        session_name: str,
    ) -> Any:
        call_id = self._next_call
        self._next_call += 1
        self._last_call_by_session[session_name] = call_id
        self._append(
            {
                "kind": "command",
                "callId": call_id,
                "session": session_name,
                "method": method,
                "params": copy.deepcopy(params or {}),
            }
        )
        self._active_call = call_id
        try:
            result = session.send(method, params or {})
        except Exception as error:
            self._append(
                {
                    "kind": "error",
                    "callId": call_id,
                    "session": session_name,
                    "method": method,
                    "error": {"type": type(error).__name__, "message": str(error)},
                }
            )
            raise
        finally:
            self._active_call = None
        self._append(
            {
                "kind": "response",
                "callId": call_id,
                "session": session_name,
                "method": method,
                "result": copy.deepcopy(result),
            }
        )
        return result

    def document(self) -> dict[str, Any]:
        return {
            "schemaVersion": 1,
            "mode": self.mode,
            "fixtureOrigin": self.fixture_origin,
            "logicalScopes": {
                "browserContext": "context-1",
                "page": "page-1",
                "cdpSession": "page-1",
                "executionContexts": "normalized in Runtime events",
            },
            "entries": self.entries,
        }

    def write(self, path: Path) -> None:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps(self.document(), indent=2, sort_keys=True) + "\n")

    def record_harness_error(self, stage: str, error: Exception) -> None:
        self._append(
            {
                "kind": "harnessError",
                "stage": stage,
                "error": {"type": type(error).__name__, "message": str(error)},
            }
        )


def _normalized_entries(document: dict[str, Any]) -> list[dict[str, Any]]:
    if not isinstance(document, dict) or "entries" not in document:
        raise ValueError("trace document must contain entries")
    entries = document["entries"]
    if not isinstance(entries, list) or not entries:
        raise ValueError("trace entries must be a non-empty list")
    prepared = []
    for index, entry in enumerate(entries, 1):
        if not isinstance(entry, dict):
            raise ValueError(f"trace entry {index} must be an object")
        if type(entry.get("sequence")) is not int or entry["sequence"] != index:
            raise ValueError(f"trace entry {index} has invalid sequence or kind")
        if not isinstance(entry.get("kind"), str):
            raise ValueError(f"trace entry {index} has invalid sequence or kind")
        prepared_entry = copy.deepcopy(entry)
        prepared_entry.pop("atMs", None)
        prepared.append(prepared_entry)
    fixture_origin = document.get("fixtureOrigin", "")
    if fixture_origin is not None and not isinstance(fixture_origin, str):
        raise ValueError("fixtureOrigin must be a string")
    return Normalizer(fixture_origin or "").normalize(prepared)


def _difference(left: Any, right: Any, path: str) -> dict[str, Any] | None:
    if type(left) is not type(right):
        return {"path": path, "left": left, "right": right}
    if isinstance(left, dict) and isinstance(right, dict):
        for key in sorted(set(left) | set(right)):
            child = f"{path}.{key}" if path else key
            if key not in left:
                return {"path": child, "left": "<missing>", "right": right[key]}
            if key not in right:
                return {"path": child, "left": left[key], "right": "<missing>"}
            found = _difference(left[key], right[key], child)
            if found:
                return found
        return None
    if isinstance(left, list) and isinstance(right, list):
        for index, (left_item, right_item) in enumerate(zip(left, right)):
            found = _difference(left_item, right_item, f"{path}[{index}]")
            if found:
                return found
        if len(left) != len(right):
            index = min(len(left), len(right))
            return {
                "path": f"{path}[{index}]",
                "left": left[index] if index < len(left) else "<missing>",
                "right": right[index] if index < len(right) else "<missing>",
            }
        return None
    if left != right:
        return {"path": path, "left": left, "right": right}
    return None


def first_divergence(left: dict[str, Any], right: dict[str, Any]) -> dict[str, Any] | None:
    difference = _difference(
        _normalized_entries(left),
        _normalized_entries(right),
        "entries",
    )
    if difference is None:
        return None
    match = re.match(r"entries\[(\d+)\]", difference["path"])
    difference["entry"] = int(match.group(1)) + 1 if match else 1
    return difference


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("left", type=Path)
    parser.add_argument("right", type=Path)
    args = parser.parse_args()
    left = json.loads(args.left.read_text())
    right = json.loads(args.right.read_text())
    divergence = first_divergence(left, right)
    print(json.dumps({"equal": divergence is None, "firstDivergence": divergence}, indent=2))
    return 0 if divergence is None else 1


if __name__ == "__main__":
    raise SystemExit(main())
