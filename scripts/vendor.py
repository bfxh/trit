#!/usr/bin/env python3
"""离线自足开关：把 crates.io 依赖整批 vendor 进仓，断网也能编；随时可还原。

用法：
  python scripts/vendor.py save [--dir vendor]     # 生成 vendor/ 并改写 .cargo/config.toml
  python scripts/vendor.py restore                 # 还原 .cargo/config.toml、删掉 vendor/
  python scripts/vendor.py status
  python scripts/vendor.py --self-test

口径：
  - 只动两处：`.cargo/config.toml`（插入带标记的 source replacement 段）与 vendor 目录；
  - 改配置前先备份字节到 `.cargo/config.toml.tritbak`，`restore` 优先按备份还原；
  - 两个方向都用 `cargo metadata --offline --locked` 验，不通就判红。
"""

from __future__ import annotations

import pathlib
import shutil
import subprocess
import sys
import tempfile

ROOT = pathlib.Path(__file__).resolve().parents[1]
CONFIG = pathlib.Path(".cargo") / "config.toml"
BACKUP = pathlib.Path(".cargo") / "config.toml.tritbak"
START = "# >>> trit-vendor（scripts/vendor.py 管理；restore 会删掉这一段）"
END = "# <<< trit-vendor"


def block_for(directory: str) -> str:
    return (
        f"{START}\n"
        "[source.crates-io]\n"
        'replace-with = "vendored-sources"\n'
        "\n"
        "[source.vendored-sources]\n"
        f'directory = "{directory}"\n'
        f"{END}\n"
    )


def splice(text: str, block: str) -> str:
    """插入或替换带标记段；其余字节保持原样（不重排、不删注释）。"""
    if START in text and END in text:
        head, _, rest = text.partition(START)
        _, _, tail = rest.partition(END)
        return head + block + tail.lstrip("\n")
    if text and not text.endswith("\n"):
        text += "\n"
    return text + ("\n" if text else "") + block


def strip_block(text: str) -> str:
    if START not in text or END not in text:
        return text
    head, _, rest = text.partition(START)
    _, _, tail = rest.partition(END)
    if head.endswith("\n\n"):  # 插段时多垫的一行
        head = head[:-1]
    return head + tail.lstrip("\n")


def cargo(args: list[str], root: pathlib.Path) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        ["cargo", *args],
        cwd=root,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        encoding="utf-8",
        errors="replace",
    )


def dir_size(path: pathlib.Path) -> int:
    return sum(p.stat().st_size for p in path.rglob("*") if p.is_file())


def backup_config(root: pathlib.Path) -> None:
    """只备份一次：第二次 save 不许把"真正的原件"覆盖成已改过的版本。"""
    config = root / CONFIG
    backup = root / BACKUP
    if config.exists() and not backup.exists():
        backup.write_bytes(config.read_bytes())


def save(root: pathlib.Path, directory: str, force: bool = False) -> int:
    target = root / directory
    config = root / CONFIG
    config.parent.mkdir(parents=True, exist_ok=True)
    text = config.read_text(encoding="utf-8") if config.exists() else ""
    mine = (root / BACKUP).exists() or (START in text and END in text)
    if target.exists() and any(target.iterdir()) and not mine and not force:
        print(f"拒绝：{directory}/ 已存在且不是本工具建的（要覆盖加 --force）")
        return 2
    backup_config(root)
    result = cargo(["vendor", "--locked", directory], root)
    if result.returncode != 0:
        print("cargo vendor 失败：")
        print(result.stdout[-2000:])
        return 1
    text = config.read_text(encoding="utf-8") if config.exists() else ""
    config.write_text(splice(text, block_for(directory)), encoding="utf-8", newline="")
    print(f"已 vendor 到 {directory}（{dir_size(target) / 1024 / 1024:.1f} MiB）")
    return verify(root, offline=True)


def restore(root: pathlib.Path, verify_build: bool = True, force: bool = False) -> int:
    config = root / CONFIG
    backup = root / BACKUP
    text = config.read_text(encoding="utf-8") if config.exists() else ""
    # 只删"我们自己的" vendor/：没有备份、也没有标记段时，别碰用户的目录
    owned = backup.exists() or (START in text and END in text)
    if backup.exists():
        config.parent.mkdir(parents=True, exist_ok=True)
        config.write_bytes(backup.read_bytes())
        backup.unlink()
        print("已按备份还原 .cargo/config.toml")
    elif config.exists():
        stripped = strip_block(config.read_text(encoding="utf-8"))
        if stripped.strip():
            config.write_text(stripped, encoding="utf-8", newline="")
        else:
            config.unlink()
        print("已移除配置里的 trit-vendor 段")
    path = root / "vendor"
    if path.is_dir() and (owned or force):
        shutil.rmtree(path)
        print("已删除 vendor/")
    elif path.is_dir():
        print("vendor/ 不是本工具建的（无备份、无标记段），保留；要删加 --force")
    return verify(root, offline=True) if verify_build else 0


def verify(root: pathlib.Path, offline: bool) -> int:
    args = ["metadata", "--locked", "--format-version", "1"]
    if offline:
        args.append("--offline")
    result = cargo(args, root)
    if result.returncode != 0:
        print("verify 失败（cargo metadata）：")
        print(result.stdout[-2000:])
        return 1
    print(f"verify 通过：cargo metadata{'（--offline）' if offline else ''} 可解析")
    return 0


def status(root: pathlib.Path) -> int:
    config = root / CONFIG
    text = config.read_text(encoding="utf-8") if config.exists() else ""
    has_block = START in text and END in text
    vendor = root / "vendor"
    size = dir_size(vendor) / 1024 / 1024 if vendor.is_dir() else 0.0
    print(f"配置段：{'在' if has_block else '不在'}｜vendor/：{'有' if vendor.is_dir() else '无'}（{size:.1f} MiB）")
    return 0


def self_test() -> int:
    failures: list[str] = []
    total = [0]

    def check(name: str, actual, expected) -> None:
        total[0] += 1
        if actual != expected:
            failures.append(name)
            print(f"FAIL {name}：期望 {expected!r}，实际 {actual!r}")

    original = "[net]\nretry = 2\n"
    spliced = splice(original, block_for("vendor"))
    check("插入后保留原有配置", "[net]\nretry = 2\n" in spliced, True)
    check("插入后带标记段", START in spliced and END in spliced, True)
    check("插入后指向目录", 'directory = "vendor"' in spliced, True)
    check("insert 使用 vendored-sources", 'replace-with = "vendored-sources"' in spliced, True)
    again = splice(spliced, block_for("vendor"))
    check("重复 save 不重复插段", again.count(START), 1)
    check("还原后与原文逐字节一致", strip_block(spliced), original)
    check("没有标记段时 strip 不动", strip_block(original), original)
    other = splice("", block_for("deps-vendor"))
    check("空配置也能插", other.startswith(START), True)
    check("自定义目录名", 'directory = "deps-vendor"' in other, True)
    # 没有尾换行的配置不许被吞掉最后一行，也不许少垫分隔空行
    check(
        "无尾换行时保留原文并垫空行",
        splice("[net]\nretry = 2", block_for("vendor")),
        "[net]\nretry = 2\n\n" + block_for("vendor"),
    )

    # 没有备份/标记段时，restore 不许删用户自己的 vendor/
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        (root / "vendor").mkdir()
        (root / "vendor" / "keep.txt").write_text("mine", encoding="utf-8")
        check("外部 vendor/ 不被删", restore(root, verify_build=False), 0)
        check("外部 vendor/ 仍在", (root / "vendor" / "keep.txt").exists(), True)
        check("--force 才删", restore(root, verify_build=False, force=True), 0)
        check("--force 后已删", (root / "vendor").exists(), False)

    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        (root / CONFIG).parent.mkdir(parents=True)
        # 配置与备份**故意不同**：只有"优先按备份"才会得到 retry = 2
        (root / CONFIG).write_bytes(b"[net]\nretry = 3\n" + block_for("vendor").encode())
        (root / BACKUP).write_bytes(b"[net]\nretry = 2\n")
        check("restore 返回 0", restore(root, verify_build=False), 0)
        check("restore 优先按备份还原", (root / CONFIG).read_bytes(), b"[net]\nretry = 2\n")
        # 备份只做一次：已有备份时再 save 不许把真原件覆盖成改过的版本
        (root / CONFIG).write_bytes(b"[net]\nretry = 9\n")
        (root / BACKUP).write_bytes(b"[net]\nretry = 2\n")
        backup_config(root)
        check("第二次备份不覆盖原件", (root / BACKUP).read_bytes(), b"[net]\nretry = 2\n")

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
    command = argv[0] if argv else "status"
    if command == "save":
        directory = argv[argv.index("--dir") + 1] if "--dir" in argv else "vendor"
        return save(root, directory, force="--force" in argv)
    if command == "restore":
        return restore(root, force="--force" in argv)
    if command == "status":
        return status(root)
    print(__doc__)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
