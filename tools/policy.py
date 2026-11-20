#!/usr/bin/env python3
"""policy: two-axis permission model (profile x network), checked BEFORE a command runs.

    policy.py -Profile read-only -Command "curl https://example.com"
    policy.py -Profile workspace-write -Root D:\\proj -Command "cargo test"

Why two axes: "should I ask the human" and "what may the process touch" are independent
questions. Codex CLI keeps them orthogonal on purpose -- a command can be pre-approved and
still be refused by the sandbox, or permitted by the sandbox and still need a human. Same
split here, with Windows-appropriate enforcement:

    axis 1  PROFILE   read-only | workspace-write | full
    axis 2  NETWORK   off by default; -AllowNetwork is an explicit, per-invocation grant

Honest about enforcement strength: this is a *command classifier plus a tree journal*, not
a kernel sandbox. Seatbelt / bubblewrap give real containment; on Windows without one, the
best a tool layer can do is refuse loudly and prove what changed afterwards. So: the
classifier decides `deny` here, and journal proves what actually got written. The final
line of every allow says so out loud -- a tool that overstates its own reach is worse than
one that admits the gap.

The DELETION family (rm/del/Remove-Item/unlink/shred/... and their smuggled forms) denies
in every profile with exactly one sanctioned route: a call that goes through trash.py
(`put`). There is no -Allow escape for raw deletion on purpose -- the bin keeps the content
and the reason, an escape hatch would not. Git operations that rewrite or discard history
(push --force, branch -D, tag -f, remote prune, checkout -- <path>, worktree-side restore,
rebase in main-line context) deny too; their one alternative is a git_safe.py checkpoint
first, and a human can grant a single invocation with `-Allow git-destructive`.

Ported from policy.ps1; decisions, wording and exit codes are kept identical.
"""

import os
import re
import subprocess
import sys

PROFILES = {"read-only", "workspace-write", "full"}

NETWORK_CALLS = ["curl", "wget", "invoke-webrequest", "invoke-restmethod", "ssh", "scp",
                 "ftp", "telnet"]
NETWORK_SUB = {
    "git": ["push", "fetch", "pull", "clone", "submodule", "ls-remote"],
    "npm": ["install", "i", "publish", "add", "ci"],
    "pnpm": ["install", "add", "publish"],
    "yarn": ["install", "add", "publish"],
    "pip": ["install", "download"],
    "poetry": ["install", "add"],
    "go": ["get", "install"],
    "cargo": ["add", "publish", "fetch", "login"],
    "docker": ["pull", "push", "login"],
}
WRITE_CALLS = ["set-content", "add-content", "out-file", "new-item", "remove-item", "move-item",
               "copy-item", "rename-item", "clear-content", "set-itemproperty",
               "new-itemproperty", "remove-itemproperty", "rm", "mv", "cp", "mkdir", "touch",
               "del", "rmdir", "tee"]
WRITE_SUB = {
    "git": ["add", "commit", "reset", "clean", "checkout", "merge", "rebase", "stash", "apply",
            "am", "restore"],
    "cargo": ["fmt", "fix", "clippy", "add"],
    "npm": ["publish", "version"],
}
DESTRUCTIVE = ["git reset --hard", "git clean -fd", "git clean -xfd", "rm -rf /", "format",
               "remove-item -recurse -force c:\\", "shutdown", "bcdedit", "diskpart"]

# The DELETION family: programs whose whole job is making content stop existing. Checked
# before the generic write branch -- rm IS a write, but it is the write the product rule
# ("agents never delete outright") cares about most. `ri`/`rd` are Remove-Item aliases.
DELETION_CALLS = {"rm", "rmdir", "rd", "del", "erase", "remove-item", "ri", "unlink", "shred"}

# Interpreters whose string arguments are likely code, so a deletion program inside them
# is a deletion command wearing a quoting disguise (bash -c 'rm x').
INTERPRETERS = {"bash", "sh", "cmd", "pwsh", "powershell", "python", "python3", "perl",
                "node", "ruby"}


def parse_args(argv):
    out = {"profile": "read-only", "command": "", "root": "", "allow_network": False,
           "explain": False, "allow": []}
    i = 0
    while i < len(argv):
        a = argv[i]
        if not a.startswith("-"):
            i += 1
            continue
        name = a.lstrip("-").lower()
        if name == "allownetwork":
            out["allow_network"] = True
            i += 1
            continue
        if name == "explain":
            out["explain"] = True
            i += 1
            continue
        if name == "allow":
            val = argv[i + 1] if i + 1 < len(argv) else ""
            out["allow"].extend([x for x in val.replace(",", " ").split() if x])
            i += 2
            continue
        key = {"profile": "profile", "command": "command", "root": "root"}.get(name, name)
        out[key] = argv[i + 1] if i + 1 < len(argv) else ""
        i += 2
    return out


def split_command(text):
    """Minimal tokenizer: whitespace separated, quotes respected, pipes/semicolons split so
    that `a && curl x` cannot hide a network call behind an allowed first word."""
    return [seg.strip() for seg in re.split(r"[;|&]{1,2}", text) if seg.strip()]


def is_trash_put(tokens):
    """True when the segment routes through trash.py's put: the ONE sanctioned deletion
    route. Matches trash/trash.py/trash.sh/trash.ps1 wherever the argv names them
    (python tools/trash.py put, bash trash.sh put, plain `trash put`)."""
    for idx, t in enumerate(tokens):
        base = re.sub(r"^.*[\\/]", "", t)
        base = re.sub(r"\.(exe|cmd|ps1|bat|py|sh)$", "", base, flags=re.I).lower()
        if base == "trash" and idx + 1 < len(tokens) and tokens[idx + 1].lower() == "put":
            return True
    return False


def git_destructive(tokens, seg_l, current_branch=""):
    """Git operations that rewrite or discard. Token shape, not substrings: a substring
    scan cannot tell `--force` from `--force-with-lease`, and `-D` from `-d` needs the
    original case. Returns a reason string, or "" when nothing destructive was found."""
    sub = tokens[1].lower() if len(tokens) > 1 else ""
    raw = tokens[2:]
    rest = [t.lower() for t in raw]
    if sub == "push":
        if "--force" in rest or "-f" in rest:
            return "git push --force/-f rewrites the remote"
        for t in rest:
            if t.startswith("+") and len(t) > 1:
                return f"git push refspec '{t}' forces a remote ref"
    if sub == "branch" and "-D" in raw:
        return "git branch -D discards unmerged branches"
    if sub == "tag" and ("-f" in rest or "--force" in rest):
        return "git tag -f moves an existing tag"
    if sub == "remote" and rest[:1] == ["prune"]:
        return "git remote prune deletes stale remote-tracking refs"
    if sub == "checkout" and "--" in rest:
        return "git checkout -- <path> discards uncommitted worktree changes"
    if sub == "restore":
        # --staged only unstages (nothing on disk changes); anything else writes over
        # worktree content -- including plain `git restore <path>`, which is the index
        # flavour of checkout --
        worktree_side = ("--source" in rest or "--worktree" in rest
                         or ("--staged" not in rest and rest
                             and not all(t.startswith("-") for t in rest)))
        if worktree_side:
            return "git restore overwrites worktree content from its source"
    if sub == "rebase":
        mentions_main = any(t in ("main", "master", "origin/main", "origin/master") for t in rest) \
            or re.search(r"\b(main|master)\b", seg_l)
        if mentions_main or current_branch in ("main", "master"):
            return "git rebase rewrites history on the protected main line"
    return ""


def deletion_reason(tokens, seg_l, program):
    """Reason string for the DELETION family, or "" when the segment deletes nothing.
    Beyond the plain programs, the family is checked in its smuggled forms: find -delete,
    perl -e unlink, mv into /dev/null, inline os.remove/shutil.rmtree, and deletion
    programs quoted inside interpreter strings."""
    if program in DELETION_CALLS:
        return f"'{program}' deletes content"
    if program == "find" and "-delete" in tokens[1:]:
        return "'find -delete' deletes what it matches"
    if program == "perl" and "unlink" in seg_l:
        return "'perl -e unlink' deletes files"
    if program in ("mv", "move-item") and "/dev/null" in seg_l:
        return "moving into /dev/null discards the content"
    if re.search(r"(os\.(remove|unlink)|shutil\.rmtree)\s*\(", seg_l):
        return "inline python deletes files"
    if program == "git" and tokens[1:2] == ["rm"]:
        return "'git rm' deletes tracked content"
    if program in INTERPRETERS:
        for t in tokens[1:]:
            base = re.sub(r"[\"']", "", t)
            base = re.sub(r"^.*[\\/]", "", base).lower()
            if base in DELETION_CALLS:
                return f"'{base}' deletes content (as an argument to '{tokens[0]}')"
    return ""


def git_branch(root_path):
    """Current branch name, for the rebase/main-context check. One call per run; empty
    when there is no repository, so the classifier then only sees what the text says."""
    if not root_path:
        return ""
    try:
        p = subprocess.run(["git", "-C", root_path, "branch", "--show-current"],
                           capture_output=True, text=True, encoding="utf-8", errors="replace")
    except OSError:
        return ""
    return p.stdout.strip().lower() if p.returncode == 0 else ""


def decision(segment, current_branch=""):
    tokens = [t for t in re.split(r"\s+", segment) if t]
    if not tokens:
        return None
    program = re.sub(r"^.*[\\/]", "", tokens[0])
    program = re.sub(r"\.(exe|cmd|ps1|bat|py|sh)$", "", program, flags=re.I).lower()
    sub = tokens[1].lower() if len(tokens) > 1 else ""

    seg_l = segment.lower()
    for d in DESTRUCTIVE:
        if d in seg_l:
            return {"kind": "destructive", "why": f"matches destructive pattern '{d}'"}

    if is_trash_put(tokens):
        return {"kind": "trash-put", "why": "deletion routed through trash.py (the bin)"}

    if program == "git":
        gd = git_destructive(tokens, seg_l, current_branch)
        if gd:
            return {"kind": "destructive", "why": gd}

    dr = deletion_reason(tokens, seg_l, program)
    if dr:
        return {"kind": "deletion", "why": dr}

    if program in NETWORK_CALLS:
        return {"kind": "network", "why": f"program '{program}' reaches the network"}
    if program in NETWORK_SUB and sub in NETWORK_SUB[program]:
        return {"kind": "network", "why": f"'{program} {sub}' reaches the network"}
    if program == "git" and re.search(r"\shttps?://", seg_l):
        return {"kind": "network", "why": "remote URL"}

    if program in WRITE_CALLS:
        return {"kind": "write", "why": f"'{program}' writes to disk"}
    if program in WRITE_SUB and sub in WRITE_SUB[program]:
        return {"kind": "write", "why": f"'{program} {sub}' writes to disk"}
    if re.search(r"(^|\s)(>|>>)\s", seg_l):
        return {"kind": "write", "why": "output redirection"}
    return {"kind": "read-or-exec", "why": f"'{program}' reads or executes"}


WIN_PATH = re.compile(r'([A-Za-z]:\\[^"\s]*|\\\\[^"\s]+)')
# POSIX absolute paths, as written by every bash caller: `/d/KF/x`, `/tmp/x`, `/`.
# The lookbehind keeps `https://host` from being read as a path (`:` or `/` before the slash).
POSIX_PATH = re.compile(r'(?<![\w:/])/[A-Za-z0-9._\-/]+')


def posix_to_win(p):
    """Map an MSYS/Git-Bash absolute path onto a Windows one.

    Git Bash rewrites argv but not environment variables and not text inside a single
    argument, so `/d/KF/x` reaches here verbatim. Without this map the classifier sees no
    absolute path at all and silently decides "nothing to check" -- i.e. allows.
    """
    m = re.match(r"^/([A-Za-z])(/.*)?$", p)
    if not m:
        return None
    drive = m.group(1).upper() + ":\\"
    rest = (m.group(2) or "").lstrip("/").replace("/", "\\")
    return drive + rest


def is_inside(full, root):
    """Path-boundary containment, not string-prefix containment.

    `startswith(root)` says D:\\proj-evil is inside D:\\proj, because "proj-evil" starts with
    "proj". That is a boundary that reads as enforced and is not -- the same silent class as
    a glob that never matches. Compare whole components: equal, or root + separator.
    """
    f, r = full.lower().replace("/", "\\"), root.lower().replace("/", "\\")
    return f == r or f.startswith(r + "\\")


def outside_root(segment, root_path):
    if not root_path:
        return []
    out = []
    for m in WIN_PATH.finditer(segment):
        p = m.group(1)
        try:
            full = os.path.normpath(p)
        except (OSError, ValueError):
            full = p
        if not is_inside(full, root_path):
            out.append(p)
    for m in POSIX_PATH.finditer(segment):
        p = m.group(0)
        full = posix_to_win(p)
        if full is None:
            # an absolute path we cannot place (bare `/`, or a form we do not model):
            # undecidable counts as outside, not as inside
            out.append(p)
            continue
        if not is_inside(full, root_path):
            out.append(p)
    return out


def main(argv):
    a = parse_args(argv)
    profile = a.get("profile", "read-only")
    if profile.lower() not in PROFILES:
        print(f"policy: -Profile 必须是 {sorted(PROFILES)} 之一")
        return 2
    profile = profile.lower()
    segments = split_command(a.get("command", ""))
    if not segments:
        print("policy: empty command")
        return 2

    root_in = a.get("root", "").strip()
    if root_in:
        if not os.path.isdir(root_in):
            # fail closed: an unreadable root would make every "inside" check meaningless
            print("policy: -Root 必须是已存在的目录（无法确认边界时不允许放行）")
            return 2
        root_path = os.path.normpath(os.path.abspath(root_in))
        if len(root_path) > 2:
            root_path = root_path.rstrip("/\\")
    else:
        root_path = ""

    denials, notes = [], []
    current_branch = git_branch(root_path)
    for seg in segments:
        d = decision(seg, current_branch)
        if d is None:
            continue
        if d["kind"] == "destructive":
            if "git-destructive" in a.get("allow", []):
                # one id, one invocation, printed out loud -- shaped like -AllowNetwork,
                # because a silent escape hatch would just be a hole
                notes.append(f"allowed by -Allow git-destructive (per-invocation grant): {d['why']}")
                continue
            alt = ("替代路径: 先 python tools/git_safe.py checkpoint --label <name> 留快照，"
                   "再由人工执行"
                   if "git" in d["why"] else
                   "替代路径: python tools/trash.py put <path> --reason <why>")
            denials.append(f"destructive: {d['why']}  ({alt})")
            continue
        if d["kind"] == "deletion":
            denials.append(f"deletion: {d['why']}  (替代路径: python tools/trash.py put <path> "
                           f"--reason <why> -- 工作区不许裸删，这是唯一放行口)")
            continue
        if d["kind"] == "trash-put":
            if profile == "read-only":
                denials.append(f"trash-put: the bin move is still a write "
                               f"(read-only profile permits no writes)")
                continue
            notes.append(f"deletion via trash.py allowed: {d['why']}")
            continue
        if d["kind"] == "network" and not a.get("allow_network"):
            denials.append(f"network: {d['why']}  (profile {profile}, network off -- "
                           f"pass -AllowNetwork to grant it explicitly)")
            continue
        if d["kind"] == "write":
            if profile == "read-only":
                denials.append(f"write: {d['why']}  (read-only profile permits no writes)")
                continue
            outside = outside_root(seg, root_path)
            if outside:
                denials.append(f"outside-root write: {', '.join(outside)}  "
                               f"(workspace-write is bounded by {root_path})")
                continue
            notes.append(f"write allowed inside workspace: {d['why']}")
            continue
        if d["kind"] == "read-or-exec" and profile == "read-only":
            # read-only still lets builds and test runs happen: they are how you observe a
            # project. They write to a cache dir, which is declared, not smuggled.
            notes.append(f"read/execute allowed under read-only (cache writes are expected): '{seg}'")
            continue
        notes.append(f"allowed: {d['why']}")

    for n in notes:
        print(f"policy: OK    {n}")
    if denials:
        for dn in denials:
            print(f"policy: DENY  {dn}")
        print("policy: 拒绝执行")
        return 1
    net = "on" if a.get("allow_network") else "off"
    print(f"policy: allow ({profile}, network={net})")
    if a.get("explain"):
        print("policy: 边界 -- 这是命令分类器 + 目录日志，不是内核沙箱："
              "越界写入只能由 journal 事后证明，不能由这里事前阻止")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
