#!/usr/bin/env python3
"""快照底座：不依赖 git 的字节级保存 / 校验 / 回退。

用法：
  python scripts/snapshot.py save [--label 说明]
  python scripts/snapshot.py list
  python scripts/snapshot.py verify [<id>]
  python scripts/snapshot.py restore [<id>] [--dry-run] [--prune] [--keep-new]
  python scripts/snapshot.py --self-test

设计约束（为什么不是 git 的薄包装）：
  - **不调用 git**：git 不在、远端不可达、工作树没提交，这套照样能存能退；
  - **字节级**：按 SHA-256 存内容寻址对象，回退后逐文件校验，换行/编码不许被顺手改掉；
  - **默认不删新文件**：回退只覆盖改动与补回删除；快照之后新写的文件只有显式 `--prune` 才清；
  - **中断可续**：回退前落 `.trit/pending.json`，中途断掉下次直接再跑同一条 restore（幂等）。
"""

from __future__ import annotations

import hashlib
import json
import os
import pathlib
import shutil
import subprocess
import sys
import tempfile
import time

ROOT = pathlib.Path(__file__).resolve().parents[1]
STORE = ".trit"
# 构建产物与本地状态不进快照：它们可再生，塞进来只会让快照变成垃圾场。
EXCLUDE_DIRS = {".git", "target", ".trit", "node_modules", "__pycache__", ".venv"}
MANIFEST_VERSION = 1


def sha256_file(path: pathlib.Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def iter_files(root: pathlib.Path) -> list[str]:
    """仓库里"进快照"的文件（POSIX 相对路径，已排序）。"""
    files: list[str] = []
    for dirpath, dirnames, filenames in os.walk(root):
        dirnames[:] = sorted(d for d in dirnames if d not in EXCLUDE_DIRS)
        for name in filenames:
            path = pathlib.Path(dirpath) / name
            if path.is_symlink() or not path.is_file():
                continue
            files.append(path.relative_to(root).as_posix())
    return sorted(files)


def blob_path(root: pathlib.Path, sha: str) -> pathlib.Path:
    return root / STORE / "objects" / sha[:2] / sha


def snapshots_dir(root: pathlib.Path) -> pathlib.Path:
    return root / STORE / "snapshots"


def write_atomic(path: pathlib.Path, data: bytes) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_suffix(path.suffix + ".tmp")
    tmp.write_bytes(data)
    os.replace(tmp, path)


def store_blob(root: pathlib.Path, path: pathlib.Path, sha: str) -> None:
    target = blob_path(root, sha)
    if target.exists():
        return
    write_atomic(target, path.read_bytes())


def git_facts(root: pathlib.Path) -> dict:
    """git 只是**附注**：拿不到也要能存能退，所以失败一律吞掉。"""
    facts: dict[str, str] = {}
    for key, argv in (
        ("head", ["git", "rev-parse", "HEAD"]),
        ("branch", ["git", "rev-parse", "--abbrev-ref", "HEAD"]),
    ):
        try:
            out = subprocess.run(
                argv, cwd=root, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True
            )
        except OSError:
            return {}
        if out.returncode == 0:
            facts[key] = out.stdout.strip()
    return facts


def new_id() -> str:
    return time.strftime("%Y%m%dT%H%M%SZ", time.gmtime()) + "-" + os.urandom(2).hex()


def save(root: pathlib.Path, label: str) -> str:
    files: dict[str, dict] = {}
    for rel in iter_files(root):
        path = root / rel
        sha = sha256_file(path)
        store_blob(root, path, sha)
        files[rel] = {"sha": sha, "size": path.stat().st_size}
    snap_id = new_id()
    manifest = {
        "version": MANIFEST_VERSION,
        "id": snap_id,
        "label": label,
        "created": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "files": files,
        "excluded_dirs": sorted(EXCLUDE_DIRS),
        "git": git_facts(root),
    }
    write_atomic(
        snapshots_dir(root) / f"{snap_id}.json",
        json.dumps(manifest, ensure_ascii=False, indent=1, sort_keys=True).encode("utf-8"),
    )
    total = sum(entry["size"] for entry in files.values())
    print(f"snapshot {snap_id}：{len(files)} 个文件，{total / 1024 / 1024:.2f} MiB（{label or '无标签'}）")
    return snap_id


def list_snapshots(root: pathlib.Path) -> list[dict]:
    out: list[dict] = []
    for path in sorted(snapshots_dir(root).glob("*.json")):
        try:
            data = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError):
            continue
        out.append(data)
    return out


def resolve(root: pathlib.Path, snap_id: str) -> dict:
    if not snap_id or snap_id == "latest":
        snaps = list_snapshots(root)
        if not snaps:
            raise SystemExit("没有任何快照（先跑 save）")
        return snaps[-1]
    path = snapshots_dir(root) / f"{snap_id}.json"
    if not path.is_file():
        raise SystemExit(f"找不到快照 {snap_id}（用 list 看有哪些）")
    return json.loads(path.read_text(encoding="utf-8"))


def diff(root: pathlib.Path, manifest: dict) -> dict[str, list[str]]:
    """清单 vs 现状：missing（被删）/ changed（改过）/ extra（快照后新增）。"""
    wanted = manifest["files"]
    current = iter_files(root)
    current_set = set(current)
    missing = sorted(set(wanted) - current_set)
    extra = sorted(current_set - set(wanted))
    changed: list[str] = []
    for rel in sorted(set(wanted) & current_set):
        if sha256_file(root / rel) != wanted[rel]["sha"]:
            changed.append(rel)
    return {"missing": missing, "changed": changed, "extra": extra}


def restore(root: pathlib.Path, manifest: dict, prune: bool, dry_run: bool) -> int:
    plan = diff(root, manifest)
    actions = plan["missing"] + plan["changed"]
    print(
        f"回退到 {manifest['id']}：补回 {len(plan['missing'])}，覆盖 {len(plan['changed'])}，"
        f"新增未动 {len(plan['extra'])}{'（--prune：删除）' if prune else '（保留；要删加 --prune）'}"
    )
    for rel in actions:
        print(f"  {'补回' if rel in plan['missing'] else '覆盖'} {rel}")
    for rel in plan["extra"]:
        print(f"  {'删除' if prune else '保留'} {rel}")
    if dry_run:
        print("dry-run：未改动任何文件")
        return 0

    pending = root / STORE / "pending.json"
    write_atomic(
        pending,
        json.dumps({"id": manifest["id"], "started": time.time()}, ensure_ascii=False).encode("utf-8"),
    )
    for rel in actions:
        data = blob_path(root, manifest["files"][rel]["sha"]).read_bytes()
        target = root / rel
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(data)
        if sha256_file(target) != manifest["files"][rel]["sha"]:
            raise SystemExit(f"回退失败：{rel} 写回后校验不一致（pending 保留，可重跑 restore）")
    if prune:
        for rel in plan["extra"]:
            (root / rel).unlink()
        # 删空的目录（自底向上）：命令新建的目录树不该留在"已回退"的工作树里
        for directory in sorted({(root / rel).parent for rel in plan["extra"]}, key=lambda p: len(p.parts), reverse=True):
            while directory != root and directory.is_dir() and not any(directory.iterdir()):
                directory.rmdir()
                directory = directory.parent
    pending.unlink(missing_ok=True)
    print("回退完成，逐文件 SHA-256 校验通过")
    return 0


def verify(root: pathlib.Path, manifest: dict) -> int:
    plan = diff(root, manifest)
    if not any(plan.values()):
        print(f"verify {manifest['id']}：工作树与快照逐字节一致")
        return 0
    print(f"verify {manifest['id']}：不一致")
    for key, label in (("missing", "缺失"), ("changed", "已改"), ("extra", "快照后新增")):
        for rel in plan[key]:
            print(f"  {label} {rel}")
    return 1


def self_test() -> int:
    """每条判定都要有反证：漏检/多退/误删/假 dry-run 都得当场红。"""
    failures: list[int] = []
    total = [0]

    def check(name: str, actual, expected) -> None:
        total[0] += 1
        if actual != expected:
            failures.append(total[0])
            print(f"FAIL {name}：期望 {expected!r}，实际 {actual!r}")

    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        (root / "src").mkdir()
        (root / "src" / "a.rs").write_bytes(b"line1\nline2\n")
        (root / "README.md").write_bytes(b"hello\r\nworld\r\n")  # CRLF 也要逐字节还原
        snap_id = save(root, "自测")

        # 1) 原样：verify 绿
        check("原样 verify 通过", verify(root, resolve(root, snap_id)), 0)

        # 2) 改一个、删一个、加一个
        (root / "src" / "a.rs").write_bytes(b"line1\nCHANGED\n")
        (root / "README.md").unlink()
        (root / "src" / "new.rs").write_bytes(b"new\n")
        plan = diff(root, resolve(root, snap_id))
        check("检出改动", plan["changed"], ["src/a.rs"])
        check("检出删除", plan["missing"], ["README.md"])
        check("检出新增", plan["extra"], ["src/new.rs"])

        # 3) dry-run 不许改任何东西
        before = sha256_file(root / "src" / "a.rs")
        restore(root, resolve(root, snap_id), prune=False, dry_run=True)
        check("dry-run 不改文件", sha256_file(root / "src" / "a.rs"), before)

        # 4) 不 prune：还原改动与删除，但保留新增
        restore(root, resolve(root, snap_id), prune=False, dry_run=False)
        check("还原 CRLF 文件字节", (root / "README.md").read_bytes(), b"hello\r\nworld\r\n")
        check("还原改动文件", (root / "src" / "a.rs").read_bytes(), b"line1\nline2\n")
        check("默认保留新增", (root / "src" / "new.rs").exists(), True)
        check("回退后 verify 仍有新增", verify(root, resolve(root, snap_id)), 1)

        # 5) prune：连新增一起清掉，verify 全绿
        restore(root, resolve(root, snap_id), prune=True, dry_run=False)
        check("prune 删除新增", (root / "src" / "new.rs").exists(), False)
        check("prune 后 verify 通过", verify(root, resolve(root, snap_id)), 0)

        # 6) 回退是幂等的（中断后重跑同一条命令要能收敛）
        restore(root, resolve(root, snap_id), prune=True, dry_run=False)
        check("二次回退仍通过", verify(root, resolve(root, snap_id)), 0)

        # 7) prune：命令新建的空目录也要清掉（不能只删文件留一堆空目录）
        (root / "made" / "deep").mkdir(parents=True)
        (root / "made" / "deep" / "x.txt").write_bytes(b"x")
        restore(root, resolve(root, snap_id), prune=True, dry_run=False)
        check("prune 清掉命令新建的目录", (root / "made").exists(), False)

        # 8) 对象库坏了必须报错，而不是"看起来成功"
        blob = blob_path(root, resolve(root, snap_id)["files"]["src/a.rs"]["sha"])
        blob.write_bytes(b"tampered")
        (root / "src" / "a.rs").write_bytes(b"x\n")
        try:
            restore(root, resolve(root, snap_id), prune=False, dry_run=False)
            restored_ok = True
        except SystemExit:
            restored_ok = False
        check("对象被篡改时回退报错", restored_ok, False)

        # 9) 构建产物不进快照
        (root / "target").mkdir()
        (root / "target" / "junk.bin").write_bytes(b"junk")
        check("target/ 不进快照", "target/junk.bin" in iter_files(root), False)

    if failures:
        print(f"self-test: {total[0] - len(failures)}/{total[0]} passed")
        return 1
    print(f"self-test: {total[0]}/{total[0]} passed")
    return 0


def main(argv: list[str]) -> int:
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    if "--self-test" in argv:
        return self_test()
    root = ROOT
    args = [a for a in argv if a != "--root"]
    if "--root" in argv:
        root = pathlib.Path(argv[argv.index("--root") + 1]).resolve()
        args = [a for a in argv if a not in ("--root", str(root))]
    if not args:
        print(__doc__)
        return 2
    command, rest = args[0], args[1:]

    pending = root / STORE / "pending.json"
    if pending.exists() and command not in ("restore", "verify"):
        print(f"检测到未完成回退（{pending}）：先跑 `restore` 收尾，别在中间状态上继续改。")
        return 1

    if command == "save":
        label = rest[rest.index("--label") + 1] if "--label" in rest else ""
        save(root, label)
        return 0
    if command == "list":
        snaps = list_snapshots(root)
        if not snaps:
            print("没有任何快照")
            return 0
        for snap in snaps:
            git = snap.get("git") or {}
            print(
                f"{snap['id']}  {snap['created']}  {len(snap['files'])} 文件  "
                f"{snap.get('label') or '无标签'}  {git.get('branch', '-')}@{git.get('head', '-')[:8]}"
            )
        return 0
    if command == "verify":
        return verify(root, resolve(root, rest[0] if rest else "latest"))
    if command == "restore":
        target = next((a for a in rest if not a.startswith("--")), "latest")
        return restore(
            root,
            resolve(root, target),
            prune="--prune" in rest,
            dry_run="--dry-run" in rest,
        )
    print(f"未知命令: {command}")
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
