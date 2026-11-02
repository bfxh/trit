#!/usr/bin/env python3
"""autoflow：把任意一条命令包进"先快照、失败自动回退"里——不依赖 git。

用法：
  python scripts/autoflow.py run -- <命令...> [--label 说明] [--keep-on-fail]

语义：
  1. 跑命令前存一份快照（`snapshot.save`）；
  2. 命令成功 ⇒ 改动保留，打印快照 id（要退就 `snapshot.py restore <id>`）；
  3. 命令失败/中断 ⇒ **自动回退到快照**（含删掉命令新建的文件），并把失败码原样返回；
  4. `--keep-on-fail`：失败也保留现场（排查用），但仍打印快照 id。

它不调用 git：git 不在、远端不可达、工作树没提交，这套照样成立。
"""

from __future__ import annotations

import importlib.util
import pathlib
import subprocess
import sys
import tempfile

ROOT = pathlib.Path(__file__).resolve().parents[1]


def load_snapshot():
    spec = importlib.util.spec_from_file_location(
        "trit_snapshot", pathlib.Path(__file__).resolve().parent / "snapshot.py"
    )
    module = importlib.util.module_from_spec(spec)
    sys.modules["trit_snapshot"] = module
    spec.loader.exec_module(module)
    return module


def run(root: pathlib.Path, argv: list[str], label: str, keep_on_fail: bool) -> int:
    snap = load_snapshot()
    snap_id = snap.save(root, label or "autoflow")
    manifest = snap.resolve(root, snap_id)
    try:
        result = subprocess.run(argv, cwd=root)
        returncode = result.returncode
    except OSError as exc:
        # 命令不存在/没权限：也是一次失败，必须走回退路径（返回 127 与 shell 一致）
        print(f"autoflow: 命令起不来：{exc}")
        returncode = 127
    except KeyboardInterrupt:
        print("autoflow: 被中断，回退到快照")
        code = snap.restore(root, manifest, prune=True, dry_run=False)
        return 130 if code == 0 else code
    if returncode == 0:
        print(f"autoflow: 成功，改动已保留；回退：python scripts/snapshot.py restore {snap_id}")
        return 0
    if keep_on_fail:
        print(f"autoflow: 失败（rc={returncode}），按 --keep-on-fail 保留现场；回退：restore {snap_id}")
        return returncode
    print(f"autoflow: 失败（rc={returncode}），自动回退到 {snap_id}")
    code = snap.restore(root, manifest, prune=True, dry_run=False)
    return returncode if code == 0 else code


def self_test() -> int:
    failures: list[str] = []
    total = [0]

    def check(name: str, actual, expected) -> None:
        total[0] += 1
        if actual != expected:
            failures.append(name)
            print(f"FAIL {name}：期望 {expected!r}，实际 {actual!r}")

    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        (root / "a.txt").write_bytes(b"orig\n")

        # 1) 成功：改动保留，快照多一条
        snap = load_snapshot()
        before = len(snap.list_snapshots(root))
        code = run(root, [sys.executable, "-c", "open('a.txt','wb').write(b'changed\\n')"], "", False)
        check("成功返回 0", code, 0)
        check("成功后改动保留", (root / "a.txt").read_bytes(), b"changed\n")
        check("成功也存了快照", len(snap.list_snapshots(root)), before + 1)

        # 2) 失败：自动回退（含删掉新建文件），失败码原样返回
        code = run(
            root,
            [
                sys.executable,
                "-c",
                "open('a.txt','wb').write(b'broken\\n');"
                "open('new.txt','wb').write(b'x\\n');"
                "raise SystemExit(7)",
            ],
            "",
            False,
        )
        check("失败码原样返回", code, 7)
        check("失败后文件被还原", (root / "a.txt").read_bytes(), b"changed\n")
        check("失败后新建文件被删", (root / "new.txt").exists(), False)

        # 3) 命令不存在：也是失败，回退并返回 127
        (root / "a.txt").write_bytes(b"orig3\n")
        code = run(root, ["definitely-not-a-command-xyz"], "", False)
        check("命令不存在→127", code, 127)
        check("命令不存在时状态不变", (root / "a.txt").read_bytes(), b"orig3\n")

        # 4) --keep-on-fail：保留现场
        code = run(
            root,
            [sys.executable, "-c", "open('a.txt','wb').write(b'kept\\n');raise SystemExit(3)"],
            "",
            True,
        )
        check("keep 时失败码返回", code, 3)
        check("keep 时现场保留", (root / "a.txt").read_bytes(), b"kept\n")

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
    if not argv or argv[0] != "run" or "--" not in argv:
        print(__doc__)
        return 2
    split = argv.index("--")
    options, command = argv[:split], argv[split + 1 :]
    if not command:
        print("run 后面要跟命令，例如：autoflow.py run -- cargo test -p trit-core")
        return 2
    label = options[options.index("--label") + 1] if "--label" in options else ""
    return run(ROOT, command, label, keep_on_fail="--keep-on-fail" in options)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
