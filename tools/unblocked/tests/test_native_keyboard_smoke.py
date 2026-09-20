from __future__ import annotations

import unittest

from tools.unblocked.native_keyboard_smoke import (
    assert_contract,
    compare_case,
    reference_worker_result,
)


class NativeKeyboardSmokeTests(unittest.TestCase):
    def test_reference_accepts_worker_and_complete_runner_result(self):
        worker = {"cases": {"insert-text": {"status": "passed"}}}
        self.assertIs(reference_worker_result(worker), worker)
        wrapper = {
            "cases": ["insert-text"],
            "engines": {"chrome": {"workerResult": worker}},
        }
        self.assertIs(reference_worker_result(wrapper), worker)

    def test_reference_rejects_malformed_documents(self):
        self.assertIsNone(reference_worker_result(None))
        self.assertIsNone(reference_worker_result({"cases": ["insert-text"]}))
        self.assertIsNone(reference_worker_result({"engines": {"chrome": {}}}))

    def test_cancellation_comparison_checks_each_phase(self):
        reference = {
            phase: {"snapshot": {"events": [phase], "values": {"edit": "abcd"}}}
            for phase in ("keydown", "keypress", "beforeinput", "insertText")
        }
        candidate = {
            phase: {"snapshot": dict(record["snapshot"])}
            for phase, record in reference.items()
        }
        candidate["beforeinput"]["snapshot"] = {
            "events": ["beforeinput", "input"],
            "values": {"edit": "abcd"},
        }
        with self.assertRaisesRegex(AssertionError, "cancellation beforeinput differs"):
            compare_case("cancellation", reference, candidate, label="candidate")

    def test_embedded_contract_requires_exact_key_phase_order(self):
        phase_order = (
            "keydown", "keypress", "beforeinput", "input",
            "keydown", "keypress", "beforeinput", "input", "keyup",
        )
        observation = {
            "actions": [{"actionError": None} for _ in range(4)],
            "snapshot": {
                "events": [
                    {"type": event_type}
                    for event_type in phase_order
                    for _ in range(3)
                ],
                "values": {"edit": {"value": "axyd"}},
            },
        }
        assert_contract("key-phases", observation, label="candidate")
        observation["snapshot"]["events"][2], observation["snapshot"]["events"][3] = (
            observation["snapshot"]["events"][3],
            observation["snapshot"]["events"][2],
        )
        with self.assertRaisesRegex(AssertionError, "phase sequence differs"):
            assert_contract("key-phases", observation, label="candidate")

    def test_embedded_contract_requires_all_poison_counters(self):
        observation = {
            "action": {"actionError": None},
            "snapshot": {
                "events": [
                    {"type": event_type}
                    for event_type in ("keydown", "keypress", "beforeinput", "input")
                ],
                "poisonCalls": {},
                "values": {"edit": {"value": "xabcd"}},
            },
        }
        with self.assertRaisesRegex(AssertionError, "counters missing"):
            assert_contract("poison", observation, label="candidate")

    def test_embedded_contract_rejects_transport_failure_as_protocol_negative(self):
        actions = [
            {"method": "Input.dispatchKeyEvent", "actionError": None},
            *[
                {
                    "method": "Input.dispatchKeyEvent",
                    "actionError": {"type": "TimeoutError", "message": "timed out"},
                }
                for _ in range(6)
            ],
        ]
        with self.assertRaisesRegex(AssertionError, "non-protocol failure accepted"):
            assert_contract(
                "protocol-negative",
                {"actions": actions, "snapshot": {"events": []}},
                label="candidate",
            )


if __name__ == "__main__":
    unittest.main()
