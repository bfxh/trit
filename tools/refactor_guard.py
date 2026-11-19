#!/usr/bin/env python3
"""Refactor guard: mechanical proof that a refactor changed nothing it should not have.

Snapshot mode records the current shape of the crate (test names, public surface, unsafe
count, per-file LOC, test binary count). Check mode recomputes and fails on regressions,
then runs clippy and fmt.

    refactor_guard.py -Mode snapshot
    refactor_guard.py -Mode check
    refactor_guard.py -Mode preflight -Repo <dir>

Why: refactors break things silently in three places only --
  1. a test disappears (merged away, or a whole module stopped being compiled),
  2. a public item disappears (module no longer re-exported),
  3. a relative path reference breaks (include_str!, CARGO_MANIFEST_DIR).
(1) and (2) are exactly what snapshot/check compare. (3) is reported as a hint.

Ported from refactor-guard.ps1; messages and exit codes are kept identical.
"""

import json
import os
import re
import subprocess
import sys

MODES = {"snapshot", "check", "preflight"}


def parse_args(argv):
    out = {"mode": "check", "crate": "trit-orch", "baseline": "tools/.refactor-baseline.json",
           "repo": ""}
    positional = [x for x in argv if not x.startswith("-")]
    i = 0
    while i < len(argv):
        a = argv[i]
        if not a.startswith("-"):
            i += 1
            continue
        name = a.lstrip("-").lower()
        val = argv[i + 1] if i + 1 < len(argv) else ""
        out[{"mode": "mode", "crate": "crate", "baseline": "baseline", "repo": "repo"}.get(name, name)] = val
        i += 2
    if positional and positional[0] in MODES:
        out["mode"] = positional[0]
    return out


def cargo(args, cwd):
    p = subprocess.run(["cargo"] + args, cwd=cwd, capture_output=True, text=True,
                       encoding="utf-8", errors="replace")
    return p.returncode, (p.stdout or "") + (p.stderr or "")


def preflight(repo, crate_name):
    """The failure mode that costs the most and shows the least: a module missing from a `mod`
    declaration. The file still sits on disk, tests still "exist", and cargo simply never
    compiles them -- the suite stays green and gets silently smaller. Reachability from the
    module roots catches it in milliseconds, before anything is written or built."""
    tests_root = os.path.join(repo, "crates", crate_name, "tests")
    if not os.path.isdir(tests_root):
        print("preflight: no tests directory")
        return 0

    roots = []
    for dp, _dn, fn in os.walk(tests_root):
        for f in fn:
            if f in ("main.rs", "mod.rs"):
                roots.append(os.path.join(dp, f))
    if not roots:
        print("preflight: no module root (main.rs/mod.rs) under tests")
        return 2

    reached, queue = set(), [r for r in roots if os.path.basename(r) == "main.rs"]
    reached.update(queue)
    problems = []
    while queue:
        f = queue.pop(0)
        d = os.path.dirname(f)
        with open(f, "r", encoding="utf-8-sig") as fh:
            text = fh.read()
        for m in re.finditer(r"(?m)^\s*(?:pub(?:\(\w+\))?\s+)?mod\s+(\w+)\s*;", text):
            name = m.group(1)
            hit = None
            for cand in (os.path.join(d, name + ".rs"), os.path.join(d, name, "mod.rs")):
                if os.path.exists(cand):
                    hit = cand
                    break
            if not hit:
                problems.append(f"mod {name} declared in {os.path.basename(f)} but no {name}.rs")
                continue
            if hit not in reached:
                reached.add(hit)
                queue.append(hit)

    for dp, _dn, fn in os.walk(tests_root):
        for f in fn:
            if not f.endswith(".rs") or f in ("main.rs", "mod.rs"):
                continue
            full = os.path.join(dp, f)
            if full not in reached:
                problems.append("unreachable test file (never compiled): "
                                + os.path.relpath(full, tests_root).replace("\\", "/"))

    if problems:
        for p in problems:
            print(f"preflight: FAIL {p}")
        print("preflight: 拒绝落盘")
        return 1
    print(f"preflight: clean ({len(reached)} modules reachable)")
    return 0


def test_names(repo, crate_name):
    code, out = cargo(["test", "-p", crate_name, "--", "--list"], repo)
    names = []
    for line in out.splitlines():
        if line.rstrip().endswith(": test"):
            names.append(line.rstrip()[:-len(": test")].strip())
    return sorted(names)


def public_surface(repo, crate_name):
    lib = os.path.join(repo, "crates", crate_name, "src", "lib.rs")
    with open(lib, "r", encoding="utf-8-sig") as fh:
        text = fh.read()
    items = []
    for m in re.finditer(r"pub use (\w+)::\{([^}]*)\}", text):
        for name in m.group(2).split(","):
            clean = name.strip().split(" as ")[0].strip()
            if clean:
                items.append(f"{m.group(1)}::{clean}")
    for m in re.finditer(r"pub use (\w+)::(\w+);", text):
        items.append(f"{m.group(1)}::{m.group(2)}")
    return sorted(set(items))


def shape(repo, crate_name):
    src_root = os.path.join(repo, "crates", crate_name, "src")
    loc, unsafe_count = {}, 0
    for dp, _dn, fn in os.walk(src_root):
        for f in fn:
            if not f.endswith(".rs"):
                continue
            full = os.path.join(dp, f)
            rel = os.path.relpath(full, src_root).replace("\\", "/")
            with open(full, "r", encoding="utf-8-sig") as fh:
                text = fh.read()
            loc[rel] = len(text.splitlines())
            unsafe_count += len(re.findall(r"\bunsafe\b", text))

    path_refs = []
    tests_dir = os.path.join(repo, "crates", crate_name, "tests")
    for dp, _dn, fn in os.walk(tests_dir):
        for f in fn:
            if not f.endswith(".rs"):
                continue
            with open(os.path.join(dp, f), "r", encoding="utf-8-sig") as fh:
                text = fh.read()
            if re.search(r"include_str!|include_bytes!|CARGO_MANIFEST_DIR", text):
                path_refs.append(f)

    binaries = 0
    code, out = cargo(["metadata", "--format-version", "1", "--no-deps"], repo)
    if code == 0 and out.strip():
        try:
            doc = json.loads(out[out.index("{"):out.rindex("}") + 1])
            for pkg in doc.get("packages", []):
                if pkg.get("name") == crate_name:
                    binaries = len([t for t in pkg.get("targets", []) if "test" in t.get("kind", [])])
        except ValueError:
            binaries = 0

    return {
        "tests": test_names(repo, crate_name),
        "public_items": public_surface(repo, crate_name),
        "unsafe_count": unsafe_count,
        "loc": loc,
        "src_loc_total": sum(loc.values()),
        "test_binaries": binaries,
        "path_refs": sorted(path_refs),
    }


def main(argv):
    a = parse_args(argv)
    mode = a["mode"]
    if mode not in MODES:
        print(f"guard: -Mode 必须是 {sorted(MODES)} 之一")
        return 2
    repo = os.path.abspath(a["repo"]) if a["repo"] else os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    if not os.path.isdir(repo):
        print("guard: -Repo 必须是已存在的目录")
        return 2
    crate_name = a["crate"]

    if mode == "preflight":
        return preflight(repo, crate_name)

    sh = shape(repo, crate_name)

    if mode == "snapshot":
        path = os.path.join(repo, a["baseline"])
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "w", encoding="utf-8", newline="\n") as fh:
            json.dump(sh, fh, ensure_ascii=False, indent=4)
            fh.write("\n")
        print(f"snapshot: tests={len(sh['tests'])} public={len(sh['public_items'])} "
              f"src_loc={sh['src_loc_total']} binaries={sh['test_binaries']}")
        return 0

    path = os.path.join(repo, a["baseline"])
    if not os.path.exists(path):
        print(f"guard: no baseline at {a['baseline']}; run -Mode snapshot first")
        return 2
    with open(path, "r", encoding="utf-8-sig") as fh:
        old = json.load(fh)

    failures = []
    for name in old.get("tests", []):
        if name not in sh["tests"]:
            failures.append(f"test disappeared: {name}")
    for name in sh["tests"]:
        if name not in old.get("tests", []):
            print(f"guard: test added (fine): {name}")
    for item in old.get("public_items", []):
        if item not in sh["public_items"]:
            failures.append(f"public item disappeared: {item}")
    if sh["unsafe_count"] > 0 and old.get("unsafe_count", 0) == 0:
        failures.append(f"unsafe count went from 0 to {sh['unsafe_count']}")

    code, _ = cargo(["fmt", "--all", "--", "--check"], repo)
    if code != 0:
        failures.append("cargo fmt --check failed")
    code, _ = cargo(["clippy", "-p", crate_name, "--all-targets", "--quiet", "--", "-D", "warnings"], repo)
    if code != 0:
        failures.append("cargo clippy -D warnings failed")

    print(f"guard: binaries={sh['test_binaries']} (baseline {old.get('test_binaries')}) "
          f"src_loc={sh['src_loc_total']} (baseline {old.get('src_loc_total')})")
    if sh["test_binaries"] > old.get("test_binaries", 0):
        print(f"guard: WARNING test binary count grew {old.get('test_binaries')} -> "
              f"{sh['test_binaries']}; every src edit now pays one more link")
    if sh["path_refs"]:
        print("guard: relative path refs to re-check: " + ", ".join(sh["path_refs"]))

    if failures:
        for f in failures:
            print(f"guard: FAIL {f}")
        return 1
    print("guard: clean")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
