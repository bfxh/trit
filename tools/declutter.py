#!/usr/bin/env python3
"""declutter: shrink the comment mass in a codebase -- explanations live in docs/, models read code.

    declutter.py audit [--json] [--root <dir>]
    declutter.py budget init|check [--root <dir>]
    declutter.py export --strip --out <dir> [--root <dir>]
    declutter.py clean --in-place [--root <dir>]

The rule this tool serves: comments are a debt, not documentation. Write few of them,
put the explanations in docs/, and let the model read the code. Two kinds of comment
survive every strip, by design:

  LANDMINE  any comment containing the marker token LANDMINE is never stripped and audit
            lists it separately. This is the exemption channel for old problems that take
            the whole system down when touched. Without the marker a comment is fair game
            for cleaning -- the marker is an exemption, not a style, so do not sprinkle it.
  shebang   a python file's first-line #! survives stripping (removing it breaks script
            invocation, which is behavior, not commentary).

audit reports per-file comment lines / code lines / density / language, sorted by
density, plus library totals and the LANDMINE list. budget keeps a per-file comment-line
baseline in .agent/comment-budget.json with module-budget semantics: only ever tighter.
export renders a comment-free reading view for models in an --out directory outside the
tree; the convention for what a stripped comment was carrying is that it belongs in
docs/, not in the view. clean is the only pass that rewrites the tree: without --in-place
it refuses, with it every original is first moved into the trash bin (tools/trash.py put,
reason declutter-clean) and any failure stops the whole run with the originals restorable.

Honest boundaries (also printed by usage): comment-looking text inside string literals is
best effort and not guaranteed; python docstrings are strings and are neither stripped nor
counted as comments; js regex literals ride on a last-token heuristic whose damage is
bounded to one line; an unterminated string makes the scanner treat the rest of the file
as string -- it always errs toward not touching, never toward deleting; files that do not
decode as UTF-8 are skipped (audit/clean) or copied verbatim (export), never guessed at.

Output is pure ASCII. Exit codes: 0 ok, 1 refused or failed, 2 bad invocation.
"""

import bisect
import json
import os
import sys

import trash  # same tools directory; clean's only deletion channel is the recycle bin

LANDMINE = "LANDMINE"
CLEAN_REASON = "declutter-clean"
BUDGET_REL = ".agent/comment-budget.json"

LANG_BY_EXT = {
    ".py": "python",
    ".rs": "rust",
    ".js": "javascript", ".mjs": "javascript", ".cjs": "javascript", ".jsx": "javascript",
    ".json": "json", ".jsonc": "json",
}

SKIP_DIRS = {".git", ".hg", ".svn", ".journal", ".agent", ".trit", ".yanagent",
             "target", "node_modules", "__pycache__", "vendor", "dist", ".venv", "venv"}


# LANDMINE: the scanners below are a three-state walk (code / string / comment) and are
# the one place in this module where a wrong guess eats user code or silently keeps
# garbage. Rules that must not be "simplified": every literal scan terminates on EOF
# (never swallows the file), an unterminated one-liner recovers at end of line, and every
# recovery errs toward NOT touching. Escape pairs are consumed as a unit. If you add a
# literal form, add a selftest case for the comment-looking text inside it first.
def _flush(items, text, start, end):
    if end > start:
        items.append((start, end, "code"))


def _py_string(text, i, items):
    n = len(text)
    q = text[i]
    if text[i:i + 3] == q * 3:
        j = i + 3
        end = n
        while j < n:
            if text[j] == "\\":
                j += 2
                continue
            if text[j:j + 3] == q * 3:
                end = j + 3
                break
            j += 1
        end = min(end, n)
        items.append((i, end, "string"))
        return end
    j = i + 1
    end = n
    while j < n:
        if text[j] == "\\":
            j += 2
            continue
        if text[j] == q:
            end = j + 1
            break
        if text[j] == "\n":
            end = j
            break
        j += 1
    end = min(end, n)
    items.append((i, end, "string"))
    return end


def _scan_python(text):
    items = []
    n = len(text)
    i = 0
    run = 0
    while i < n:
        c = text[i]
        if c == "#":
            _flush(items, text, run, i)
            end = text.find("\n", i)
            if end == -1:
                end = n
            items.append((i, end, "comment"))
            i = run = end
        elif c == "\"" or c == "'":
            _flush(items, text, run, i)
            i = _py_string(text, i, items)
            run = i
        else:
            i += 1
    _flush(items, text, run, n)
    return items


def _scan_json(text):
    items = []
    n = len(text)
    i = 0
    run = 0
    while i < n:
        c = text[i]
        if text[i:i + 2] == "//" and c == "/":
            _flush(items, text, run, i)
            end = text.find("\n", i)
            if end == -1:
                end = n
            items.append((i, end, "comment"))
            i = run = end
        elif text[i:i + 2] == "/*" and c == "/":
            _flush(items, text, run, i)
            e = text.find("*/", i + 2)
            end = n if e == -1 else e + 2
            items.append((i, end, "comment"))
            i = run = end
        elif c == "\"":
            _flush(items, text, run, i)
            j = i + 1
            end = n
            while j < n:
                if text[j] == "\\":
                    j += 2
                    continue
                if text[j] == "\"":
                    end = j + 1
                    break
                if text[j] == "\n":
                    end = j
                    break
                j += 1
            end = min(end, n)
            items.append((i, end, "string"))
            i = run = end
        else:
            i += 1
    _flush(items, text, run, n)
    return items


def _rust_char_at(text, i):
    n = len(text)
    j = i + 1
    if j >= n:
        return 0
    if text[j] == "\\":
        j += 2
    elif text[j] != "'" and text[j] != "\\":
        j += 1
    else:
        return 0
    if j < n and text[j] == "'":
        return j + 1 - i
    return 0


def _rust_raw_or_byte(text, i):
    n = len(text)
    j = i + 1
    if text[i] == "b":
        if j < n and text[j] == "r":
            j += 1
        elif j < n and text[j] == "\"":
            k = j + 1
            while k < n:
                if text[k] == "\\":
                    k += 2
                elif text[k] == "\"":
                    return k + 1 - i
                else:
                    k += 1
            return n - i
        else:
            return 0
    hashes = 0
    while j < n and text[j] == "#":
        hashes += 1
        j += 1
    if j >= n or text[j] != "\"":
        return 0
    closer = "\"" + "#" * hashes
    k = text.find(closer, j + 1)
    if k == -1:
        return n - i
    return k + len(closer) - i


def _scan_rust(text):
    items = []
    n = len(text)
    i = 0
    run = 0

    def ident(ch):
        return ch.isalnum() or ch == "_"

    while i < n:
        c = text[i]
        if text[i:i + 2] == "//":
            _flush(items, text, run, i)
            end = text.find("\n", i)
            if end == -1:
                end = n
            items.append((i, end, "comment"))
            i = run = end
        elif text[i:i + 2] == "/*":
            _flush(items, text, run, i)
            depth = 1
            j = i + 2
            end = n
            while j < n:
                if text[j:j + 2] == "/*":
                    depth += 1
                    j += 2
                elif text[j:j + 2] == "*/":
                    depth -= 1
                    j += 2
                    if depth == 0:
                        end = j
                        break
                else:
                    j += 1
            items.append((i, end, "comment"))
            i = run = end
        elif c == "\"":
            _flush(items, text, run, i)
            j = i + 1
            end = n
            while j < n:
                if text[j] == "\\":
                    j += 2
                    continue
                if text[j] == "\"":
                    end = j + 1
                    break
                j += 1
            end = min(end, n)
            items.append((i, end, "string"))
            i = run = end
        elif (c == "r" or c == "b") and (i == 0 or not ident(text[i - 1])):
            length = _rust_raw_or_byte(text, i)
            if length:
                _flush(items, text, run, i)
                items.append((i, i + length, "string"))
                i = run = i + length
            else:
                i += 1
        elif c == "'":
            length = _rust_char_at(text, i)
            if length:
                _flush(items, text, run, i)
                items.append((i, i + length, "string"))
                i = run = i + length
            else:
                i += 1
        else:
            i += 1
    _flush(items, text, run, n)
    return items


_JS_REGEX_KEYWORDS = {"return", "case", "typeof", "instanceof", "in", "of", "new",
                      "delete", "void", "throw", "else", "do", "yield", "await"}
_JS_DIVISION_PUNCT = {")", "]"}


def _js_regex_allowed(seen_sig, last_word, last_punct, last_literal):
    if not seen_sig:
        return True
    if last_literal:
        return False
    if last_word:
        return last_word in _JS_REGEX_KEYWORDS
    return last_punct not in _JS_DIVISION_PUNCT


def _js_regex(text, i, items):
    n = len(text)
    j = i + 1
    in_class = False
    end = n
    closed = False
    while j < n:
        c = text[j]
        if c == "\\":
            j += 2
            continue
        if c == "\n":
            end = j
            break
        if in_class:
            if c == "]":
                in_class = False
        elif c == "[":
            in_class = True
        elif c == "/":
            end = j + 1
            closed = True
            break
        j += 1
    k = end
    if closed:
        while k < n and text[k].isalpha():
            k += 1
    items.append((i, k, "string"))
    return k


def _js_quoted(text, i, items):
    n = len(text)
    q = text[i]
    j = i + 1
    end = n
    while j < n:
        if text[j] == "\\":
            j += 2
            continue
        if text[j] == q:
            end = j + 1
            break
        if text[j] == "\n":
            end = j
            break
        j += 1
    end = min(end, n)
    items.append((i, end, "string"))
    return end


def _js_interpolation(text, j, items):
    return _js_code(text, j, items, True)


def _js_template(text, i, items):
    n = len(text)
    j = i + 1
    chunk = i

    def flush_to(to):
        if to > chunk:
            items.append((chunk, to, "string"))

    while j < n:
        c = text[j]
        if c == "\\":
            j += 2
            continue
        if c == "`":
            flush_to(j)
            return j + 1
        if c == "$" and text[j + 1:j + 2] == "{":
            flush_to(j)
            j = _js_interpolation(text, j + 2, items)
            chunk = j
            continue
        j += 1
    flush_to(n)
    return n


def _js_code(text, start, items, until_brace):
    n = len(text)
    i = start
    run = start
    last_word = ""
    last_punct = ""
    last_literal = False
    seen_sig = False
    depth = 1
    while i < n:
        c = text[i]
        if text[i:i + 2] == "//":
            _flush(items, text, run, i)
            end = text.find("\n", i)
            if end == -1:
                end = n
            items.append((i, end, "comment"))
            i = run = end
            continue
        if text[i:i + 2] == "/*":
            _flush(items, text, run, i)
            e = text.find("*/", i + 2)
            end = n if e == -1 else e + 2
            items.append((i, end, "comment"))
            i = run = end
            continue
        if c == "/" and _js_regex_allowed(seen_sig, last_word, last_punct, last_literal):
            _flush(items, text, run, i)
            i = _js_regex(text, i, items)
            run = i
            last_word = ""
            last_punct = ""
            last_literal = True
            seen_sig = True
            continue
        if c == "\"" or c == "'":
            _flush(items, text, run, i)
            i = _js_quoted(text, i, items)
            run = i
            last_word = ""
            last_punct = ""
            last_literal = True
            seen_sig = True
            continue
        if c == "`":
            _flush(items, text, run, i)
            i = _js_template(text, i, items)
            run = i
            last_word = ""
            last_punct = ""
            last_literal = True
            seen_sig = True
            continue
        if c == "}" and until_brace:
            depth -= 1
            if depth == 0:
                _flush(items, text, run, i)
                return i + 1
        if c == "{":
            depth += 1
        if not c.isspace():
            seen_sig = True
            if c.isalnum() or c in "_$":
                last_word += c
                last_punct = ""
            else:
                last_word = ""
                last_punct = c
            last_literal = False
        i += 1
    _flush(items, text, run, n)
    return n


def _scan_js(text):
    items = []
    _js_code(text, 0, items, False)
    return items


def _segments(text, lang):
    if lang == "python":
        return _scan_python(text)
    if lang == "rust":
        return _scan_rust(text)
    if lang == "javascript":
        return _scan_js(text)
    return _scan_json(text)


def _comment_marks(text, items, lang):
    """(start, end, preserved) for every comment segment."""
    marks = []
    for s, e, kind in items:
        if kind != "comment":
            continue
        body = text[s:e]
        preserved = LANDMINE in body or (lang == "python" and s == 0 and body.startswith("#!"))
        marks.append((s, e, preserved))
    return marks


def _count_lines(text, items):
    comment_lines = set()
    code_lines = set()
    line = 0
    for s, e, kind in items:
        for p in range(s, e):
            ch = text[p]
            if ch == "\n":
                line += 1
            elif kind == "comment":
                comment_lines.add(line)
            elif ch not in " \t\r\f\v":
                code_lines.add(line)
    return len(comment_lines), len(code_lines)


def strip_comments(text, lang):
    """Returns (stripped_text, removed_count). LANDMINE comments and the python shebang
    survive; one output line per input line so file:line references stay valid."""
    marks = _comment_marks(text, _segments(text, lang), lang)
    drops = [(s, e) for s, e, preserved in marks if not preserved]
    if not drops:
        return text, 0
    parts = []
    pos = 0
    for s, e in drops:
        parts.append(text[pos:s])
        for p in range(s, e):
            if text[p] == "\n":
                parts.append("\n")
        pos = e
    parts.append(text[pos:])
    stripped = "".join(parts)
    newlines = [p for p, ch in enumerate(text) if ch == "\n"]

    def line_of(pos_):
        return bisect.bisect_left(newlines, pos_)

    touched = set()
    for s, e in drops:
        last = e - 1
        while last >= s and text[last] == "\n":
            last -= 1
        for ln in range(line_of(s), line_of(last) + 1):
            touched.add(ln)
    lines = stripped.split("\n")
    for ln in touched:
        lines[ln] = lines[ln].rstrip()
    return "\n".join(lines), len(drops)


def _default_root():
    return os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def scan_files(root):
    files = {}
    for dirpath, dirnames, filenames in os.walk(root):
        dirnames[:] = sorted(d for d in dirnames
                             if d not in SKIP_DIRS and not d.startswith("."))
        for name in sorted(filenames):
            lang = LANG_BY_EXT.get(os.path.splitext(name)[1].lower())
            if not lang:
                continue
            full = os.path.join(dirpath, name)
            if os.path.islink(full):
                continue
            rel = os.path.relpath(full, root).replace(os.sep, "/")
            files[rel] = lang
    return files


def _read_text(path):
    with open(path, "rb") as fh:
        raw = fh.read()
    return raw, raw.decode("utf-8")


def audit_data(root):
    rows = []
    landmines = []
    undecodable = []
    for rel, lang in sorted(scan_files(root).items()):
        path = os.path.join(root, rel.replace("/", os.sep))
        try:
            _raw, text = _read_text(path)
        except UnicodeDecodeError:
            undecodable.append(rel)
            continue
        except OSError:
            continue
        items = _segments(text, lang)
        comments, code = _count_lines(text, items)
        rows.append({"path": rel, "lang": lang, "comments": comments, "code": code,
                     "density": round(comments / max(1, code), 4)})
        for s, e, preserved in _comment_marks(text, items, lang):
            if preserved and LANDMINE in text[s:e]:
                landmines.append({"path": rel, "line": text.count("\n", 0, s) + 1,
                                  "text": " ".join(text[s:e].split())})
    rows.sort(key=lambda r: (-r["density"], r["path"]))
    total_comments = sum(r["comments"] for r in rows)
    total_code = sum(r["code"] for r in rows)
    langs = {}
    for r in rows:
        langs[r["lang"]] = langs.get(r["lang"], 0) + 1
    totals = {"files": len(rows), "comments": total_comments, "code": total_code,
              "density": round(total_comments / max(1, total_code), 4),
              "languages": langs, "undecodable": len(undecodable)}
    return {"files": rows, "landmines": landmines, "totals": totals,
            "undecodable": undecodable}


def _summary_line(tag, data):
    t = data["totals"]
    return (f"declutter: {tag} files={t['files']} comments={t['comments']} "
            f"code={t['code']} density={t['density']:.3f} "
            f"landmines={len(data['landmines'])} undecodable={t['undecodable']}")


def cmd_audit(root, as_json):
    data = audit_data(root)
    if as_json:
        print(json.dumps(data, ensure_ascii=True, separators=(",", ":")))
        return 0
    print(_summary_line("audit", data))
    for r in data["files"]:
        print(f"declutter: {r['path']}  lang={r['lang']}  comments={r['comments']}  "
              f"code={r['code']}  density={r['density']:.3f}")
    for m in data["landmines"]:
        print(f"declutter: landmine {m['path']}:{m['line']}  {m['text'][:72]}")
    for rel in data["undecodable"]:
        print(f"declutter: undecodable {rel} (not utf-8, skipped)")
    return 0


def _budget_path(root):
    return os.path.join(root, BUDGET_REL)


def _load_budget(path):
    if not os.path.exists(path):
        return None, "no baseline at .agent/comment-budget.json -- run budget init first"
    try:
        with open(path, "r", encoding="utf-8-sig") as fh:
            doc = json.load(fh)
        files = doc.get("files")
        if not isinstance(files, dict):
            raise ValueError("files must be an object")
        out = {}
        for k, v in files.items():
            if not isinstance(v, int) or isinstance(v, bool) or v < 0:
                raise ValueError(f"baseline for {k} must be a non-negative integer")
            out[k] = v
        return out, ""
    except (OSError, ValueError) as exc:
        return None, f"baseline unreadable ({exc}) -- fail closed, refusing to judge"


def _save_budget(path, counts):
    doc = {"version": 1, "unit": "comment-lines",
           "note": ("per-file comment-line baseline; init only tightens (register new, "
                    "shrink stale, drop orphans) and refuses while any file grew -- "
                    "never raise a baseline"),
           "files": {k: counts[k] for k in sorted(counts)}}
    os.makedirs(os.path.dirname(path), exist_ok=True)
    tmp = path + ".partial"
    with open(tmp, "w", encoding="utf-8", newline="\n") as fh:
        json.dump(doc, fh, ensure_ascii=True, indent=2, sort_keys=True)
        fh.write("\n")
    os.replace(tmp, path)


def _comment_counts(root):
    counts = {}
    for rel, lang in scan_files(root).items():
        path = os.path.join(root, rel.replace("/", os.sep))
        try:
            _raw, text = _read_text(path)
        except UnicodeDecodeError:
            continue
        except OSError:
            continue
        comments, _code = _count_lines(text, _segments(text, lang))
        counts[rel] = comments
    return counts


def cmd_budget(sub, root):
    path = _budget_path(root)
    if sub == "init":
        counts = _comment_counts(root)
        if os.path.exists(path):
            base, err = _load_budget(path)
            if base is None:
                print(f"declutter: budget init refused -- {err}")
                return 1
            grew = sorted(r for r in counts if r in base and counts[r] > base[r])
            if grew:
                for rel in grew:
                    print(f"declutter: GREW {rel} {counts[rel]} > baseline {base[rel]} "
                          "-- init never raises a baseline")
                print("declutter: budget init REFUSED -- a baseline is only ever tightened")
                return 1
            registered = sorted(r for r in counts if r not in base)
            removed = sorted(r for r in base if r not in counts)
            tightened = sorted(r for r in counts if r in base and counts[r] < base[r])
            _save_budget(path, counts)
            print(f"declutter: budget tightened files={len(counts)} "
                  f"registered={len(registered)} tightened={len(tightened)} "
                  f"removed={len(removed)} total-comments={sum(counts.values())}")
            return 0
        _save_budget(path, counts)
        print(f"declutter: budget baseline written files={len(counts)} "
              f"total-comments={sum(counts.values())} at .agent/comment-budget.json")
        return 0
    base, err = _load_budget(path)
    if base is None:
        print(f"declutter: budget check refused -- {err}")
        return 1
    counts = _comment_counts(root)
    failures = []
    notices = []
    for rel in sorted(counts):
        if rel not in base:
            failures.append(f"UNCAPPED: {rel} ({counts[rel]} comment lines) is not in the "
                            "baseline -- budget init registers it")
            continue
        if counts[rel] > base[rel]:
            failures.append(f"GREW: {rel} {counts[rel]} > baseline {base[rel]}")
        elif counts[rel] < base[rel]:
            notices.append(f"STALE: {rel} shrank to {counts[rel]} (baseline {base[rel]}) "
                           "-- budget init tightens")
    for rel in sorted(base):
        if rel not in counts:
            notices.append(f"ORPHAN: {rel} is gone from the scan surface -- budget init removes")
    for n in notices:
        print(f"declutter: {n}")
    for f in failures:
        print(f"declutter: {f}")
    if failures:
        print(f"declutter: budget check RED -- {len(failures)} file(s) over baseline, "
              "baselines only tighten")
        return 1
    print(f"declutter: budget check ok files={len(counts)} "
          f"total-comments={sum(counts.values())}")
    return 0


def cmd_export(root, out_arg, do_strip):
    out_arg = out_arg.strip()
    if not out_arg:
        print("declutter: export needs --out <dir>")
        return 2
    out_abs = os.path.abspath(out_arg).rstrip("/\\")
    if trash.is_inside(out_abs, root):
        print("declutter: refuse -- --out is inside --root; export must never write into "
              "the source tree (give a directory outside the tree)")
        return 1
    if os.path.exists(out_abs):
        if not os.path.isdir(out_abs):
            print("declutter: refuse -- --out exists and is not a directory")
            return 1
        if os.listdir(out_abs):
            print("declutter: refuse -- --out exists and is not empty (never merge into "
                  "an existing view)")
            return 1
    else:
        os.makedirs(out_abs)
    files_n = stripped_n = verbatim_n = removed = 0
    for rel, lang in sorted(scan_files(root).items()):
        src = os.path.join(root, rel.replace("/", os.sep))
        dst = os.path.join(out_abs, rel.replace("/", os.sep))
        parent = os.path.dirname(dst)
        if parent:
            os.makedirs(parent, exist_ok=True)
        with open(src, "rb") as fh:
            raw = fh.read()
        if do_strip:
            try:
                text = raw.decode("utf-8")
            except UnicodeDecodeError:
                with open(dst, "wb") as fh:
                    fh.write(raw)
                verbatim_n += 1
                files_n += 1
                continue
            stripped, ndrops = strip_comments(text, lang)
            if ndrops:
                with open(dst, "wb") as fh:
                    fh.write(stripped.encode("utf-8"))
                stripped_n += 1
                removed += ndrops
            else:
                with open(dst, "wb") as fh:
                    fh.write(raw)
                verbatim_n += 1
        else:
            with open(dst, "wb") as fh:
                fh.write(raw)
            verbatim_n += 1
        files_n += 1
    print(f"declutter: export files={files_n} stripped={stripped_n} verbatim={verbatim_n} "
          f"comments-removed={removed} out={out_arg}")
    return 0


def cmd_clean(root, in_place):
    if not in_place:
        print("declutter: clean refused -- --in-place is missing")
        print("  clean is the destructive pass: it rewrites files in the source tree, so")
        print("  it must cost an explicit flag. With --in-place every original goes into")
        print("  the trash bin first (reason declutter-clean) and the stripped file is")
        print("  written back; any failure stops the run and the bin restores originals:")
        print("  python tools/trash.py restore <entry-id> -Root <owner-root>")
        print("  Run audit first and look before you leap.")
        return 1
    before = audit_data(root)
    print(_summary_line("audit-before", before))
    store = os.path.join(root, ".journal")
    worklist = []
    for rel, lang in sorted(scan_files(root).items()):
        path = os.path.join(root, rel.replace("/", os.sep))
        try:
            _raw, text = _read_text(path)
        except UnicodeDecodeError:
            print(f"declutter: clean skips {rel} (not utf-8, never guessed at)")
            continue
        except OSError as exc:
            print(f"declutter: clean skips {rel} (unreadable: {exc})")
            continue
        drops = [(s, e) for s, e, preserved in _comment_marks(text, _segments(text, lang), lang)
                 if not preserved]
        if drops:
            worklist.append((rel, lang, path, text, len(drops)))
    if not worklist:
        print("declutter: clean nothing to do -- no strippable comments on the scan surface")
        return 0
    for rel, lang, path, text, ndrops in worklist:
        entries, err = trash.put_paths(root, store, [rel], CLEAN_REASON)
        if err:
            print(f"declutter: clean stopped before changing {rel} -- trash refused: {err}")
            print("declutter: fail closed, run aborted, nothing further was touched")
            return 1
        entry_id = entries[0]["id"] if entries else "?"
        try:
            stripped, nd = strip_comments(text, lang)
            if nd != ndrops:
                raise OSError(f"strip count drifted between plan ({ndrops}) and write ({nd})")
            with open(path, "wb") as fh:
                fh.write(stripped.encode("utf-8"))
            _raw, reread = _read_text(path)
            check = [(s, e) for s, e, preserved
                     in _comment_marks(reread, _segments(reread, lang), lang)
                     if not preserved]
            if check:
                raise OSError("written file still has strippable comments")
        except (OSError, UnicodeDecodeError) as exc:
            print(f"declutter: clean FAILED on {rel} after binning the original ({exc})")
            print(f"declutter: fail closed, run aborted; original is safe in the bin as "
                  f"{entry_id}, restore with:")
            print(f"declutter:   python tools/trash.py restore {entry_id} -Root <owner-root>")
            return 1
        print(f"declutter: clean {rel} removed={nd} original={entry_id}")
    after = audit_data(root)
    print(_summary_line("audit-after", after))
    print(f"declutter: clean done files={len(worklist)} originals restorable from the bin")
    return 0


def parse_args(argv):
    out = {"mode": "", "sub": "", "root": "", "out": "",
           "json": False, "strip": False, "in_place": False, "bad_flag": ""}
    positional = []
    i = 0
    while i < len(argv):
        a = argv[i]
        if not a.startswith("-"):
            positional.append(a)
            i += 1
            continue
        name = a.lstrip("-").replace("-", "_").lower()
        if name == "json":
            out["json"] = True
            i += 1
            continue
        if name == "strip":
            out["strip"] = True
            i += 1
            continue
        if name == "in_place":
            out["in_place"] = True
            i += 1
            continue
        if name in ("root", "out"):
            out[name] = argv[i + 1] if i + 1 < len(argv) else ""
            i += 2
            continue
        out["bad_flag"] = a
        break
    if positional and positional[0] in ("audit", "budget", "export", "clean"):
        out["mode"] = positional[0]
        rest = positional[1:]
        if out["mode"] == "budget":
            out["sub"] = rest[0] if rest else ""
    return out


def usage(code):
    print("""declutter: shrink the comment mass in a codebase -- explanations live in docs/,
models read code. The tool measures (audit), ratchets (budget), renders a comment-free
view (export) and, only on explicit command, cleans (clean) with every original first
moved into the trash bin.

The LANDMINE convention, written once here and enforced everywhere:
  A comment containing the marker token LANDMINE marks an old problem that takes the
  whole system down when touched. It is never stripped by export/clean, and audit lists
  every one separately. Without the marker a comment is fair game for cleaning -- the
  marker is an exemption channel, not a style, so do not sprinkle it on fresh comments.

usage:
  declutter.py audit [--json] [--root <dir>]
      Comment-density report: per file comments/code/density/language, sorted by
      density (worst first), plus library totals; LANDMINE comments listed separately.
      --json prints a single ASCII line with the same data.

  declutter.py budget init|check [--root <dir>]
      Per-file comment-line ratchet in .agent/comment-budget.json, same semantics as
      scripts/module_budget.py. check goes red (exit 1) on GREW (a file holds more
      comment lines than its baseline), on UNCAPPED (a scanned file missing from the
      baseline) and on an unreadable baseline (fail closed); STALE/ORPHAN are notices,
      not red. init writes the baseline; on an existing baseline it only tightens --
      registers new files, shrinks stale caps, drops orphans -- and REFUSES while any
      file grew. A baseline is never raised; raising would legalize the fat.

  declutter.py export --strip --out <dir> [--root <dir>]
      A reading view for models: copies the tree into <dir> (the source tree is never
      touched) and, with --strip, removes non-LANDMINE comments from python/rust/js/json
      files. --out must live outside --root: an export inside the tree would dirty it.
      Output keeps one line per source line, so file:line references stay valid. The
      convention for what a stripped comment was carrying: it belongs in docs/, not in
      this view.

  declutter.py clean --in-place [--root <dir>]
      Destructive and gated. Without --in-place it exits without touching anything, on
      purpose: a bulk rewrite that goes wrong must cost a deliberate flag. With it, per
      file: the original is first moved into the trash bin (tools/trash.py, reason
      declutter-clean), then comments are stripped and the result written back, then the
      written file is re-scanned as a self-check; audit summaries print before and after.
      The bin site is <Root>/.journal/trash and must be git-ignored (trash refuses
      otherwise). Any failure stops the whole run (fail closed). Restores refuse to
      overwrite the cleaned file -- move it aside, then
      python tools/trash.py restore <entry-id> -Root <owner-root>.

What stripping does NOT guarantee (honesty boundaries, restated here because tools lie
less than memories do):
  - Comment-looking text inside string literals is best effort: the scanner runs a
    three-state walk (code / string / comment), but exotic literal forms are not
    guaranteed to be recognized as strings, and no promise is made about them.
  - python docstrings are strings, not comments: neither stripped nor counted.
  - js regex literals ride on a last-token heuristic; a wrong guess is bounded to one
    line because a regex scan stops at end of line.
  - An unterminated string makes the scanner treat the rest of the file as string: it
    errs toward not touching, never toward deleting.
  - A python shebang line survives; files that do not decode as UTF-8 are skipped
    (audit/clean) or copied verbatim (export), never guessed at; symlinks are skipped.

exit codes: 0 ok, 1 refused or failed, 2 bad invocation.""")
    return code


def main(argv):
    a = parse_args(argv)
    if a["bad_flag"]:
        print(f"declutter: unknown flag {a['bad_flag']} -- refusing to guess what it meant")
        return 2
    if not a["mode"]:
        return usage(2)
    if a["mode"] == "budget" and a["sub"] not in ("init", "check"):
        print("declutter: budget needs a subcommand: init or check")
        return 2
    root_in = a["root"].strip() or _default_root()
    if not os.path.isdir(root_in):
        print("declutter: --root must be an existing directory (no cwd guessing)")
        return 2
    root = os.path.abspath(root_in).rstrip("/\\")
    if a["mode"] == "audit":
        return cmd_audit(root, a["json"])
    if a["mode"] == "budget":
        return cmd_budget(a["sub"], root)
    if a["mode"] == "export":
        return cmd_export(root, a["out"], a["strip"])
    return cmd_clean(root, a["in_place"])


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
