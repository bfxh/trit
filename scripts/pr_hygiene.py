#!/usr/bin/env python3
"""pr_hygiene —— 开审 PR 前的机械检查（重复树 / 描述与代码不符 / 冲突面）。

判据（每条都能用 `--self-test` 复算）：
  DUP      两个 open PR 的**整棵树内容相同**（fork 双开、重复提交）
  MISMATCH 描述里点名的 `xxx.rs` 模块在 diff 的改动文件里不存在（#128 形态）
  OVERLAP  两个 open PR 改到同一文件（冲突面 / 隐性依赖）

设计：只用 `gh` + 标准库，不 clone、不改仓库；`--enforce` 时 DUP/MISMATCH 非零退出。
默认仓库 `bfxh/trit`。

用法：
  python scripts/pr_hygiene.py --repo bfxh/trit
  python scripts/pr_hygiene.py --json --enforce
  python scripts/pr_hygiene.py --self-test
"""

from __future__ import annotations

import hashlib
import json
import re
import subprocess
import sys

MODULE_TOKEN = re.compile(r"([a-z0-9_]+\.rs)")
DEFAULT_REPO = "bfxh/trit"


def gh_json(*args: str):
    result = subprocess.run(
        ["gh", "api", *args],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        encoding="utf-8",
        errors="replace",
    )
    if result.returncode != 0:
        raise SystemExit(f"gh api 失败: {' '.join(args)}\n{result.stderr.strip()}")
    return json.loads(result.stdout)


def tree_hash(repo: str, sha: str) -> tuple[str, dict[str, str]]:
    data = gh_json(f"repos/{repo}/git/trees/{sha}?recursive=1")
    files = {}
    for item in data.get("tree", []):
        if item.get("type") == "blob":
            files[item["path"]] = item["sha"]
    digest = hashlib.sha256(
        "\n".join(f"{path}:{files[path]}" for path in sorted(files)).encode("utf-8")
    ).hexdigest()[:16]
    return digest, files


def changed_files(repo: str, number: int) -> list[str]:
    paths: list[str] = []
    page = 1
    while True:
        batch = gh_json(f"repos/{repo}/pulls/{number}/files?per_page=100&page={page}")
        if not batch:
            return paths
        paths.extend(item["filename"] for item in batch)
        if len(batch) < 100:
            return paths
        page += 1


def dedupe_by_tree(entries: list[dict]) -> list[dict]:
    """同树分组：保留编号最大（最新）的一份，其余判 DUP。"""
    groups: dict[str, list[dict]] = {}
    for entry in entries:
        groups.setdefault(entry["tree"], []).append(entry)
    findings = []
    for tree, members in groups.items():
        if len(members) < 2:
            continue
        members = sorted(members, key=lambda item: item["number"])
        keep = members[-1]["number"]
        for item in members[:-1]:
            findings.append({
                "code": "DUP",
                "target": f"#{item['number']}",
                "message": f"与 #{keep} 同树（{tree}）：只保留一份，另一份关闭并注明同树于 #{keep}",
            })
    return findings


def body_mismatch(number: int, body: str, files: list[str]) -> list[dict]:
    changed_basenames = {path.rsplit("/", 1)[-1] for path in files}
    mentioned = sorted(set(MODULE_TOKEN.findall(body or "")))
    missing = [name for name in mentioned if name not in changed_basenames]
    # 单个零星提及可能是举例；成批点名却不在 diff 里才是 #128 那种"描述与代码不符"。
    if len(missing) < 2:
        return []
    return [{
        "code": "MISMATCH",
        "target": f"#{number}",
        "message": f"描述点名但 diff 里没有：{', '.join(missing)}（先要作者澄清，不按描述评审）",
    }]


def near_duplicates(entries: list[dict], trees: dict[int, dict[str, str]]) -> list[dict]:
    """改动文件集合相同、内容只差少量文件 ⇒ 同一改动的两个版本（#126/#128 形态）。"""
    findings = []
    for index, left in enumerate(entries):
        for right in entries[index + 1:]:
            if set(left["files"]) != set(right["files"]):
                continue
            shared = set(left["files"])
            if not shared:
                continue
            left_tree = trees.get(left["number"], {})
            right_tree = trees.get(right["number"], {})
            differ = [path for path in shared
                      if left_tree.get(path) != right_tree.get(path)]
            if 0 < len(differ) <= max(2, len(shared) // 10):
                findings.append({
                    "code": "NEAR-DUP",
                    "target": f"#{left['number']} ↔ #{right['number']}",
                    "message": f"改动文件集合完全相同，仅 {len(differ)} 个文件内容不同"
                               f"（{', '.join(differ[:3])}）⇒ 同一改动的两个版本，只留一份",
                })
    return findings


def overlaps(entries: list[dict]) -> list[dict]:
    findings = []
    for index, left in enumerate(entries):
        for right in entries[index + 1:]:
            shared = sorted(set(left["files"]) & set(right["files"]))
            if shared:
                findings.append({
                    "code": "OVERLAP",
                    "target": f"#{left['number']} ↔ #{right['number']}",
                    "message": f"{len(shared)} 个共同文件：{', '.join(shared[:3])}"
                               + (" …" if len(shared) > 3 else ""),
                })
    return findings


def collect(repo: str) -> list[dict]:
    numbers = [
        item["number"] for item in gh_json(f"repos/{repo}/pulls?state=open&per_page=100")
    ]
    entries = []
    trees: dict[int, dict[str, str]] = {}
    for number in numbers:
        meta = gh_json(f"repos/{repo}/pulls/{number}")
        digest, blobs = tree_hash(repo, meta["head"]["sha"])
        trees[number] = blobs
        entries.append({
            "number": number,
            "title": meta["title"],
            "draft": meta["draft"],
            "tree": digest,
            "files": changed_files(repo, number),
            "body": meta.get("body") or "",
        })
    return entries, trees


def report(entries: list[dict], findings: list[dict], as_json: bool) -> None:
    if as_json:
        print(json.dumps({"prs": entries, "findings": findings}, ensure_ascii=False, indent=2))
        return
    print(f"open PR {len(entries)} 个：")
    for item in entries:
        print(f"  #{item['number']:<4} tree={item['tree']} files={len(item['files']):<3}"
              f"{' draft' if item['draft'] else ''}  {item['title'][:52]}")
    if not findings:
        print("无 finding。")
        return
    print()
    for item in findings:
        print(f"{item['code']:<8} {item['target']:<18} {item['message']}")


def self_test() -> int:
    entries = [
        {"number": 126, "tree": "aaa", "title": "a", "draft": False,
         "files": ["crates/x/src/rle.rs", "CHANGELOG.md"], "body": "`rle.rs`"},
        {"number": 128, "tree": "aaa", "title": "b", "draft": False,
         "files": ["crates/x/src/rle.rs", "CHANGELOG.md"], "body": "`rle.rs` `huffman.rs`"},
        {"number": 130, "tree": "bbb", "title": "c", "draft": False,
         "files": ["CHANGELOG.md", "crates/y/src/lib.rs"], "body": "`lib.rs`"},
    ]
    dup = dedupe_by_tree(entries)
    assert dup and dup[0]["target"] == "#126", dup
    trees = {126: {"crates/x/src/rle.rs": "a", "CHANGELOG.md": "c"},
             128: {"crates/x/src/rle.rs": "b", "CHANGELOG.md": "c"},
             130: {"CHANGELOG.md": "c", "crates/y/src/lib.rs": "d"}}
    assert near_duplicates(entries, trees), "同文件集合的两版未判红"
    mismatch = body_mismatch(128, "base32.rs huffman.rs rle.rs", entries[1]["files"])
    assert mismatch and "huffman.rs" in mismatch[0]["message"], mismatch
    assert body_mismatch(126, "rle.rs 一个零星提及", entries[0]["files"]) == []
    over = overlaps(entries)
    assert any("CHANGELOG.md" in item["message"] for item in over), over
    assert dedupe_by_tree([{"number": 1, "tree": "z", "title": ""}]) == []
    print("SELF-TEST OK（DUP/MISMATCH/OVERLAP 均判红）")
    return 0


def main(argv: list[str]) -> int:
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    repo = DEFAULT_REPO
    as_json = False
    enforce = False
    rest = list(argv)
    while rest:
        arg = rest.pop(0)
        if arg == "--repo" and rest:
            repo = rest.pop(0)
        elif arg.startswith("--repo="):
            repo = arg.split("=", 1)[1]
        elif arg == "--json":
            as_json = True
        elif arg == "--enforce":
            enforce = True
        elif arg == "--self-test":
            return self_test()
        else:
            raise SystemExit(f"未知参数: {arg}")
    entries, trees = collect(repo)
    findings = dedupe_by_tree(entries) + near_duplicates(entries, trees)
    for item in entries:
        findings += body_mismatch(item["number"], item["body"], item["files"])
    findings += overlaps(entries)
    report(entries, findings, as_json)
    if enforce and any(item["code"] in ("DUP", "NEAR-DUP", "MISMATCH") for item in findings):
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
