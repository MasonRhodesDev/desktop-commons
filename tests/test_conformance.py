import unittest
from datetime import date

from scripts.conformance import (
    classify,
    evaluate_assertion,
    has_failure,
)


class FakeProvider:
    def __init__(self) -> None:
        self.files = {
            ("example", "config.txt"): "enabled = true\n",
            (
                "example",
                ".github/workflows/ci.yml",
            ): "steps:\n  - uses: actions/checkout@v4\n",
        }

    def content(self, repository: str, path: str) -> str | None:
        return self.files.get((repository, path))

    def paths(self, repository: str) -> list[str]:
        return [path for repo, path in self.files if repo == repository]

    def metadata(self, repository: str) -> dict:
        return {"archived": False}


class ConformanceTests(unittest.TestCase):
    def setUp(self) -> None:
        self.provider = FakeProvider()

    def assertion(self, kind: str, **values: object) -> dict:
        row = {
            "id": "example-check",
            "kind": kind,
            "repository": "example",
            "barriers": ["BAR-001"],
            "concerns": ["example"],
            "severity": "error",
            "message": "example message",
        }
        row.update(values)
        return row

    def test_file_contains_passes(self) -> None:
        assertion = self.assertion(
            "file_contains",
            path="config.txt",
            pattern="enabled = true",
        )
        self.assertEqual(evaluate_assertion(assertion, self.provider), [])

    def test_conditional_requires_target(self) -> None:
        assertion = self.assertion(
            "conditional_contains",
            when_path="config.txt",
            when_pattern="enabled",
            path="config.txt",
            pattern="required = true",
        )
        failures = evaluate_assertion(assertion, self.provider)
        self.assertEqual(len(failures), 1)

    def test_mutable_workflow_reference_fails(self) -> None:
        assertion = self.assertion("workflow_references_pinned")
        failures = evaluate_assertion(assertion, self.provider)
        self.assertEqual(len(failures), 1)
        self.assertIn("actions/checkout@v4", failures[0])

    def test_active_waiver_does_not_fail_default_mode(self) -> None:
        assertion = self.assertion(
            "file_contains",
            path="config.txt",
            pattern="missing",
            waiver={"reason": "known", "expires": "2026-10-01"},
        )
        failures = evaluate_assertion(assertion, self.provider)
        finding = classify(assertion, failures, date(2026, 8, 14))
        self.assertEqual(finding.status, "waived")
        self.assertFalse(has_failure([finding], strict=False))
        self.assertTrue(has_failure([finding], strict=True))

    def test_expired_waiver_fails(self) -> None:
        assertion = self.assertion(
            "file_contains",
            path="config.txt",
            pattern="missing",
            waiver={"reason": "known", "expires": "2026-08-01"},
        )
        failures = evaluate_assertion(assertion, self.provider)
        finding = classify(assertion, failures, date(2026, 8, 14))
        self.assertEqual(finding.status, "expired-waiver")
        self.assertTrue(has_failure([finding], strict=False))


if __name__ == "__main__":
    unittest.main()
