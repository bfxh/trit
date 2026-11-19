#!/usr/bin/env python3
"""Emit a file-level python import graph as JSON.

Output: {"edges": {"importer_rel": ["imported_rel", ...]}, "unresolved": [...], "dynamic": [...]}

Why a separate helper: the graph must be computed by the language's own parser, not by
regex. A regex import scanner silently misses relative imports and multi-line imports,
and a silent miss turns into a silent under-selection -- exactly the failure mode the
verification reduction must not have.
"""
import ast
import json
import os
import sys


def rel(root, path):
    return os.path.relpath(path, root).replace("\\", "/")


def module_candidates(root, modname, importer_rel):
    """Map a dotted module name to plausible repo-relative file paths."""
    out = []
    base = modname.replace(".", "/")
    for suffix in (".py", "/__init__.py"):
        out.append(base + suffix)
        out.append("src/" + base + suffix)
    if importer_rel.startswith("tests/"):
        out.append("src/" + base + ".py")
    return out


def main():
    root = sys.argv[1]
    files = []
    skip_dirs = {"vendor", ".venv", "node_modules", "__pycache__", ".git", "build", "dist"}
    for dirpath, dirnames, filenames in os.walk(root):
        dirnames[:] = [d for d in dirnames if d not in skip_dirs]
        for f in filenames:
            if f.endswith(".py"):
                files.append(os.path.join(dirpath, f))

    existing = {rel(root, f) for f in files}
    edges = {}
    unresolved = []
    dynamic = []

    for f in files:
        r = rel(root, f)
        try:
            tree = ast.parse(open(f, encoding="utf-8", errors="replace").read())
        except SyntaxError:
            unresolved.append(r)
            continue
        targets = []
        for node in ast.walk(tree):
            if isinstance(node, ast.Import):
                for a in node.names:
                    targets.append(a.name)
                    # `import importlib` is a statement, not a call -- but it is still the
                    # tell-tale of a dynamic import, so the file must be treated as unknown.
                    if a.name.split(".")[0] in {"importlib", "runpy", "pkgutil", "imp"}:
                        dynamic.append(r)
            elif isinstance(node, ast.ImportFrom):
                if node.level and node.level > 0:
                    # relative import: resolve against the importer's package
                    pkg = r.split("/")[:-1]
                    up = node.level - 1
                    if up > 0:
                        pkg = pkg[:-up] if up <= len(pkg) else []
                    base = "/".join(pkg + ([node.module] if node.module else []))
                    targets.append(base.replace("/", "."))
                elif node.module:
                    targets.append(node.module)
            elif isinstance(node, ast.Call):
                fn = node.func
                name = getattr(fn, "id", None) or getattr(fn, "attr", None)
                if name in {"importlib", "exec", "eval", "__import__", "load_module", "spec_from_file_location"}:
                    dynamic.append(r)
        resolved = []
        for t in targets:
            hit = None
            for cand in module_candidates(root, t, r):
                if cand in existing:
                    hit = cand
                    break
            if hit:
                resolved.append(hit)
            else:
                # a stdlib or third-party import: not part of the repo graph
                top = t.split(".")[0]
                if any(x in existing for x in (top + ".py", top + "/__init__.py", "src/" + top + ".py")):
                    unresolved.append(f"{r} -> {t}")
        edges[r] = sorted(set(resolved))

    print(json.dumps({"edges": edges, "unresolved": unresolved, "dynamic": sorted(set(dynamic))}, indent=1))


if __name__ == "__main__":
    main()
