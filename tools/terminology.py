#!/usr/bin/env python3
"""terminology: the term registry (.agent/terms.json) is a concept contract; drift is
machine-checkable, not a docs decoration.

WHY THIS EXISTS
  Concept drift is how faking starts: the same word quietly changes meaning between the
  authority that defined it, the receipts that report it, and the tests that pin it
  ("censored" becomes "failed", a declared budget starts reading as a measured one).
  Each entry binds one term to the exact lines that define it, so anyone can ask a
  machine "does the authority still say what the registry says it says?" -- anchors are
  verbatim quotes, not vibes.

MODES
  check    scan the default surface (AGENT-GUARDRAILS.md, docs/ERRORS.md, tools/*.py,
           tools/selftest.ps1; override with --files) and report:
    ANCHOR-LOST   the authority no longer contains the quoted defining line
    ANCHOR-MOVED  the quote still exists but at another line (re-anchor with --reason)
    MIX           one line uses a term together with a word it must not be
                  interchanged with (phrase-shape match, see honest boundary)
    ORPHAN        the term occurs nowhere in code, scan surface or orphan homes --
                  a registry entry that outlived its vocabulary is a rotting registry
  add      register a new term. fail-closed: anchors must verify against the tree NOW
           and duplicate terms are refused -- a registry never records a lie.
  update   modify a term. any loosening (definition change, anchor/forbidden/token
           removal, status change) requires --reason -- the registry only tightens.
  list     print terms (--json for machine output).

REGISTRY SHAPE (additive like every other registry here)
  {"version":1,"owner_root":".","terms":[{term, definition, forbidden_interchange,
    tokens, mix(co|forms|exclusive|none), mix_forms, orphan_homes, anchors, status}]}
  anchors are "path:line:quoted text" with repo-relative paths; machine paths are
  refused at the door (the registry is committed).

HONEST BOUNDARY (travels with the tool, per house rule)
  phrase-shape matching is NOT semantic understanding. It catches the shape of two words
  being swapped in one line; it cannot catch the quieter drift where a word keeps its
  spelling and its meaning slides (that needs reading -- which is why anchors are
  verbatim quotes that break loudly instead of silently). Guards are substring
  heuristics: a theft sentence that merely contains "not" elsewhere escapes, and
  false-positive/negative rates are unknown. mix:"none" terms document their forbidden
  pairs but detect nothing -- stated, not hidden.

EXIT  check: 0 clean; 1 drift found; 2 missing files / bad registry / bad invocation.
      add/update: refusals are 2 (fail-closed), success 0.
"""

import argparse
import glob
import json
import os
import re
import sys
from datetime import datetime

REQUIRED = ("term", "definition", "forbidden_interchange", "tokens", "anchors", "status")
MIX_MODES = {"co", "forms", "exclusive", "none"}

DEFAULT_SCAN_DOCS = ["AGENT-GUARDRAILS.md", "docs/ERRORS.md"]
DEFAULT_SCAN_EXTRA = ("tools/*.py", "tools/selftest.ps1")
CODE_GLOBS = ("tools/*.py", "tools/*.ps1", "scripts/*.py")

# A line that already argues against the interchange is not a theft line. Substring
# heuristics, deliberately broad -- false negatives here are acceptable, false positives
# on the authorities would get the check switched off, which is worse (L59's lesson).
GUARDS = ["not ", "n't", "cannot", "never", "instead of", "rather than", "\u2260",
          "becomes", "变成", "不是", "不等于", "不许", "不冒充", "不准", "严禁", "禁止",
          "拒绝", "而不是", "永不", "并不", "不在此", "不拦"]


def now():
    return datetime.now().astimezone().isoformat(timespec="seconds")


def ascii_safe(text):
    return text.encode("ascii", "replace").decode("ascii")[:110]


def load_registry(path):
    if not os.path.exists(path):
        return None
    try:
        with open(path, "r", encoding="utf-8-sig") as fh:
            return json.load(fh)
    except (json.JSONDecodeError, UnicodeDecodeError, OSError) as exc:
        print(f"terms: registry is not readable JSON -- fail closed ({exc.__class__.__name__})")
        print(f"terms: {path}")
        sys.exit(2)


def save_registry(path, doc):
    os.makedirs(os.path.dirname(os.path.abspath(path)), exist_ok=True)
    with open(path, "w", encoding="utf-8", newline="\n") as fh:
        json.dump(doc, fh, ensure_ascii=False, indent=2)
        fh.write("\n")


def parse_anchor(anchor):
    """'path:line:quoted text' -> (path, lineno, text) or (None, reason)."""
    parts = anchor.split(":", 2)
    if len(parts) != 3 or not parts[0] or not parts[1].isdigit():
        return None, "want path:line:text"
    path, line, text = parts[0], int(parts[1]), parts[2]
    if not text.strip():
        return None, "empty quote"
    if re.match(r"^[A-Za-z]:", path) or path.startswith(("/", "\\")) or ".." in path.split("/"):
        return None, "machine or escaping path -- registry stores repo-relative paths"
    return (path, line, text), None


def token_rx(word):
    if re.fullmatch(r"[A-Za-z0-9_.\-]+", word):
        return re.compile(r"\b" + re.escape(word) + r"\b", re.IGNORECASE)
    return None  # CJK and friends: substring


def line_has(line, word):
    rx = token_rx(word)
    return bool(rx.search(line)) if rx else word in line


def line_guarded(line):
    low = line.lower()
    return any(g in line for g in GUARDS if not g.isascii()) or \
        any(g in low for g in GUARDS if g.isascii())


def resolve_files(root, patterns):
    out = []
    for pat in patterns:
        if any(ch in pat for ch in "*?["):
            hits = sorted(glob.glob(os.path.join(root, pat)))
            rel = [os.path.relpath(h, root).replace(os.sep, "/") for h in hits]
            if not rel:
                return None, pat
            out.extend(rel)
        else:
            out.append(pat)
    return out, None


def read_lines(root, rel):
    with open(os.path.join(root, rel), "r", encoding="utf-8-sig", errors="replace") as fh:
        return fh.read().split("\n")


def default_scan(root):
    pats = list(DEFAULT_SCAN_DOCS) + list(DEFAULT_SCAN_EXTRA)
    files, _ = resolve_files(root, pats)
    return files


def check(root, reg_path, files, as_json=False):
    reg = load_registry(reg_path)
    if reg is None:
        print(f"terms: no registry at {reg_path} (add terms first)")
        return 2
    terms = reg.get("terms") or []
    if not terms:
        print("terms: registry has no terms -- a concept contract with zero concepts")
        return 2

    if files is None:
        files = default_scan(root)
    missing = [f for f in files if not os.path.isfile(os.path.join(root, f))]
    for f in missing:
        print(f"terms: scan file missing: {f}")
    if missing:
        return 2

    corpus = {f: read_lines(root, f) for f in files}

    findings = []
    hard = False
    n_anchors = 0

    # --- a. anchor drift: does the authority still say what the registry cites? ------
    for t in terms:
        for key in REQUIRED:
            if key not in t or t[key] is None or (key != "forbidden_interchange" and t[key] in ("", [])):
                print(f"terms: registry malformed: term '{t.get('term', '?')}' lacks {key}")
                hard = True
        if t.get("mix") not in MIX_MODES:
            print(f"terms: registry malformed: '{t.get('term', '?')}' mix={t.get('mix')!r}")
            hard = True
        for anchor in t.get("anchors") or []:
            n_anchors += 1
            parsed, why = parse_anchor(anchor)
            if parsed is None:
                print(f"terms: registry malformed anchor for '{t['term']}': {why}")
                hard = True
                continue
            path, line, text = parsed
            full = os.path.join(root, path)
            if not os.path.isfile(full):
                print(f"terms: anchor file missing: {path} (term {t['term']})")
                hard = True
                continue
            lines = read_lines(root, path)
            if line <= len(lines) and text in lines[line - 1]:
                continue
            moved = next((i + 1 for i, l in enumerate(lines) if text in l), None)
            if moved:
                findings.append({"kind": "ANCHOR-MOVED", "term": t["term"],
                                 "where": f"{path}:{line}-> {moved}",
                                 "note": ascii_safe(text)})
            else:
                findings.append({"kind": "ANCHOR-LOST", "term": t["term"],
                                 "where": f"{path}:{line}", "note": ascii_safe(text)})
    if hard:
        return 2

    # --- b. interchange mixing: phrase shapes, guarded lines excused -----------------
    seen_pairs = set()
    for t in terms:
        tokens = t.get("tokens") or [t["term"]]
        forbidden = t.get("forbidden_interchange") or []
        mode = t.get("mix", "co")
        for rel, lines in corpus.items():
            for no, line in enumerate(lines, 1):
                guarded = line_guarded(line)
                if mode == "co":
                    if guarded:
                        continue
                    hit_t = any(line_has(line, w) for w in tokens)
                    hit_f = [w for w in forbidden if line_has(line, w)]
                    if hit_t and hit_f:
                        for w in hit_f:
                            key = (rel, no, tuple(sorted((t["term"], w))))
                            if key in seen_pairs:
                                continue
                            seen_pairs.add(key)
                            findings.append({"kind": "MIX", "term": t["term"],
                                             "where": f"{rel}:{no}",
                                             "note": f"{t['term']} <-> {w} | {ascii_safe(line.strip())}"})
                elif mode == "forms":
                    if guarded:
                        continue
                    for form in t.get("mix_forms") or []:
                        if re.search(form, line):
                            findings.append({"kind": "MIX", "term": t["term"],
                                             "where": f"{rel}:{no}",
                                             "note": f"form {form} | {ascii_safe(line.strip())}"})
                elif mode == "exclusive":
                    if guarded:
                        continue
                    for w in forbidden:
                        if line_has(line, w):
                            findings.append({"kind": "MIX", "term": t["term"],
                                             "where": f"{rel}:{no}",
                                             "note": f"foreign word '{w}' | {ascii_safe(line.strip())}"})

    # --- c. orphan terms: does the vocabulary still live anywhere real? --------------
    code_files, _ = resolve_files(root, list(CODE_GLOBS))
    code_text = {}
    for rel in code_files or []:
        code_text[rel] = "\n".join(read_lines(root, rel))
    scan_text = {rel: "\n".join(ls) for rel, ls in corpus.items()}
    for t in terms:
        tokens = t.get("tokens") or [t["term"]]
        homes = t.get("orphan_homes") or []
        home_missing = [h for h in homes if not os.path.isfile(os.path.join(root, h))]
        alive = any(any(line_has(body, w) for w in tokens)
                    for body in list(code_text.values()) + list(scan_text.values()))
        if not alive and homes:
            alive = any(any(line_has("\n".join(read_lines(root, h)), w) for w in tokens)
                        for h in homes if os.path.isfile(os.path.join(root, h)))
        if not alive:
            note = "no occurrence in code/scan/homes"
            if home_missing:
                note += f"; homes missing: {','.join(home_missing)}"
            findings.append({"kind": "ORPHAN", "term": t["term"], "where": "-", "note": note})

    if as_json:
        doc = {"ok": not findings and not hard, "files": len(files), "terms": len(terms),
               "anchors": n_anchors, "findings": findings}
        print(json.dumps(doc, ensure_ascii=True))
        return 1 if findings else 0

    for f in findings:
        print(f"terms: {f['kind']} {f['term']} {f['where']} {f['note']}")
    print(f"terms: files={len(files)} terms={len(terms)} anchors={n_anchors} "
          f"drift={len(findings)}")
    if findings:
        print("terms: RED -- the concept contract drifted; fix the authority or re-anchor "
              "with --reason (the registry only tightens)")
        return 1
    print("terms: clean -- authorities still say what the registry cites")
    return 0


def verify_anchor_now(root, anchor):
    parsed, why = parse_anchor(anchor)
    if parsed is None:
        return why
    path, line, text = parsed
    full = os.path.join(root, path)
    if not os.path.isfile(full):
        return f"anchor file missing: {path}"
    lines = read_lines(root, path)
    if line > len(lines) or text not in lines[line - 1]:
        return f"anchor does not verify today: {path}:{line} (quote not on that line)"
    return None


def cmd_add(root, reg_path, a):
    if not a["term"] or not a["definition"]:
        print("terms: add needs --term and --definition")
        return 2
    if not a["anchor"]:
        print("terms: add needs at least one --anchor path:line:text (fail-closed)")
        return 2
    reg = load_registry(reg_path)
    if reg is None:
        print(f"terms: no registry at {reg_path}")
        return 2
    terms = reg.setdefault("terms", [])
    if any(t.get("term") == a["term"] for t in terms):
        print(f"terms: term '{a['term']}' already exists -- terms are never silently redefined (use update)")
        return 2
    for anchor in a["anchor"]:
        why = verify_anchor_now(root, anchor)
        if why:
            print(f"terms: refuse -- {why}")
            return 2
    if a["mix"] == "forms" and not a["mix_forms"]:
        print("terms: mix=forms needs --mix-forms")
        return 2
    for form in a["mix_forms"]:
        try:
            re.compile(form)
        except re.error as exc:
            print(f"terms: refuse -- bad mix form {form!r}: {exc}")
            return 2
    entry = {
        "term": a["term"], "definition": a["definition"],
        "forbidden_interchange": [w for w in a["forbidden"].split(",") if w],
        "tokens": [w for w in a["tokens"].split(",") if w] or [a["term"]],
        "mix": a["mix"] or "co",
        "anchors": list(a["anchor"]),
        "status": a["status"] or "active",
    }
    if a["mix_forms"]:
        entry["mix_forms"] = list(a["mix_forms"])
    if a["orphan_homes"]:
        entry["orphan_homes"] = [w for w in a["orphan_homes"].split(",") if w]
    terms.append(entry)
    reg.setdefault("history", []).append(
        {"ts": now(), "action": "add", "term": a["term"],
         "reason": a["reason"] or "no reason given"})
    save_registry(reg_path, reg)
    print(f"terms: added {a['term']} (anchors={len(entry['anchors'])})")
    return 0


def cmd_update(root, reg_path, a):
    if not a["term"]:
        print("terms: update needs --term")
        return 2
    reg = load_registry(reg_path)
    if reg is None:
        print(f"terms: no registry at {reg_path}")
        return 2
    terms = reg.setdefault("terms", [])
    entry = next((t for t in terms if t.get("term") == a["term"]), None)
    if entry is None:
        print(f"terms: unknown term '{a['term']}'")
        return 2
    reason = a["reason"] or ""
    loosening = []

    if a["definition"] and a["definition"] != entry["definition"]:
        if not reason:
            print("terms: refuse -- definition changes need --reason "
                  "(the concept contract only tightens; say why the meaning may move)")
            return 2
        entry["definition"] = a["definition"]
        loosening.append("definition")

    if a["anchor"]:
        for anchor in a["anchor"]:
            why = verify_anchor_now(root, anchor)
            if why:
                print(f"terms: refuse -- {why}")
                return 2
            if anchor not in entry["anchors"]:
                entry["anchors"].append(anchor)

    if a["drop_anchor"]:
        if not reason:
            print("terms: refuse -- dropping an anchor loosens the contract, needs --reason")
            return 2
        if a["drop_anchor"] not in entry["anchors"]:
            print("terms: refuse -- --drop-anchor must match an existing anchor verbatim")
            return 2
        entry["anchors"].remove(a["drop_anchor"])
        loosening.append("drop-anchor")

    if a["forbidden"]:
        new = [w for w in a["forbidden"].split(",") if w]
        dropped = [w for w in entry.get("forbidden_interchange", []) if w not in new]
        if dropped and not reason:
            print(f"terms: refuse -- removing forbidden words ({','.join(dropped)}) "
                  "loosens the contract, needs --reason")
            return 2
        entry["forbidden_interchange"] = new

    if a["tokens"]:
        new = [w for w in a["tokens"].split(",") if w] or [a["term"]]
        dropped = [w for w in entry.get("tokens", []) if w not in new]
        if dropped and not reason:
            print(f"terms: refuse -- removing tokens ({','.join(dropped)}) "
                  "narrows detection, needs --reason")
            return 2
        entry["tokens"] = new

    if a["mix"] and a["mix"] != entry.get("mix"):
        if not reason:
            print("terms: refuse -- changing mix mode changes what is detected, needs --reason")
            return 2
        entry["mix"] = a["mix"]

    if a["status"] and a["status"] != entry.get("status"):
        if not reason:
            print("terms: refuse -- status changes need --reason")
            return 2
        entry["status"] = a["status"]

    if reason:
        reg.setdefault("history", []).append(
            {"ts": now(), "action": "update", "term": a["term"],
             "reason": reason, "loosened": ",".join(loosening) if loosening else ""})
        save_registry(reg_path, reg)
        print(f"terms: updated {a['term']} (reason recorded)")
    else:
        reg.setdefault("history", []).append(
            {"ts": now(), "action": "update", "term": a["term"], "reason": "(tightening)"})
        save_registry(reg_path, reg)
        print(f"terms: updated {a['term']} (tightening, no reason required)")
    return 0


def cmd_list(reg_path, as_json):
    reg = load_registry(reg_path)
    if reg is None:
        print(f"terms: no registry at {reg_path}")
        return 2
    terms = reg.get("terms") or []
    if as_json:
        print(json.dumps({"terms": terms, "history": reg.get("history", [])},
                         ensure_ascii=True))
        return 0
    for t in terms:
        print(f"terms: {ascii_safe(t['term']):<12} mix={t.get('mix', 'co'):<9} "
              f"anchors={len(t.get('anchors', []))} status={t.get('status', '?')}")
    print(f"terms: {len(terms)} registered")
    return 0


def main(argv):
    p = argparse.ArgumentParser(prog="terminology", add_help=False)
    p.add_argument("mode", nargs="?", default="check")
    p.add_argument("--root", default=os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
    p.add_argument("--registry", default="")
    p.add_argument("--files", default="")
    p.add_argument("--json", action="store_true")
    p.add_argument("--term", default="")
    p.add_argument("--definition", default="")
    p.add_argument("--anchor", action="append", default=[])
    p.add_argument("--drop-anchor", default="")
    p.add_argument("--forbidden", default="")
    p.add_argument("--tokens", default="")
    p.add_argument("--mix", default="")
    p.add_argument("--mix-forms", action="append", default=[])
    p.add_argument("--orphan-homes", default="")
    p.add_argument("--status", default="")
    p.add_argument("--reason", default="")
    a = vars(p.parse_args(argv))
    if a["mode"] not in {"check", "add", "update", "list"}:
        print("terms: mode must be one of check|add|update|list")
        return 2
    root = os.path.abspath(a["root"])
    if not os.path.isdir(root):
        print("terms: --root must be an existing directory")
        return 2
    reg_path = a["registry"] or os.path.join(root, ".agent", "terms.json")
    files = None
    if a["files"]:
        files, bad = resolve_files(root, [x for x in a["files"].split(",") if x])
        if files is None:
            print(f"terms: --files pattern matched nothing: {bad}")
            return 2

    if a["mode"] == "list":
        return cmd_list(reg_path, a["json"])
    if a["mode"] == "add":
        return cmd_add(root, reg_path, a)
    if a["mode"] == "update":
        return cmd_update(root, reg_path, a)

    return check(root, reg_path, files, as_json=a["json"])


if __name__ == "__main__":
    if os.name == "nt":
        try:
            sys.stdout.reconfigure(encoding="utf-8", errors="replace")
        except Exception:
            pass
    sys.exit(main(sys.argv[1:]))
