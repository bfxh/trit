#!/usr/bin/env python3
"""Structural audit of a pinned openai/math checkout.

This checks catalogue/file/module consistency and dependency pins. It does not
build Lean and does not claim Comparator/kernel validity. Standard library only.
"""

from __future__ import annotations

import argparse
import json
import pathlib
import re
import subprocess
import sys
from collections import Counter

HERE = pathlib.Path(__file__).resolve().parent
DEFAULT_PIN = HERE / "pin.json"
LOCAL_TOOLCHAIN = HERE.parent / "lean-toolchain"
EXPECTED_AXIOMS = {"propext", "Quot.sound", "Classical.choice"}


def run(cmd: list[str], cwd: pathlib.Path) -> str:
    return subprocess.check_output(cmd, cwd=cwd, text=True, encoding="utf-8").strip()


def parse_catalogue(text: str) -> tuple[str, int, list[dict[str, str]]]:
    version = ""
    sources = 0
    entries: list[dict[str, str]] = []
    section = ""
    current: dict[str, str] | None = None
    for line in text.splitlines():
        top = re.match(r"^([A-Za-z_][\w-]*):", line)
        if top:
            section = top.group(1)
            if section == "version":
                version = line.split(":", 1)[1].strip().strip('"')
            continue
        if section == "sources" and re.match(r"^  - title:", line):
            sources += 1
            continue
        if section != "status":
            continue
        head = re.match(r"^    - comparator_config:\s*(\S.*)$", line)
        if head:
            current = {"comparator_config": head.group(1).strip()}
            entries.append(current)
            continue
        field = re.match(r"^      (declaration|file):\s*(\S.*)$", line)
        if field and current is not None:
            current[field.group(1)] = field.group(2).strip()
    return version, sources, entries


def parse_requires(text: str) -> list[dict[str, str | None]]:
    lines = text.splitlines()
    out: list[dict[str, str | None]] = []
    for i, line in enumerate(lines):
        head = re.match(r"^require\s+(.+)$", line)
        if not head:
            continue
        block = "\n".join(lines[i : i + 4])
        sha = re.search(r'@\s*"([0-9a-f]{40})"', block)
        url = re.search(r'from\s+git\s*\n\s*"([^"]+)"', block)
        out.append(
            {
                "name": head.group(1).strip(),
                "url": url.group(1) if url else None,
                "sha": sha.group(1) if sha else None,
            }
        )
    return out


def module_path(module: str) -> str:
    return "lean/" + module.replace(".", "/") + ".lean"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", required=True, help="openai/math checkout")
    parser.add_argument("--pin", default=str(DEFAULT_PIN))
    parser.add_argument("--json", help="write the report to this path")
    args = parser.parse_args()

    repo = pathlib.Path(args.repo).resolve()
    pin = json.loads(pathlib.Path(args.pin).read_text(encoding="utf-8"))
    errors: list[str] = []
    warnings: list[str] = []

    if not (repo / ".git").exists():
        print(f"not a git checkout: {repo}", file=sys.stderr)
        return 2

    commit = run(["git", "rev-parse", "HEAD"], repo)
    if commit != pin["commit"]:
        errors.append(f"commit {commit} != pinned {pin['commit']}")
    if run(["git", "status", "--porcelain"], repo):
        warnings.append("checkout is dirty; audit counts still come from HEAD")

    paths = set(
        run(["git", "ls-tree", "-r", "--name-only", "HEAD"], repo).splitlines()
    )
    toolchain = (repo / "lean/lean-toolchain").read_text(encoding="utf-8").strip()
    if toolchain != pin["lean_toolchain"]:
        errors.append(
            f"lean-toolchain {toolchain} != pinned {pin['lean_toolchain']}"
        )
    local_toolchain = (
        LOCAL_TOOLCHAIN.read_text(encoding="utf-8").strip()
        if LOCAL_TOOLCHAIN.exists()
        else None
    )
    if local_toolchain and local_toolchain != toolchain:
        warnings.append(
            f"toolchains differ (openai/math {toolchain}, this repo {local_toolchain}); do not mix Lake projects"
        )

    version, sources, entries = parse_catalogue(
        (repo / "lean/formalization.yaml").read_text(encoding="utf-8")
    )
    if version != pin["catalogue_version"]:
        warnings.append(
            f"catalogue version {version or '<missing>'} != pinned {pin['catalogue_version']}"
        )

    require_list = parse_requires(
        (repo / "lean/lakefile.lean").read_text(encoding="utf-8")
    )
    unpinned = [r["name"] for r in require_list if not r["sha"]]
    if unpinned:
        errors.append(f"unpinned git requirements: {unpinned}")
    mathlib = next((r for r in require_list if "mathlib" in (r["url"] or "")), None)

    missing_files: list[str] = []
    missing_configs: list[str] = []
    unreadable_configs: list[str] = []
    declaration_missing: list[str] = []
    missing_solution_modules: list[str] = []
    missing_challenge_modules: list[str] = []
    incomplete: list[int] = []
    axioms: Counter[str] = Counter()
    nanoda: Counter[str] = Counter()
    referenced: set[str] = set()

    for index, entry in enumerate(entries):
        if not {"comparator_config", "declaration", "file"} <= entry.keys():
            incomplete.append(index)
            continue
        cfg_rel = "lean/" + entry["comparator_config"]
        file_rel = "lean/" + entry["file"]
        if file_rel not in paths:
            missing_files.append(file_rel)
        if cfg_rel not in paths:
            missing_configs.append(cfg_rel)
            continue
        referenced.add(cfg_rel)
        cfg_path = repo / cfg_rel
        if not cfg_path.exists():
            unreadable_configs.append(cfg_rel)
            continue
        try:
            config = json.loads(cfg_path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as exc:
            unreadable_configs.append(f"{cfg_rel}: {exc}")
            continue
        theorem_names = config.get("theorem_names", [])
        if entry["declaration"] not in theorem_names:
            declaration_missing.append(
                f"{cfg_rel}: {entry['declaration']} not in {theorem_names}"
            )
        solution = config.get("solution_module", "")
        if module_path(solution) not in paths:
            missing_solution_modules.append(f"{cfg_rel}: {solution}")
        challenge = config.get("challenge_module", "")
        if module_path(challenge) not in paths:
            missing_challenge_modules.append(f"{cfg_rel}: {challenge}")
        axioms["+".join(sorted(config.get("permitted_axioms", [])))] += 1
        key = config.get("enable_nanoda")
        nanoda["absent" if key is None else str(bool(key)).lower()] += 1

    if incomplete:
        errors.append(f"incomplete catalogue entries at indexes {incomplete}")
    for label, values in [
        ("missing OAI files", missing_files),
        ("missing comparator configs", missing_configs),
        ("unreadable comparator configs", unreadable_configs),
        ("declaration not in theorem_names", declaration_missing),
        ("missing solution modules", missing_solution_modules),
        ("missing challenge modules", missing_challenge_modules),
    ]:
        if values:
            errors.append(f"{label}: {values}")

    unexpected_axioms = [
        key for key in axioms if set(key.split("+")) - EXPECTED_AXIOMS
    ]
    if unexpected_axioms:
        warnings.append(f"unexpected permitted_axioms sets: {unexpected_axioms}")

    tree_json = sorted(
        p
        for p in paths
        if p.startswith("lean/ComparatorChallenges/") and p.endswith(".json")
    )
    tree_lean = sorted(
        p
        for p in paths
        if p.startswith("lean/ComparatorChallenges/") and p.endswith(".lean")
    )
    unreferenced = [p for p in tree_json if p not in referenced]
    per_config = Counter("lean/" + e["comparator_config"] for e in entries)

    report = {
        "schema": "trit.openai-math-audit/v1",
        "repo": pin["repo"],
        "commit": commit,
        "pin_commit": pin["commit"],
        "pin_observed_at": pin.get("observed_at"),
        "toolchain": {
            "corpus": toolchain,
            "this_repo": local_toolchain,
            "matches": toolchain == local_toolchain,
        },
        "catalogue": {
            "version": version,
            "sources": sources,
            "main_results": len(entries),
            "complete_entries": len(entries) - len(incomplete),
            "unique_comparator_configs": len(referenced),
            "max_results_per_config": max(per_config.values()) if per_config else 0,
            "challenge_json": len(tree_json),
            "challenge_lean": len(tree_lean),
            "unreferenced_configs": len(unreferenced),
        },
        "requires": {
            "total": len(require_list),
            "pinned": len(require_list) - len(unpinned),
            "unpinned": unpinned,
            "mathlib_sha": mathlib["sha"] if mathlib else None,
        },
        "checks": {
            "ok": not errors,
            "missing_files": missing_files,
            "missing_configs": missing_configs,
            "unreadable_configs": unreadable_configs,
            "declaration_not_in_theorem_names": declaration_missing,
            "missing_solution_modules": missing_solution_modules,
            "missing_challenge_modules": missing_challenge_modules,
        },
        "distribution": {
            "permitted_axioms": dict(sorted(axioms.items())),
            "nanoda": dict(sorted(nanoda.items())),
        },
        "evidence": {
            "catalogue_counts": "A (parsed from pinned commit)",
            "dynamic_comparator": "not_run",
            "readme_coverage_claim": "B (README headline; not derived from formalization.yaml)",
        },
        "readme_coverage_claim": pin.get("readme_coverage_claim"),
        "warnings": warnings,
        "errors": errors,
    }

    if args.json:
        out = pathlib.Path(args.json)
        out.parent.mkdir(parents=True, exist_ok=True)
        out.write_text(
            json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
        )

    cat = report["catalogue"]
    print(f"openai/math audit @ {commit[:7]} ({toolchain})")
    print(
        "sources={sources} main_results={main_results} unique_configs={unique_comparator_configs} "
        "challenge_json={challenge_json} challenge_lean={challenge_lean}".format(**cat)
    )
    print(
        f"requires={len(require_list)}/{len(require_list)} pinned; "
        f"mathlib={mathlib['sha'][:7] if mathlib else 'none'}"
    )
    print(
        "checks: "
        f"files={len(missing_files)} missing, configs={len(missing_configs)} missing, "
        f"declarations={len(entries) - len(declaration_missing)}/{len(entries)}, "
        f"solution_modules={len(entries) - len(missing_solution_modules)}/{len(entries)}"
    )
    print(
        f"unreferenced_configs={len(unreferenced)} "
        f"(catalogue main_results are not the full challenge set)"
    )
    for warning in warnings:
        print(f"WARN: {warning}")
    for error in errors:
        print(f"ERROR: {error}", file=sys.stderr)
    print("audit: " + ("green" if not errors else "red"))
    return 0 if not errors else 1


if __name__ == "__main__":
    raise SystemExit(main())
