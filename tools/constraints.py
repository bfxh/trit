#!/usr/bin/env python3
"""constraints: forbidden operations in this repo, as machine-checked rules.

Why a table and not a document: "don't touch the kernel's public API" written in a README
is enforced by whoever remembers it. Written as a rule with a diff pattern, it is enforced
by the next run. The difference is not discipline, it is whether a violation can fail.

Where the rules live: .agent/constraints.json, next to the workspace memory, owned by the
same repo. They travel with the project, not with the agent.

What a rule is: a pattern over the diff, scoped by path glob, with a severity.
  block -> the change is refused (exit 1)
  warn  -> the change is allowed but the finding is printed and must be acknowledged
Some rules also demand a companion (e.g. "unsafe needs a // SAFETY: note"), because the
real failure is not "used unsafe", it is "used unsafe and said nothing about why". The
companion can also be a fact outside the diff: `no-raw-delete` is satisfied by a trash
manifest entry, not by anything the patch itself could say.

Ported from constraints.ps1; refusal texts and exit codes are kept identical.
"""

import json
import os
import re
import subprocess
import sys

MODES = {"init", "list", "check"}

DEFAULT_RULES = [
    {
        "id": "no-test-deletion", "title": "不许删测试用例（要删必须显式允许）",
        "severity": "block", "scope": ["crates/**/tests/**"], "on": "removed",
        "pattern": r"#\[(test|tokio::test)\]",
        "message": "删掉一个 #[test] 等于静默缩小覆盖，允许的话用 -Allow no-test-deletion 显式说明",
    },
    {
        "id": "unsafe-needs-safety-note", "title": "新增 unsafe 必须同时给出 // SAFETY: 说明",
        "severity": "block", "scope": ["crates/**/src/**"], "on": "added",
        "pattern": r"\bunsafe\b",
        "companion": r"//\s*SAFETY:",
        "message": 'unsafe 本身不是错，"用了 unsafe 却不说为什么安全"才是',
    },
    {
        "id": "public-api-change", "title": "改公开 API 签名要单独放行",
        "severity": "block", "scope": ["crates/**/src/**"], "on": "removed",
        "pattern": r"^\s*pub\s+(async\s+)?(fn|struct|enum|trait|type|const|static)\b",
        "message": "公开面变化会波及所有调用方与集成测试；确认要改就 -Allow public-api-change",
    },
    {
        "id": "forecast-stays-out-of-verdict",
        "title": "预测不进判定层：Verdict/Evidence 文件不许出现 Forecast",
        "severity": "block",
        "scope": ["crates/**/src/verdict.rs", "crates/**/src/evidence.rs", "crates/**/src/**/derive*.rs"],
        "on": "added", "pattern": r"\bForecast\b",
        "message": "Forecast 只许收紧不许放宽，永不进 derive()；一旦出现在判定层就失去独立性",
    },
    {
        "id": "no-debug-output", "title": "src 里不许留 dbg!/println!",
        "severity": "block", "scope": ["crates/**/src/**"], "on": "added",
        "pattern": r"\b(dbg!|println!|eprintln!)",
        "message": "调试输出进库会污染调用方输出，也会拖慢热路径",
    },
    {
        "id": "no-new-unwrap", "title": "新增 unwrap/expect 需要说明（warn 不拦）",
        "severity": "warn", "scope": ["crates/**/src/**"], "on": "added",
        "pattern": r"\.(unwrap|expect)\(",
        "message": "内核里 panic 会打断判定链，宁可返回 Err；确实不会失败的路径请加注释",
    },
    {
        "id": "no-raw-delete", "title": "删已跟踪文件必须先进回收站（清单里要有对应 entry）",
        "severity": "block", "scope": ["**"], "on": "file-deleted",
        "companion": "trash-entry",
        "pattern": r"",
        "message": "裸删 = 内容无处可寻；先 python tools/trash.py put <path> --reason <why>，"
                  "回收站清单里有这条路径的 entry 才放行（命令侧对应 policy 的 DELETION 家族）",
    },
]


def parse_args(argv):
    out = {"allow": []}
    positional = [x for x in argv if not x.startswith("-")]
    i = 0
    while i < len(argv):
        a = argv[i]
        if not a.startswith("-"):
            i += 1
            continue
        name = a.lstrip("-")
        key = {"Mode": "mode", "Root": "root", "Diff": "diff", "Allow": "allow",
               "Commands": "commands"}.get(name, name.lower())
        val = argv[i + 1] if i + 1 < len(argv) else ""
        if key == "allow":
            out["allow"].extend([x for x in val.replace(",", " ").split() if x])
        else:
            out[key] = val
        i += 2
    if "mode" not in out and positional and positional[0] in MODES:
        out["mode"] = positional[0]
    return out


def glob_to_regex(g):
    """Placeholders first, then escape the literal parts, then swap placeholders back.

    Escaping before replacing was the bug: it turned '**' into '\\*\\*' and the wildcard
    silently stopped matching, so every rule scoped by path quietly skipped -- the rules all
    reported "pass" while checking nothing.
    """
    t = g.replace("**", "ZZDSZZ").replace("*", "ZZSSZZ")
    esc = re.escape(t)
    esc = esc.replace("ZZDSZZ", ".*").replace("ZZSSZZ", "[^/]*")
    return "^" + esc + "$"


def in_scope(path, globs):
    # A single glob and a list of globs are both natural to pass, and iterating a bare
    # string silently turns "crates/**/src/**" into its characters -- every rule would then
    # match nothing and report "pass". Accept both shapes rather than relying on callers.
    if isinstance(globs, str):
        globs = [globs]
    return any(re.search(glob_to_regex(g), path) for g in globs)


def read_text(path):
    if not os.path.exists(path):
        return ""
    with open(path, "r", encoding="utf-8-sig", newline="") as fh:
        return fh.read()


def diff_files(root, diff_file):
    if diff_file:
        raw = read_text(diff_file)
    else:
        p = subprocess.run(["git", "-C", root, "diff", "--unified=0", "HEAD"],
                           capture_output=True, text=True, encoding="utf-8", errors="replace")
        raw = p.stdout + p.stderr
    files = {}
    cur = ""
    pending = ""  # the --- a/<path> of the current header, kept until its +++ line says
                  # whether the file was modified (b/<path>) or deleted (/dev/null)
    for line in raw.split("\n"):
        m = re.match(r"^--- a/(.*)$", line)
        if m:
            pending = m.group(1).strip()
            continue
        if re.match(r"^\+\+\+ /dev/null", line):
            # whole-file deletion: git prints no `+++ b/` line here, so the old parser
            # never registered the file at all and every rule scoped to it silently
            # skipped -- deleting a whole test file would not even trip no-test-deletion
            if pending:
                cur = pending
                files.setdefault(cur, {"added": [], "removed": []})
                files[cur]["deleted"] = True
            pending = ""
            continue
        m = re.match(r"^\+\+\+ b/(.*)$", line)
        if m:
            cur = m.group(1).strip()
            files.setdefault(cur, {"added": [], "removed": []})
            pending = ""
            continue
        if not cur:
            continue
        m = re.match(r"^\+(.*)$", line)
        if m:
            files[cur]["added"].append(m.group(1))
            continue
        m = re.match(r"^-(.*)$", line)
        if m:
            files[cur]["removed"].append(m.group(1))
    return files


def trash_entry_exists(root, rel):
    """Companion for no-raw-delete: does the bin manifest carry an entry for this exact
    path (any state -- a purged entry still proves the deletion went through the bin)?

    The manifest is read from the DEFAULT store (<root>/.journal/trash), the same store
    journal.py and trash.py fall back to; a custom -Store site cannot be seen from here
    and reads as "no entry" -- fail closed, because a false block costs one retry while
    a false pass loses content.
    """
    manifest = os.path.join(root, ".journal", "trash", "manifest.json")
    try:
        with open(manifest, "r", encoding="utf-8-sig") as fh:
            entries = json.load(fh).get("entries", [])
    except (OSError, ValueError):
        return False
    want = rel.replace("\\", "/")
    return any(e.get("path") == want for e in entries)


def command_deletion_segments(text):
    """Segments that policy's DELETION family would refuse. One family, one home: the
    classification itself lives in policy.py and is imported, not re-implemented."""
    import policy  # same tools directory; the DELETION family has exactly one classifier
    out = []
    for seg in policy.split_command(text):
        d = policy.decision(seg)
        if d and d["kind"] == "deletion":
            out.append((seg, d["why"]))
    return out


def cmd_init(store, root, rules_file):
    if os.path.exists(rules_file):
        print(f"constraints: already present at {rules_file}")
        return 0
    os.makedirs(store, exist_ok=True)
    with open(rules_file, "w", encoding="utf-8", newline="\n") as fh:
        json.dump(DEFAULT_RULES, fh, ensure_ascii=False, indent=2)
        fh.write("\n")
    print(f"constraints: wrote {len(DEFAULT_RULES)} default rules to {rules_file}")
    return 0


def cmd_list(rules_file):
    if not os.path.exists(rules_file):
        print("constraints: not initialised, run -Mode init")
        return 2
    rules = json.loads(read_text(rules_file))
    print(f"{'id':<34} {'severity':<7} title")
    for r in rules:
        print(f"{r['id']:<34} {r['severity']:<7} {r['title']}")
    return 0


def cmd_check(root, rules_file, a):
    if not os.path.exists(rules_file):
        print("constraints: not initialised, run -Mode init")
        return 2
    rules = json.loads(read_text(rules_file))
    files = diff_files(root, a.get("diff", ""))
    cmd_segs = command_deletion_segments(a.get("commands", "")) if a.get("commands", "").strip() else []

    if not files and not cmd_segs:
        print("constraints: clean (no diff against HEAD)")
        return 0

    blocks, warns = [], []
    allow = a.get("allow", [])
    for seg, why in cmd_segs:
        finding = {"id": "no-raw-delete", "severity": "block", "file": "(command)",
                   "lines": 1, "sample": seg, "message": "命令侧裸删被拦：" + why}
        if "no-raw-delete" in allow:
            print(f"constraints: ALLOWED-BY-FLAG [no-raw-delete] (command) {seg}")
            continue
        blocks.append(finding)
    for path in sorted(files.keys()):
        added = files[path]["added"]
        removed = files[path]["removed"]
        for r in rules:
            if not in_scope(path, r.get("scope", [])):
                continue
            if r.get("on") == "file-deleted":
                # whole-file deletion; the companion is not a comment but a trash entry
                if not files[path].get("deleted"):
                    continue
                if r.get("companion") == "trash-entry" and trash_entry_exists(root, path):
                    continue  # the bin holds the content, rule is satisfied
                finding = {"id": r["id"], "severity": r["severity"], "file": path,
                           "lines": 1, "sample": "(file deleted)", "message": r.get("message", "")}
                if r["id"] in allow:
                    print(f"constraints: ALLOWED-BY-FLAG [{r['id']}] {path} (file deleted)")
                    continue
                (blocks if r["severity"] == "block" else warns).append(finding)
                continue
            side = {"added": added, "removed": removed}.get(r.get("on"), added + removed)
            hits = [ln for ln in side if re.search(r["pattern"], ln)]
            if not hits:
                continue
            if r.get("companion") and any(re.search(r["companion"], ln) for ln in added):
                continue  # the note is there, rule is satisfied
            finding = {"id": r["id"], "severity": r["severity"], "file": path,
                       "lines": len(hits), "sample": hits[0].strip(), "message": r.get("message", "")}
            if r["id"] in allow:
                print(f"constraints: ALLOWED-BY-FLAG [{r['id']}] {path} ({len(hits)} line(s))")
                continue
            (blocks if r["severity"] == "block" else warns).append(finding)

    for w in warns:
        print(f"constraints: WARN [{w['id']}] {w['file']} ({w['lines']} line(s)): {w['sample']}")
    for b in blocks:
        print(f"constraints: BLOCK [{b['id']}] {b['file']} ({b['lines']} line(s)): {b['sample']}\n"
              f"                      {b['message']}")

    if blocks:
        print(f"constraints: 拒绝 -- {len(blocks)} 条禁止操作（warn {len(warns)} 条）")
        return 1
    print(f"constraints: pass ({len(warns)} warning(s), 0 block)")
    return 0


def main(argv):
    a = parse_args(argv)
    mode = a.get("mode", "")
    root_in = a.get("root", "").strip()
    if mode not in MODES:
        print(f"constraints: -Mode 必须是 {sorted(MODES)} 之一")
        return 2
    if not root_in or not os.path.isdir(root_in):
        print("constraints: -Root 必须是已存在的目录")
        return 2
    root = os.path.abspath(root_in).rstrip("/\\")
    store = os.path.join(root, ".agent")
    rules_file = os.path.join(store, "constraints.json")
    if mode == "init":
        return cmd_init(store, root, rules_file)
    if mode == "list":
        return cmd_list(rules_file)
    return cmd_check(root, rules_file, a)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
