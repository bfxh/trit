#!/usr/bin/env python3
"""多环境 / 多容器验证矩阵。

同一份工作区、同一 seed，分别在：
- 原生宿主（Windows / Linux）；
- WSL Linux；
- Rust 官方容器（默认 1.100 与 1.88 两个镜像）
跑 `device_adapt`（合成仿真 + 策略）与 `device_env_check`（CPU/环境检测），
对比 `device_adapt` 的 64 行 JSON 读数与 `device_env_check` 的 OS/核数/后端。

边界：容器/WSL 共享同一台物理机，不是多硬件；本脚本验证**可移植性与
环境检测**，不做跨机器性能结论。容器 CPU 配额不同，CPU 延迟只作检测项，
不要求与原生相同。
"""

from __future__ import annotations

import argparse
import json
import os
import platform
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

if hasattr(sys.stdout, "reconfigure"):
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")

ROOT = Path(__file__).resolve().parents[1]


def run(cmd, *, cwd=None, env=None, timeout=1800):
    merged = os.environ.copy()
    merged["CARGO_TERM_COLOR"] = "never"
    merged["RUSTC_WRAPPER"] = ""
    if env:
        merged.update(env)
    return subprocess.run(
        [str(part) for part in cmd],
        cwd=str(cwd) if cwd else None,
        env=merged,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        timeout=timeout,
    )


def parse_jsonl(text):
    rows = []
    for line in text.splitlines():
        line = line.strip()
        if not line.startswith("{"):
            continue
        try:
            rows.append(json.loads(line))
        except json.JSONDecodeError:
            continue
    return rows


def compare_value(left, right, path, diffs, *, tol=1e-5):
    if isinstance(left, dict) and isinstance(right, dict):
        for key in sorted(set(left) | set(right)):
            if key not in left or key not in right:
                diffs.append(f"{path}.{key}: key missing on one side")
                continue
            if key == "path" or key.endswith("_ms") or key == "ms":
                continue
            compare_value(left[key], right[key], f"{path}.{key}", diffs, tol=tol)
        return
    if isinstance(left, list) and isinstance(right, list):
        if len(left) != len(right):
            diffs.append(f"{path}: length {len(left)} != {len(right)}")
            return
        for index, (a, b) in enumerate(zip(left, right)):
            compare_value(a, b, f"{path}[{index}]", diffs, tol=tol)
        return
    if isinstance(left, bool) or isinstance(right, bool):
        if left != right:
            diffs.append(f"{path}: {left!r} != {right!r}")
        return
    if isinstance(left, (int, float)) and isinstance(right, (int, float)):
        scale = max(1.0, abs(float(left)), abs(float(right)))
        if abs(float(left) - float(right)) > tol * scale:
            diffs.append(f"{path}: {left!r} != {right!r}")
        return
    if left != right:
        diffs.append(f"{path}: {left!r} != {right!r}")


def compare_runs(baseline, candidate):
    diffs = []
    if len(baseline) != len(candidate):
        diffs.append(f"line count {len(baseline)} != {len(candidate)}")
    for index, (left, right) in enumerate(zip(baseline, candidate)):
        compare_value(left, right, f"line{index}", diffs)
    return diffs


def first_train_index(rows):
    for index, row in enumerate(rows):
        if row.get("stage") == "train":
            return index
    return len(rows)


def max_numeric_delta(left, right, path="root"):
    best = (0.0, None)
    if isinstance(left, dict) and isinstance(right, dict):
        for key in set(left) & set(right):
            if key == "path" or key.endswith("_ms") or key == "ms":
                continue
            delta, where = max_numeric_delta(left[key], right[key], f"{path}.{key}")
            if delta > best[0]:
                best = (delta, where)
        return best
    if isinstance(left, list) and isinstance(right, list):
        for index, (a, b) in enumerate(zip(left, right)):
            delta, where = max_numeric_delta(a, b, f"{path}[{index}]")
            if delta > best[0]:
                best = (delta, where)
        return best
    if (
        isinstance(left, (int, float))
        and not isinstance(left, bool)
        and isinstance(right, (int, float))
        and not isinstance(right, bool)
    ):
        delta = abs(float(left) - float(right))
        return (delta, f"{path}: {left!r} != {right!r}")
    return best


def summarize_env(rows):
    for row in rows:
        if row.get("stage") == "env":
            return {
                "os": row.get("os"),
                "arch": row.get("arch"),
                "cpus": row.get("cpus"),
                "simd_backend": row.get("simd_backend"),
                "rustc": row.get("rustc"),
            }
    return {}


def native_command(seed, target):
    env = {}
    if target:
        env["CARGO_TARGET_DIR"] = str(target)
    adapt = run(
        [
            "cargo",
            "run",
            "--locked",
            "-q",
            "--release",
            "-p",
            "trit-gpu",
            "--example",
            "device_adapt",
            "--",
            "--seed",
            str(seed),
        ],
        cwd=ROOT,
        env=env,
    )
    env_check = run(
        [
            "cargo",
            "run",
            "--locked",
            "-q",
            "--release",
            "-p",
            "trit-gpu",
            "--example",
            "device_env_check",
        ],
        cwd=ROOT,
        env=env,
    )
    return adapt, env_check


def wsl_path(path: Path) -> str:
    posix = path.resolve().as_posix()
    if len(posix) >= 3 and posix[1] == ":":
        return "/mnt/" + posix[0].lower() + "/" + posix[3:]
    return posix


def wsl_run(distro, seed):
    workspace = wsl_path(ROOT)
    command = (
        f"cd {workspace} && export CARGO_TARGET_DIR=/tmp/trit-matrix-target "
        f"RUSTC_WRAPPER= CARGO_TERM_COLOR=never && "
        f"cargo run --locked -q --release -p trit-gpu --example device_adapt -- --seed {seed}"
    )
    adapt = run(["wsl.exe", "-d", distro, "--", "bash", "-lc", command])
    env_command = command.replace(
        "device_adapt -- --seed " + str(seed), "device_env_check"
    )
    env_check = run(["wsl.exe", "-d", distro, "--", "bash", "-lc", env_command])
    return adapt, env_check


def docker_present() -> bool:
    try:
        return run(["docker", "version", "--format", "{{.Server.Version}}"], timeout=30).returncode == 0
    except (OSError, subprocess.TimeoutExpired):
        return False


def docker_ensure_image(image, timeout):
    if run(["docker", "image", "inspect", image], timeout=60).returncode == 0:
        return None
    return run(["docker", "pull", image], timeout=timeout)


def docker_run(image, label, seed, cpus, timeout):
    volume = "trit-matrix-target-" + "".join(
        char if char.isalnum() else "-" for char in label
    )
    mount = f"{ROOT.resolve().as_posix()}:/src"
    base = [
        "docker",
        "run",
        "--rm",
        "-v",
        mount,
        "-w",
        "/src",
        "-e",
        "CARGO_TARGET_DIR=/tmp/target",
        "-e",
        "RUSTC_WRAPPER=",
        "-e",
        "CARGO_TERM_COLOR=never",
        "-v",
        "trit-matrix-registry:/usr/local/cargo/registry",
        "-v",
        f"{volume}:/tmp/target",
    ]
    if cpus:
        base += ["--cpus", str(cpus)]
    adapt = run(
        base
        + [
            image,
            "bash",
            "-lc",
            f"cargo run --locked -q --release -p trit-gpu --example device_adapt -- --seed {seed}",
        ],
        timeout=timeout,
    )
    env_check = run(
        base
        + [
            image,
            "bash",
            "-lc",
            "cargo run --locked -q --release -p trit-gpu --example device_env_check",
        ],
        timeout=timeout,
    )
    return adapt, env_check


def docker_local_run(image, entry_dir, cpuset, seed):
    base = ["docker", "run", "--rm"]
    if cpuset:
        base += ["--cpuset-cpus", cpuset]
    adapt = run(
        base + [image, f"{entry_dir}/device_adapt", "--seed", str(seed)]
    )
    env_check = run(base + [image, f"{entry_dir}/device_env_check"])
    return adapt, env_check


def detect_wsl():
    try:
        result = run(["wsl.exe", "-l", "-q"], timeout=30)
    except (OSError, subprocess.TimeoutExpired):
        return None
    if result.returncode != 0:
        return None
    names = []
    for line in result.stdout.splitlines():
        name = line.replace("\x00", "").strip()
        if name:
            names.append(name)
    return names[0] if names else None


def record(label, kind, adapt, env_check, baseline):
    rows = parse_jsonl(adapt.stdout)
    env_rows = parse_jsonl(env_check.stdout)
    if baseline is None:
        status = "PASS" if adapt.returncode == 0 and rows else "FAIL"
        structural_diffs = []
        metric_diffs = []
    else:
        cut = min(first_train_index(baseline), first_train_index(rows))
        structural_diffs = compare_runs(baseline[:cut], rows[:cut])
        if len(baseline) != len(rows):
            structural_diffs.append(f"line count {len(baseline)} != {len(rows)}")
        metric_diffs = compare_runs(baseline[cut:], rows[cut:])
        if adapt.returncode != 0 or structural_diffs:
            status = "FAIL"
        elif metric_diffs:
            status = "DRIFT"
        else:
            status = "PASS"
    delta, where = (0.0, None)
    if baseline is not None:
        delta, where = max_numeric_delta(baseline, rows)
    return {
        "label": label,
        "kind": kind,
        "status": status,
        "adapt_exit": adapt.returncode,
        "adapt_rows": len(rows),
        "structural_diffs": structural_diffs[:10],
        "metric_diffs": metric_diffs[:10],
        "max_metric_delta": round(delta, 6),
        "max_metric_where": where,
        "env_exit": env_check.returncode,
        "env": summarize_env(env_rows),
        "adapt_stderr_tail": adapt.stderr.splitlines()[-5:],
        "env_stderr_tail": env_check.stderr.splitlines()[-5:],
        "rows": rows,
        "env_rows": env_rows,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--seed", type=int, default=7)
    parser.add_argument("--native-target", default=os.environ.get("CARGO_TARGET_DIR"))
    parser.add_argument("--skip-native", action="store_true")
    parser.add_argument("--skip-wsl", action="store_true")
    parser.add_argument("--skip-docker", action="store_true")
    parser.add_argument("--wsl-distro", default=None)
    parser.add_argument("--docker-image", action="append", default=None)
    parser.add_argument("--docker-cpus", type=float, default=None)
    parser.add_argument("--docker-local-image", default=None,
                        help="无网络的本地镜像（含 device_adapt/device_env_check 二进制）")
    parser.add_argument("--docker-local-dir", default="/app")
    parser.add_argument("--docker-local-cpusets", default="0-1,0-7")
    parser.add_argument("--timeout", type=int, default=1800)
    parser.add_argument("--out", default=None)
    args = parser.parse_args()

    images = args.docker_image or ["rust:1.100-bookworm", "rust:1.88-bookworm"]
    records = []
    baseline = None

    if not args.skip_native:
        adapt, env_check = native_command(args.seed, args.native_target)
        item = record("native", "host", adapt, env_check, None)
        baseline = item["rows"]
        records.append(item)

    if not args.skip_wsl:
        distro = args.wsl_distro or detect_wsl()
        if distro:
            adapt, env_check = wsl_run(distro, args.seed)
            records.append(record(f"wsl:{distro}", "wsl", adapt, env_check, baseline))
        else:
            records.append({"label": "wsl", "kind": "wsl", "skipped": "未检测到 WSL 发行版"})

    if not args.skip_docker:
        if not docker_present():
            records.append({"label": "docker", "kind": "docker", "skipped": "Docker daemon 不可用"})
        else:
            for image in images:
                pull = docker_ensure_image(image, args.timeout)
                if pull is not None and pull.returncode != 0:
                    records.append({
                        "label": image,
                        "kind": "docker",
                        "skipped": f"镜像拉取失败：{pull.stderr.splitlines()[-1] if pull.stderr else 'unknown'}",
                    })
                    continue
                adapt, env_check = docker_run(
                    image, image, args.seed, args.docker_cpus, args.timeout
                )
                records.append(record(image, "docker", adapt, env_check, baseline))

    if args.docker_local_image and docker_present():
        for cpuset in [part.strip() for part in args.docker_local_cpusets.split(",") if part.strip()]:
            adapt, env_check = docker_local_run(
                args.docker_local_image, args.docker_local_dir, cpuset, args.seed
            )
            records.append(
                record(
                    f"local-container:cpuset={cpuset}",
                    "container-local",
                    adapt,
                    env_check,
                    baseline,
                )
            )

    report = {
        "seed": args.seed,
        "root": str(ROOT),
        "host": {
            "platform": platform.platform(),
            "python": platform.python_version(),
        },
        "records": records,
    }
    text = json.dumps(report, ensure_ascii=False, indent=2)
    out = Path(args.out) if args.out else Path(tempfile.mkdtemp(prefix="trit-matrix-")) / "matrix.json"
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(text, encoding="utf-8")

    print("| environment | kind | adapt | status | env | os/arch | cpus | backend | rustc |")
    print("|---|---|---|---|---|---|---|---|---|")
    for item in records:
        if "skipped" in item:
            print(f"| {item['label']} | {item['kind']} | SKIP | - | {item['skipped']} | - | - | - | - |")
            continue
        env = item["env"]
        adapt_state = f"{item['adapt_exit']}/{item['adapt_rows']}"
        print(
            f"| {item['label']} | {item['kind']} | {adapt_state} | {item['status']} | "
            f"{item['env_exit']} | {env.get('os')}/{env.get('arch')} | {env.get('cpus')} | "
            f"{env.get('simd_backend')} | {env.get('rustc')} |"
        )
        if item.get("max_metric_delta", 0) > 0:
            print(f"| -> metric delta | | | {item['max_metric_delta']} | | | | | |")
        for diff in item["structural_diffs"][:5]:
            print(f"| -> structural | | | {diff} | | | | | |")
        for diff in item["metric_diffs"][:3]:
            print(f"| -> drift | | | {diff} | | | | | |")
    print(f"\nreport: {out}")
    failed = any(
        "skipped" not in item and item["status"] == "FAIL" for item in records
    )
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
