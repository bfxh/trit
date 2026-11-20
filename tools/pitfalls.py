#!/usr/bin/env python3
"""pitfalls: the collaboration-pain corpus -- real operational pitfalls, one JSON line
each, written so the NEXT coding model can be taught what actually goes wrong before it
does it again.

    pitfalls.py add --cat <c> --trigger <s> --consequence <s> --prevention <s>
                    [--source <s>] [-Root <dir>]
    pitfalls.py list [--cat <c>] [-Root <dir>]
    pitfalls.py export --jsonl <path> [--cat <c>] [-Root <dir>]
    pitfalls.py count [-Root <dir>]
    pitfalls.py seed [-Root <dir>]

Store: <Root>/.agent/pitfalls.jsonl, inside the repo on purpose -- the corpus travels
with the project and is meant to be committed. One JSON object per line: {id, cat,
trigger, consequence, prevention, source, ts}. cat is one of git|github|infra|api|
semantics: collaboration pain is not only git -- review flow, the public tree, CI
platforms and API constants bite the same way.

seed: one-shot injection of the founding entries, distilled from docs/ERRORS.md
(L15/L63/L93/L95-L100) plus the two collaboration rules this tool exists for (force-push
after review; machine paths into the public tree). Idempotent by id: an id already in
the store is never written twice.

LANDMINE: seed ids are load-bearing. Renaming a seed id re-imports the entry and splits
the corpus; the selftest pins the ids.

Honest boundaries: a corpus informs, it does not enforce -- nothing here refuses a
dangerous diff, that is policy/constraints' job; this is what the next model reads.
Entries are added by whoever runs the tool: the privacy refusal below catches machine
user paths, but the rest of an entry is only as true as its source line. Concurrent
writers overwrite each other's line (last write wins) -- this is a corpus, not a ledger.
stdout is pure ASCII by design: non-ASCII text is escaped, never dropped. Exit 0 ok,
1 refused or failed, 2 bad invocation.
"""

import datetime
import hashlib
import json
import os
import re
import sys

MODES = {"add", "list", "export", "count", "seed"}
CATS = ("git", "github", "infra", "api", "semantics")

# privacy refusal (L93): the corpus is meant for the public tree, so a machine user
# path in an entry is a leak, not a note. Hard rule only -- drive-letter work roots are
# left to scripts/privacy_scan.py's ratchet, which sees the committed file.
USER_PATH = re.compile(
    r"[A-Za-z]:\\Users\\"
    r"|/Users/[A-Za-z0-9._-]+/"
    r"|/home/[A-Za-z0-9._-]+/"
    r"|/mnt/[a-z]/"
)


def parse_args(argv):
    out = {"mode": "", "cat": "", "trigger": "", "consequence": "", "prevention": "",
           "source": "", "jsonl": "", "root": ""}
    positional = []
    i = 0
    while i < len(argv):
        a = argv[i]
        if not a.startswith("-"):
            positional.append(a)
            i += 1
            continue
        name = a.lstrip("-").lower()
        key = {"cat": "cat", "trigger": "trigger", "consequence": "consequence",
               "prevention": "prevention", "source": "source", "jsonl": "jsonl",
               "root": "root"}.get(name, name)
        if key in out and key != "mode":
            out[key] = argv[i + 1] if i + 1 < len(argv) else ""
            i += 2
            continue
        i += 1
    if positional and positional[0] in MODES:
        out["mode"] = positional[0]
    return out


def asc(s):
    """stdout is ASCII-only: escape anything else, drop nothing."""
    return json.dumps(str(s), ensure_ascii=True)[1:-1]


def store_path(root):
    return os.path.join(root, ".agent", "pitfalls.jsonl")


def load_entries(path):
    """Every line must parse -- a corrupt line refuses the whole read (fail closed:
    half-reading a corpus quietly teaches the next model half the truth)."""
    if not os.path.exists(path):
        return []
    out = []
    with open(path, "r", encoding="utf-8-sig") as fh:
        for n, line in enumerate(fh, 1):
            line = line.strip()
            if not line:
                continue
            try:
                out.append(json.loads(line))
            except ValueError:
                raise ValueError(f"corrupt JSON at line {n} of {path} -- fix it by hand, "
                                 "nothing was written")
    return out


def save_entries(path, entries):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    tmp = path + ".partial"
    with open(tmp, "w", encoding="utf-8", newline="\n") as fh:
        for e in entries:
            fh.write(json.dumps(e, ensure_ascii=False) + "\n")
    os.replace(tmp, path)


def entry_id(cat, trigger):
    return "p" + hashlib.sha256(f"{cat}\0{trigger}".encode("utf-8")).hexdigest()[:12]


def check_privacy(fields):
    for name, value in fields:
        if value and USER_PATH.search(value):
            return name
    return ""


def entry_valid(e):
    return (isinstance(e, dict) and e.get("id") and e.get("cat") in CATS
            and e.get("trigger") and e.get("consequence") and e.get("prevention"))


def cmd_add(a, root, path):
    cat = a.get("cat", "").strip()
    trigger = a.get("trigger", "").strip()
    consequence = a.get("consequence", "").strip()
    prevention = a.get("prevention", "").strip()
    source = a.get("source", "").strip()
    if cat not in CATS:
        print(f"pitfalls: -Cat must be one of {'|'.join(CATS)}")
        return 2
    if not trigger or not consequence or not prevention:
        print("pitfalls: add needs --trigger, --consequence and --prevention "
              "(a pitfall without its consequence teaches nothing)")
        return 2
    leak = check_privacy([("trigger", trigger), ("consequence", consequence),
                          ("prevention", prevention), ("source", source)])
    if leak:
        print(f"pitfalls: refuse -- {leak} carries a machine user path (privacy gate, L93); "
              "the corpus is committed to the public tree -- rewrite it machine-free")
        return 1
    now = datetime.datetime.now().astimezone().isoformat(timespec="seconds")
    eid = entry_id(cat, trigger)
    try:
        entries = load_entries(path)
    except ValueError as exc:
        print(f"pitfalls: refuse -- {exc}")
        return 1
    updated = False
    for e in entries:
        if e.get("id") == eid:
            e.update({"consequence": consequence, "prevention": prevention,
                      "source": source, "ts": now})
            updated = True
            break
    if not updated:
        entries.append({"id": eid, "cat": cat, "trigger": trigger,
                        "consequence": consequence, "prevention": prevention,
                        "source": source, "ts": now})
    save_entries(path, entries)
    verb = "updated" if updated else "added"
    print(f"pitfalls: {verb} {eid} cat={cat} total={len(entries)}")
    return 0


def select(entries, cat):
    if not cat:
        return entries
    return [e for e in entries if e.get("cat") == cat]


def cmd_list(a, root, path):
    cat = a.get("cat", "").strip()
    if cat and cat not in CATS:
        print(f"pitfalls: -Cat must be one of {'|'.join(CATS)}")
        return 2
    try:
        entries = load_entries(path)
    except ValueError as exc:
        print(f"pitfalls: refuse -- {exc}")
        return 1
    rows = select(entries, cat)
    scope = f" cat={cat}" if cat else ""
    print(f"pitfalls: store .agent/pitfalls.jsonl ({len(rows)} entry(ies){scope})")
    for e in rows:
        if not entry_valid(e):
            print(f"pitfalls: {asc(e.get('id', '?'))}  (malformed entry, skipped)")
            continue
        print(f"pitfalls: {asc(e['id'])}  {e['cat']}: {asc(e['trigger'][:88])}"
              f"  => {asc(e['consequence'][:72])}")
    return 0


def cmd_export(a, root, path):
    cat = a.get("cat", "").strip()
    if cat and cat not in CATS:
        print(f"pitfalls: -Cat must be one of {'|'.join(CATS)}")
        return 2
    target = a.get("jsonl", "").strip()
    if not target:
        print("pitfalls: export needs --jsonl <path> (the ML-ready line-per-entry file)")
        return 2
    try:
        entries = load_entries(path)
    except ValueError as exc:
        print(f"pitfalls: refuse -- {exc}")
        return 1
    if os.path.abspath(target) == os.path.abspath(path):
        print("pitfalls: refuse -- export target is the store itself")
        return 1
    parent = os.path.dirname(os.path.abspath(target))
    if not os.path.isdir(parent):
        print(f"pitfalls: refuse -- export target directory does not exist: {asc(parent)}")
        return 1
    rows = select(entries, cat)
    with open(target, "w", encoding="utf-8", newline="\n") as fh:
        for e in rows:
            fh.write(json.dumps(e, ensure_ascii=False) + "\n")
    print(f"pitfalls: exported {len(rows)} line(s) -> {asc(target)}")
    return 0


def cmd_count(a, root, path):
    try:
        entries = load_entries(path)
    except ValueError as exc:
        print(f"pitfalls: refuse -- {exc}")
        return 1
    counts = {}
    for e in entries:
        counts[e.get("cat", "?")] = counts.get(e.get("cat", "?"), 0) + 1
    parts = " ".join(f"{c}={counts.get(c, 0)}" for c in CATS)
    print(f"pitfalls: count={len(entries)} {parts}")
    return 0


def cmd_seed(a, root, path):
    try:
        entries = load_entries(path)
    except ValueError as exc:
        print(f"pitfalls: refuse -- {exc}")
        return 1
    taken = {e.get("id") for e in entries}
    added = kept = 0
    for s in SEEDS:
        if s["id"] in taken:
            kept += 1
            print(f"pitfalls: seed kept {s['id']} (already present)")
            continue
        entries.append(dict(s, ts=datetime.datetime.now().astimezone()
                            .isoformat(timespec="seconds")))
        taken.add(s["id"])
        added += 1
        print(f"pitfalls: seed added {s['id']}")
    save_entries(path, entries)
    print(f"pitfalls: seed done (added={added} kept={kept} total={len(entries)})")
    return 0


# The founding corpus: distilled from docs/ERRORS.md real lines plus the two
# collaboration rules (GitHub domain) this tool exists to carry. English text on
# purpose -- the corpus is training data and stdout stays ASCII. Ids are load-bearing.
SEEDS = [
    {
        "id": "pf-git-001", "cat": "git",
        "trigger": "git reset --hard while new files exist only in the index (never committed)",
        "consequence": "the staged-only files are deleted and live in no commit, no stash, "
                       "no reflog -- the work is gone without a trace",
        "prevention": "segment the commit chain, or stash first; never assume reset "
                      "preserves what only the index holds",
        "source": "docs/ERRORS.md L15",
    },
    {
        "id": "pf-gh-001", "cat": "github",
        "trigger": "force-pushing a branch after review comments were already given on it",
        "consequence": "the PR loses its review context: comments anchor to lines and "
                       "commits that no longer exist, approvals are invalidated, and the "
                       "discussion history is orphaned",
        "prevention": "append a fixup commit and let reviewers re-review the delta; "
                      "reserve force push for branches nobody has reviewed",
        "source": "collaboration-pain rule (no ledger line yet)",
    },
    {
        "id": "pf-gh-002", "cat": "github",
        "trigger": "committing task docs, agent memory logs, or machine-absolute paths "
                   "to a PUBLIC repository",
        "consequence": "usernames, home-directory layout and internal plans leak into the "
                       "public tree; secret_scan only recognizes key shapes, not content "
                       "that belongs to the public face",
        "prevention": "run the privacy gate (scripts/privacy_scan.py, shrink-only ratchet) "
                      "before pushing; keep local state under ignored paths",
        "source": "docs/ERRORS.md L93",
    },
    {
        "id": "pf-git-002", "cat": "git",
        "trigger": "restore/cleanup logic that deletes 'extra' files directly (os.remove) "
                   "inside a tool",
        "consequence": "deletion bypasses every ledger -- no entry, no reason, no way back; "
                       "one wrong snapshot silently destroys newer work",
        "prevention": "route every deletion through the recycle bin (trash put_paths) and "
                      "fail the restore when the bin refuses",
        "source": "docs/ERRORS.md L95",
    },
    {
        "id": "pf-git-003", "cat": "git",
        "trigger": "parsing git diff with only '+++ b/<path>' as the file header",
        "consequence": "whole-file deletions print '+++ /dev/null' with no b/ line, so "
                       "deleted files never enter the parser's table and every rule scoped "
                       "to them silently skips",
        "prevention": "keep the '--- a/<path>' as pending and mark it deleted on "
                      "'+++ /dev/null' (the L96 fix in tools/constraints.py)",
        "source": "docs/ERRORS.md L96",
    },
    {
        "id": "pf-api-001", "cat": "api",
        "trigger": "writing Win32 Job Object class ids and LimitFlags from memory",
        "consequence": "a wrong class id reports 'invalid parameter' (87) and a wrong flag "
                       "bit masquerades as a length error (24) -- the error text points at "
                       "the wrong layer and the fix drifts",
        "prevention": "derive constants by scanning with an all-zero buffer for the "
                      "accepted (class, size) pair and verify each flag bit separately; "
                      "fail closed when the call refuses",
        "source": "docs/ERRORS.md L97",
    },
    {
        "id": "pf-sem-001", "cat": "semantics",
        "trigger": "adding an orchestration step without updating the expected-step-sequence "
                   "assertion in the selftest",
        "consequence": "a never-green assertion sits in the suite looking like coverage -- "
                       "expectation and implementation have drifted",
        "prevention": "derive the expected step list from the orchestration code's "
                      "unconditional adds (skipped steps still appear) and change both "
                      "sides in one commit",
        "source": "docs/ERRORS.md L98",
    },
    {
        "id": "pf-sem-002", "cat": "semantics",
        "trigger": "selftest fixtures referencing a machine-specific repo root, or files "
                   "that an upstream refactor has deleted",
        "consequence": "assertions pass on one machine as an environment lottery and fail "
                       "everywhere else, or reference files that no longer exist",
        "prevention": "derive every fixture path from the test's own location and assert "
                      "against files verified to exist in the tree",
        "source": "docs/ERRORS.md L99",
    },
    {
        "id": "pf-infra-001", "cat": "infra",
        "trigger": "asserting on Chinese output piped between processes on a Windows console",
        "consequence": "the writer encodes by locale (GBK) while the reader decodes by "
                       "console codepage -- the same code is green in a UTF-8 console and "
                       "pseudo-red in a GBK one",
        "prevention": "pin UTF-8 on both sides (console OutputEncoding plus "
                      "PYTHONUTF8/PYTHONIOENCODING) and keep machine-readable receipts "
                      "pure ASCII",
        "source": "docs/ERRORS.md L100",
    },
    {
        "id": "pf-infra-002", "cat": "infra",
        "trigger": "committing shell scripts from Windows, where git does not record the "
                   "exec bit",
        "consequence": "scripts land as 100644 and CI on linux fails with exit 126 "
                       "Permission denied, far from the real cause",
        "prevention": "git update-index --chmod=+x the scripts (the permission bit is part "
                      "of the script); assert executability from the index, cross-platform",
        "source": "docs/ERRORS.md L63",
    },
]


def usage(code):
    print("""pitfalls: the collaboration-pain corpus -- real pitfalls, one JSON line each,
written so the next coding model can learn what actually goes wrong (not only git:
github review flow, the public tree, CI platforms, API constants).

usage:
  tools/pitfalls.py add --cat <git|github|infra|api|semantics> --trigger <s>
                        --consequence <s> --prevention <s> [--source <s>] [-Root <dir>]
  tools/pitfalls.py list [--cat <c>] [-Root <dir>]
  tools/pitfalls.py export --jsonl <path> [--cat <c>] [-Root <dir>]
  tools/pitfalls.py count [-Root <dir>]
  tools/pitfalls.py seed [-Root <dir>]

store: <Root>/.agent/pitfalls.jsonl (committed on purpose -- the corpus travels with the
project). seed is idempotent by id. boundary: a corpus informs, it does not enforce --
policy/constraints do the refusing, this is what the next model reads. Content carrying
a machine user path is refused (privacy gate, L93). stdout stays pure ASCII; exit
0 ok, 1 refused or failed, 2 bad invocation.""")
    return code


def main(argv):
    a = parse_args(argv)
    if not a["mode"]:
        return usage(2)
    root_in = a.get("root", "").strip()
    if not root_in or not os.path.isdir(root_in):
        print("pitfalls: -Root must be an existing directory "
              "(no store probed without an owner root)")
        return 2
    path = store_path(os.path.abspath(root_in).rstrip("/\\"))
    if a["mode"] == "add":
        return cmd_add(a, root_in, path)
    if a["mode"] == "list":
        return cmd_list(a, root_in, path)
    if a["mode"] == "export":
        return cmd_export(a, root_in, path)
    if a["mode"] == "count":
        return cmd_count(a, root_in, path)
    return cmd_seed(a, root_in, path)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
