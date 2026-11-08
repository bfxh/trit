#!/usr/bin/env python3
"""Self-test for audit.py: a green fixture plus a counterexample per check.

Kept out of audit.py so each file stays inside the repository's 800-line module
budget. Run with `python3 audit.py --self-test`; prints `FAIL <name>` on failure.
"""

from __future__ import annotations

import contextlib
import io
import json
import pathlib
import subprocess
import tempfile

from audit import audit_repo, exit_code, render_report, run, write_report

FIXTURE_TOOLCHAIN = "leanprover/lean4:v4.34.1"
FIXTURE_MATHLIB_SHA = "d13f23b723b8a846827a245b89c10fc7d3f11612"
FIXTURE_EXTRA_SHA = "770940ddf9878cf61952ed53d910b92bca841838"
FIXTURE_CATALOGUE = """\
# fixture catalogue
version: "v0.4"

project:
  name: "fixture"
  authors: ["OpenAI"]

sources:
  - title: "Fixture paper"
    authors: ["OpenAI"]
    id: ../preprints/fixture/paper.pdf
    type: article

status:
  scope: "Fixture."
  main_results:
    - comparator_config: ComparatorChallenges/A.json
      declaration: OAI.A.theorem
      file: OAI/A.lean
"""
FIXTURE_LAKEFILE = f"""\
import Lake

require mathlib from git
  "https://github.com/leanprover-community/mathlib4" @ "{FIXTURE_MATHLIB_SHA}"

require fixed-point-theorems from git
  "https://github.com/harfe/fixed-point-theorems-lean4.git" @ "{FIXTURE_EXTRA_SHA}"
"""
FIXTURE_CONFIG = """\
{
  "challenge_module": "ComparatorChallenges.A",
  "solution_module": "OAI.A",
  "theorem_names": ["OAI.A.theorem"],
  "definition_names": [],
  "permitted_axioms": ["propext", "Quot.sound", "Classical.choice"],
  "enable_nanoda": false
}
"""

def _write_fixture(
    root: pathlib.Path, overrides: dict[str, str], removed: set[str]
) -> None:
    files = {
        "lean/lean-toolchain": FIXTURE_TOOLCHAIN + "\n",
        "lean/lakefile.lean": FIXTURE_LAKEFILE,
        "lean/formalization.yaml": FIXTURE_CATALOGUE,
        "lean/ComparatorChallenges/A.json": FIXTURE_CONFIG,
        "lean/ComparatorChallenges/A.lean": "",
        "lean/OAI/A.lean": "",
    }
    files.update(overrides)
    for rel in removed:
        files.pop(rel)
    for rel, text in files.items():
        path = root / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding="utf-8")


def _git(root: pathlib.Path, *args: str) -> None:
    subprocess.run(
        ["git", *args],
        cwd=root,
        check=True,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        text=True,
    )


def _fixture(
    root: pathlib.Path,
    *,
    overrides: dict[str, str] | None = None,
    removed: set[str] | None = None,
    dirty: bool = False,
) -> str:
    _write_fixture(root, overrides or {}, removed or set())
    _git(root, "init", "-q")
    _git(root, "config", "user.email", "self-test@example.invalid")
    _git(root, "config", "user.name", "self-test")
    _git(root, "add", "-A")
    _git(root, "commit", "-qm", "fixture")
    if dirty:
        (root / "lean/OAI/A.lean").write_text("-- uncommitted\n", encoding="utf-8")
    return run(["git", "rev-parse", "HEAD"], root)


def _pin(sha: str, **overrides: object) -> dict:
    pin = {
        "repo": "https://example.invalid/openai-math",
        "commit": sha,
        "lean_toolchain": FIXTURE_TOOLCHAIN,
        "catalogue_version": "v0.4",
        "observed_at": "2026-11-08",
        "readme_coverage_claim": "300 / 719 (B, fixture)",
    }
    pin.update(overrides)
    return pin


def self_test() -> int:
    failures: list[str] = []
    seen = 0

    def check(name: str, condition: bool, detail: str = "") -> None:
        nonlocal seen
        seen += 1
        if condition:
            print("PASS " + name)
        else:
            print("FAIL " + name + (f": {detail}" if detail else ""))
            failures.append(name)

    def expect_error(name: str, report: dict, needle: str) -> None:
        check(
            name,
            any(needle in err for err in report["errors"])
            and not report["checks"]["ok"],
            f"errors={report['errors']}",
        )

    def expect_warning(name: str, report: dict, needle: str) -> None:
        check(
            name,
            any(needle in warn for warn in report["warnings"]),
            f"warnings={report['warnings']}",
        )

    with tempfile.TemporaryDirectory() as tmp:
        base = pathlib.Path(tmp)

        green_repo = base / "green"
        green_sha = _fixture(green_repo)
        green = audit_repo(
            green_repo, _pin(green_sha), local_toolchain=FIXTURE_TOOLCHAIN
        )
        check(
            "green: catalogue counts",
            green["catalogue"]
            == {
                "version": "v0.4",
                "sources": 1,
                "main_results": 1,
                "complete_entries": 1,
                "unique_comparator_configs": 1,
                "max_results_per_config": 1,
                "challenge_json": 1,
                "challenge_lean": 1,
                "unreferenced_configs": 0,
            },
            str(green["catalogue"]),
        )
        check(
            "green: requires pinned",
            green["requires"]
            == {
                "total": 2,
                "pinned": 2,
                "unpinned": [],
                "mathlib_sha": FIXTURE_MATHLIB_SHA,
            },
            str(green["requires"]),
        )
        check(
            "green: checks empty",
            green["checks"]
            == {
                "ok": True,
                "missing_files": [],
                "missing_configs": [],
                "unreadable_configs": [],
                "declaration_not_in_theorem_names": [],
                "missing_solution_modules": [],
                "missing_challenge_modules": [],
            },
            str(green["checks"]),
        )
        check(
            "green: distribution",
            green["distribution"]
            == {
                "permitted_axioms": {"Classical.choice+Quot.sound+propext": 1},
                "nanoda": {"false": 1},
            },
            str(green["distribution"]),
        )
        check("green: no warnings", green["warnings"] == [], str(green["warnings"]))
        check("green: exit code 0", exit_code(green) == 0)

        report_out = base / "report.json"
        write_report(green, report_out)
        raw_report = report_out.read_bytes()
        check(
            "green: report is LF-only",
            b"\r\n" not in raw_report and raw_report.endswith(b"\n"),
            f"bytes={len(raw_report)}",
        )
        check(
            "green: report round-trips",
            json.loads(raw_report.decode("utf-8")) == green,
        )

        commit_bad = base / "commit-bad"
        _fixture(commit_bad)
        expect_error(
            "red: commit mismatch",
            audit_repo(commit_bad, _pin("0" * 40), local_toolchain=FIXTURE_TOOLCHAIN),
            "commit ",
        )

        toolchain_bad = base / "toolchain-bad"
        toolchain_sha = _fixture(
            toolchain_bad,
            overrides={"lean/lean-toolchain": "leanprover/lean4:v4.35.0\n"},
        )
        expect_error(
            "red: corpus toolchain mismatch",
            audit_repo(
                toolchain_bad, _pin(toolchain_sha), local_toolchain=FIXTURE_TOOLCHAIN
            ),
            "lean-toolchain ",
        )

        version_bad = base / "version-bad"
        version_sha = _fixture(version_bad)
        expect_warning(
            "warn: catalogue version mismatch",
            audit_repo(
                version_bad,
                _pin(version_sha, catalogue_version="v9.9"),
                local_toolchain=FIXTURE_TOOLCHAIN,
            ),
            "catalogue version ",
        )

        dirty_repo = base / "dirty"
        dirty_sha = _fixture(dirty_repo, dirty=True)
        expect_warning(
            "warn: dirty checkout",
            audit_repo(dirty_repo, _pin(dirty_sha), local_toolchain=FIXTURE_TOOLCHAIN),
            "dirty",
        )

        local_diff = base / "local-diff"
        local_sha = _fixture(local_diff)
        local_report = audit_repo(
            local_diff,
            _pin(local_sha),
            local_toolchain="leanprover/lean4:v4.35.0",
        )
        expect_warning(
            "warn: local toolchain differs", local_report, "toolchains differ"
        )
        check(
            "green: toolchain match flag",
            local_report["toolchain"]["matches"] is False,
            str(local_report["toolchain"]),
        )

        missing_file = base / "missing-file"
        missing_sha = _fixture(missing_file, removed={"lean/OAI/A.lean"})
        missing_report = audit_repo(
            missing_file, _pin(missing_sha), local_toolchain=FIXTURE_TOOLCHAIN
        )
        expect_error("red: missing OAI file", missing_report, "missing OAI files")
        expect_error(
            "red: missing solution module", missing_report, "missing solution modules"
        )

        missing_cfg = base / "missing-config"
        missing_cfg_sha = _fixture(
            missing_cfg, removed={"lean/ComparatorChallenges/A.json"}
        )
        expect_error(
            "red: missing comparator config",
            audit_repo(
                missing_cfg, _pin(missing_cfg_sha), local_toolchain=FIXTURE_TOOLCHAIN
            ),
            "missing comparator configs",
        )

        unreadable = base / "unreadable"
        unreadable_sha = _fixture(
            unreadable, overrides={"lean/ComparatorChallenges/A.json": "{"}
        )
        expect_error(
            "red: unreadable comparator config",
            audit_repo(
                unreadable, _pin(unreadable_sha), local_toolchain=FIXTURE_TOOLCHAIN
            ),
            "unreadable comparator configs",
        )

        declaration_bad = base / "declaration-bad"
        declaration_sha = _fixture(
            declaration_bad,
            overrides={
                "lean/ComparatorChallenges/A.json": FIXTURE_CONFIG.replace(
                    '"OAI.A.theorem"', '"OAI.A.other"'
                )
            },
        )
        expect_error(
            "red: declaration not in theorem_names",
            audit_repo(
                declaration_bad,
                _pin(declaration_sha),
                local_toolchain=FIXTURE_TOOLCHAIN,
            ),
            "declaration not in theorem_names",
        )

        solution_bad = base / "solution-bad"
        solution_sha = _fixture(
            solution_bad,
            overrides={
                "lean/ComparatorChallenges/A.json": FIXTURE_CONFIG.replace(
                    '"solution_module": "OAI.A"',
                    '"solution_module": "OAI.Missing"',
                )
            },
        )
        expect_error(
            "red: missing solution module (config)",
            audit_repo(
                solution_bad, _pin(solution_sha), local_toolchain=FIXTURE_TOOLCHAIN
            ),
            "missing solution modules",
        )

        challenge_bad = base / "challenge-bad"
        challenge_sha = _fixture(
            challenge_bad,
            overrides={
                "lean/ComparatorChallenges/A.json": FIXTURE_CONFIG.replace(
                    '"challenge_module": "ComparatorChallenges.A"',
                    '"challenge_module": "ComparatorChallenges.Missing"',
                )
            },
        )
        expect_error(
            "red: missing challenge module",
            audit_repo(
                challenge_bad, _pin(challenge_sha), local_toolchain=FIXTURE_TOOLCHAIN
            ),
            "missing challenge modules",
        )

        unpinned = base / "unpinned"
        unpinned_sha = _fixture(
            unpinned,
            overrides={
                "lean/lakefile.lean": FIXTURE_LAKEFILE.replace(
                    f' @ "{FIXTURE_EXTRA_SHA}"', ""
                )
            },
        )
        unpinned_report = audit_repo(
            unpinned, _pin(unpinned_sha), local_toolchain=FIXTURE_TOOLCHAIN
        )
        expect_error(
            "red: unpinned requirement",
            unpinned_report,
            "unpinned git requirements",
        )
        check(
            "red: unpinned names",
            unpinned_report["requires"]["unpinned"] == ["fixed-point-theorems"],
            str(unpinned_report["requires"]["unpinned"]),
        )

        incomplete = base / "incomplete"
        incomplete_sha = _fixture(
            incomplete,
            overrides={
                "lean/formalization.yaml": FIXTURE_CATALOGUE.replace(
                    "      file: OAI/A.lean\n", ""
                )
            },
        )
        expect_error(
            "red: incomplete catalogue entry",
            audit_repo(
                incomplete, _pin(incomplete_sha), local_toolchain=FIXTURE_TOOLCHAIN
            ),
            "incomplete catalogue entries",
        )

        axioms_bad = base / "axioms-bad"
        axioms_sha = _fixture(
            axioms_bad,
            overrides={
                "lean/ComparatorChallenges/A.json": FIXTURE_CONFIG.replace(
                    '"propext", "Quot.sound", "Classical.choice"',
                    '"propext", "sorryAx"',
                )
            },
        )
        axioms_report = audit_repo(
            axioms_bad, _pin(axioms_sha), local_toolchain=FIXTURE_TOOLCHAIN
        )
        expect_warning(
            "warn: unexpected permitted axioms",
            axioms_report,
            "unexpected permitted_axioms",
        )
        check(
            "warn: axioms do not fail the audit",
            axioms_report["checks"]["ok"] is True and exit_code(axioms_report) == 0,
            str(axioms_report["errors"]),
        )

        extra_cfg = base / "extra-config"
        extra_sha = _fixture(
            extra_cfg,
            overrides={
                "lean/ComparatorChallenges/B.json": FIXTURE_CONFIG,
                "lean/ComparatorChallenges/B.lean": "",
            },
        )
        extra_report = audit_repo(
            extra_cfg, _pin(extra_sha), local_toolchain=FIXTURE_TOOLCHAIN
        )
        check(
            "green: unreferenced config counted",
            extra_report["catalogue"]["challenge_json"] == 2
            and extra_report["catalogue"]["unique_comparator_configs"] == 1
            and extra_report["catalogue"]["unreferenced_configs"] == 1,
            str(extra_report["catalogue"]),
        )

        nanoda_true = base / "nanoda-true"
        nanoda_true_sha = _fixture(
            nanoda_true,
            overrides={
                "lean/ComparatorChallenges/A.json": FIXTURE_CONFIG.replace(
                    '"enable_nanoda": false', '"enable_nanoda": true'
                )
            },
        )
        nanoda_true_report = audit_repo(
            nanoda_true, _pin(nanoda_true_sha), local_toolchain=FIXTURE_TOOLCHAIN
        )
        check(
            "green: nanoda true counted",
            nanoda_true_report["distribution"]["nanoda"] == {"true": 1},
            str(nanoda_true_report["distribution"]["nanoda"]),
        )

        nanoda_absent = base / "nanoda-absent"
        nanoda_absent_sha = _fixture(
            nanoda_absent,
            overrides={
                "lean/ComparatorChallenges/A.json": FIXTURE_CONFIG.replace(
                    ',\n  "enable_nanoda": false\n', "\n"
                )
            },
        )
        nanoda_absent_report = audit_repo(
            nanoda_absent,
            _pin(nanoda_absent_sha),
            local_toolchain=FIXTURE_TOOLCHAIN,
        )
        check(
            "green: nanoda absent counted",
            nanoda_absent_report["distribution"]["nanoda"] == {"absent": 1},
            str(nanoda_absent_report["distribution"]["nanoda"]),
        )

        shared = base / "shared-config"
        shared_sha = _fixture(
            shared,
            overrides={
                "lean/ComparatorChallenges/A.json": FIXTURE_CONFIG.replace(
                    '"OAI.A.theorem"', '"OAI.A.theorem", "OAI.A.second"'
                ),
                "lean/formalization.yaml": FIXTURE_CATALOGUE.replace(
                    "      file: OAI/A.lean\n",
                    "      file: OAI/A.lean\n"
                    "\n"
                    "    - comparator_config: ComparatorChallenges/A.json\n"
                    "      declaration: OAI.A.second\n"
                    "      file: OAI/A.lean\n",
                ),
            },
        )
        shared_report = audit_repo(
            shared, _pin(shared_sha), local_toolchain=FIXTURE_TOOLCHAIN
        )
        check(
            "green: shared config max results",
            shared_report["catalogue"]["main_results"] == 2
            and shared_report["catalogue"]["unique_comparator_configs"] == 1
            and shared_report["catalogue"]["max_results_per_config"] == 2,
            str(shared_report["catalogue"]),
        )

        not_git = base / "not-git"
        not_git.mkdir()
        try:
            audit_repo(not_git, _pin("0" * 40), local_toolchain=FIXTURE_TOOLCHAIN)
            check("red: not a git checkout raises", False, "no ValueError")
        except ValueError as exc:
            check("red: not a git checkout raises", "not a git checkout" in str(exc))

        render_bad = audit_repo(
            missing_file, _pin(missing_sha), local_toolchain=FIXTURE_TOOLCHAIN
        )
        buffer = io.StringIO()
        with contextlib.redirect_stdout(buffer), contextlib.redirect_stderr(
            io.StringIO()
        ):
            rc = render_report(render_bad)
        check(
            "red: render reports red",
            rc == 1 and "audit: red" in buffer.getvalue(),
            buffer.getvalue(),
        )

    total = seen
    print(f"self-test: {total - len(failures)}/{total} passed")
    return 0 if not failures else 1


if __name__ == "__main__":
    raise SystemExit(self_test())
