#!/usr/bin/env python3
"""trash: the recycle bin. An agent may not delete outright -- it may only move content
into here, and every move leaves an entry that says what went in, why, and how to get it
back.

    trash.py put <path>... -Reason <why>        -Root D:\\proj   -> move into the bin
    trash.py list [--json]                      -Root D:\\proj   -> the manifest
    trash.py restore <entry-id> [-Into D:\\dir]  -Root D:\\proj   -> back to the original spot
    trash.py purge <entry-id> -Confirm <id>     -Root D:\\proj   -> destroy (id typed twice)
    trash.py status                             -Root D:\\proj

Why a bin and not `del`: the #1 product rule for agents is "never delete outright". A
one-way unlink turns a wrong guess into lost work; a manifest-ed move keeps the content
until a human types the same entry id twice. The bin's site shares the journal snapshot's
store resolution (tools/journal.py): -Store override, else <Root>/.journal. That site must
never show up in `git status` -- when Root is inside a git work tree and the site is not
ignored, put refuses (fail closed) instead of dirtying the tree.

The no-content-vanishes guarantee, spelled out: same volume -> rename, one atomic step.
Cross volume (or TRASH_FORCE_COPY=1, a test seam) -> copy first, sha256-verify the copy
against the ledgered digest, and only then remove the source. There is no step where the
content exists nowhere.

Honest boundary: this is a rename ledger on the same machine, not a backup. Anyone who can
write the workspace can also empty the bin; purge is deliberately the only destructive step
and demands the entry id twice. Restores refuse to overwrite (occupancy is an error, never
a rename-over). Writes are serialized by a lock file -- a second concurrent writer waits,
then refuses; nothing is merged. Output is machine-readable ASCII; exit 0 ok, 1 refused or
failed, 2 bad invocation.
"""

import datetime
import hashlib
import json
import os
import shutil
import subprocess
import sys
import time

MODES = {"put", "list", "restore", "purge", "status"}

LOCK_TIMEOUT_S = 30

# _site_clean caches per process: journal restore calls put once per file and the git
# answer cannot change underneath a single invocation.
_SITE_CLEAN_CACHE = {}


def parse_args(argv):
    out = {"mode": "", "entry": "", "paths": [], "reason": "", "root": "", "store": "",
           "into": "", "confirm": "", "json": False}
    positional = []
    i = 0
    while i < len(argv):
        a = argv[i]
        if not a.startswith("-"):
            positional.append(a)
            i += 1
            continue
        name = a.lstrip("-").lower()
        if name == "json":
            out["json"] = True
            i += 1
            continue
        key = {"reason": "reason", "root": "root", "store": "store",
               "into": "into", "confirm": "confirm"}.get(name, name)
        out[key] = argv[i + 1] if i + 1 < len(argv) else ""
        i += 2
    if positional and positional[0] in MODES:
        out["mode"] = positional[0]
        rest = positional[1:]
        if out["mode"] == "put":
            # a dash-prefixed path is ambiguous with the next flag -- refuse rather than
            # guess, same direction as every other fail-closed check here
            bad = [p for p in rest if p.startswith("-")]
            if bad:
                out["dash_paths"] = bad
            seen = set()
            out["paths"] = [p for p in rest if not (p in seen or seen.add(p))]
        else:
            out["entry"] = rest[0] if rest else ""
    return out


def sha256_of(path):
    h = hashlib.sha256()
    with open(path, "rb") as fh:
        for chunk in iter(lambda: fh.read(65536), b""):
            h.update(chunk)
    return h.hexdigest().upper()


def walk_tree(base):
    """Yield (rel-forward-slashed, full, size) for every regular file under base."""
    for dirpath, _dirnames, filenames in os.walk(base):
        for fn in filenames:
            full = os.path.join(dirpath, fn)
            try:
                st = os.stat(full)
            except OSError:
                continue
            rel = os.path.relpath(full, base).replace(os.sep, "/")
            yield rel, full, st.st_size


def digest_of(path, kind):
    """(sha256, bytes) for one entry. A directory gets a composite digest: a fold over the
    sorted per-file listing. Any byte change inside moves it, which is what the ledger
    needs; it is not a single-content hash and is never presented as one."""
    if kind == "file":
        st = os.stat(path)
        return sha256_of(path), st.st_size
    h = hashlib.sha256()
    total = 0
    for rel, full, size in sorted(walk_tree(path), key=lambda t: t[0]):
        h.update(f"{rel}\0{sha256_of(full)}\0{size}\n".encode("utf-8"))
        total += size
    return h.hexdigest().upper(), total


def is_inside(full, root):
    """Path-component containment, not string-prefix containment (D:\\proj-evil is not
    inside D:\\proj) -- same rule as policy.py's boundary check."""
    f, r = full.lower().replace("/", "\\"), root.lower().replace("/", "\\")
    return f == r or f.startswith(r + "\\")


def git_toplevel(root):
    try:
        p = subprocess.run(["git", "-C", root, "rev-parse", "--show-toplevel"],
                           capture_output=True, text=True, encoding="utf-8", errors="replace")
    except OSError:
        return ""
    return p.stdout.strip() if p.returncode == 0 else ""


def site_clean(root, site):
    """The bin must never appear in `git status`.

    Outside a repo there is no status to dirty. Inside one: a site within the work tree
    must be ignored (git check-ignore), otherwise put refuses -- a bin that shows up as
    untracked files would get committed, copied or cleaned by the very commands it guards.
    A site outside the work tree (the -Store form agent-verify uses) is fine by default.
    """
    key = os.path.normcase(os.path.normpath(site))
    if key in _SITE_CLEAN_CACHE:
        return _SITE_CLEAN_CACHE[key]
    top = git_toplevel(root)
    ok, why = True, ""
    if top and is_inside(os.path.normpath(site), top):
        p = subprocess.run(["git", "-C", root, "check-ignore", "-q", "--", site],
                           capture_output=True, text=True)
        if p.returncode != 0:
            ok, why = False, ("the bin site is not git-ignored, so puts would dirty git "
                              "status -- add it to .gitignore first (fail closed)")
    _SITE_CLEAN_CACHE[key] = (ok, why)
    return ok, why


def rel_site(root, site):
    """Site as printed: owner-root relative when it is inside root (privacy: outputs carry
    no machine-absolute paths), a marker when a custom -Store points elsewhere."""
    if is_inside(os.path.normpath(site), os.path.normpath(root)):
        return os.path.relpath(site, root).replace(os.sep, "/")
    return "(store outside owner root)"


def acquire_lock(site):
    """Single-writer lock. Stale locks (crashed run) are stolen after the timeout and the
    theft is printed -- a bin that refuses forever after one crash would push people back
    to plain deletes."""
    lock = os.path.join(site, "lock")
    deadline = time.time() + LOCK_TIMEOUT_S
    while True:
        try:
            fd = os.open(lock, os.O_CREAT | os.O_EXCL | os.O_WRONLY)
            os.write(fd, str(os.getpid()).encode("ascii"))
            os.close(fd)
            return lock
        except FileExistsError:
            try:
                if time.time() - os.stat(lock).st_mtime > LOCK_TIMEOUT_S:
                    os.remove(lock)
                    print(f"trash: stole a stale lock (older than {LOCK_TIMEOUT_S}s)")
                    continue
            except OSError:
                pass
            if time.time() > deadline:
                return None
            time.sleep(0.1)


def load_manifest(site):
    path = os.path.join(site, "manifest.json")
    if not os.path.exists(path):
        return {"entries": []}
    with open(path, "r", encoding="utf-8-sig") as fh:
        return json.load(fh)


def save_manifest(site, manifest):
    path = os.path.join(site, "manifest.json")
    tmp = path + ".partial"
    with open(tmp, "w", encoding="utf-8", newline="\n") as fh:
        json.dump(manifest, fh, ensure_ascii=False, indent=4)
        fh.write("\n")
    os.replace(tmp, path)


def move_into_bin(src, blob_path, kind, digest, force_copy):
    """Return 'rename' or 'copy+verify'. The copy branch never touches the source until
    the copy's digest matches the one already computed from the source: nothing ends up
    in a state where the content is nowhere."""
    if not force_copy:
        try:
            os.rename(src, blob_path)
            return "rename"
        except OSError:
            pass  # cross volume, or a handle held open -- the verified copy follows
    tmp = blob_path + ".partial"
    if kind == "file":
        shutil.copy2(src, tmp)
    else:
        shutil.copytree(src, tmp)
    try:
        got, _ = digest_of(tmp, kind)
        if got != digest:
            raise OSError(f"copy digest mismatch ({got[:12]} != {digest[:12]})")
        os.replace(tmp, blob_path)
    except OSError:
        if os.path.isdir(tmp):
            shutil.rmtree(tmp, ignore_errors=True)
        elif os.path.exists(tmp):
            os.remove(tmp)
        raise
    if kind == "file":
        os.remove(src)
    else:
        shutil.rmtree(src)
    return "copy+verify"


def move_out_of_bin(blob_path, target, kind, digest):
    """Bin -> workspace. Same verified-copy discipline, mirrored: the blob is only removed
    after the landed copy's digest matches the entry."""
    try:
        os.rename(blob_path, target)
        return "rename"
    except OSError:
        pass
    tmp = target + ".trash-partial"
    if kind == "file":
        shutil.copy2(blob_path, tmp)
    else:
        shutil.copytree(blob_path, tmp)
    got, _ = digest_of(tmp, kind)
    if got != digest:
        if os.path.isdir(tmp):
            shutil.rmtree(tmp, ignore_errors=True)
        elif os.path.exists(tmp):
            os.remove(tmp)
        raise OSError("restored copy digest mismatch -- bin content does not match its entry")
    os.replace(tmp, target)
    if kind == "file":
        os.remove(blob_path)
    else:
        shutil.rmtree(blob_path)
    return "copy+verify"


def put_paths(root, store, paths, reason):
    """Shared engine for CLI put and journal restore. Returns (entries, error).

    Validation is all-or-nothing (nothing moves until every path checks out); the moves
    then run in order and a failure stops the rest -- earlier moves keep their entries,
    the failed path stays where it was.
    """
    site = os.path.join(store, "trash")
    ok, why = site_clean(root, site)
    if not ok:
        return [], why
    checked = []
    for p in paths:
        if not p.strip():
            return [], "empty path (refusing to guess what to bin)"
        # relative paths resolve against -Root, not against the caller's cwd: the deletion
        # boundary is the owner root, so that is also the base every path is read from
        src = os.path.normpath(p if os.path.isabs(p) else os.path.join(root, p))
        if not is_inside(src, os.path.normpath(root)):
            return [], f"outside -Root boundary, refusing: {p}"
        if not os.path.exists(src):
            return [], f"path does not exist: {p}"
        if os.path.normpath(src) == os.path.normpath(root):
            return [], "cannot put the owner root itself"
        site_abs = os.path.normpath(site)
        if is_inside(src, site_abs) or is_inside(site_abs, src):
            return [], f"path overlaps the bin site itself: {p}"
        checked.append(src)

    os.makedirs(site, exist_ok=True)
    lock = acquire_lock(site)
    if lock is None:
        return [], f"another trash writer holds the lock (waited {LOCK_TIMEOUT_S}s)"
    try:
        manifest = load_manifest(site)
        entries = manifest.get("entries", [])
        seq = int(manifest.get("counter", len(entries)))
        taken = {e["id"] for e in entries}
        now = datetime.datetime.now().astimezone()
        moved = []
        for src in checked:
            rel = os.path.relpath(src, root).replace(os.sep, "/")
            kind = "dir" if os.path.isdir(src) else "file"
            base = os.path.basename(src.rstrip("/\\"))
            digest, total = digest_of(src, kind)
            seq += 1
            entry_id = f"t{now.strftime('%Y%m%d-%H%M%S')}-{seq}"
            while entry_id in taken:
                seq += 1
                entry_id = f"t{now.strftime('%Y%m%d-%H%M%S')}-{seq}"
            taken.add(entry_id)
            blob_dir = os.path.join(site, "files", entry_id)
            os.makedirs(blob_dir, exist_ok=True)
            blob_path = os.path.join(blob_dir, base)
            via = move_into_bin(src, blob_path, kind, digest,
                                force_copy=os.environ.get("TRASH_FORCE_COPY", "").strip() != "")
            entry = {
                "id": entry_id, "path": rel, "kind": kind, "sha256": digest,
                "bytes": total,
                "mtime": datetime.datetime.fromtimestamp(os.stat(blob_path).st_mtime)
                         .astimezone().isoformat(timespec="seconds"),
                "reason": reason, "state": "kept",
                "blob": f"files/{entry_id}/{base}".replace(os.sep, "/"),
                "restore_cmd": f"python tools/trash.py restore {entry_id} -Root <owner-root>",
                "at": now.isoformat(timespec="seconds"),
            }
            entries.append(entry)
            moved.append(entry)
            print(f"trash: put {entry_id} {rel} kind={kind} sha256={digest} "
                  f"bytes={total} via={via}")
            print(f"trash: restore cmd: {entry['restore_cmd']}")
        manifest["entries"] = entries
        manifest["counter"] = seq
        save_manifest(site, manifest)
        return moved, ""
    finally:
        os.remove(lock)


def cmd_put(a, root, store):
    if a.get("dash_paths"):
        return _refuse("path arguments may not start with '-': " + ", ".join(a["dash_paths"]))
    if not a.get("reason", "").strip():
        return _refuse("put needs -Reason <why> -- a deletion without a stated reason is "
                       "exactly what the bin exists to prevent")
    if not a.get("paths"):
        return _refuse("put needs at least one path (empty paths are refused, not guessed)")
    entries, err = put_paths(root, store, a["paths"], a["reason"].strip())
    if err:
        return _refuse(err)
    if entries:
        print(f"trash: {len(entries)} path(s) binned; site {rel_site(root, store + os.sep + 'trash')}")
    return 0


def _refuse(msg):
    print(f"trash: refuse -- {msg}")
    return 1


def _find_entry(entries, entry_id):
    for e in entries:
        if e["id"] == entry_id:
            return e
    return None


def cmd_restore(a, root, store):
    entry_id = a.get("entry", "")
    if not entry_id:
        return _refuse("restore needs an entry id")
    site = os.path.join(store, "trash")
    manifest = load_manifest(site)
    entry = _find_entry(manifest.get("entries", []), entry_id)
    if entry is None:
        return _refuse(f"no entry '{entry_id}' in the manifest")
    if entry["state"] != "kept":
        return _refuse(f"entry '{entry_id}' is {entry['state']}, not kept -- restore runs once")
    blob = os.path.join(site, entry["blob"].replace("/", os.sep))
    if not os.path.exists(blob):
        return _refuse(f"bin content for '{entry_id}' is gone while its entry remains -- "
                       "do not purge this entry, investigate first")

    base = os.path.normpath(os.path.abspath(a.get("into", "").strip() or root))
    if not os.path.exists(base):
        os.makedirs(base, exist_ok=True)
    target = os.path.join(base, *entry["path"].split("/"))
    if os.path.exists(target):
        # occupancy is an error, never a rename-over: silent overwrite is deletion
        # wearing a friendly name
        return _refuse(f"target occupied, refusing to overwrite: {entry['path']}")
    parent = os.path.dirname(target)
    if parent and not os.path.exists(parent):
        os.makedirs(parent, exist_ok=True)
    try:
        via = move_out_of_bin(blob, target, entry["kind"], entry["sha256"])
    except OSError as exc:
        return _refuse(f"restore failed, bin content untouched: {exc}")
    entry["state"] = "restored"
    entry["restored_at"] = datetime.datetime.now().astimezone().isoformat(timespec="seconds")
    save_manifest(site, manifest)
    print(f"trash: restore {entry_id} -> {entry['path']} via={via}")
    print(f"trash: verify sha256={entry['sha256']} bytes={entry['bytes']} ok")
    return 0


def cmd_purge(a, root, store):
    entry_id = a.get("entry", "")
    if not entry_id:
        return _refuse("purge needs an entry id")
    confirm = a.get("confirm", "").strip()
    if not confirm:
        # the double confirmation is the whole point: purge is the one step that destroys
        return _refuse("purge needs --Confirm <the same entry id> typed again "
                       "(explicit double confirmation, one missing is one too many)")
    if confirm != entry_id:
        return _refuse("--Confirm does not match the entry id, refusing")
    site = os.path.join(store, "trash")
    manifest = load_manifest(site)
    entry = _find_entry(manifest.get("entries", []), entry_id)
    if entry is None:
        return _refuse(f"no entry '{entry_id}' in the manifest")
    if entry["state"] == "purged":
        return _refuse(f"entry '{entry_id}' is already purged")
    if entry["state"] == "restored":
        return _refuse(f"entry '{entry_id}' was already restored -- nothing is in the bin")
    blob = os.path.join(site, entry["blob"].replace("/", os.sep))
    if not os.path.exists(blob):
        return _refuse(f"bin content for '{entry_id}' is missing -- refusing to mark it purged")
    if entry["kind"] == "dir":
        shutil.rmtree(blob)
    else:
        os.remove(blob)
    os.rmdir(os.path.dirname(blob))
    entry["state"] = "purged"
    entry["purged_at"] = datetime.datetime.now().astimezone().isoformat(timespec="seconds")
    save_manifest(site, manifest)
    print(f"trash: purge {entry_id} {entry['path']} freed={entry['bytes']}")
    return 0


def cmd_list(a, root, store):
    manifest = load_manifest(os.path.join(store, "trash"))
    entries = manifest.get("entries", [])
    if a.get("json"):
        doc = {"site": rel_site(root, os.path.join(store, "trash")), "entries": entries}
        # ensure_ascii keeps stdout pure ASCII even when a reason was typed in Chinese
        print(json.dumps(doc, ensure_ascii=True, indent=2))
        return 0
    counts = {}
    for e in entries:
        counts[e["state"]] = counts.get(e["state"], 0) + 1
    summary = ", ".join(f"{k} {v}" for k, v in sorted(counts.items())) or "empty"
    print(f"trash: site {rel_site(root, os.path.join(store, 'trash'))} "
          f"(entries: {len(entries)}, {summary})")
    for e in entries:
        reason = str(e.get("reason", ""))[:60]
        print(f"trash: {e['id']}  {e['state']:<8} {e['kind']:<4} {e['path']}  "
              f"sha256={e['sha256'][:12]} bytes={e['bytes']} reason=\"{reason}\"")
        print(f"trash: restore cmd: {e['restore_cmd']}")
    return 0


def cmd_status(root, store):
    site = os.path.join(store, "trash")
    entries = load_manifest(site).get("entries", [])
    counts = {}
    for e in entries:
        counts[e["state"]] = counts.get(e["state"], 0) + 1
    kept_bytes = sum(e["bytes"] for e in entries if e["state"] == "kept")
    print(f"trash: site {rel_site(root, site)}")
    print(f"trash: entries={len(entries)} kept={counts.get('kept', 0)} "
          f"restored={counts.get('restored', 0)} purged={counts.get('purged', 0)} "
          f"bytes-kept={kept_bytes}")
    return 0


def usage(code):
    print("""trash: versioned recycle bin -- an agent moves content here instead of deleting it

usage:
  tools/trash.py put <path>... -Reason <why> [-Root <dir>] [-Store <dir>]
  tools/trash.py list [--json] [-Root <dir>] [-Store <dir>]
  tools/trash.py restore <entry-id> [-Into <dir>] [-Root <dir>] [-Store <dir>]
  tools/trash.py purge <entry-id> -Confirm <entry-id> [-Root <dir>] [-Store <dir>]
  tools/trash.py status [-Root <dir>] [-Store <dir>]

boundary: a rename ledger on the same machine, not a backup -- whoever can write the
workspace can empty the bin too; purge demands the entry id twice; restores refuse to
overwrite. TRASH_FORCE_COPY=1 forces the copy+verify branch (test seam).""")
    return code


def main(argv):
    a = parse_args(argv)
    if not a["mode"]:
        return usage(2)
    if a["mode"] == "put" and a.get("dash_paths"):
        return _refuse("path arguments may not start with '-': " + ", ".join(a["dash_paths"]))
    root_in = a.get("root", "").strip()
    if not root_in or not os.path.isdir(root_in):
        # no cwd fallback: guessing the owner root would guess the deletion boundary too
        print("trash: -Root must be an existing directory (no owner root probed, no put)")
        return 2
    root = os.path.abspath(root_in).rstrip("/\\")
    store = a.get("store", "").strip() or os.path.join(root, ".journal")
    if a["mode"] == "put":
        return cmd_put(a, root, store)
    if a["mode"] == "list":
        return cmd_list(a, root, store)
    if a["mode"] == "restore":
        return cmd_restore(a, root, store)
    if a["mode"] == "purge":
        return cmd_purge(a, root, store)
    return cmd_status(root, store)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
