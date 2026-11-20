#!/usr/bin/env python3
"""affected: proof-carrying verification reduction.

Contract (this is the whole point):
  A subset plan may only claim "equivalent to full" when it carries an affected set A with
  A >= {modules actually reachable from the change}. When the graph is incomplete or the
  change type is not modelled, the plan must say `unproven` and the caller must fall back
  to a full run. Silent under-selection is the one failure mode this tool must never have.

Ported from affected.ps1 so the default execution surface is bash + python rather than
PowerShell. Behaviour is kept byte-compatible where it matters (plan fields, exit paths,
fail-safe reasons); the only intentional difference is that the python adapter calls the
interpreter that is running this script instead of assuming a `python` on PATH.
"""

import json
import os
import re
import subprocess
import sys

PY = sys.executable
SKIP_DIRS = {"node_modules", "dist", "build", ".git", "target", "__pycache__", ".venv"}


def parse_args(argv):
    out = {"changed": []}
    i = 0
    while i < len(argv):
        a = argv[i]
        if not a.startswith("-"):
            i += 1
            continue
        name = a.lstrip("-")
        key = {
            "Path": "path", "Changed": "changed", "Lang": "lang",
            "Out": "out", "AssumeIsolated": "assume_isolated",
        }.get(name, name.lower())
        if key == "assume_isolated":
            out[key] = True
            i += 1
            continue
        val = argv[i + 1] if i + 1 < len(argv) else ""
        if key == "changed":
            # PowerShell binds a string[] either as a,b or as repeated flags
            out["changed"].extend([x for x in val.split(",") if x.strip()])
        else:
            out[key] = val
        i += 2
    return out


def norm(p):
    return p.replace("\\", "/")


def rel_to(root, full):
    r = norm(os.path.abspath(full))
    base = norm(os.path.abspath(root)).rstrip("/")
    if r.startswith(base + "/"):
        return r[len(base) + 1:]
    return os.path.basename(r)


def run(cmd, cwd=None, want_json=False):
    try:
        p = subprocess.run(cmd, cwd=cwd, capture_output=True, text=True,
                           encoding="utf-8", errors="replace", shell=isinstance(cmd, str))
    except FileNotFoundError:
        return 127, "", ""
    return p.returncode, p.stdout, p.stderr


def full_plan(lang, root, changed, reason):
    cmd = {"rust": "cargo test --workspace --no-fail-fast", "python": "pytest -q",
           "node": "node --test", "go": "go test ./..."}.get(lang, "")
    return {
        "lang": lang, "root": root, "changed": changed, "affected": ["<全量>"],
        "proven": False, "unproven": [reason], "commands": [cmd],
        "notes": ["fail-safe: a full run is always sound, an unproven subset is not"],
    }


def plan(lang, root, changed, affected, unproven, commands, notes):
    return {
        "lang": lang, "root": root, "changed": changed, "affected": affected,
        "proven": len(unproven) == 0, "unproven": unproven,
        "commands": commands, "notes": notes,
    }


# --- rust ---------------------------------------------------------------------------
def owners_of(root, changed, packages):
    """Longest manifest-directory prefix wins, so nested crates attribute to the inner one."""
    changed_pkgs, unproven = [], []
    for f in changed:
        abs_path = norm(os.path.join(root, f))
        owner = None
        for p in packages:
            d = norm(os.path.dirname(p["manifest_path"]))
            if abs_path.startswith(d) and (owner is None or len(d) > len(owner[1])):
                owner = (p, d)
        if owner:
            changed_pkgs.append(owner[0]["id"])
        else:
            unproven.append(f"no package owns {f}")
    for f in changed:
        if re.search(r"build\.rs$", f):
            unproven.append(f"build.rs changes are not modelled: {f}")
    return changed_pkgs, unproven


def reverse_closure(seed, edges, universe, include_seed=True):
    """Files brought in by a change.

    include_seed matters for parity: the rust adapter counts the changed package itself in
    `affected`, while the python/node adapters count only the *importers*. That distinction
    is not cosmetic -- it decides what "affected=N" means in a receipt -- so it is a
    parameter rather than something each caller quietly decides.
    """
    affected = list(seed) if include_seed else []
    grew = True
    while grew:
        grew = False
        for node in universe:
            if node in affected or node in seed and include_seed:
                continue
            if any(d in affected or d in seed for d in edges.get(node, [])):
                affected.append(node)
                grew = True
    return affected


def affected_rust(root, changed):
    code, out, err = run(["cargo", "metadata", "--format-version", "1"])
    if code != 0:
        return None
    meta = json.loads(out)
    packages = meta.get("packages", [])
    nodes = meta.get("resolve", {}).get("nodes", [])
    changed_pkgs, unproven = owners_of(root, changed, packages)
    edges = {n["id"]: n.get("dependencies", []) for n in nodes}
    affected = reverse_closure(changed_pkgs, edges, [n["id"] for n in nodes])
    by_id = {p["id"]: p["name"] for p in packages}
    names = sorted({by_id.get(i, i) for i in affected})
    cmd = "cargo test " + " ".join(f"-p {n}" for n in names) + " --no-fail-fast"
    note = "manifest-level graph: feature unification and cfg-specific deps are NOT modelled"
    return plan("rust", root, changed, names, unproven, [cmd], [note])


# --- python -------------------------------------------------------------------------
def is_test(p):
    return bool(re.search(r"(^|/)tests?/|^test_.*\.py$|/test_.*\.py$", p))


def affected_python(root, changed):
    graph_py = os.path.join(os.path.dirname(os.path.abspath(__file__)), "import_graph.py")
    code, out, _ = run([PY, graph_py, root])
    if code != 0:
        return None
    g = json.loads(out)
    unproven = []
    for f in changed:
        if re.search(r"(conftest\.py|pytest\.ini|pyproject\.toml|setup\.cfg|tox\.ini)$", f):
            unproven.append(f"config change affects the whole suite: {f}")
    edges = g.get("edges", {})
    affected = reverse_closure(changed, edges, list(edges.keys()), include_seed=False)
    tests = [f for f in affected if re.search(r"(^|/)tests?/|^test_.*\.py$|/test_.*\.py$", f)]
    # Unknown dependencies are resolvable by INCLUSION, not by giving up: a test file with
    # dynamic imports or an unparseable body is force-included. Running it makes the plan
    # sound without knowing what it imports.
    unknown = [d for d in g.get("dynamic", []) if is_test(d)]
    for u in g.get("unresolved", []):
        if is_test(u) and u not in unknown:
            unknown.append(u)
    widened = list(tests)
    for d in unknown:
        if d not in widened:
            widened.append(d)
    tests = sorted(set(widened))
    if not tests:
        unproven.append("no test file reached from the change; reduction would run nothing")
    cmd = "pytest -q " + " ".join(tests)
    note = (f"file-level import graph; {len(unknown)} unknown-dependency test files "
            "force-included; conftest.py/plugins not modelled")
    return plan("python", root, changed, affected, unproven, [cmd], [note])


# --- node ---------------------------------------------------------------------------
IMPORT_RE = re.compile(r"""import\s*\(?\s*['"]([^'"]+)['"]|require\s*\(\s*['"]([^'"]+)['"]""")


def affected_node(root, changed):
    edges, unproven = {}, []
    for cur, dirs, files in os.walk(root):
        dirs[:] = [d for d in dirs if d not in {"node_modules", ".git", "dist", "build"}]
        for name in files:
            if not name.endswith((".js", ".mjs", ".ts", ".tsx")):
                continue
            full = os.path.join(cur, name)
            r = rel_to(root, full)
            with open(full, "r", encoding="utf-8", errors="replace") as fh:
                txt = fh.read()
            targets = []
            for m in IMPORT_RE.finditer(txt):
                spec = m.group(1) or m.group(2)
                if not spec or not spec.startswith("."):
                    continue
                cand = os.path.abspath(os.path.join(os.path.dirname(full), spec))
                for ext in ("", ".js", ".mjs", ".ts", "/index.js"):
                    if os.path.exists(cand + ext):
                        targets.append(rel_to(root, cand + ext))
                        break
            if re.search(r"importlib|eval\(|new Function", txt):
                unproven.append(f"dynamic code in {r}")
            edges[r] = targets
    affected = reverse_closure(changed, edges, list(edges.keys()), include_seed=False)
    tests = [f for f in affected if re.search(r"(^|/)test/|\.test\.|\.spec\.", f)]
    if not tests:
        unproven.append("no test file reached from the change")
    cmd = "node --test " + " ".join(tests)
    note = "static ESM/CJS imports only; path aliases and bundler resolution are not modelled"
    return plan("node", root, changed, affected, unproven, [cmd], [note])


def main(argv):
    a = parse_args(argv)
    root_in = a.get("path", "").strip()
    lang = a.get("lang", "").strip()
    if not root_in or not os.path.isdir(root_in):
        print("affected: -Path 必须是已存在的目录")
        return 2
    if lang not in ("rust", "python", "node"):
        print("affected: -Lang 必须是 rust|python|node")
        return 2
    if not a.get("changed"):
        print("affected: -Changed is required")
        return 2
    root = os.path.abspath(root_in)
    changed = [rel_to(root, os.path.join(root, c) if not os.path.isabs(c) else c) for c in a["changed"]]

    # --- fail-safe preconditions -----------------------------------------------------
    # Same rule Turborepo applies to shallow checkouts: when the graph cannot be trusted,
    # consider everything changed. A full run is always sound; a guessed subset is not.
    failsafe = []
    # .git has two forms: a DIRECTORY on the main checkout, a FILE in a linked worktree
    # (gitdir pointer). Judging history by the directory form alone read every worktree
    # as "no git history" and silently fell back to the full run.
    if not (os.path.isdir(os.path.join(root, ".git"))
            or os.path.isfile(os.path.join(root, ".git"))):
        failsafe.append("no git history at the project root: cannot establish a change set")
    manifests = [m for m in ("Cargo.toml", "package.json", "go.mod", "pyproject.toml")
                 if os.path.exists(os.path.join(root, m))]
    lang_count = sum(os.path.exists(os.path.join(root, m))
                     for m in ("Cargo.toml", "package.json", "go.mod"))
    if not a.get("assume_isolated") and lang_count > 1:
        failsafe.append(
            f"multi-language project ({', '.join(manifests)}): cross-language call edges are "
            "not modelled; pass -AssumeIsolated only if no component calls another")
    if lang == "rust":
        if run(["cargo", "metadata", "--format-version", "1", "--no-deps"])[0] != 0:
            failsafe.append("cargo metadata failed: the dependency graph is unavailable")

    builder = {"rust": affected_rust, "python": affected_python, "node": affected_node}[lang]
    p = None if failsafe else builder(root, changed)

    if failsafe or p is None:
        reason = "; ".join(failsafe) if failsafe else "graph unavailable"
        p = full_plan(lang, root, changed, reason)

    if a.get("out"):
        with open(a["out"], "w", encoding="utf-8", newline="\n") as fh:
            json.dump(p, fh, ensure_ascii=False, indent=2)
            fh.write("\n")

    proven_txt = "True" if p["proven"] else "False"
    print(f"lang={p['lang']} changed={len(p['changed'])} affected={len(p['affected'])} proven={proven_txt}")
    for u in p["unproven"]:
        print(f"  UNPROVEN: {u}")
    for c in p["commands"]:
        print(f"  plan: {c}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
