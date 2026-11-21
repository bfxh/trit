#!/usr/bin/env python3
"""journal: checkpoint the workspace so a failed experiment can be undone, and so the
tool layer can prove what a command actually wrote.

    journal.py begin   -Root D:\\proj   -> .journal/latest
    journal.py end     -Root D:\\proj   -> diff; exit 1 on change
    journal.py restore -Root D:\\proj   -> put the tree back
    journal.py status  -Root D:\\proj

Why this exists: a permission model that only says "deny"/"allow" is a promise. The journal
is the proof -- it records content hashes and copies every file before a risky step, so
afterwards you can answer "what changed" without trusting the command's exit code. Same role
as `/rewind` in Claude Code, except here it is a filesystem fact rather than a chat feature.

Scope, stated plainly: the journal covers the tree under -Root, with skip directories.
Writes outside that tree are NOT in scope -- use policy to refuse them up front. Covering
"the world" would mean an OS-level snapshot; this tool does not pretend to.

Ported from journal.ps1; messages and exit codes are kept identical.
"""

import datetime
import hashlib
import json
import os
import shutil
import sys

import trash  # same tools directory; restore never unlinks outright, it bins (see below)

MODES = {"begin", "end", "restore", "status"}

SKIP = {"node_modules", "target", ".git", ".journal", "__pycache__", ".venv", "dist", "build",
        "obj", ".qoder", ".pytest_cache", ".ruff_cache", ".mypy_cache", ".turbo", ".next", "coverage"}

MAX_BYTES = 8 * 1024 * 1024


def parse_args(argv):
    out = {"store": ""}
    positional = [x for x in argv if not x.startswith("-")]
    i = 0
    while i < len(argv):
        a = argv[i]
        if not a.startswith("-"):
            i += 1
            continue
        name = a.lstrip("-").lower()
        key = {"mode": "mode", "root": "root", "store": "store"}.get(name, name)
        out[key] = argv[i + 1] if i + 1 < len(argv) else ""
        i += 2
    if "mode" not in out and positional and positional[0] in MODES:
        out["mode"] = positional[0]
    return out


def walk_files(root):
    """Yield (rel, abspath, size) for every regular file worth snapshotting.

    Skip rules are by top-level directory only, and files >= 8MB are left out: a journal
    that silently copied a 5GB artifact would be a worse bug than the change it catches.
    """
    for dirpath, dirnames, filenames in os.walk(root):
        rel_dir = os.path.relpath(dirpath, root)
        if rel_dir == ".":
            rel_dir = ""
            dirnames[:] = [d for d in dirnames if d not in SKIP]
        for fn in filenames:
            full = os.path.join(dirpath, fn)
            rel = os.path.join(rel_dir, fn) if rel_dir else fn
            try:
                st = os.stat(full)
            except OSError:
                continue
            if st.st_size >= MAX_BYTES:
                continue
            yield rel, full, st.st_size


def sha256_of(path):
    h = hashlib.sha256()
    with open(path, "rb") as fh:
        for chunk in iter(lambda: fh.read(65536), b""):
            h.update(chunk)
    return h.hexdigest().upper()


def load_manifest(manifest_path):
    with open(manifest_path, "r", encoding="utf-8-sig") as fh:
        return json.load(fh)


def manifest_or_refuse(manifest_path, action):
    """Manifest or exit 2 with a clean diagnosis. A half-written manifest (a run killed
    between begin and end) must not raise a traceback that a caller's verdict mapping
    then misreads -- journal end previously exited 1 on the crash and agent-verify
    recorded that as verdict "changed", i.e. the audit chain broke and the receipt LIED
    about it (docs/ERRORS.md L114)."""
    try:
        return load_manifest(manifest_path)
    except (OSError, ValueError) as e:
        print(f"journal: manifest corrupt or unreadable ({e}) -- refusing to {action} "
              f"(fail-closed: a half-written snapshot must not read as clean)")
        sys.exit(2)


def cmd_begin(root, latest):
    manifest_path = os.path.join(latest, "manifest.json")
    if os.path.exists(manifest_path):
        try:
            load_manifest(manifest_path)
        except (OSError, ValueError) as e:
            print(f"journal: existing manifest is corrupt ({e}) -- a previous run did "
                  f"not finish cleanly; inspect .journal/latest (status) or remove it, "
                  f"then rerun (a corrupt snapshot must not be silently overwritten)")
            return 2
    files_dir = os.path.join(latest, "files")
    os.makedirs(files_dir, exist_ok=True)
    entries = []
    for rel, full, size in sorted(walk_files(root), key=lambda t: t[0].lower()):
        digest = sha256_of(full)
        entries.append({"rel": rel, "sha256": digest, "size": size})
        dest = os.path.join(files_dir, rel)
        os.makedirs(os.path.dirname(dest) or files_dir, exist_ok=True)
        shutil.copy2(full, dest)
    now = datetime.datetime.now().astimezone().isoformat(timespec="microseconds")
    manifest = {"root": root, "at": now, "count": len(entries), "files": entries}
    with open(os.path.join(latest, "manifest.json"), "w", encoding="utf-8", newline="\n") as fh:
        json.dump(manifest, fh, ensure_ascii=False, indent=4)
        fh.write("\n")
    print(f"journal: begin -- {len(entries)} files, copies under {files_dir}")
    return 0


def cmd_end(root, latest):
    manifest_path = os.path.join(latest, "manifest.json")
    if not os.path.exists(manifest_path):
        print("journal: no snapshot, run -Mode begin first")
        return 2
    man = manifest_or_refuse(manifest_path, "diff against it")
    prev = {e["rel"]: e["sha256"] for e in man["files"]}
    now = {rel: sha256_of(full) for rel, full, _ in walk_files(root)}

    added = sorted((k for k in now if k not in prev), key=str.lower)
    deleted = sorted((k for k in prev if k not in now), key=str.lower)
    modified = sorted((k for k in now if k in prev and prev[k] != now[k]), key=str.lower)

    for a in added:
        print(f"journal: added    {a}")
    for d in deleted:
        print(f"journal: deleted  {d}")
    for m in modified:
        print(f"journal: modified {m}")
    total = len(added) + len(deleted) + len(modified)
    print(f"journal: end -- {total} changed (added {len(added)}, deleted {len(deleted)}, "
          f"modified {len(modified)})")
    return 1 if total > 0 else 0


def cmd_restore(root, latest, store):
    manifest_path = os.path.join(latest, "manifest.json")
    if not os.path.exists(manifest_path):
        print("journal: no snapshot to restore")
        return 2
    man = manifest_or_refuse(manifest_path, "restore from it")
    keep = {e["rel"]: e for e in man["files"]}

    # files that appeared after the snapshot are not unlinked: they move into the recycle
    # bin (same store as the snapshot), so a restore never destroys content and every
    # removal carries the reason it happened. A refused move leaves the file in place and
    # turns the restore red -- an incomplete restore must not look like success.
    reason = f"journal-restore:{man['at']}"
    removed = 0
    trashed = []
    blocked = []
    for rel, full, _ in list(walk_files(root)):
        if rel not in keep:
            entries, err = trash.put_paths(root, store, [full], reason)
            if err:
                blocked.append(rel)
                continue
            removed += 1
            trashed.extend(entries)
    for rel in blocked:
        print(f"journal: kept in place (trash refused): {rel}")

    back = 0
    for rel, e in keep.items():
        src = os.path.join(latest, "files", rel)
        dst = os.path.join(root, rel)
        cur = sha256_of(dst) if os.path.exists(dst) else ""
        if cur != e["sha256"]:
            os.makedirs(os.path.dirname(dst) or root, exist_ok=True)
            shutil.copy2(src, dst)
            # copy2 keeps the snapshot's mtime, and a stale mtime makes cargo believe
            # nothing changed -- the tree would be correct and the build still wrong.
            now_ts = datetime.datetime.now().timestamp()
            os.utime(dst, (now_ts, now_ts))
            back += 1

    print(f"journal: restore -- removed {removed} new file(s), restored {back} file(s)")
    for e in trashed:
        print(f"journal: trash {e['id']} {e['path']}")
    return 1 if blocked else 0


def cmd_status(latest):
    manifest_path = os.path.join(latest, "manifest.json")
    if not os.path.exists(manifest_path):
        print("journal: no snapshot")
        return 2
    man = manifest_or_refuse(manifest_path, "report on it")
    print(f"journal: snapshot of {man['count']} files at {man['at']} from {man['root']}")
    return 0


def main(argv):
    a = parse_args(argv)
    mode = a.get("mode", "")
    root_in = a.get("root", "").strip()
    if mode not in MODES:
        print(f"journal: -Mode 必须是 {sorted(MODES)} 之一")
        return 2
    if not root_in or not os.path.isdir(root_in):
        print("journal: -Root 必须是已存在的目录")
        return 2
    root = os.path.abspath(root_in).rstrip("/\\")
    store = a.get("store", "").strip() or os.path.join(root, ".journal")
    latest = os.path.join(store, "latest")
    if mode == "begin":
        return cmd_begin(root, latest)
    if mode == "end":
        return cmd_end(root, latest)
    if mode == "restore":
        return cmd_restore(root, latest, store)
    return cmd_status(latest)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
