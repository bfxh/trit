#!/usr/bin/env python3
"""git-safe: checkpoints you can always fall back to, without touching HEAD.

    git_safe.py checkpoint [--Label wip]  -Root D:\\proj   -> refs/recycle/checkpoint/<ts>-<label>
    git_safe.py list                      -Root D:\\proj
    git_safe.py rescue                    -Root D:\\proj   -> reflog + refs + copy-paste commands
    git_safe.py verify <ref>              -Root D:\\proj

Why refs instead of a branch: a checkpoint must be free -- no branch to move, no index to
stage into, no worktree to clean. The mechanism is exactly the plumbing git exposes for
that: a temporary GIT_INDEX_FILE (the real index is never opened for writing), `git add -A`
into it (untracked non-ignored files included), `git write-tree`, `git commit-tree -p HEAD`,
`git update-ref refs/recycle/checkpoint/<ts>-<label>`. HEAD, the index and the worktree are
bit-identical before and after -- which is also what the selftest pins.

Fail-closed, stated plainly: no git repository, or a repository without any commit, refuses
with the reason. A checkpoint that cannot prove which repo state it captured is worse than
no checkpoint; so is a checkpoint silently "recovered" onto an unrelated base.

Honest boundary: checkpoint refs live in this clone's .git -- `git gc --prune` or a deleted
ref removes them, and ignored files are not captured (add -A respects .gitignore). Push
refs/recycle/checkpoint/* if a checkpoint must survive the machine. The restore commands
printed here overwrite worktree/index content on purpose; that is what "rescue" means, so
run them as a human or after policy allows it.
"""

import datetime
import os
import subprocess
import sys
import tempfile

MODES = {"checkpoint", "list", "rescue", "verify"}

REF_BASE = "refs/recycle/checkpoint"


def parse_args(argv):
    out = {"mode": "", "ref": "", "root": "", "label": ""}
    positional = []
    i = 0
    while i < len(argv):
        a = argv[i]
        if not a.startswith("-"):
            positional.append(a)
            i += 1
            continue
        name = a.lstrip("-").lower()
        if name == "label":
            out["label"] = argv[i + 1] if i + 1 < len(argv) else ""
            i += 2
            continue
        key = {"root": "root"}.get(name, name)
        out[key] = argv[i + 1] if i + 1 < len(argv) else ""
        i += 2
    if positional and positional[0] in MODES:
        out["mode"] = positional[0]
        out["ref"] = positional[1] if len(positional) > 1 else ""
    return out


def git_run(root, *args, env=None):
    p = subprocess.run(["git", "-C", root] + list(args), capture_output=True, text=True,
                       encoding="utf-8", errors="replace", env=env)
    return p.returncode, (p.stdout or "").strip(), (p.stderr or "").strip()


def cmd_checkpoint(root, label):
    rc, _top, _err = git_run(root, "rev-parse", "--show-toplevel")
    if rc != 0:
        print("git-safe: refuse -- not inside a git repository "
              "(fail closed: no fabricated checkpoints)")
        return 1
    rc, head, _err = git_run(root, "rev-parse", "--verify", "--quiet", "HEAD")
    if rc != 0 or not head:
        print("git-safe: refuse -- no commit yet (checkpoint needs HEAD as the parent)")
        return 1

    label = label.strip()
    if label:
        # ref-component rules, enforced up front: [A-Za-z0-9._-], no '..', no trailing
        # '.'/'.lock' -- a bad label would surface as a confusing git error otherwise
        if not all(c.isalnum() or c in "._-" for c in label) or ".." in label \
                or label.endswith(".") or label.endswith(".lock") or not label[0].isalnum():
            print("git-safe: refuse -- label must be ASCII [A-Za-z0-9._-], start/end "
                  "alphanumeric, no '..' or '.lock' suffix")
            return 1

    ts = datetime.datetime.now().strftime("%Y%m%d-%H%M%S")
    name = f"{ts}-{label}" if label else ts
    n = 2
    while git_run(root, "rev-parse", "--verify", "--quiet", f"{REF_BASE}/{name}")[0] == 0:
        name = f"{ts}-{label}-{n}" if label else f"{ts}-{n}"
        n += 1

    # the whole point of the temp index: add -A stages everything INTO THE TEMP FILE, the
    # real index is never opened for writing, so nothing shows up in `git status` later.
    # mkstemp reserves the NAME only: git refuses a 0-byte existing file as a corrupt
    # index ("index file smaller than expected"), so unlink it and let git create it.
    fd, tmp_index = tempfile.mkstemp(prefix="git-safe-index-")
    os.close(fd)
    os.unlink(tmp_index)
    try:
        env = dict(os.environ, GIT_INDEX_FILE=tmp_index)
        rc, _out, err = git_run(root, "add", "-A", env=env)
        if rc != 0:
            print(f"git-safe: refuse -- 'git add -A' failed: {err}")
            return 1
        rc, tree, _err = git_run(root, "write-tree", env=env)
        if rc != 0:
            print(f"git-safe: refuse -- write-tree failed: {err}")
            return 1
    finally:
        if os.path.exists(tmp_index):
            os.remove(tmp_index)
    msg = f"recycle checkpoint {ts} {label}".strip()
    rc, commit, err = git_run(root, "commit-tree", tree, "-p", head, "-m", msg)
    if rc != 0:
        print(f"git-safe: refuse -- commit-tree failed: {err}")
        return 1
    ref = f"{REF_BASE}/{name}"
    rc, _out, err = git_run(root, "update-ref", ref, commit)
    if rc != 0:
        print(f"git-safe: refuse -- update-ref failed: {err}")
        return 1

    print(f"git-safe: checkpoint {commit}")
    print(f"git-safe: ref {ref}")
    print(f"git-safe: tree {tree}")
    print(f"git-safe: parent {head}")
    print("git-safe: HEAD/index/worktree untouched")
    print("git-safe: restore:")
    print(f"git-safe:   git -C <owner-root> restore --source={commit} --worktree -- :/")
    print(f"git-safe:   git -C <owner-root> restore --source={commit} --staged --worktree -- :/")
    return 0


def _checkpoints(root):
    rc, out, _err = git_run(root, "for-each-ref", REF_BASE, "--sort=-committerdate",
                            "--format=%(refname)|%(objectname)|%(committerdate:iso-strict)|"
                            "%(contents:subject)")
    if rc != 0:
        return []
    rows = []
    for line in out.splitlines():
        parts = line.split("|", 3)
        if len(parts) == 4:
            rows.append(parts)
    return rows


def cmd_list(root):
    rows = _checkpoints(root)
    print(f"git-safe: checkpoints {len(rows)}")
    for ref, sha, date, subject in rows:
        print(f"git-safe: {sha[:12]}  {date}  {ref}  {subject}")
    return 0


def cmd_rescue(root):
    rc, top, _err = git_run(root, "rev-parse", "--show-toplevel")
    if rc != 0:
        print("git-safe: refuse -- not inside a git repository")
        return 1
    print("git-safe: rescue -- everything below is copy-paste ready")
    rc, reflog, _err = git_run(root, "reflog", "-n", "10", "--date=iso")
    print("git-safe: [1] reflog (last 10)")
    if rc != 0 or not reflog:
        print("git-safe:   (no reflog -- the repository has no commits yet)")
    else:
        for line in reflog.splitlines():
            print(f"git-safe:   {line}")
    rows = _checkpoints(root)
    print(f"git-safe: [2] checkpoint refs ({len(rows)})")
    for ref, sha, date, subject in rows:
        print(f"git-safe:   {sha[:12]}  {ref}  {subject}")
    print("git-safe: [3] restore commands (newest checkpoint first)")
    for ref, sha, _date, subject in rows:
        print(f"git-safe:   # {subject}")
        print(f"git-safe:   git -C <owner-root> restore --source={sha} --worktree -- :/")
        print(f"git-safe:   git -C <owner-root> restore --source={sha} --staged --worktree -- :/")
    if rows:
        newest = rows[0][1]
        print("git-safe: last resort (moves HEAD and the branch; policy DENYs this for agents):")
        print(f"git-safe:   git -C <owner-root> reset --hard {newest}")
    return 0


def cmd_verify(root, ref_arg):
    if not ref_arg:
        print("git-safe: refuse -- verify needs a ref or entry name")
        return 2
    candidates = [ref_arg]
    if not ref_arg.startswith("refs/"):
        candidates.append(f"{REF_BASE}/{ref_arg}")
    for cand in candidates:
        rc, sha, _err = git_run(root, "rev-parse", "--verify", "--quiet", f"{cand}^{{commit}}")
        if rc == 0 and sha:
            _rc, subject, _e = git_run(root, "log", "-1", "--format=%s", sha)
            _rc, tree, _e = git_run(root, "rev-parse", f"{sha}^{{tree}}")
            _rc, parents, _e = git_run(root, "rev-list", "--parents", "-n", "1", sha)
            n_parents = max(0, len(parents.split()) - 1)
            print(f"git-safe: verify {ref_arg} -> {sha}")
            print(f"git-safe: subject {subject}")
            print(f"git-safe: tree {tree}")
            print(f"git-safe: parents {n_parents}")
            return 0
    print(f"git-safe: refuse -- cannot resolve '{ref_arg}' as a commit "
          f"(checked as-is and under {REF_BASE}/)")
    return 1


def usage(code):
    print("""git-safe: checkpoint the workspace as a git ref, without touching HEAD/index/worktree

usage:
  tools/git_safe.py checkpoint [--Label <name>] -Root <dir>
  tools/git_safe.py list   -Root <dir>
  tools/git_safe.py rescue -Root <dir>
  tools/git_safe.py verify <ref> -Root <dir>

boundary: checkpoint refs live in this clone's .git -- gc/prune or a deleted ref removes
them, and ignored files are not captured (git add -A respects .gitignore). The restore
commands overwrite worktree/index content on purpose; run them as a human.""")
    return code


def main(argv):
    a = parse_args(argv)
    if not a["mode"]:
        return usage(2)
    root_in = a.get("root", "").strip()
    if not root_in or not os.path.isdir(root_in):
        print("git-safe: -Root must be an existing directory (fail closed: nothing probed)")
        return 2
    root = os.path.abspath(root_in).rstrip("/\\")
    if a["mode"] == "checkpoint":
        return cmd_checkpoint(root, a.get("label", ""))
    if a["mode"] == "list":
        return cmd_list(root)
    if a["mode"] == "rescue":
        return cmd_rescue(root)
    return cmd_verify(root, a.get("ref", ""))


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
