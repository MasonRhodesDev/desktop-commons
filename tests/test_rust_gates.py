import unittest
from datetime import date

from scripts.rust_gates import (
    REPOSITORY,
    advisory_problems,
    lint_allows,
    pin_problems,
    unsafe_words,
)

SHA = "0123456789abcdef0123456789abcdef01234567"


def lock(*packages):
    return "version = 4\n" + "".join(
        f'\n[[package]]\nname = "{name}"\nversion = "0.1.0"\nsource = "{source}"\n'
        for name, source in packages
    )


def pinned(rev, commit=SHA, url=REPOSITORY):
    return f"git+{url}?rev={rev}#{commit}"


class PinTests(unittest.TestCase):
    def test_a_full_sha_pin_passes(self):
        text = lock(("vigil-log", pinned(SHA)), ("panic-exit", pinned(SHA)))
        self.assertEqual(pin_problems(text, REPOSITORY), [])

    def test_a_short_rev_fails_though_the_fragment_is_full(self):
        text = lock(("vigil-log", pinned(SHA[:7])), ("panic-exit", pinned(SHA)))
        problems = pin_problems(text, REPOSITORY)
        self.assertEqual(len(problems), 1)
        self.assertIn("not a full 40-character commit SHA", problems[0])

    def test_a_branch_or_tag_pin_fails(self):
        for source in (f"git+{REPOSITORY}?branch=main#{SHA}", f"git+{REPOSITORY}?tag=v1#{SHA}", f"git+{REPOSITORY}#{SHA}"):
            text = lock(("vigil-log", source), ("panic-exit", pinned(SHA)))
            self.assertEqual(len(pin_problems(text, REPOSITORY)), 1, source)

    def test_a_registry_copy_fails(self):
        text = lock(
            ("vigil-log", "registry+https://github.com/rust-lang/crates.io-index"),
            ("panic-exit", pinned(SHA)),
        )
        self.assertIn("not git pinned", pin_problems(text, REPOSITORY)[0])

    def test_two_copies_fail(self):
        other = "fedcba9876543210fedcba9876543210fedcba98"
        text = lock(
            ("vigil-log", pinned(SHA)),
            ("vigil-log", pinned(other, other)),
            ("panic-exit", pinned(SHA)),
        )
        self.assertIn("2 entries", pin_problems(text, REPOSITORY)[0])

    def test_another_url_fails(self):
        fork = "https://example.com/fork/desktop-commons"
        text = lock(("vigil-log", pinned(SHA, url=fork)), ("panic-exit", pinned(SHA)))
        self.assertIn("comes from", pin_problems(text, REPOSITORY)[0])


class AdvisoryTests(unittest.TestCase):
    today = date(2026, 11, 20)

    def problems(self, *ignores):
        return advisory_problems({"advisories": {"ignore": list(ignores)}}, self.today)

    def test_a_dated_reason_within_the_horizon_passes(self):
        self.assertEqual(self.problems({"id": "RUSTSEC-2026-0001", "reason": "expires 2027-01-01: why"}), [])

    def test_undated_expired_and_far_reasons_fail(self):
        for reason in ("why", "expires 2026-11-19: why", "expires 2027-06-01: why", "expires 2026-13-01: why"):
            self.assertEqual(len(self.problems({"id": "RUSTSEC-2026-0001", "reason": reason})), 1, reason)

    def test_a_bare_id_fails(self):
        self.assertEqual(len(self.problems("RUSTSEC-2026-0001")), 1)


class FenceTests(unittest.TestCase):
    def test_lint_allows_count_only_unsafe_clippy_and_warnings(self):
        text = (
            "#[allow(dead_code)]\n"
            "#[allow(unsafe_code)]\n"
            '#[expect(\n    clippy::disallowed_methods,\n    reason = "a ] in a reason"\n)]\n'
            "#[cfg_attr(test, allow(warnings))]\n"
            "#![warn(clippy::all)]\n"
            'let s = "#[allow(unsafe_code)]";\n'
        )
        self.assertEqual(lint_allows(text), 4)

    def test_unsafe_words_skip_the_lint_name(self):
        self.assertEqual(unsafe_words("#![deny(unsafe_code)]\nunsafe { f() }\nunsafe fn g() {}\n"), 2)


if __name__ == "__main__":
    unittest.main()
