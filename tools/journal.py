#!/usr/bin/env python3
"""journal: checkpoint the workspace so a failed experiment can be undone, and so the
tool layer can prove what a command actually wrote.

    journal.py begin   -Root D:\\proj   -> .journal/latest
    journal.py end     -Root D:\\proj   -> diff; exit 1 on change
    journal.py restore -Root D:\\proj   -> put the tree back
    journal.py status  -Root D:\\proj
    journal.py get     -Root D:\\proj -Hash <sha256>   -> hash-addressable retrieval:
                        prints the stored copy whose content hashes to <hex>
                        (verified, not just manifest-claimed); exit 1 on miss

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
import re
import shutil
import sys

import trash  # same tools directory; restore never unlinks outright, it bins (see below)

MODES = {"begin", "end", "restore", "status", "get"}

HEX64 = re.compile(r"^[0-9a-fA-F]{64}$")

SKIP = {"node_modules", "target", ".git", ".journal", "__pycache__", ".venv", "dist", "build",
        "obj", ".qoder", ".pytest_cache", ".ruff_cache", ".mypy_cache", ".turbo", ".next", "coverage"}

MAX_BYTES = 8 * 1024 * 1024


def parse_args(argv):
    out = {"store": "", "hash": ""}
    positional = [x for x in argv if not x.startswith("-")]
    i = 0
    while i < len(argv):
        a = argv[i]
        if not a.startswith("-"):
            i += 1
            continue
        name = a.lstrip("-").lower()
        key = {"mode": "mode", "root": "root", "store": "store", "hash": "hash"}.get(name, name)
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


def safe_rel(rel):
    """Normalize a manifest rel into path components, or None when it is damaged.

    The journal writes rels with os.path.join, so on Windows a backslash is a
    legitimate separator -- reading "\\" as corruption marked every normal Windows
    snapshot damaged: restore skipped the copies it had just made (regression G5/G8)
    and status libelled a healthy store. A rel is damaged only when it is empty,
    absolute (POSIX root, drive-relative or UNC), or carries an empty, "." or ".."
    component -- the shapes that turn a corrupted manifest into a write outside the
    root. Both separators are read as separators so either flavor of manifest works.
    """
    if not isinstance(rel, str) or not rel.strip():
        return None
    if rel.startswith(("/", "\\")) or os.path.isabs(rel):
        return None
    if len(rel) > 1 and rel[1] == ":":  # C:foo is drive-relative, not root-relative
        return None
    parts = re.split(r"[\\/]", rel)
    if any(p in ("", ".", "..") for p in parts):
        return None
    return parts


def rel_key(rel):
    """Canonical comparison key: separators unified. A manifest written with one
    separator flavor must not make the other flavor look like a different file."""
    return "/".join(re.split(r"[\\/]", str(rel)))


def manifest_files(man):
    """The manifest's files table, or None when the shape itself is damaged.
    A store that lost its table is not a store with zero files: restoring from it
    would bin every file in the root, so each caller refuses instead."""
    files = man.get("files") if isinstance(man, dict) else None
    return files if isinstance(files, list) else None


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
    files = manifest_files(man)
    if files is None:
        print("journal: manifest files table is not a list -- refusing to diff against "
              "it (damaged manifest: a lost table would read as every file added)")
        return 2
    prev = {}
    damaged = 0
    for e in files:
        if isinstance(e, dict) and isinstance(e.get("sha256"), str) \
                and HEX64.match(e.get("sha256")) and safe_rel(e.get("rel")) is not None:
            prev[rel_key(e["rel"])] = e["sha256"]
        else:
            damaged += 1
    if damaged:
        print(f"journal: {damaged} damaged manifest entr(y/ies) -- refusing to diff "
              f"(a broken snapshot must not be read as a changed tree)")
        return 2
    now = {rel_key(rel): sha256_of(full) for rel, full, _ in walk_files(root)}

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
    files = manifest_files(man)
    if files is None:
        print("journal: manifest files table is not a list -- refusing to restore "
              "(damaged manifest: restoring would read as zero files and bin the root)")
        return 2
    keep = {}
    damaged = []
    for e in files:
        if not isinstance(e, dict):
            damaged.append("<malformed entry>")
            continue
        rel = str(e.get("rel", ""))
        parts = safe_rel(rel)
        # a bit-flipped rel must never become a write path: absolute, traversal,
        # drive-letter or empty-component forms are damaged entries, named and
        # skipped -- restore refuses to turn store corruption into writes outside
        # the root. Windows-native backslashes are separators, not damage.
        if parts is None or not isinstance(e.get("sha256"), str) \
                or not HEX64.match(e.get("sha256")):
            damaged.append(rel or "<missing rel>")
            continue
        keep[rel_key(rel)] = (parts, e)

    # files that appeared after the snapshot are not unlinked: they move into the recycle
    # bin (same store as the snapshot), so a restore never destroys content and every
    # removal carries the reason it happened. A refused move leaves the file in place and
    # turns the restore red -- an incomplete restore must not look like success.
    reason = f"journal-restore:{man.get('at', 'unknown')}"
    removed = 0
    trashed = []
    blocked = []
    # a damaged manifest cannot tell "added after the snapshot" from "entry the
    # corruption lost", so with damage present nothing is binned at all: the restore
    # is already red, and guessing would move files the snapshot may still claim.
    bin_new = not damaged
    if not bin_new:
        print("journal: damaged entries present -- not binning post-snapshot files "
              "(cannot distinguish 'added after' from 'lost entry')")
    if bin_new:
        for rel, full, _ in list(walk_files(root)):
            if rel_key(rel) not in keep:
                entries, err = trash.put_paths(root, store, [full], reason)
                if err:
                    blocked.append(rel)
                    continue
                removed += 1
                trashed.extend(entries)
    for rel in blocked:
        print(f"journal: kept in place (trash refused): {rel}")
    for rel in damaged:
        # named, not silent: a corrupted manifest entry is damage the user must see
        print(f"journal: damaged entry skipped: {rel}")

    back = 0
    for parts, e in keep.values():
        src = os.path.join(latest, "files", *parts)
        dst = os.path.join(root, *parts)
        cur = sha256_of(dst) if os.path.exists(dst) else ""
        if cur != e["sha256"]:
            os.makedirs(os.path.dirname(dst) or root, exist_ok=True)
            try:
                shutil.copy2(src, dst)
            except OSError as exc:
                # the stored copy may be damaged or absent even when rel looks safe:
                # a per-file failure is named and counted, never a crash, and the
                # restore stays red -- an incomplete restore must not look like success.
                name = str(e.get("rel", "")) or "<missing rel>"
                damaged.append(f"{name} ({exc})")
                print(f"journal: damaged entry skipped: {name} ({exc})")
                continue
            # copy2 keeps the snapshot's mtime, and a stale mtime makes cargo believe
            # nothing changed -- the tree would be correct and the build still wrong.
            now_ts = datetime.datetime.now().timestamp()
            os.utime(dst, (now_ts, now_ts))
            back += 1

    print(f"journal: restore -- removed {removed} new file(s), restored {back} file(s)")
    for e in trashed:
        print(f"journal: trash {e['id']} {e['path']}")
    # damaged entries turn the restore red even when nothing was blocked: an
    # incomplete restore must not look like success (L124).
    return 1 if (blocked or damaged) else 0


def find_by_hash(store, digest):
    """Hash-addressable retrieval over the latest snapshot: the manifest entry names
    the content, the stored copy must actually hash to it (a manifest claim alone is
    not proof). Returns the stored copy's path, or None. Digest form matches the
    manifest's own sha256 (uppercase hex); consumer hashes (e.g. delta receipts)
    use the same family, so one surface answers every hash."""
    want = (digest or "").strip().upper()
    try:
        man = load_manifest(os.path.join(store, "latest", "manifest.json"))
    except (OSError, ValueError):
        return None
    files = manifest_files(man)
    if files is None:
        return None
    for e in files:
        if not isinstance(e, dict) or str(e.get("sha256", "")).upper() != want:
            continue
        parts = safe_rel(e.get("rel"))
        if parts is None:
            continue  # damaged entry never becomes a read/write path
        copy = os.path.join(store, "latest", "files", *parts)
        if os.path.isfile(copy):
            try:
                if sha256_of(copy) == want:
                    return copy
            except OSError:
                return None
    return None


def cmd_get(root, store, digest):
    if not HEX64.match((digest or "").strip()):
        print("journal: get needs a 64-hex sha256 (-Hash <hex>)")
        return 2
    hit = find_by_hash(store, digest)
    if hit is None:
        print(f"journal: no snapshot artifact hashes to {digest.strip().upper()}")
        return 1
    print(hit)
    return 0


def cmd_status(latest):
    manifest_path = os.path.join(latest, "manifest.json")
    if not os.path.exists(manifest_path):
        print("journal: no snapshot")
        return 2
    man = manifest_or_refuse(manifest_path, "report on it")
    files = manifest_files(man)
    if files is None:
        print("journal: manifest files table is not a list -- damaged snapshot, "
              "cannot be enumerated or restored")
        return 1
    # a healthy-looking count with damaged entries behind it is the L124 lie: status
    # verifies each entry's stored copy exists (cheap stat walk, no content hashing)
    # and reports damage explicitly instead of cosplaying health.
    missing = []
    for e in files:
        rel = str(e.get("rel", "")) if isinstance(e, dict) else ""
        parts = safe_rel(rel) if isinstance(e, dict) else None
        sha = e.get("sha256") if isinstance(e, dict) else None
        if parts is None or not isinstance(sha, str) or not HEX64.match(sha) \
                or not os.path.isfile(os.path.join(latest, "files", *parts)):
            missing.append(rel or "<malformed entry>")
    damage = f", damaged: {len(missing)} entr(y/ies)" if missing else ""
    for rel in missing[:5]:
        print(f"journal: damaged entry: {rel or '<missing rel>'}")
    if len(missing) > 5:
        print(f"journal: ... and {len(missing) - 5} more damaged entries")
    count = man.get("count")
    count = count if isinstance(count, int) else len(files)
    print(f"journal: snapshot of {count} files at {man.get('at', 'unknown')} "
          f"from {man.get('root', 'unknown')}{damage}")
    return 1 if missing else 0


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
    if mode == "get":
        return cmd_get(root, store, a.get("hash", ""))
    return cmd_status(latest)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
