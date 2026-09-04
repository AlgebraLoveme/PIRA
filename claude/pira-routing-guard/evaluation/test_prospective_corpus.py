"""Freeze and validate the prospective validation corpus.

The SHA-256 digests below were recorded when the corpus was committed, before the v2 adaptive
classifier existed. A digest mismatch means the corpus was edited after the fact and its results
no longer count as prospective (see PROSPECTIVE_PROTOCOL.md).
"""

from __future__ import annotations

import hashlib
import json
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
FROZEN = {
    "prospective.json": "d5ef721db84f1672f598107ca78dc5a6d3bafd36fb814966c042a87ebadb4f6e",
    "prospective-multiturn.json": "85b24b2e269107e01a29a968c0517321c593e6cb3867f7c6ed605520eb7754ab",
}
MODULES = {
    "user_profile", "research", "paper_reading", "coding", "writing", "public_figure", "explain", "guidance", "maintenance",
}
REQUIRED_CATEGORIES = {
    "coding", "homonym", "writing", "write_boundary", "paper_boundary", "figure_boundary", "explain_boundary",
    "guidance", "user_profile", "maintenance", "multi_module", "ambiguous", "long", "adversarial", "none",
}
CHINESE = "一-鿿"


def check_case(test: unittest.TestCase, case: dict, *, turn: bool) -> None:
    test.assertIsInstance(case["prompt"], str)
    test.assertTrue(case["prompt"].strip())
    test.assertIn("expected_route", case)
    test.assertIn("expected_loaded", case)
    test.assertLessEqual(set(case["expected_loaded"]), MODULES)
    test.assertLessEqual(set(case["expected_route"]) - {"none"}, MODULES)
    test.assertIn(case.get("expect_adaptive"), (None, "abstain"))
    if case.get("expect_adaptive") == "abstain" and case["expected_loaded"] and not turn:
        # Abstain with a required module is allowed only for long/adversarial prompts.
        test.assertIn(case.get("category"), {"long", "adversarial"})


class ProspectiveCorpusTests(unittest.TestCase):
    def test_corpus_files_are_frozen(self) -> None:
        for name, digest in FROZEN.items():
            with self.subTest(file=name):
                self.assertEqual(hashlib.sha256((HERE / name).read_bytes()).hexdigest(), digest)

    def test_single_turn_corpus_shape_and_coverage(self) -> None:
        document = json.loads((HERE / "prospective.json").read_text(encoding="utf-8"))
        self.assertEqual(document["schema_version"], 1)
        scenarios = document["scenarios"]
        ids = [case["id"] for case in scenarios]
        self.assertEqual(len(ids), len(set(ids)))
        self.assertGreaterEqual(len(scenarios), 40)
        categories = {case["category"] for case in scenarios}
        self.assertLessEqual(REQUIRED_CATEGORIES, categories)
        for case in scenarios:
            with self.subTest(case=case["id"]):
                check_case(self, case, turn=False)
        abstain = [case for case in scenarios if case.get("expect_adaptive") == "abstain"]
        module_requiring = [case for case in scenarios if case["expected_loaded"] and case.get("expect_adaptive") != "abstain"]
        self.assertGreaterEqual(len(abstain), 8)
        self.assertGreaterEqual(len(module_requiring), 25)
        chinese = [case for case in scenarios if any("一" <= ch <= "鿿" for ch in case["prompt"])]
        self.assertGreaterEqual(len(chinese), 8)
        self.assertTrue(any(len(case["prompt"]) > 2000 for case in scenarios))
        self.assertTrue(any(len(case["expected_loaded"]) >= 3 for case in scenarios))

    def test_multiturn_corpus_shape(self) -> None:
        document = json.loads((HERE / "prospective-multiturn.json").read_text(encoding="utf-8"))
        sessions = document["scenarios"]
        self.assertGreaterEqual(len(sessions), 5)
        kinds = set()
        for session in sessions:
            self.assertGreaterEqual(len(session["turns"]), 2)
            for turn in session["turns"]:
                kinds.add(turn["kind"])
                check_case(self, turn, turn=True)
        self.assertLessEqual({"silent_switch", "vague_continuation", "continuation", "switch"}, kinds)


if __name__ == "__main__":
    unittest.main()
