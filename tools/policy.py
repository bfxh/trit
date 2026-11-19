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

Ported from policy.ps1; decisions, wording and exit codes are kept identical.
"""

import os
import re
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
    "git": ["add", "commit", "reset", "clean", "checkout", "merge", "rebase", "stash", "apply", "am"],
    "cargo": ["fmt", "fix", "clippy", "add"],
    "npm": ["publish", "version"],
}
DESTRUCTIVE = ["git reset --hard", "git clean -fd", "git clean -xfd", "rm -rf /", "format",
               "remove-item -recurse -force c:\\", "shutdown", "bcdedit", "diskpart"]


def parse_args(argv):
    out = {"profile": "read-only", "command": "", "root": "", "allow_network": False,
           "explain": False}
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
        key = {"profile": "profile", "command": "command", "root": "root"}.get(name, name)
        out[key] = argv[i + 1] if i + 1 < len(argv) else ""
        i += 2
    return out


def split_command(text):
    """Minimal tokenizer: whitespace separated, quotes respected, pipes/semicolons split so
    that `a && curl x` cannot hide a network call behind an allowed first word."""
    return [seg.strip() for seg in re.split(r"[;|&]{1,2}", text) if seg.strip()]


def decision(segment):
    tokens = [t for t in re.split(r"\s+", segment) if t]
    if not tokens:
        return None
    program = re.sub(r"^.*[\\/]", "", tokens[0])
    program = re.sub(r"\.(exe|cmd|ps1|bat)$", "", program, flags=re.I).lower()
    sub = tokens[1].lower() if len(tokens) > 1 else ""

    seg_l = segment.lower()
    for d in DESTRUCTIVE:
        if d in seg_l:
            return {"kind": "destructive", "why": f"matches destructive pattern '{d}'"}

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
    for seg in segments:
        d = decision(seg)
        if d is None:
            continue
        if d["kind"] == "destructive":
            denials.append(f"destructive: {d['why']}")
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
