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

Rust goes one level deeper than the manifest graph: within each changed package the plan
selects individual cargo targets (lib / bins / tests / examples / benches) instead of the
whole package. A target is selected when a changed file is reachable from its root via
`mod` declarations; unreachable targets are dropped from the test command, e.g.
`cargo test -p trit-parts --bin parts-cli --no-fail-fast`.

Soundness of the target-level cut (a target may be excluded ONLY when its mod tree is
fully walked, so "changed file not reachable" is a proof, not a guess):

  * Every statically resolvable `mod x;` / `mod x { .. }` is followed (x.rs or x/mod.rs,
    relative to the module directory; rustc's rules, verified against rustc diagnostics).
  * cfg-gated mods (any cfg, feature/platform/test) are counted REACHABLE when the file
    exists and skipped when it does not -- an over-approximation that stays sound under
    any feature activation, so the walk never has to know which features are on.
  * `#[cfg(test)]` subtrees are remembered separately: a unit-test-only file never lands
    in the rlib that bins/tests/examples link, so changing one selects `--lib` alone.
  * Literal `#[path = "..."]` is resolved against both rustc's bases (containing-file
    dir and module dir, whichever exists); if neither file exists the target cannot be
    proven and the package falls back.
  * Fall back to the package-level command (today's shape), with an UNPROVEN reason,
    when: a plain mod resolves to no file (macro-generated / virtual), `cfg_attr` adds
    conditional attributes, `include!()` pastes items, a `macro_rules!` body declares a
    mod, or the walk exceeds -MaxFiles (default 5000) files read.
  * `include_str!` / `include_bin!` are NOT fallbacks: they expand to string/byte
    literals and cannot declare modules, so they cannot change what a mod tree reaches.
  * build.rs changes and non-Rust files (Cargo.toml, data) keep the package-level
    behaviour exactly as before, including the existing unproven reasons.
  * Integration tests are extern against the lib: a NON-test lib file change affects
    every target in the package (they all link the lib), which degrades to the
    package-level command -- the same semantics as before this feature existed.
  * Targets whose required-features are not in the package's default feature set are
    never selected explicitly (cargo would refuse); the package falls back to the
    package-level command, which skips them gracefully.

Plan JSON stays additive: `commands`/`proven`/`unproven`/`affected`/`notes` keep their
meaning; rust plans additionally carry a `targets` array describing, per changed
package, what was selected and why (mode: target-level | package-level).
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


def plan(lang, root, changed, affected, unproven, commands, notes, targets=None):
    p = {
        "lang": lang, "root": root, "changed": changed, "affected": affected,
        "proven": len(unproven) == 0, "unproven": unproven,
        "commands": commands, "notes": notes,
    }
    if targets is not None:
        # Additive field (rust only): per changed package, what the target-level cut
        # selected and why. Old consumers keep reading commands/proven as before.
        p["targets"] = targets
    return p


# --- rust ---------------------------------------------------------------------------
def owners_of(root, changed, packages):
    """Longest manifest-directory prefix wins, so nested crates attribute to the inner one."""
    changed_pkgs, unproven = [], []
    owner_of = {}
    for f in changed:
        abs_path = norm(os.path.join(root, f))
        owner = None
        for p in packages:
            d = norm(os.path.dirname(p["manifest_path"]))
            if abs_path.startswith(d) and (owner is None or len(d) > len(owner[1])):
                owner = (p, d)
        if owner:
            changed_pkgs.append(owner[0]["id"])
            owner_of[f] = owner[0]
        else:
            unproven.append(f"no package owns {f}")
    for f in changed:
        if re.search(r"build\.rs$", f):
            unproven.append(f"build.rs changes are not modelled: {f}")
    return changed_pkgs, unproven, owner_of


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


def affected_rust(root, changed, max_files=None):
    code, out, err = run(["cargo", "metadata", "--format-version", "1"], cwd=root)
    if code != 0:
        return None
    meta = json.loads(out)
    packages = meta.get("packages", [])
    nodes = meta.get("resolve", {}).get("nodes", [])
    changed_pkgs, unproven, owner_of = owners_of(root, changed, packages)
    edges = {n["id"]: n.get("dependencies", []) for n in nodes}
    affected = reverse_closure(changed_pkgs, edges, [n["id"] for n in nodes])
    by_id = {p["id"]: p["name"] for p in packages}
    names = sorted({by_id.get(i, i) for i in affected})
    note = "manifest-level graph: feature unification and cfg-specific deps are NOT modelled"
    notes = [note]

    # --- target-level pass over the changed packages ---------------------------------
    # Only attempted when nothing above already forces the package-level answer
    # (no-owner files, build.rs). It either yields per-package `cargo test -p X --bin y
    # ...` commands or a package-level fallback with an unproven reason; either way the
    # emitted commands are sound to run as-is.
    target_cmds, target_entries = [], []
    pkg_level_changed = set()
    build_rs = any(re.search(r"build\.rs$", f) for f in changed)
    if changed_pkgs and not build_rs:
        cache, counted = {}, [0]
        try:
            for pid in sorted(set(changed_pkgs)):
                pkg = next(p for p in packages if p["id"] == pid)
                pname = by_id.get(pid, pid)
                pkg_files = [f for f, own in owner_of.items() if own["id"] == pid]
                if any(not f.endswith(".rs") for f in pkg_files):
                    # Manifests/data are not part of any mod tree and can change every
                    # target: keep today's package-level answer, proven untouched.
                    pkg_level_changed.add(pname)
                    target_entries.append({
                        "package": pname, "mode": "package-level",
                        "reason": "non-Rust file changed: no target-level cut is provable",
                        "total": len(pkg.get("targets", [])),
                    })
                    continue
                res = _analyze_pkg(pkg, [norm(os.path.join(root, f)) for f in pkg_files],
                                   cache, counted, max_files or MAX_WALK_FILES)
                if res["mode"] == "target-level":
                    target_cmds.append(res["command"])
                    target_entries.append({
                        "package": pname, "mode": "target-level",
                        "selected": res["selected"], "total": res["total"],
                    })
                    notes.append(f"target-level: {len(res['selected'])}/{res['total']} "
                                 f"targets selected for {pname}")
                else:
                    pkg_level_changed.add(pname)
                    if res.get("unproven"):
                        unproven.append(f"{pname}: {res['unproven']}")
                    target_entries.append({
                        "package": pname, "mode": "package-level",
                        "reason": res["reason"], "total": res["total"],
                    })
        except _WalkCap:
            reason = (f"file visit cap {max_files or MAX_WALK_FILES} exceeded while walking "
                      "mod trees: target-level analysis aborted")
            unproven.append(reason)
            pkg_level_changed = {by_id.get(pid, pid) for pid in set(changed_pkgs)}
            target_entries = [{"package": by_id.get(pid, pid), "mode": "package-level",
                               "reason": reason,
                               "total": len(next(p for p in packages if p["id"] == pid)
                                                .get("targets", []))}
                              for pid in sorted(set(changed_pkgs))]
            target_cmds = []
        if any(e["mode"] == "target-level" for e in target_entries):
            notes.append("target-level soundness: cfg-gated mods count as reachable when the "
                         "file exists; #[path] literals are resolved (missing -> package "
                         "fallback); include!()/cfg_attr/macro-generated mods and build.rs "
                         "fall back to package level")

    changed_names = {by_id.get(pid, pid) for pid in changed_pkgs}
    deps = [n for n in names if n not in changed_names]
    rest = sorted(pkg_level_changed | set(deps))
    commands = list(target_cmds)
    if rest:
        commands.append("cargo test " + " ".join(f"-p {n}" for n in rest) + " --no-fail-fast")
    if not commands:
        # Byte-compatible with the pre-target-level behaviour (e.g. no file owned by any
        # package): the package selection collapses to today's string, plan is unproven.
        commands.append("cargo test " + " ".join(f"-p {n}" for n in names) + " --no-fail-fast")
    return plan("rust", root, changed, names, unproven, commands, notes,
                targets=target_entries or None)


# --- rust target-level analysis ------------------------------------------------------
# The cut is proven only when every excluded target's mod tree is walked to the end:
# each `mod` declaration resolved (or accounted for by the fail-safe rules in the
# module docstring). Anything the walker cannot decide degrades to the package-level
# command with an unproven reason -- the same direction the manifest graph already
# takes (full run is always sound, a guessed subset is not).
MAX_WALK_FILES = 5000

_KIND_ORDER = {"lib": 0, "bin": 1, "test": 2, "example": 3, "bench": 4}
MOD_TOKEN = re.compile(r"\bmod\b")
RS_IDENT = re.compile(r"\s*((?:r#)?[A-Za-z_][A-Za-z0-9_]*)")


class _WalkCap(Exception):
    """Raised when the walker exceeds its file-read budget; aborts the whole pass."""


def _string_start(text, i):
    """(hash_count, quote_index) when a Rust string literal starts at i, else None.
    Covers "..." and b"...", r"..." and r#"..."# (with the br#"..."# combination)."""
    n = len(text)
    if text[i] == '"':
        return 0, i
    if text[i] == "b":
        j = i + 1
        if j < n and text[j] == "r":
            j += 1
        elif j < n and text[j] == '"':
            return 0, j
        else:
            return None
    elif text[i] == "r":
        j = i + 1
    else:
        return None
    k = j
    while k < n and text[k] == "#":
        k += 1
    if k < n and text[k] == '"':
        return k - j, k
    return None


def strip_rs(text):
    """Length-preserving blanking of comments and string/char literal contents.

    Offsets in the result map 1:1 onto the source, so a literal value such as the
    #[path = "..."] string can be re-read from the original text at the same span.
    Lifetimes ('a) are left alone -- they never hide tokens the walker looks for."""
    n = len(text)
    buf = list(text)
    i = 0
    while i < n:
        c = text[i]
        nxt = text[i + 1] if i + 1 < n else ""
        if c == "/" and nxt == "/":
            j = text.find("\n", i)
            j = n if j < 0 else j
            while i < j:
                buf[i] = " "
                i += 1
            continue
        if c == "/" and nxt == "*":
            depth = 1
            buf[i] = buf[i + 1] = " "
            i += 2
            while i < n and depth:
                if text.startswith("/*", i):
                    depth += 1
                    buf[i] = buf[i + 1] = " "
                    i += 2
                elif text.startswith("*/", i):
                    depth -= 1
                    buf[i] = buf[i + 1] = " "
                    i += 2
                else:
                    if text[i] != "\n":
                        buf[i] = " "
                    i += 1
            continue
        if c in ('"', "r", "b") and not (i > 0 and (text[i - 1].isalnum() or text[i - 1] == "_")):
            st = _string_start(text, i)
            if st is not None:
                hashes, qpos = st
                j = qpos + 1
                if hashes:
                    term = '"' + "#" * hashes
                    k = text.find(term, j)
                    k = n if k < 0 else k + len(term)
                else:
                    k = j
                    while k < n:
                        if text[k] == "\\":
                            k += 2
                            continue
                        if text[k] == '"':
                            k += 1
                            break
                        k += 1
                while i < k:
                    if text[i] != "\n":
                        buf[i] = " "
                    i += 1
                continue
        if c == "'":
            if nxt == "\\":
                k = i + 2
                while k < n and text[k] != "'":
                    k += 1
                for t in range(i + 1, min(k, n)):
                    buf[t] = " "
                i = k + 1
                continue
            if i + 2 < n and text[i + 2] == "'":
                buf[i + 1] = " "
                i += 3
                continue
            i += 1
            continue
        i += 1
    return "".join(buf)


def _skip_ws_back(s, j):
    while j >= 0 and s[j] in " \t\r\n":
        j -= 1
    return j


def _attrs_before(s, pos):
    """Span of the attribute chain (plus any `pub` visibility) immediately before pos.
    Inner attributes (#![...]) terminate the scan without being collected."""
    i = pos
    first = None
    while True:
        j = _skip_ws_back(s, i - 1)
        if j >= 2 and s[j - 2:j + 1] == "pub" and (j == 2 or not (s[j - 3].isalnum() or s[j - 3] == "_")):
            j = _skip_ws_back(s, j - 3)
        elif j >= 0 and s[j] == ")":
            # pub(crate) / pub(in path): skip the parenthesised restriction
            depth, k = 0, j
            while k >= 0:
                if s[k] == ")":
                    depth += 1
                elif s[k] == "(":
                    depth -= 1
                    if depth == 0:
                        break
                k -= 1
            if k < 0:
                break
            j2 = _skip_ws_back(s, k - 1)
            if j2 >= 2 and s[j2 - 2:j2 + 1] == "pub":
                j = _skip_ws_back(s, j2 - 3)
            else:
                break
        if j < 0 or s[j] != "]":
            break
        depth, k = 0, j
        while k >= 0:
            if s[k] == "]":
                depth += 1
            elif s[k] == "[":
                depth -= 1
                if depth == 0:
                    break
            k -= 1
        if k < 0:
            break
        m = _skip_ws_back(s, k - 1)
        if m < 0 or s[m] != "#":
            break
        if m >= 1 and s[m - 1] == "!":
            break
        first = m
        i = m
    if first is None:
        return None
    return (first, pos)


def _scan_mods(stripped, raw, offset=0):
    """Mod declarations at one nesting level. Inline bodies are scanned recursively;
    `offset` maps slice positions back onto the raw source for #[path] extraction."""
    mods = []
    for m in MOD_TOKEN.finditer(stripped):
        nm = RS_IDENT.match(stripped, m.end())
        if not nm:
            continue
        name = nm.group(1)
        if name.startswith("r#"):
            name = name[2:]
        j = nm.end()
        while j < len(stripped) and stripped[j] in " \t\r\n":
            j += 1
        if j >= len(stripped):
            continue
        children = None
        if stripped[j] == ";":
            kind = "file"
        elif stripped[j] == "{":
            depth, k = 0, j
            while k < len(stripped):
                if stripped[k] == "{":
                    depth += 1
                elif stripped[k] == "}":
                    depth -= 1
                    if depth == 0:
                        break
                k += 1
            if depth != 0:
                continue
            kind = "inline"
            children = _scan_mods(stripped[j + 1:k], raw, offset + j + 1)
        else:
            continue
        span = _attrs_before(stripped, m.start())
        attrs = stripped[span[0]:m.start()] if span else ""
        cfg_test = bool(re.search(r"\bcfg\s*\(\s*test\s*\)", attrs))
        cfg_any = bool(re.search(r"\bcfg\s*\(", attrs))
        cfg_attr = bool(re.search(r"\bcfg_attr\b", attrs))
        path_value = None
        if re.search(r"\bpath\s*=", attrs):
            pm = (re.search(r'path\s*=\s*"([^"]*)"', raw[span[0] + offset:m.start() + offset])
                  if span else None)
            # Empty string means "present but not a plain literal": unresolvable.
            path_value = pm.group(1) if pm else ""
        mods.append({"name": name, "kind": kind, "cfg_test": cfg_test, "cfg_any": cfg_any,
                     "cfg_attr": cfg_attr, "path": path_value, "children": children})
    return mods


def _parse_rs_file(path, cache, counted, max_files):
    np = norm(os.path.abspath(path))
    info = cache.get(np)
    if info is not None:
        return info
    counted[0] += 1
    if counted[0] > max_files:
        raise _WalkCap()
    try:
        with open(path, "r", encoding="utf-8", errors="replace") as fh:
            raw = fh.read()
    except OSError:
        cache[np] = None
        return None
    stripped = strip_rs(raw)
    # include!() pastes items (possibly `mod`s from arbitrary paths) -> poison.
    # include_str!/include_bin! expand to literals and cannot declare modules -> inert.
    include_item = re.search(r"\binclude!\s*\(", stripped) is not None
    macro_mod = False
    for mm in re.finditer(r"\bmacro_rules!\s*", stripped):
        j = mm.end()
        while j < len(stripped) and (stripped[j].isalnum() or stripped[j] == "_"):
            j += 1
        while j < len(stripped) and stripped[j] in " \t\r\n":
            j += 1
        if j >= len(stripped) or stripped[j] not in "{([":
            continue
        open_c, close_c = stripped[j], {"{": "}", "(": ")", "[": "]"}[stripped[j]]
        depth, k = 0, j
        while k < len(stripped):
            if stripped[k] == open_c:
                depth += 1
            elif stripped[k] == close_c:
                depth -= 1
                if depth == 0:
                    break
            k += 1
        if depth == 0 and re.search(r"\bmod\b", stripped[j:k + 1]):
            macro_mod = True
    info = {"mods": _scan_mods(stripped, raw), "include": include_item, "macro_mod": macro_mod}
    cache[np] = info
    return info


def _walk_target_tree(root_path, cache, counted, max_files):
    """Reachable set of one target root. Returns (ok, reach, reason):
    reach maps norm-abs file -> test_only (False wins: any non-test path marks the file
    as part of the rlib, so a file reached both ways degrades to the wider answer)."""
    reach = {}
    stack = [("file", root_path, norm(os.path.dirname(os.path.abspath(root_path))), False)]
    while stack:
        kind, payload, base, to = stack.pop()
        if kind == "file":
            np = norm(os.path.abspath(payload))
            prev = reach.get(np)
            if prev is False:
                continue
            if prev and to:
                continue
            reach[np] = bool(to) and prev is not False
            info = _parse_rs_file(payload, cache, counted, max_files)
            if info is None:
                return False, None, f"unreadable source file {payload}"
            fdir = os.path.dirname(os.path.abspath(payload))
            if info["include"]:
                return False, None, f"include!() in {payload}: item-level inclusion is not modelled"
            if info["macro_mod"]:
                return False, None, f"macro_rules! body declares mod in {payload}"
            nxt = []
            for m in info["mods"]:
                r = _follow_mod(m, base, to, fdir, nxt)
                if r:
                    return False, None, r
            stack.extend(nxt)
        else:
            for m in payload:
                r = _follow_mod(m, base, to, None, stack)
                if r:
                    return False, None, r
    return True, reach, None


def _follow_mod(m, base, to, fdir, out):
    """Resolve one mod declaration; append work items to out. Returns a fail reason."""
    if m["cfg_attr"]:
        return (f"cfg_attr on mod {m['name']}: conditional attributes are not modelled")
    child_to = to or m["cfg_test"]
    if m["path"] is not None:
        if not m["path"]:
            return f"#[path] on mod {m['name']}: value is not a plain string literal"
        cands = []
        if fdir:
            cands.append(os.path.join(fdir, norm(m["path"])))
        cands.append(os.path.join(base, norm(m["path"])))
        found = [c for c in cands if os.path.isfile(c)]
        if not found:
            return (f"#[path = \"{m['path']}\"] on mod {m['name']}: no file at any "
                    "resolution base")
        for c in found:
            out.append(("file", c, norm(os.path.dirname(os.path.abspath(c))), child_to))
        return None
    if m["kind"] == "inline":
        out.append(("mods", m["children"], os.path.join(base, m["name"]), child_to))
        return None
    c1 = os.path.join(base, m["name"] + ".rs")
    c2 = os.path.join(base, m["name"], "mod.rs")
    found = [c for c in (c1, c2) if os.path.isfile(c)]
    if not found:
        if m["cfg_any"]:
            # cfg'd out of the build: the file may legitimately not exist.
            return None
        return (f"mod {m['name']}: neither {m['name']}.rs nor {m['name']}/mod.rs found "
                "(macro-generated or virtual module?)")
    for c in found:
        out.append(("file", c, os.path.join(base, m["name"]), child_to))
    return None


def _sel_target(t):
    kinds = t.get("kind", [])
    for k in ("lib", "bin", "test", "example", "bench"):
        if k in kinds:
            return {"kind": k, "name": t["name"]}
    return {"kind": kinds[0] if kinds else "?", "name": t["name"]}


def _primary_kind(t):
    kinds = t.get("kind", [])
    for k in ("lib", "bin", "test", "example", "bench"):
        if k in kinds:
            return k
    return kinds[0] if kinds else "?"


def _target_command(pname, selected):
    ordered = sorted(selected, key=lambda s: (_KIND_ORDER.get(s["kind"], 9), s["name"]))
    parts = ["cargo", "test", "-p", pname]
    for s in ordered:
        if s["kind"] == "lib":
            parts.append("--lib")
        else:
            parts += ["--" + s["kind"], s["name"]]
    parts.append("--no-fail-fast")
    return " ".join(parts)


def _default_feature_closure(pkg):
    """Feature names active for an isolated `-p <pkg>` run (its own default features)."""
    feats = pkg.get("features") or {}
    seen = set()
    frontier = ["default"] if "default" in feats else []
    while frontier:
        f = frontier.pop()
        if f in seen:
            continue
        seen.add(f)
        for d in feats.get(f, []):
            base = d.split("/", 1)[0]
            if base.startswith("dep:"):
                base = base[4:]
            if base in feats:
                frontier.append(base)
    return seen


def _analyze_pkg(pkg, changed_abs, cache, counted, max_files):
    """One changed package -> target-level selection or a package-level fallback."""
    targets = pkg.get("targets", [])
    total = len(targets)
    lib_t = next((t for t in targets if "lib" in t.get("kind", [])), None)
    others = [t for t in targets if t is not lib_t]
    changed_set = set(changed_abs)

    lib_test_affected = False
    if lib_t is not None:
        lroot = norm(lib_t["src_path"])
        if lroot in changed_set:
            # The lib root itself changed: every target links the lib, walk not needed.
            return {"mode": "package-level",
                    "reason": "lib source changed: every target links the lib", "total": total}
        ok, reach, reason = _walk_target_tree(lroot, cache, counted, max_files)
        if not ok:
            return {"mode": "package-level",
                    "reason": f"lib mod tree not statically provable: {reason}",
                    "total": total, "unproven": reason}
        if any(reach.get(f) is False for f in changed_set):
            # Non-test lib file: the rlib changes, so does every target linking it.
            return {"mode": "package-level",
                    "reason": "lib affected: every target links the lib", "total": total}
        lib_test_affected = any(reach.get(f) is True for f in changed_set)

    selected = []
    if lib_test_affected:
        # Unit-test-only lib code never lands in the rlib; `--lib` alone is affected.
        selected.append({"kind": "lib", "name": lib_t["name"]})
    for t in sorted(others, key=lambda t: (_KIND_ORDER.get(_primary_kind(t), 9), t["name"])):
        troot = norm(t["src_path"])
        if troot in changed_set:
            selected.append(_sel_target(t))
            continue
        ok, reach, reason = _walk_target_tree(troot, cache, counted, max_files)
        if not ok:
            kdesc = "/".join(t.get("kind", ["?"]))
            return {"mode": "package-level",
                    "reason": f"{kdesc} target {t['name']} mod tree not statically provable: {reason}",
                    "total": total, "unproven": reason}
        if any(f in reach for f in changed_set):
            selected.append(_sel_target(t))
    if not selected:
        reason = "no cargo target reached from the change; reduction would run nothing"
        return {"mode": "package-level", "reason": reason, "total": total, "unproven": reason}
    dflt = _default_feature_closure(pkg)
    sel_names = {s["name"] for s in selected}
    for t in targets:
        if t["name"] in sel_names:
            rf = t.get("required-features") or []
            missing = [f for f in rf if f not in dflt]
            if missing:
                return {"mode": "package-level",
                        "reason": (f"target {t['name']} requires features outside the default "
                                   f"set ({' '.join(missing)}): explicit selection would "
                                   "refuse to build; package-level skips it gracefully"),
                        "total": total}
    return {"mode": "target-level", "selected": selected, "total": total,
            "command": _target_command(pkg["name"], selected)}


# --- python -------------------------------------------------------------------------
def is_test(p):
    return bool(re.search(r"(^|/)tests?/|^test_.*\.py$|/test_.*\.py$", p))


def affected_python(root, changed, max_files=None):
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


def affected_node(root, changed, max_files=None):
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
        if run(["cargo", "metadata", "--format-version", "1", "--no-deps"], cwd=root)[0] != 0:
            failsafe.append("cargo metadata failed: the dependency graph is unavailable")

    def _max_files():
        v = a.get("maxfiles")
        try:
            return int(v) if v not in (None, "") else None
        except (TypeError, ValueError):
            return None

    builder = {"rust": affected_rust, "python": affected_python, "node": affected_node}[lang]
    p = None if failsafe else builder(root, changed, _max_files())

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
