#!/usr/bin/env python3
"""统一验证入口：Verus 规格证明 + 与主库实现的对拍。

用法：
    python scripts/verify.py              # 检查工具链 → cargo-verus verify → cargo test
    python scripts/verify.py --install    # 下载并校验固定版本 Verus / Z3 到 ~/tools
    python scripts/verify.py --json       # 机器可读报告（CI / 下游复用）
    python scripts/verify.py --only verify|bridge

环境变量：
    TRIT_VERUS_DIR          Verus 解压目录（默认 ~/tools/verus-x86-linux 或 verus-x86-win）
    TRIT_Z3_DIR             Z3 解压目录（默认 ~/tools/z3-4.16.0-x64-glibc-2.39 / win）
    TRIT_VERUS_TOOLCHAIN    rustup toolchain（默认 1.100.0-<host>）

平台说明：
    Linux 走官方 x86-linux release；Windows release 需要 MSVC linker
    （Visual Studio Build Tools）。没有 MSVC 时请在 WSL 里运行本脚本。
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import shutil
import subprocess
import sys
import urllib.request
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
VERUS_DIR = ROOT / "verus"

VERUS_VERSION = "0.2026.11.22.1ce6a59"
VERUS_SHA256 = {
    "linux": "c955d5958877f4f907cce80aa996d4239a481984829d82e325938c7bf242b0db",
    "windows": "3e7d238871baf6fe90d4fa978ee2fdf99f2f4589b198e3eeb1f2ddc7038f08a5",
}
VERUS_ASSET = {
    "linux": f"verus-{VERUS_VERSION}-x86-linux.zip",
    "windows": f"verus-{VERUS_VERSION}-x86-win.zip",
}
Z3_VERSION = "4.16.0"
Z3_SHA256 = {
    "linux": "7288c49a5bd6dbafd7b0b0d1f65956b91672da24b08f09242919af159be3418e",
    "windows": "de4d813e47202394a093547dbdb5699ee076529aa853463e007539775cd7e836",
}
Z3_ASSET = {
    "linux": "z3-4.16.0-x64-glibc-2.39.zip",
    "windows": "z3-4.16.0-x64-win.zip",
}
RELEASE_BASE = "https://github.com/verus-lang/verus/releases/download"
Z3_BASE = "https://github.com/Z3Prover/z3/releases/download/z3-4.16.0"


def host_os() -> str:
    system = platform.system().lower()
    if system == "windows":
        return "windows"
    if system == "linux":
        return "linux"
    raise SystemExit(f"verify: 暂不支持 {platform.system()}；请在 Linux / WSL 运行")


def host_toolchain() -> str:
    explicit = os.environ.get("TRIT_VERUS_TOOLCHAIN")
    if explicit:
        return explicit
    host = "x86_64-pc-windows-msvc" if host_os() == "windows" else "x86_64-unknown-linux-gnu"
    return f"1.100.0-{host}"


def default_verus_dir() -> Path:
    if os.environ.get("TRIT_VERUS_DIR"):
        return Path(os.environ["TRIT_VERUS_DIR"])
    name = "verus-x86-win" if host_os() == "windows" else "verus-x86-linux"
    return Path.home() / "tools" / name


def default_z3_dir() -> Path:
    if os.environ.get("TRIT_Z3_DIR"):
        return Path(os.environ["TRIT_Z3_DIR"])
    suffix = "win" if host_os() == "windows" else "glibc-2.39"
    return Path.home() / "tools" / f"z3-{Z3_VERSION}-x64-{suffix}"


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def download(url: str, dest: Path) -> None:
    print(f"verify: 下载 {url}")
    with urllib.request.urlopen(url) as response, dest.open("wb") as handle:
        shutil.copyfileobj(response, handle)


def install() -> None:
    """下载固定版本 Verus / Z3，校验 SHA-256 后解压到 ~/tools。"""
    os_name = host_os()
    tools = Path.home() / "tools"
    tools.mkdir(parents=True, exist_ok=True)

    targets = [
        (
            f"{RELEASE_BASE}/release%2F{VERUS_VERSION}/{VERUS_ASSET[os_name]}",
            VERUS_SHA256[os_name],
            tools / VERUS_ASSET[os_name],
            tools,
        ),
        (
            f"{Z3_BASE}/{Z3_ASSET[os_name]}",
            Z3_SHA256[os_name],
            tools / Z3_ASSET[os_name],
            tools,
        ),
    ]
    for url, expected, archive, out_dir in targets:
        if not archive.exists():
            download(url, archive)
        actual = sha256(archive)
        if actual != expected:
            raise SystemExit(f"verify: {archive.name} SHA-256 不匹配\n  期望 {expected}\n  实际 {actual}")
        print(f"verify: {archive.name} SHA-256 校验通过")
        with zipfile.ZipFile(archive) as bundle:
            bundle.extractall(out_dir)
    print("verify: 安装完成；再跑一次 python scripts/verify.py 执行验证")


def tool_env() -> dict[str, str]:
    env = os.environ.copy()
    verus_dir = default_verus_dir()
    z3_dir = default_z3_dir()
    z3_bin = z3_dir / "bin"
    paths = [str(verus_dir), str(z3_bin), str(Path.home() / ".cargo" / "bin")]
    env["PATH"] = os.pathsep.join(paths + [env.get("PATH", "")])
    env.setdefault("RUSTUP_HOME", str(Path.home() / ".rustup"))
    env.setdefault("CARGO_HOME", str(Path.home() / ".cargo"))
    # 本机 Windows 的 ~/.cargo/config.toml 可能配置 sccache；WSL 里不可执行。
    # 空字符串按 cargo 语义表示“禁用 wrapper”。
    env.setdefault("RUSTC_WRAPPER", "")
    env["RUSTUP_TOOLCHAIN"] = host_toolchain()
    env.setdefault("CARGO_TERM_COLOR", "never")
    return env


def find_tool(name: str) -> str:
    # 以 tool_env 的 PATH 为准：CI 里 verus/z3 刚解压到 ~/tools，当前进程 PATH 还没有它们。
    candidate = shutil.which(name, path=tool_env()["PATH"])
    if candidate:
        return candidate
    raise SystemExit(f"verify: 找不到 {name}；先跑 python scripts/verify.py --install")


def run(cmd: list[str], env: dict[str, str], cwd: Path) -> subprocess.CompletedProcess[str]:
    print("verify: $", " ".join(cmd))
    return subprocess.run(cmd, cwd=cwd, env=env, text=True, capture_output=True)


def check_toolchain(env: dict[str, str]) -> str:
    verus = find_tool("verus.exe" if host_os() == "windows" else "verus")
    result = run([verus, "--version"], env, ROOT)
    if result.returncode != 0:
        sys.stderr.write(result.stdout + result.stderr)
        raise SystemExit("verify: verus --version 失败；检查 TRIT_VERUS_TOOLCHAIN 与 rustup")
    version = next(
        (line.strip() for line in result.stdout.splitlines() if "Version:" in line),
        result.stdout.strip().splitlines()[0] if result.stdout.strip() else "unknown",
    )
    rustup = find_tool("rustup")
    installed = run([rustup, "toolchain", "list"], env, ROOT)
    if host_toolchain() not in installed.stdout:
        raise SystemExit(
            f"verify: 缺少 rustup toolchain {host_toolchain()}；"
            f"运行 rustup toolchain install {host_toolchain()} --profile minimal"
        )
    return version


def main() -> int:
    parser = argparse.ArgumentParser(description="统一验证入口（Verus + 主库对拍）")
    parser.add_argument("--install", action="store_true", help="下载并校验 Verus / Z3")
    parser.add_argument("--json", action="store_true", help="输出机器可读报告")
    parser.add_argument("--only", choices=["verify", "bridge"], help="只跑其中一步")
    args = parser.parse_args()

    if args.install:
        install()
        return 0

    env = tool_env()
    report: dict[str, object] = {
        "engine": "verus",
        "verus_version": VERUS_VERSION,
        "toolchain": host_toolchain(),
        "host": host_os(),
        "ok": False,
    }

    version = check_toolchain(env)
    report["verus_cli"] = version

    cargo_verus = find_tool("cargo-verus.exe" if host_os() == "windows" else "cargo-verus")
    if args.only != "bridge":
        result = run([cargo_verus, "verify"], env, VERUS_DIR)
        output = result.stdout + result.stderr
        if not args.json:
            sys.stdout.write(output)
        report["verify"] = {
            "ok": result.returncode == 0,
            "summary": next((line.strip() for line in output.splitlines() if "verification results::" in line), ""),
        }
        if result.returncode != 0:
            report["ok"] = False
            if args.json:
                print(json.dumps(report, ensure_ascii=False, indent=2))
            return 1

    if args.only != "verify":
        cargo = find_tool("cargo")
        result = run([cargo, "test"], env, VERUS_DIR)
        output = result.stdout + result.stderr
        if not args.json:
            sys.stdout.write(output)
        report["bridge"] = {
            "ok": result.returncode == 0,
            "summary": next((line.strip() for line in output.splitlines() if "test result:" in line), ""),
        }
        if result.returncode != 0:
            report["ok"] = False
            if args.json:
                print(json.dumps(report, ensure_ascii=False, indent=2))
            return 1

    report["ok"] = True
    if args.json:
        print(json.dumps(report, ensure_ascii=False, indent=2))
    else:
        print("verify: OK（Verus 证明 + bridge 对拍均通过）")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
