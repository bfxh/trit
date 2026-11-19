#!/usr/bin/env python3
"""costprobe: language-agnostic cost measurement for the agent edit->verify loop.

Design rules (learned the hard way):
  1. MEASURE IN PLACE. Copying a repo breaks path-sensitive builds (a copied Rust workspace
     failed with 22 target-resolution errors and still produced a "0.83 s cold build" --
     a failure looks fast, and fast looks cheap).
  2. FAIL CLOSED. Every stage carries an exit code. If any stage fails, the component is
     marked invalid and NO total is reported.
  3. ONE SOURCE MTIME, RESTORED. Content is never modified; caches stay as found.
  4. MULTI-COMPONENT. A repo is a set of components, each with its own adapter.
  5. ACCOUNTING: totals are DERIVED, never ACCUMULATED. Every incremental run is stored as
     a round with a stable key; re-running with -Model merges by key so the same round can
     never be counted twice.

Ported from costprobe.ps1. Two intentional differences, both bug fixes rather than drift:
  * CARGO_TARGET_DIR is set in the child's environment instead of being prefixed into the
    command string, so the recorded `cmd` is what a human would actually type;
  * stage output is captured from the child directly rather than through a PowerShell
    wrapper, so a stage's wall time no longer includes a shell startup.
"""

import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import time

SKIP = {"vendor", "node_modules", "target", ".git", "__pycache__", ".venv", "dist", "build", "obj"}


def native(p):
    """MSYS paths -> Windows paths.

    Git Bash converts argv that look like paths, but it does NOT convert environment
    variables. `-WorkRoot /d/KF/_costprobe` therefore reached cargo as
    CARGO_TARGET_DIR=/d/KF/... and every stage exited 101 -- a whole run reported
    "invalid" for a path-format reason, not a build failure.
    """
    if os.name == "nt":
        m = re.match(r"^/([a-zA-Z])(/.*)?$", p or "")
        if m:
            rest = (m.group(2) or "/").replace("/", "\\")
            return f"{m.group(1).upper()}:{rest}"
    return p


def parse_args(argv):
    out = {"incremental_runs": 3, "work_root": "D:\\KF\\_costprobe"}
    i = 0
    while i < len(argv):
        a = argv[i]
        if not a.startswith("-"):
            i += 1
            continue
        name = a.lstrip("-")
        key = {"Path": "path", "Out": "out", "Model": "model",
               "IncrementalRuns": "incremental_runs", "WorkRoot": "work_root"}.get(name, name.lower())
        val = argv[i + 1] if i + 1 < len(argv) else ""
        out[key] = int(val) if key == "incremental_runs" else native(val)
        i += 2
    return out


def run_stage(cmd, cwd, env_extra=None):
    env = os.environ.copy()
    if env_extra:
        env.update(env_extra)
    t0 = time.time()
    try:
        p = subprocess.run(cmd, cwd=cwd, shell=True, capture_output=True, text=True,
                           encoding="utf-8", errors="replace", env=env)
        code = p.returncode
        tail = " | ".join([ln for ln in (p.stdout + p.stderr).strip().splitlines()[-2:]])
    except FileNotFoundError:
        code, tail = 127, "command not found"
    return round(time.time() - t0, 3), code, tail


def has_py_tests(tests_dir):
    # Kept separate from the caller's `or` chain: a generator's `if` clause filters items,
    # it does not stop os.listdir from being evaluated on a directory that may not exist.
    if not os.path.isdir(tests_dir):
        return False
    return any(f.endswith(".py") for f in os.listdir(tests_dir))


def components(root):
    found = [(".", root)]
    for name in sorted(os.listdir(root)):
        full = os.path.join(root, name)
        if os.path.isdir(full) and name not in SKIP:
            found.append((name, full))
    out = []
    for rel, d in found:
        lang = None
        if os.path.exists(os.path.join(d, "Cargo.toml")):
            lang = "rust"
        elif os.path.exists(os.path.join(d, "package.json")):
            lang = "node"
        elif os.path.exists(os.path.join(d, "go.mod")):
            lang = "go"
        elif (os.path.exists(os.path.join(d, "pyproject.toml"))
              or os.path.exists(os.path.join(d, "pytest.ini"))
              or has_py_tests(os.path.join(d, "tests"))):
            lang = "python"
        if lang:
            out.append({"rel": rel, "dir": d, "lang": lang})
    return out


def stages(lang, d, sandbox):
    if lang == "rust":
        env = {"CARGO_TARGET_DIR": sandbox}
        return [("lint", "cargo clippy --quiet -- -D warnings", env),
                ("build", "cargo test --no-run --quiet", env),
                ("run", "cargo test --quiet", env)]
    if lang == "node":
        lib = os.path.join(d, "lib")
        files = [f for f in sorted(os.listdir(lib)) if f.endswith(".js")] if os.path.isdir(lib) else []
        check = "; ".join(f'node --check "{os.path.join(lib, f)}"' for f in files) or "node --check ."
        test = 'node --test "test/*.test.js"' if os.path.isdir(os.path.join(d, "test")) else "node --test"
        return [("lint", check, None), ("build", 'node -e "process.exit(0)"', None), ("run", test, None)]
    if lang == "python":
        return [("lint", "ruff check .", None), ("build", "pytest --collect-only -q", None),
                ("run", "pytest -q", None)]
    if lang == "go":
        return [("lint", "go vet ./...", None), ("build", "go build ./...", None),
                ("run", "go test ./...", None)]
    return []


# Filter on path segments, not on the full path: a fixture living under a directory whose
# name happens to contain "test" must still be measurable. (This bug cost a whole self-test
# round: the touch target came back empty and every round vanished.)
def under_source(full, root, segment_re):
    rel = os.path.relpath(full, root).replace("\\", "/")
    return not segment_re.search(rel)


def touch_target(lang, d):
    if lang == "rust":
        cands = []
        for cur, dirs, files in os.walk(d):
            dirs[:] = [x for x in dirs if x not in SKIP]
            for f in files:
                if f.endswith(".rs"):
                    full = os.path.join(cur, f)
                    if under_source(full, d, re.compile(r"(^|/)(target|tests)/")):
                        cands.append(full)
        if not cands:
            return None
        return {"path": max(cands, key=os.path.getsize)}
    if lang == "node":
        for cur, dirs, files in os.walk(d):
            dirs[:] = [x for x in dirs if x not in SKIP]
            for f in files:
                if f.endswith(".js"):
                    full = os.path.join(cur, f)
                    if under_source(full, d, re.compile(r"(^|/)(node_modules|test)/")):
                        return {"path": full}
        return None
    if lang == "python":
        for cur, dirs, files in os.walk(d):
            dirs[:] = [x for x in dirs if x not in SKIP]
            for f in files:
                if f.endswith(".py"):
                    full = os.path.join(cur, f)
                    if under_source(full, d, re.compile(r"(^|/)(\.venv|vendor|tests)/")):
                        return {"path": full}
        return None
    if lang == "go":
        for cur, dirs, files in os.walk(d):
            for f in files:
                if f.endswith(".go"):
                    return {"path": os.path.join(cur, f)}
        return None
    return None


def median(values):
    if not values:
        return 0.0
    s = sorted(values)
    return s[len(s) // 2]


def file_hash(path):
    h = hashlib.sha256()
    with open(path, "rb") as fh:
        for chunk in iter(lambda: fh.read(65536), b""):
            h.update(chunk)
    return h.hexdigest().upper()


def main(argv):
    a = parse_args(argv)
    root_in = native(a.get("path", "").strip())
    if not root_in or not os.path.isdir(root_in):
        print("costprobe: -Path 必须是已存在的目录")
        return 2
    root = os.path.abspath(native(root_in))
    sandbox = os.path.join(native(a["work_root"]), "target-" + os.path.basename(root))
    comps = components(root)
    if not comps:
        print(f"costprobe: no buildable component under {root}")
        return 2

    existing = None
    if a.get("model") and os.path.exists(a["model"]):
        with open(a["model"], "r", encoding="utf-8-sig") as fh:
            existing = json.load(fh)

    models = []
    for c in comps:
        st = stages(c["lang"], c["dir"], sandbox)
        if not st:
            continue
        touch = touch_target(c["lang"], c["dir"])
        valid = True

        cold = []
        for name, cmd, env in st:
            sec, code, tail = run_stage(cmd, c["dir"], env)
            if code != 0:
                valid = False
            cold.append({"name": name, "cmd": cmd, "cold_s": sec, "exit": code})
        noop = []
        for name, cmd, env in st:
            sec, code, _ = run_stage(cmd, c["dir"], env)
            noop.append({"name": name, "noop_s": sec, "exit": code})

        rounds, key = [], None
        if touch:
            path = touch["path"]
            key = f"{os.path.basename(path)}|{file_hash(path)}"
            stat = os.stat(path)
            orig = (stat.st_atime, stat.st_mtime)
            try:
                for i in range(a["incremental_runs"]):
                    os.utime(path, None)
                    time.sleep(0.2)
                    rs = []
                    for name, cmd, env in st:
                        sec, code, _ = run_stage(cmd, c["dir"], env)
                        if code != 0:
                            valid = False
                        rs.append({"name": name, "seconds": sec, "exit": code})
                    rounds.append({
                        "key": key, "touched": os.path.basename(path), "index": i,
                        "total_s": round(sum(x["seconds"] for x in rs), 3), "stages": rs,
                    })
            finally:
                os.utime(path, orig)

        # Merge with the prior model: a round whose key is already recorded measured the
        # same thing again, so keep the old measurement and discard the new one. This is
        # the whole accounting rule -- append-by-key, never add-by-count.
        if existing and touch:
            prev = next((p for p in existing.get("components", []) if p.get("component") == c["rel"]), None)
            if prev:
                prev_rounds = [r for r in (prev.get("rounds") or []) if r]
                if any(r.get("key") == key for r in prev_rounds):
                    rounds = prev_rounds
                else:
                    rounds = prev_rounds + rounds

        incr = median([float(r["total_s"]) for r in rounds])
        cold_total = round(sum(s["cold_s"] for s in cold), 3)
        models.append({
            "component": c["rel"], "dir": c["dir"], "lang": c["lang"], "valid": valid,
            "touched": os.path.basename(touch["path"]) if touch else "",
            # DERIVED, not accumulated: recomputed from rounds on every write
            "cold_total_s": cold_total if valid else None,
            "incr_total_s": incr if valid else None,
            "stage_cold": cold, "stage_noop": noop, "rounds": rounds,
            "accounting": "incr_total_s = median(rounds[].total_s); never sum per-run totals",
        })

    doc = {
        "project": root,
        "measured_at": time.strftime("%Y-%m-%dT%H:%M:%S", time.gmtime()) + "Z",
        "accounting": "totals are derived from rounds by key; merging the same model twice cannot double count",
        "components": models,
    }
    out_path = a.get("out") or a.get("model")
    if out_path:
        with open(out_path, "w", encoding="utf-8", newline="\n") as fh:
            json.dump(doc, fh, ensure_ascii=False, indent=2)
            fh.write("\n")

    for m in models:
        if not m["valid"]:
            print(f"{m['component']} [{m['lang']}]  无效：有阶段退出码非 0，不报数字")
            continue
        print(f"{m['component']} [{m['lang']}]  cold={m['cold_total_s']}s  "
              f"incr={m['incr_total_s']}s  rounds={len(m['rounds'])}  (touch {m['touched']})")
        for s in m["stage_cold"]:
            print(f"   {s['name']:<6} cold={s['cold_s']:>7}s  {s['cmd']}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
