#!/usr/bin/env python3
"""checks_selftest: the guard must be guarded.

WHY
  A rule that can never fire and a rule that always passes produce the same green output.
  Every failure mode this registry has actually had was of that kind:
    - `rev-list "$local_sha" --not --remotes` read as a regex never matches (`$` anchors)
    - a scope whose file was renamed matched nothing and passed anyway
    - the comment explaining why a form is banned counted as using it
  So the tests below assert the *mechanism*, not the project's current state: literal
  patterns, cut at the test module, comment exclusion, fail-closed on an empty scope,
  and the ledger rule that a prose-only entry is debt at or above the baseline.

RUN   python tools/checks_selftest.py      (stdlib only, milliseconds, no toolchain)
"""

import os
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import checks  # noqa: E402


class Grep(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.mkdtemp()
        self.f = os.path.join(self.dir, "sample.rs")
        with open(self.f, "w", encoding="utf-8") as fh:
            fh.write('let a = env["y_p"];\n')
            fh.write("#[cfg(test)]\n")
            fh.write("mod tests { let b = env[\"y_p\"]; }\n")

    def test_regex_dollar_is_an_anchor(self):
        # This is the bug the registry hit: a literal with `$` matches nothing.
        hits = checks.grep_files([self.f], 'rev-list "$sha" --not')
        self.assertEqual(hits, [])

    def test_literal_matches_the_same_text(self):
        with open(self.f, "a", encoding="utf-8") as fh:
            fh.write('let c = "rev-list $sha --not";\n')
        hits = checks.grep_files([self.f], "rev-list $sha --not", literal=True)
        self.assertEqual(len(hits), 1)

    def test_cut_stops_at_the_test_module(self):
        hits = checks.grep_files([self.f], r'env\["y_p"\]', cut=r"#\[cfg\(test\)\]")
        self.assertEqual(len(hits), 1, "only the production line counts")

    def test_without_cut_the_test_line_is_counted(self):
        hits = checks.grep_files([self.f], r'env\["y_p"\]')
        self.assertEqual(len(hits), 2)

    def test_exclude_line_skips_comments(self):
        with open(self.f, "a", encoding="utf-8") as fh:
            fh.write("# 不要用 rev-list -n 50\n")
        hits = checks.grep_files([self.f], "rev-list -n 50", exclude_line=r"^\s*#")
        self.assertEqual(hits, [])

    def test_unreadable_file_counts_as_a_hit(self):
        # Fail-closed: if the rule cannot be checked, it is not satisfied.
        hits = checks.grep_files([os.path.join(self.dir, "missing.rs")], "x")
        self.assertEqual(len(hits), 1)
        self.assertIn("unreadable", hits[0][2])


class Evaluate(unittest.TestCase):
    def setUp(self):
        self.root = tempfile.mkdtemp()

    def test_empty_scope_fails_closed(self):
        chk = {"kind": "forbid", "pattern": "x", "scope": ["nope/*.rs"]}
        status, detail = checks.evaluate(chk, self.root)
        self.assertEqual(status, "FAIL")
        self.assertIn("cannot be proven", detail)

    def test_empty_scope_may_skip_only_when_declared(self):
        chk = {"kind": "forbid", "pattern": "x", "scope": ["nope/*.rs"], "allow_skip": True}
        self.assertEqual(checks.evaluate(chk, self.root)[0], "SKIP")

    def test_forbid_hit_is_a_failure(self):
        p = os.path.join(self.root, "a.txt")
        with open(p, "w", encoding="utf-8") as fh:
            fh.write("panic!\n")
        chk = {"kind": "forbid", "pattern": "panic!", "scope": ["a.txt"]}
        self.assertEqual(checks.evaluate(chk, self.root)[0], "FAIL")

    def test_require_short_of_min_is_a_failure(self):
        p = os.path.join(self.root, "a.txt")
        with open(p, "w", encoding="utf-8") as fh:
            fh.write("ok\n")
        chk = {"kind": "require", "pattern": "needle", "scope": ["a.txt"], "min": 1}
        self.assertEqual(checks.evaluate(chk, self.root)[0], "FAIL")

    def test_cmd_exit_mismatch_is_a_failure(self):
        chk = {"kind": "cmd", "run": "exit 3", "expect_exit": 0, "scope": ["a.txt"]}
        self.assertEqual(checks.evaluate(chk, self.root)[0], "FAIL")


class Ledger(unittest.TestCase):
    def test_em_dash_is_debt(self):
        rows = checks.parse_ledger("| L01 | t | r | f | — |\n")
        self.assertTrue(rows["L01"]["debt"])

    def test_check_id_is_new_style(self):
        rows = checks.parse_ledger("| L56 | t | r | f | C001、C002 |\n")
        self.assertFalse(rows["L56"]["debt"])
        self.assertEqual(rows["L56"]["fix"], ["C001", "C002"])

    def test_selftest_group_is_legacy(self):
        rows = checks.parse_ledger("| L01 | t | r | f | I12、M10 |\n")
        self.assertEqual(rows["L01"]["fix"], ["I12", "M10"])

    def test_new_entry_without_a_check_is_hard_red(self):
        root = tempfile.mkdtemp()
        reg = {"version": 1, "ledger_from": "L56", "checks": []}
        ledger = {"L99": {"title": "x", "fix": [], "debt": True, "fix_text": "—"}}
        rc = checks.cmd_audit(reg, ledger, {"strict": False})
        self.assertEqual(rc, 1)

    def test_old_debt_is_visible_but_not_fatal(self):
        reg = {"version": 1, "ledger_from": "L56", "checks": []}
        ledger = {"L02": {"title": "x", "fix": [], "debt": True, "fix_text": "—"}}
        self.assertEqual(checks.cmd_audit(reg, ledger, {"strict": False}), 0)
        self.assertEqual(checks.cmd_audit(reg, ledger, {"strict": True}), 1)


if __name__ == "__main__":
    unittest.main(verbosity=1, buffer=True)
