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
            *[
                {"method": "Input.setIgnoreInputEvents", "actionError": {"type": "Error", "message": "Protocol error (Input.setIgnoreInputEvents): Invalid parameters"}}
                for _ in range(4)
            ],
        ]
        with self.assertRaisesRegex(AssertionError, "non-protocol failure accepted"):
            assert_contract(
                "protocol-negative",
                {"actions": actions, "snapshot": {"events": []}},
                label="candidate",
            )

    def test_ignore_input_contract_requires_all_active_cdp_input_phases(self):
        def action(method, params):
            return {"method": method, "params": params, "response": {}, "actionError": None}

        def field(value):
            return {"value": value, "start": len(value), "end": len(value)}

        def phase(value, ignored):
            target_types = ["beforeinput", "input"] if ignored else [
                "pointermove", "pointerdown", "mousedown", "pointerup", "mouseup",
                "keydown", "keypress", "beforeinput", "input", "keyup",
                "beforeinput", "input",
            ]
            events = [{"type": item, "currentTarget": "edit"} for item in target_types]
            if not ignored:
                events.append({"type": "wheel", "currentTarget": "scrollbox"})
            return {
                "before": {"scrollTop": 0},
                "actions": [
                    action("Input.setIgnoreInputEvents", {"ignore": ignored}),
                    action("Input.dispatchMouseEvent", {"type": "mouseMoved"}),
                    action("Input.dispatchMouseEvent", {"type": "mousePressed"}),
                    action("Input.dispatchMouseEvent", {"type": "mouseReleased"}),
                    action("Input.dispatchMouseEvent", {"type": "mouseWheel"}),
                    action("Input.dispatchKeyEvent", {"type": "keyDown"}),
                    action("Input.dispatchKeyEvent", {"type": "keyUp"}),
                    action("Input.insertText", {"text": "x"}),
                ],
                "after": {
                    "scrollTop": 0 if ignored else 40,
                    "events": events,
                    "values": {"edit": field(value)},
                },
            }

        def lifecycle(value, text, key_suppressed, explicit_insert):
            event_types = ["beforeinput", "input"] if key_suppressed else [
                "keydown", "keypress", "beforeinput", "input"
            ]
            if explicit_insert and not key_suppressed:
                event_types += ["beforeinput", "input"]
            actions = [action("Input.dispatchKeyEvent", {"type": "keyDown"})]
            if explicit_insert:
                actions.append(action("Input.insertText", {"text": text}))
            return {
                "actions": actions,
                "snapshot": {
                    "events": [
                        {"type": event_type, "currentTarget": "edit", "data": (
                            text if event_type in ("beforeinput", "input") else None
                        )}
                        for event_type in event_types
                    ],
                    "values": {"edit": field(value)},
                },
            }

        observation = {
            "true": phase("abcdx", True),
            "false": phase("abcdxax", False),
            "sibling": {
                "actions": [action("Input.setIgnoreInputEvents", {"ignore": True}),
                            action("Input.setIgnoreInputEvents", {"ignore": False}),
                            action("Input.dispatchKeyEvent", {"type": "keyDown"}),
                            action("Input.dispatchKeyEvent", {"type": "keyDown"}),
                            action("Input.setIgnoreInputEvents", {"ignore": True}),
                            action("Input.dispatchKeyEvent", {"type": "keyDown"}),
                            action("Input.setIgnoreInputEvents", {"ignore": False}),
                            action("Input.dispatchKeyEvent", {"type": "keyDown"})],
                "snapshot": {
                    "events": [],
                    "values": {"edit": field("abcd")},
                },
            },
            "navigation": lifecycle("abcdn", "n", True, True),
            "reattach": lifecycle("abcdgr", "r", False, True),
            "otherTarget": {
                "actions": [
                    action("Input.setIgnoreInputEvents", {"ignore": True}),
                    action("Input.dispatchKeyEvent", {"type": "keyDown"}),
                    action("Input.dispatchKeyEvent", {"type": "keyDown"}),
                ],
                "firstSnapshot": {
                    "events": [],
                    "values": {"edit": field("abcdgr")},
                },
                "snapshot": lifecycle("abcdh", "h", False, False)["snapshot"],
            },
            "cleanupAction": action("Input.setIgnoreInputEvents", {"ignore": False}),
        }
        assert_contract("ignore-input", observation, label="candidate")
        observation["otherTarget"]["firstSnapshot"]["events"] = [
            {"type": "keydown", "currentTarget": "edit"}
        ]
        with self.assertRaisesRegex(AssertionError, "first target was not isolated"):
            assert_contract("ignore-input", observation, label="candidate")
        observation["otherTarget"]["firstSnapshot"]["events"] = []
        observation["true"]["after"]["events"] = []
        with self.assertRaisesRegex(AssertionError, "ignore=true did not suppress"):
            assert_contract("ignore-input", observation, label="candidate")

    def test_ignore_input_comparator_detects_snapshot_mutation(self):
        action = {"method": "Input.insertText", "params": {"text": "x"}, "response": {}, "actionError": None}
        reference = {"true": {"before": {}, "after": {"events": [1]}, "actions": [action]},
                     "false": {"before": {}, "after": {"events": [2]}, "actions": [action]}}
        candidate = {"true": {"before": {}, "after": {"events": [1]}, "actions": [{**action, "params": {"text": "y"}}]},
                     "false": {"before": {}, "after": {"events": [2]}, "actions": [action]}}
        with self.assertRaisesRegex(AssertionError, "ignore-input true action result differs"):
            compare_case("ignore-input", reference, candidate, label="candidate")

    def test_ignore_input_comparator_scopes_independent_mouse_parity(self):
        action = {
            "method": "Input.dispatchMouseEvent",
            "params": {"type": "mousePressed", "x": 25, "y": 25},
            "response": {},
            "actionError": None,
        }
        state = {
            "poisonCalls": {"KeyboardEvent": 0},
            "active": "edit",
            "scrollTop": 40,
            "values": {"edit": {"value": "abcdxax", "start": 7, "end": 7}},
        }
        reference = {
            "false": {
                "before": {"scrollTop": 0},
                "actions": [action],
                "after": {
                    **state,
                    "events": [
                        {
                            "type": "mousedown", "target": "edit",
                            "currentTarget": "edit", "constructor": "MouseEvent",
                            "which": 1, "targetValue": "abcdx",
                            "targetSelectionStart": 0, "targetSelectionEnd": 0,
                        },
                        {"type": "click", "target": "edit", "which": 1},
                    ],
                },
            },
            "cleanupAction": action,
        }
        candidate = {
            "false": {
                "before": {"scrollTop": 0},
                "actions": [dict(action)],
                "after": {
                    **state,
                    "events": [{
                        "type": "mousedown", "target": "edit",
                        "currentTarget": "edit", "constructor": "MouseEvent",
                        "which": None, "targetValue": None,
                        "targetSelectionStart": 5, "targetSelectionEnd": 5,
                    }],
                },
            },
            "cleanupAction": action,
        }
        compare_case("ignore-input", reference, candidate, label="candidate")
        candidate["false"]["after"]["events"][0]["target"] = "other"
        with self.assertRaisesRegex(AssertionError, "ignore-input false snapshot differs"):
            compare_case("ignore-input", reference, candidate, label="candidate")

    def test_maxlength_contract_requires_utf16_truncation_and_actual_event_data(self):
        def record(node, value, caret, events, states=None):
            if states is None:
                states = [(value, caret, caret, None)] * len(events)
            return {
                "action": {"actionError": None},
                "snapshot": {
                    "values": {node: {"value": value, "start": caret, "end": caret}},
                    "events": [
                        {
                            "currentTarget": node,
                            "type": event_type,
                            "data": data,
                            "targetValue": state[0],
                            "targetSelectionStart": state[1],
                            "targetSelectionEnd": state[2],
                            "targetMaxlength": state[3],
                        }
                        for (event_type, data), state in zip(events, states, strict=True)
                    ],
                },
            }

        observation = {
            "partial": record("edit", "A😀B", 4, [("beforeinput", "A😀BC"), ("input", "A😀B")]),
            "zero": record("edit", "", 0, [("beforeinput", "A")]),
            "selection": record(
                "edit", "A😀D", 4, [("beforeinput", "😀X"), ("input", "😀")],
                [("ABCD", 1, 3, "4"), ("A😀D", 4, 4, "4")],
            ),
            "dynamicShrink": record(
                "edit", "XA", 2, [("beforeinput", "ABC"), ("input", "A")],
                [("X", 1, 1, "4"), ("XA", 2, 2, "2")],
            ),
            "dynamicGrow": record(
                "edit", "XABC", 4, [("beforeinput", "ABC"), ("input", "ABC")],
                [("X", 1, 1, "2"), ("XABC", 4, 4, "4")],
            ),
            "reentryValueSelection": record(
                "edit", "QXY", 3, [("beforeinput", "XY"), ("input", "XY")],
                [("ABCD", 4, 4, "4"), ("QXY", 3, 3, "4")],
            ),
            "overlongDelete": record("edit", "A", 1, [("beforeinput", ""), ("input", "")]),
            "selectionNoCapacity": record("edit", "AC", 2, [("beforeinput", "X"), ("input", "")]),
            "parsedPrefix": record("edit", "ABCD", 4, [("beforeinput", "ABCDE"), ("input", "ABCD")]),
            "negativeUnbounded": record("edit", "ABCDE", 5, [("beforeinput", "ABCDE"), ("input", "ABCDE")]),
            "inputNewline": record("edit", "A ", 2, [("beforeinput", "A\r\nB"), ("input", "A ")]),
            "textarea": record(
                "other",
                "A\nB",
                3,
                [("beforeinput", "A\r\nB😀"), ("input", "A"), ("input", None), ("input", "B")],
                [
                    ("", 0, 0, "3"),
                    ("A\nB", 3, 3, "3"),
                    ("A\nB", 3, 3, "3"),
                    ("A\nB", 3, 3, "3"),
                ],
            ),
            "keyText": record(
                "edit",
                "A😀D",
                3,
                [("keydown", None), ("keypress", None), ("beforeinput", "😀X"), ("input", "😀")],
            ),
            "keyTextNoCapacity": record(
                "edit",
                "AC",
                1,
                [("keydown", None), ("keypress", None), ("beforeinput", "X"), ("input", "")],
                [
                    ("ABC", 1, 2, "2"),
                    ("ABC", 1, 2, "2"),
                    ("ABC", 1, 2, "2"),
                    ("AC", 1, 1, "2"),
                ],
            ),
            "lineBreakFull": record(
                "other", "ABC", 3,
                [("keydown", None), ("keypress", None), ("beforeinput", None)],
            ),
            "lineBreakOneCapacity": record(
                "other", "AB\n", 3,
                [("keydown", None), ("keypress", None), ("beforeinput", None), ("input", None)],
            ),
            "locatorFill": record("edit", "A😀B", 4, [("beforeinput", "A😀BC"), ("input", "A😀B")]),
        }
        assert_contract("maxlength", observation, label="candidate")
        observation["partial"]["snapshot"]["events"][1]["data"] = "A😀BC"
        with self.assertRaisesRegex(AssertionError, "maxlength partial events differ"):
            assert_contract("maxlength", observation, label="candidate")

    def test_maxlength_comparison_keeps_each_raw_snapshot_exact(self):
        reference = {
            name: {"action": {"actionError": None}, "snapshot": {"name": name}}
            for name in (
                "partial", "zero", "selection", "dynamicShrink", "dynamicGrow",
                "reentryValueSelection", "overlongDelete", "selectionNoCapacity",
                "parsedPrefix", "negativeUnbounded", "inputNewline", "textarea",
                "keyText", "keyTextNoCapacity", "lineBreakFull",
                "lineBreakOneCapacity", "locatorFill",
            )
        }
        candidate = {
            name: {"action": dict(record["action"]), "snapshot": dict(record["snapshot"])}
            for name, record in reference.items()
        }
        compare_case("maxlength", reference, candidate, label="candidate")
        candidate["textarea"]["snapshot"] = {"name": "changed"}
        with self.assertRaisesRegex(AssertionError, "maxlength textarea differs"):
            compare_case("maxlength", reference, candidate, label="candidate")


if __name__ == "__main__":
    unittest.main()
