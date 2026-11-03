#!/usr/bin/env python3
"""A4 扫描器：一次跑 N 个深度头配置 → 自动复核 → 自动重建结果目录。

用法:
    python scripts/depth_sweep.py --spec sweeps/depth-basic.json [--dry-run] [--bin-dir target/release]

规格（JSON）:
{
  "family": "depth",
  "data": ["E:/.../scene0000", ...],            // 与 data_globs 二选一
  "data_globs": ["E:/.../scene*", "E:/.../pt*"],  // 排序后展开
  "holdout": ["scene0050", "scene0051"],
  "out_dir": "runs/depth/sweep-basic",
  "base": {"size": 64, "dil": 2, "accum": 8, "lr": 0.002, "epochs": 3, "stride": 2},
  "grid": {"hidden": [32, 64, 128], "lr": [0.001, 0.002]},
  "verify": true,
  "catalogue": "docs/DEPTH-RESULTS.md"
}

行为：对 `grid` 做笛卡尔积（`base` 里同名键被覆盖），逐个训练（写清单）→ 复核（写
`.verify.json` / `.verify.reject.json`）→ 最后调 `depth_catalogue.py` 重建目录。
汇总写到 `<out_dir>/sweep.json`；任一配置复核失败则退出码 1。
"""

import argparse
import glob
import itertools
import json
import pathlib
import subprocess
import sys

if hasattr(sys.stdout, "reconfigure"):
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
import time


def build_configs(base, grid):
    keys = sorted(grid)
    out = []
    for combo in itertools.product(*(grid[k] for k in keys)):
        cfg = dict(base)
        cfg.update(dict(zip(keys, combo)))
        out.append(cfg)
    return out


def tag_of(cfg, grid_keys, base):
    parts = []
    for k in grid_keys:
        v = cfg[k]
        if isinstance(v, float):
            parts.append(f"{k}{v:g}")
        else:
            parts.append(f"{k}{v}")
    for k in ("hidden", "dil"):
        if k in base and k not in grid_keys:
            parts.append(f"{k}{cfg[k]}")
    return "-".join(parts) or "run"


def resolve_data(spec):
    if "data" in spec:
        return list(spec["data"])
    dirs = []
    for g in spec.get("data_globs", []):
        dirs.extend(sorted(glob.glob(g)))
    if not dirs:
        sys.exit("规格里 data / data_globs 都没有展开出目录")
    return dirs


def cli_map(cfg):
    """把配置字典转成命令行参数（与 parts-depth-gpu 的开关一一对应）。"""
    flags = []
    for k in ("size", "hidden", "dil", "epochs", "stride", "pyr", "accum", "seed"):
        if k in cfg:
            flags += [f"--{k}", str(cfg[k])]
    for k in ("lr",):
        if k in cfg:
            flags += [f"--{k}", f"{cfg[k]:g}"]
    for k in ("full", "f32"):
        if cfg.get(k):
            flags.append(f"--{k}")
    return flags


def run(cmd, log, timeout):
    t0 = time.time()
    with open(log, "w", encoding="utf-8") as f:
        p = subprocess.run(cmd, stdout=f, stderr=subprocess.STDOUT, timeout=timeout)
    return p.returncode, time.time() - t0


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--spec", required=True)
    ap.add_argument("--bin-dir", default="target/release")
    ap.add_argument("--dry-run", action="store_true")
    ap.add_argument("--timeout", type=int, default=3600, help="单个训练/复核的超时（秒）")
    args = ap.parse_args()

    spec = json.loads(pathlib.Path(args.spec).read_text(encoding="utf-8"))
    data = resolve_data(spec)
    holdout = spec["holdout"]
    base = spec.get("base", {})
    grid = spec.get("grid", {})
    out_dir = pathlib.Path(spec.get("out_dir", "runs/depth/sweep"))
    out_dir.mkdir(parents=True, exist_ok=True)
    bin_dir = pathlib.Path(args.bin_dir)
    train_bin = bin_dir / "parts-depth-gpu.exe"
    verify_bin = bin_dir / "parts-depth-verify.exe"
    if not args.dry_run:
        for b in (train_bin, verify_bin):
            if not b.exists():
                sys.exit(f"找不到 {b}（先 cargo build --release --features gpu --bin parts-depth-gpu）")

    configs = build_configs(base, grid)
    grid_keys = sorted(grid)
    data_arg = ",".join(data)
    hold = ",".join(holdout)
    print(f"共 {len(configs)} 个配置；数据 {len(data)} 目录；输出 {out_dir}")

    rows = []
    for i, cfg in enumerate(configs, 1):
        tag = tag_of(cfg, grid_keys, base)
        model = out_dir / f"{tag}.dcn"
        tlog = out_dir / f"{tag}.train.log"
        vlog = out_dir / f"{tag}.verify.log"
        train_cmd = [
            str(train_bin), "cnn-train", "--data", data_arg, "--holdout", hold,
            "--out", str(model), *cli_map(cfg),
        ]
        verify_cmd = [
            str(verify_bin), "--dcn", str(model), "--data", data_arg,
            "--holdout", hold, "--size", str(cfg.get("size", 64)),
            "--stride", str(cfg.get("stride", 2)), "--pyr", str(cfg.get("pyr", 0)),
        ]
        if cfg.get("full"):
            verify_cmd.append("--full")
        if args.dry_run:
            short = " ".join(train_cmd).replace(data_arg, f"<{len(data)} dirs>")
            print("  " + short)
            continue

        print(f"[{i}/{len(configs)}] {tag} 训练…", flush=True)
        rc, secs = run(train_cmd, tlog, args.timeout)
        row = {"tag": tag, "config": cfg, "model": str(model), "train_rc": rc, "train_secs": round(secs, 1)}
        if rc != 0:
            row["status"] = "train-failed"
            rows.append(row)
            print(f"  训练失败 rc={rc}（日志 {tlog}）")
            continue
        if spec.get("verify", True):
            print(f"  复核…", flush=True)
            vrc, vsecs = run(verify_cmd, vlog, args.timeout)
            row["verify_rc"] = vrc
            row["verify_secs"] = round(vsecs, 1)
            vpath = pathlib.Path(str(model) + ".verify.json")
            if vrc == 0 and vpath.exists():
                v = json.loads(vpath.read_text(encoding="utf-8"))
                row["metrics"] = v.get("metrics")
                row["status"] = "verified"
                m = row["metrics"]
                print(f"  PASS {m['rel_pct']:.2f}% / δ {m['delta']:.3f} / 基线 {m['base_pct']:.2f}%")
            else:
                row["status"] = "verify-failed"
                print(f"  FAIL 复核 rc={vrc}（日志 {vlog}）")
        else:
            row["status"] = "trained"
        rows.append(row)

    if args.dry_run:
        return

    summary = out_dir / "sweep.json"
    summary.write_text(json.dumps({"spec": spec, "rows": rows}, ensure_ascii=False, indent=2), encoding="utf-8")
    ok = sum(1 for r in rows if r["status"] in ("verified", "trained"))
    print(f"\n完成 {ok}/{len(rows)}；汇总 {summary}")

    cat = spec.get("catalogue")
    if cat:
        subprocess.run([sys.executable, "scripts/depth_catalogue.py",
                        "--dir", str(out_dir), "--out", cat], check=False)
        print(f"目录已重建 {cat}")

    if any(r["status"] in ("train-failed", "verify-failed") for r in rows):
        sys.exit(1)


if __name__ == "__main__":
    main()
