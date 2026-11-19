#!/usr/bin/env python3
"""memory: workspace memory that outlives the agent.

WHY THIS EXISTS
  An agent's context dies with its session. A project does not. So the durable record has
  to live in the project, on disk, in the repo -- and be readable by a fresh process with
  zero prior context. That is the whole test: kill the agent, run `brief`, and you still
  know what was done, what broke, and what is still open.

WHY IT IS NOT THE AGENT'S PRIVATE NOTEBOOK
  Anything the agent "just remembers" is proprietary and unverifiable. Here every entry
  must carry evidence (the command, its exit code, the commit), so a later reader can
  re-run the check instead of trusting a claim. Entries without evidence are refused at
  write time and flagged at audit time -- fail-closed, same rule as everywhere else.

WHY IT CANNOT ABSORB THE KERNEL
  Workspace memory belongs to the project it sits in. Two mechanical barriers:
    1. meta.json records owner_root; a memory directory copied into another project is
       refused (its owner does not match), so kernel notes cannot travel into a client repo.
    2. scope=kernel is only accepted when the directory was initialised with --kernel.
       A normal project cannot file "kernel" entries even by mistake.
  Plus: evidence paths must stay inside the root, and secret-looking content is refused.

Ported from memory.ps1; behaviour is meant to be identical, including the refusal texts.
"""

import hashlib
import json
import os
import re
import sys
import uuid
from datetime import datetime

MODES = {"init", "note", "brief", "recall", "audit", "list", "revise"}
KINDS = {"decision", "pitfall", "fact", "todo", "command"}
SCOPES = {"project", "kernel"}

SECRET_PATTERNS = [
    r"password", r"passwd", r"secret=", r"api[_\-]?key", r"token=",
    r"BEGIN (RSA |OPENSSH |EC )?PRIVATE KEY", r"sk-[A-Za-z0-9]{8}",
]
# Drive-letter paths are unambiguous. A leading "/" is only absolute when it is NOT
# preceded by a path character -- otherwise "tools/costprobe.ps1" gets misread as
# "/costprobe.ps1" and every legitimate entry is refused.
PATH_RE = re.compile(r"([A-Za-z]:\\[^ ;,|]*|(?:^|[\s\"'(=])(/[^\s;,|]+))")


def parse_args(argv):
    # bare_mode: the first non-flag token is the mode, so `memory.sh brief -Root x` works
    # without writing `-Mode`. Both forms are accepted because both are natural in a shell.
    positional = [x for x in argv if not x.startswith("-")]
    out = {"kind": "fact", "scope": "project", "max_lines": 40, "kernel": False}
    i = 0
    while i < len(argv):
        a = argv[i]
        if not a.startswith("-"):
            i += 1
            continue
        name = a.lstrip("-")
        key = {
            "Mode": "mode", "Root": "root", "Kind": "kind", "Title": "title",
            "Detail": "detail", "Evidence": "evidence", "Tags": "tags",
            "Scope": "scope", "Query": "query", "Id": "id", "MaxLines": "max_lines",
            "Kernel": "kernel",
        }.get(name, name.lower())
        if key == "kernel":
            out["kernel"] = True
            i += 1
            continue
        val = argv[i + 1] if i + 1 < len(argv) else ""
        out[key] = int(val) if key == "max_lines" and val else val
        i += 2
    if "mode" not in out and positional and positional[0] in MODES:
        out["mode"] = positional[0]
    return out


def iso_now():
    return datetime.now().astimezone().isoformat(timespec="seconds")


def parse_iso(s):
    if not s:
        return None
    s = str(s).strip()
    m = re.search(r"(\d{2}:\d{2}:\d{2})\.(\d+)", s)
    if m:
        s = s[:m.start()] + f"{m.group(1)}.{m.group(2)[:6]}" + s[m.end():]
    try:
        dt = datetime.fromisoformat(s)
    except ValueError:
        for fmt in ("%Y-%m-%d", "%m/%d/%Y %H:%M:%S"):
            try:
                return datetime.strptime(s, fmt)
            except ValueError:
                continue
        return None
    if dt.tzinfo is None:
        dt = dt.replace(tzinfo=datetime.now().astimezone().tzinfo)
    return dt


def sort_key(e):
    # Sort on the parsed instant, not the string: producers differ in fractional precision,
    # and "…:33+08:00" sorts before "…:33.1+08:00" even though they are the same moment.
    return parse_iso(e.get("at")) or datetime.min.replace(tzinfo=datetime.now().astimezone().tzinfo)


class Store:
    def __init__(self, root):
        self.root = root.rstrip("/\\")
        self.mem = os.path.join(self.root, ".agent", "memory")
        self.meta = os.path.join(self.mem, "meta.json")
        self.log = os.path.join(self.mem, "log.jsonl")
        self.brief = os.path.join(self.mem, "BRIEF.md")

    def read(self, path):
        if not os.path.exists(path):
            return ""
        with open(path, "r", encoding="utf-8-sig", newline="") as fh:
            return fh.read()

    def write(self, path, text):
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "w", encoding="utf-8", newline="\n") as fh:
            fh.write(text)

    def append(self, path, text):
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "a", encoding="utf-8", newline="\n") as fh:
            fh.write(text)

    def entries(self):
        if not os.path.exists(self.log):
            return []
        out = []
        for line in self.read(self.log).split("\n"):
            if not line.strip():
                continue
            try:
                out.append(json.loads(line))
            except json.JSONDecodeError:
                continue
        return out

    def rewrite(self, entries):
        body = "\n".join(json.dumps(e, ensure_ascii=False, separators=(",", ":"))
                         for e in sorted(entries, key=sort_key))
        self.write(self.log, body + "\n")


def entry_key(kind, title, detail):
    return f"{kind}|{title.strip()}|{hashlib.sha256(detail.encode('utf-8')).hexdigest()[:8]}"


def leak_of(text):
    for p in SECRET_PATTERNS:
        if re.search(p, text, re.IGNORECASE):
            return f"looks like a credential ('{p}')"
    return ""


def outside_path(text, root):
    for m in PATH_RE.finditer(text):
        p = m.group(1) or m.group(0).strip()
        if not p:
            continue
        if not os.path.isabs(p) and not p.startswith("/"):
            continue
        norm = os.path.abspath(p)
        if not norm.lower().startswith(root.lower()):
            return p
    return ""


def load_meta(st):
    if not os.path.exists(st.meta):
        return None
    try:
        return json.loads(st.read(st.meta))
    except json.JSONDecodeError:
        return None


def cmd_init(st, a):
    m = load_meta(st)
    if m:
        print(f"memory: already initialised at {st.mem} (owner={m.get('owner_root')}, kind={m.get('kind')})")
        return 0
    repo_kind = "kernel" if a.get("kernel") else "project"
    st.write(st.meta, json.dumps({
        "version": 1, "owner_root": st.root, "kind": repo_kind, "created": iso_now(),
    }, ensure_ascii=False, indent=2) + "\n")
    st.write(st.log, "")
    st.write(st.brief, "# 工作区记忆（自动生成，勿手改）\n")
    print(f"memory: initialised ({repo_kind}) at {st.mem}")
    return 0


def cmd_note(st, a):
    m = load_meta(st)
    if not m:
        print("memory: not initialised, run -Mode init first")
        return 2
    if m.get("owner_root") != st.root:
        print(f"memory: 拒绝 -- 这个记忆目录属于 {m.get('owner_root')}，不属于 {st.root}（记忆不可跨项目搬运）")
        return 1
    if a.get("scope") == "kernel" and m.get("kind") != "kernel":
        print("memory: 拒绝 -- 本项目初始化时不是 kernel 仓，不能记 scope=kernel 的条目（内核不许塞进项目记忆）")
        return 1
    title = a.get("title", "")
    kind = a.get("kind", "fact")
    if not title:
        print("memory: -Title is required")
        return 2
    if kind != "todo" and not a.get("evidence", ""):
        print(f"memory: 拒绝 -- '{kind}' 条目必须带 -Evidence（命令/退出码/commit），否则审计时无法复核")
        return 1
    detail, tags = a.get("detail", ""), a.get("tags", "")
    leak = leak_of(f"{title}\n{detail}\n{tags}")
    if leak:
        print(f"memory: 拒绝 -- 内容 {leak}，不要写进项目记忆")
        return 1
    bad = outside_path(f"{detail} {a.get('evidence', '')}", st.root)
    if bad:
        print(f"memory: 拒绝 -- 证据里出现工作区之外的路径 {bad}（记忆只记本项目的事）")
        return 1

    key = entry_key(kind, title, detail)
    entries = st.entries()
    same = [e for e in entries if e.get("key") == key]
    if same:
        # same claim, same evidence class: refresh the timestamp instead of stacking duplicates
        e = same[0]
        e["at"] = iso_now()
        e["evidence"] = a.get("evidence", "")
        st.rewrite(entries)
        print(f"memory: updated '{title}' (same claim, no duplicate)")
        return 0
    eid = "m" + uuid.uuid4().hex[:8]
    entries.append({
        "id": eid, "at": iso_now(), "kind": kind, "scope": a.get("scope", "project"),
        "title": title, "detail": detail, "evidence": a.get("evidence", ""),
        "tags": tags, "key": key,
    })
    st.append(st.log, json.dumps(entries[-1], ensure_ascii=False, separators=(",", ":")) + "\n")
    print(f"memory: recorded {eid} [{kind}] {title}")
    return 0


def cmd_revise(st, a):
    # A stale claim is worse than a missing one: it reads as fact and nobody rechecks it.
    # Revising keeps the id (so references stay valid) and stamps when it was corrected.
    m = load_meta(st)
    if not m:
        print("memory: not initialised, run -Mode init first")
        return 2
    iid = a.get("id", "")
    if not iid:
        print("memory: -Id is required for revise（先用 -Mode list 找 id）")
        return 2
    if m.get("owner_root") != st.root:
        print(f"memory: 拒绝 -- 该记忆属于 {m.get('owner_root')}")
        return 1
    entries = st.entries()
    target = [e for e in entries if e.get("id") == iid]
    if not target:
        print(f"memory: 没有 id={iid} 的条目")
        return 2
    e = target[0]
    if e.get("kind") != "todo" and not a.get("evidence", ""):
        print("memory: 拒绝 -- revise 后仍需 -Evidence（否则这次更正本身无法复核）")
        return 1
    leak = leak_of(f"{a.get('title', '')}\n{a.get('detail', '')}\n{a.get('tags', '')}")
    if leak:
        print(f"memory: 拒绝 -- 内容 {leak}")
        return 1
    bad = outside_path(f"{a.get('detail', '')} {a.get('evidence', '')}", st.root)
    if bad:
        print(f"memory: 拒绝 -- 证据里出现工作区之外的路径 {bad}")
        return 1

    if a.get("title"):
        e["title"] = a["title"]
    if a.get("detail"):
        e["detail"] = a["detail"]
    if a.get("evidence"):
        e["evidence"] = a["evidence"]
    if a.get("tags"):
        e["tags"] = a["tags"]
    e["revised"] = iso_now()
    e["key"] = entry_key(e.get("kind", "fact"), e.get("title", ""), e.get("detail", ""))
    st.rewrite(entries)
    print(f"memory: revised {iid} -> {e.get('title')}")
    return 0


def cmd_brief(st, a):
    if not os.path.exists(st.log):
        print("memory: not initialised")
        return 2
    entries = st.entries()
    lines = ["# 工作区记忆", "", f"项目：{st.root}（共 {len(entries)} 条记录）", ""]

    def top(kind, n):
        return sorted([e for e in entries if e.get("kind") == kind], key=sort_key, reverse=True)[:n]

    for kind, n, head in (("pitfall", 5, "## 已知坑（别踩第二次）"),
                          ("decision", 5, "## 已定决策"),
                          ("todo", 3, "## 未解决"),
                          ("command", 3, "## 验证过的命令")):
        sel = top(kind, n)
        if sel:
            lines.append(head)
            lines += [f"- {e.get('title')}" for e in sel]
            lines.append("")
    lines.append("> 详细内容用 -Mode recall -Query <词> 取，别把整本日志灌进上下文。")

    # Hard budget: this file is read at the start of every session, so its size is a
    # per-session cost paid forever. Cap it, and say so when capped.
    max_lines = a.get("max_lines") or 40
    capped = lines[:max_lines]
    note = f"\n> （已截断到 {max_lines} 行，原文 {len(lines)} 行）" if len(lines) > max_lines else ""
    text = "\n".join(capped) + note + "\n"
    st.write(st.brief, text)
    print(text, end="")
    return 0


def cmd_recall(st, a):
    q = a.get("query", "")
    if not q:
        print("memory: -Query is required")
        return 2
    entries = st.entries()
    hits = [e for e in entries
            if q in (e.get("title") or "") or q in (e.get("detail") or "") or q in (e.get("tags") or "")]
    hits = sorted(hits, key=sort_key, reverse=True)[:5]
    if not hits:
        print(f"memory: no hit for '{q}'")
        return 0
    for h in hits:
        d = h.get("detail") or ""
        if len(d) > 120:
            d = d[:120] + "…"
        print(f"[{h.get('kind')}] {h.get('title')}")
        if d:
            print(f"        {d}")
        if h.get("evidence"):
            print(f"        evidence: {h.get('evidence')}")
    return 0


def cmd_audit(st, a):
    if not os.path.exists(st.log):
        print("memory: not initialised")
        return 2
    entries = st.entries()
    bad = []
    ids = {}
    for e in entries:
        eid = e.get("id")
        if eid in ids:
            bad.append(f"duplicate id {eid}")
        else:
            ids[eid] = True
        if not e.get("title"):
            bad.append(f"{eid}: empty title")
        if e.get("kind") != "todo" and not e.get("evidence"):
            bad.append(f"{eid} [{e.get('kind')}]: 无证据（写的时候不该被接受）")
        leak = leak_of(f"{e.get('title')} {e.get('detail')}")
        if leak:
            bad.append(f"{eid}: {leak}")
    if bad:
        for b in bad:
            print(f"memory: AUDIT FAIL {b}")
        return 1
    print(f"memory: audit clean ({len(entries)} entries, all carry evidence)")
    return 0


def cmd_list(st, a):
    entries = st.entries()
    print(f"{'id':<10} {'kind':<9} {'scope':<8} title")
    for e in sorted(entries, key=sort_key, reverse=True):
        print(f"{e.get('id'):<10} {e.get('kind'):<9} {e.get('scope'):<8} {e.get('title')}")
    return 0


def main(argv):
    a = parse_args(argv)
    mode = a.get("mode", "")
    if mode not in MODES:
        print(f"memory: -Mode 必须是 {sorted(MODES)} 之一")
        return 2
    root = a.get("root", "").strip()
    if not root or not os.path.isdir(root):
        print("memory: -Root 必须是已存在的目录")
        return 2
    if a.get("kind") not in KINDS:
        print(f"memory: -Kind 必须是 {sorted(KINDS)} 之一")
        return 2
    if a.get("scope") not in SCOPES:
        print("memory: -Scope 必须是 project|kernel")
        return 2
    st = Store(os.path.abspath(root))
    return {
        "init": cmd_init, "note": cmd_note, "revise": cmd_revise, "brief": cmd_brief,
        "recall": cmd_recall, "audit": cmd_audit, "list": cmd_list,
    }[mode](st, a)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
