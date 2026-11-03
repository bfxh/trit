#!/usr/bin/env python3
"""A4 扫描器：一次跑 N 个深度头配置 → 自动复核 → 按指标晋级 → 自动重建结果目录。

用法:
    python scripts/depth_sweep.py --spec sweeps/depth-basic.json [--jobs 2] [--dry-run]
    python scripts/depth_sweep.py --self-test

规格（JSON）:
{
  "family": "depth",
  "data": ["E:/.../scene0000", ...],              // 与 data_globs 二选一
  "data_globs": ["E:/.../scene*", "E:/.../pt*"],  // 排序后展开
  "holdout": ["scene0050", "scene0051"],
  "out_dir": "runs/depth/sweep-basic",
  "base": {"size": 64, "dil": 2, "accum": 8, "lr": 0.002, "epochs": 3, "stride": 2},
  "grid": {"hidden": [32, 64, 128], "lr": [0.001, 0.002]},
  "jobs": 2,
  "verify": true,
  "promote": {"max": 8.0, "beat_baseline_pp": 0.5},
  "catalogue": "docs/DEPTH-RESULTS.md"
}

行为：对 `grid` 做笛卡尔积（`base` 里同名键被覆盖），**并发**跑（每个配置一个进程：
训练 → 复核），最后按 `promote` 规则评晋级，写 `<out_dir>/sweep.json` 与
`<out_dir>/promoted.json`，并调 `depth_catalogue.py` 重建目录。任一配置复核失败则退出码 1。
"""

import argparse
import concurrent.futures
import glob
import itertools
import json
import pathlib
import queue
import subprocess
import sys
import threading
import time

if hasattr(sys.stdout, "reconfigure"):
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")

PRINT_LOCK = threading.Lock()


def out(msg):
    with PRINT_LOCK:
        print(msg, flush=True)


def build_configs(base, grid):
    keys = sorted(grid)
    cfgs = []
    for combo in itertools.product(*(grid[k] for k in keys)):
        cfg = dict(base)
        cfg.update(dict(zip(keys, combo)))
        cfgs.append(cfg)
    return cfgs


def tag_of(cfg, grid_keys, base):
    parts = []
    for k in grid_keys:
        v = cfg[k]
        parts.append(f"{k}{v:g}" if isinstance(v, float) else f"{k}{v}")
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
    if "lr" in cfg:
        flags += ["--lr", f"{cfg['lr']:g}"]
    for k in ("full", "f32"):
        if cfg.get(k):
            flags.append(f"--{k}")
    return flags


def slot_gpus(jobs, pool):
    """给 `jobs` 个并发槽分配 GPU 下标（从 `pool` 里轮转、确定性）。"""
    pool = list(pool) or [0]
    return [pool[i % len(pool)] for i in range(max(1, jobs))]


def parse_gpu_pool(text):
    """从 `--list-gpus` 输出里挑可用卡：优先独显；没有独显就用非 CPU 的全部。"""
    lines = [ln for ln in text.splitlines() if ln.strip()]
    idx = [i for i, ln in enumerate(lines) if "(DiscreteGpu)" in ln]
    if not idx:
        idx = [i for i, ln in enumerate(lines) if "(Cpu)" not in ln]
    return idx or [0]


def detect_gpu_pool(bin_path):
    """探测卡池；失败按 `[0]`。"""
    try:
        p = subprocess.run(
            [str(bin_path), "--list-gpus"], capture_output=True, text=True, timeout=60
        )
        if p.returncode != 0:
            return [0]
        return parse_gpu_pool(p.stdout)
    except Exception:
        return [0]


def run(cmd, log, timeout):
    t0 = time.time()
    with open(log, "w", encoding="utf-8") as f:
        p = subprocess.run(cmd, stdout=f, stderr=subprocess.STDOUT, timeout=timeout)
    return p.returncode, time.time() - t0


def evaluate_promotion(row, rule):
    """按指标判晋级。返回 `(promoted, reason)`。

    规则键：`max`（`metric` 的上限）、`beat_baseline_pp`（要比常数基线至少低多少个百分点）、
    `metric`（默认 `rel_pct`）。只对**已复核**的行生效。
    """
    if not rule:
        return False, "未配置规则"
    if row.get("status") != "verified":
        return False, "未通过复核"
    m = row.get("metrics") or {}
    metric = rule.get("metric", "rel_pct")
    rel = m.get(metric)
    if rel is None:
        return False, "无指标"
    if "max" in rule and rel > rule["max"]:
        return False, f"{rel:.2f}% > 上限 {rule['max']}%"
    if "beat_baseline_pp" in rule:
        base = m.get("base_pct")
        if base is None:
            return False, "缺常数基线"
        gap = base - rel
        if gap < rule["beat_baseline_pp"]:
            return False, f"只比基线低 {gap:.2f}pp（需 ≥{rule['beat_baseline_pp']}pp）"
    return True, "达标"


def run_one(cfg, tag, gpu, ctx):
    model = ctx["out_dir"] / f"{tag}.dcn"
    tlog = ctx["out_dir"] / f"{tag}.train.log"
    vlog = ctx["out_dir"] / f"{tag}.verify.log"
    train_cmd = [
        str(ctx["train_bin"]), "cnn-train", "--data", ctx["data_arg"],
        "--holdout", ctx["hold"], "--out", str(model), "--gpu-index", str(gpu),
        *cli_map(cfg),
    ]
    out(f"[{tag}] 训练…")
    rc, secs = run(train_cmd, tlog, ctx["timeout"])
    row = {
        "tag": tag, "config": cfg, "model": str(model), "gpu": gpu,
        "train_rc": rc, "train_secs": round(secs, 1),
    }
    if rc != 0:
        row["status"] = "train-failed"
        out(f"[{tag}] 训练失败 rc={rc}（日志 {tlog}）")
        return row
    if ctx["verify"]:
        verify_cmd = [
            str(ctx["verify_bin"]), "--dcn", str(model), "--data", ctx["data_arg"],
            "--holdout", ctx["hold"], "--size", str(cfg.get("size", 64)),
            "--stride", str(cfg.get("stride", 2)), "--pyr", str(cfg.get("pyr", 0)),
        ]
        if cfg.get("full"):
            verify_cmd.append("--full")
        out(f"[{tag}] 复核…")
        vrc, vsecs = run(verify_cmd, vlog, ctx["timeout"])
        row["verify_rc"] = vrc
        row["verify_secs"] = round(vsecs, 1)
        vpath = pathlib.Path(str(model) + ".verify.json")
        if vrc == 0 and vpath.exists():
            v = json.loads(vpath.read_text(encoding="utf-8"))
            row["metrics"] = v.get("metrics")
            row["status"] = "verified"
            m = row["metrics"]
            out(f"[{tag}] PASS {m['rel_pct']:.2f}% / δ {m['delta']:.3f} / 基线 {m['base_pct']:.2f}%")
        else:
            row["status"] = "verify-failed"
            out(f"[{tag}] FAIL 复核 rc={vrc}（日志 {vlog}）")
    else:
        row["status"] = "trained"
    return row


def self_test():
    good = {"status": "verified", "metrics": {"rel_pct": 7.3, "base_pct": 7.91}}
    over = {"status": "verified", "metrics": {"rel_pct": 8.5, "base_pct": 7.91}}
    weak = {"status": "verified", "metrics": {"rel_pct": 7.8, "base_pct": 7.91}}
    unver = {"status": "verify-failed", "metrics": {"rel_pct": 7.0, "base_pct": 7.91}}
    rule = {"max": 8.0, "beat_baseline_pp": 0.5}
    cases = [
        ("达标", evaluate_promotion(good, rule), (True, "达标")),
        ("超上限", evaluate_promotion(over, rule)[0], False),
        ("赢基线不够", evaluate_promotion(weak, rule)[0], False),
        ("未复核", evaluate_promotion(unver, rule)[0], False),
        ("无规则", evaluate_promotion(good, None)[0], False),
        ("缺指标", evaluate_promotion({"status": "verified"}, rule)[0], False),
        ("只给上限", evaluate_promotion(weak, {"max": 8.0})[0], True),
        ("只给基线", evaluate_promotion(good, {"beat_baseline_pp": 0.5})[0], True),
        ("槽分配 4x2", slot_gpus(4, [0, 1]), [0, 1, 0, 1]),
        ("槽分配 3x1", slot_gpus(3, [0]), [0, 0, 0]),
        ("槽分配 2x[2,5]", slot_gpus(2, [2, 5]), [2, 5]),
        (
            "卡池：有独显只取独显",
            parse_gpu_pool("0: A (DiscreteGpu)\n1: B (IntegratedGpu)\n2: W (Cpu)\n"),
            [0],
        ),
        (
            "卡池：没独显用非 CPU",
            parse_gpu_pool("0: A (IntegratedGpu)\n1: W (Cpu)\n"),
            [0],
        ),
        (
            "自定义指标",
            evaluate_promotion(
                {"status": "verified", "metrics": {"mae_log": 0.07}},
                {"metric": "mae_log", "max": 0.08},
            )[0],
            True,
        ),
    ]
    bad = [n for n, got, want in cases if got != want]
    for n, got, want in cases:
        print(f"  {n}: {'ok' if got == want else 'FAIL'}（got={got} want={want}）")
    if bad:
        print(f"FAIL {len(bad)} 条：{bad}")
        sys.exit(1)
    print(f"self-test {len(cases)}/{len(cases)} 通过")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--spec")
    ap.add_argument("--bin-dir", default="target/release")
    ap.add_argument("--jobs", type=int, default=0, help="并发进程数（0 = 用规格里的 jobs，缺省 max(2, 卡数)）")
    ap.add_argument("--gpus", type=int, default=0, help="卡数（0 = 自动探测 --list-gpus）")
    ap.add_argument("--dry-run", action="store_true")
    ap.add_argument("--timeout", type=int, default=3600, help="单个训练/复核的超时（秒）")
    ap.add_argument("--self-test", action="store_true")
    args = ap.parse_args()
    if args.self_test:
        self_test()
        return
    if not args.spec:
        ap.error("需要 --spec（或 --self-test）")

    spec = json.loads(pathlib.Path(args.spec).read_text(encoding="utf-8"))
    data = resolve_data(spec)
    holdout = spec["holdout"]
    base = spec.get("base", {})
    grid = spec.get("grid", {})
    out_dir = pathlib.Path(spec.get("out_dir", "runs/depth/sweep"))
    out_dir.mkdir(parents=True, exist_ok=True)
    bin_dir = pathlib.Path(args.bin_dir)
    pool = detect_gpu_pool(bin_dir / "parts-depth-gpu.exe")
    if args.gpus > 0:
        pool = pool[: args.gpus] or list(range(args.gpus))
    jobs = max(1, args.jobs or int(spec.get("jobs", 0)) or max(2, len(pool)))
    slots = slot_gpus(jobs, pool)

    configs = build_configs(base, grid)
    grid_keys = sorted(grid)
    data_arg = ",".join(data)
    hold = ",".join(holdout)
    out(f"共 {len(configs)} 个配置；数据 {len(data)} 目录；并发 {jobs}；卡池 {pool}（槽 {slots}）；输出 {out_dir}")

    if args.dry_run:
        for i, cfg in enumerate(configs):
            tag = tag_of(cfg, grid_keys, base)
            cmd = " ".join([
                str(bin_dir / "parts-depth-gpu.exe"), "cnn-train", "--data", f"<{len(data)} dirs>",
                "--holdout", hold, "--out", str(out_dir / f"{tag}.dcn"),
                "--gpu-index", str(slots[i % len(slots)]), *cli_map(cfg),
            ])
            print("  " + cmd)
        return

    ctx = {
        "out_dir": out_dir, "data_arg": data_arg, "hold": hold,
        "train_bin": bin_dir / "parts-depth-gpu.exe",
        "verify_bin": bin_dir / "parts-depth-verify.exe",
        "verify": spec.get("verify", True), "timeout": args.timeout,
    }
    for b in (ctx["train_bin"], ctx["verify_bin"]):
        if not b.exists():
            sys.exit(f"找不到 {b}（先 cargo build --release --features gpu --bin parts-depth-gpu）")

    t0 = time.time()
    rows = []
    tokens = queue.Queue()
    for g in slots:
        tokens.put(g)

    def task(cfg, tag):
        g = tokens.get()
        try:
            return run_one(cfg, tag, g, ctx)
        finally:
            tokens.put(g)

    with concurrent.futures.ThreadPoolExecutor(max_workers=jobs) as ex:
        futs = [ex.submit(task, cfg, tag_of(cfg, grid_keys, base)) for cfg in configs]
        for fut in concurrent.futures.as_completed(futs):
            rows.append(fut.result())
    wall = time.time() - t0
    order = {tag_of(c, grid_keys, base): i for i, c in enumerate(configs)}
    rows.sort(key=lambda r: order.get(r["tag"], 0))

    rule = spec.get("promote")
    promoted, rejected = [], []
    for r in rows:
        ok, why = evaluate_promotion(r, rule)
        r["promoted"] = ok
        r["promote_reason"] = why
        (promoted if ok else rejected).append({"tag": r["tag"], "why": why})

    summary = out_dir / "sweep.json"
    summary.write_text(
        json.dumps(
            {"spec": spec, "jobs": jobs, "wall_secs": round(wall, 1), "rows": rows},
            ensure_ascii=False,
            indent=2,
        ),
        encoding="utf-8",
    )
    (out_dir / "promoted.json").write_text(
        json.dumps(
            {"rule": rule, "promoted": [p["tag"] for p in promoted], "rejected": rejected},
            ensure_ascii=False,
            indent=2,
        ),
        encoding="utf-8",
    )
    ok = sum(1 for r in rows if r["status"] in ("verified", "trained"))
    out(f"\n完成 {ok}/{len(rows)}；墙钟 {wall:.0f}s；晋级 {len(promoted)} 条；汇总 {summary}")
    for p in promoted:
        out(f"  晋级 {p['tag']}")

    cat = spec.get("catalogue")
    if cat:
        # 目录覆盖整棵结果树（默认 runs/depth），不是只扫本次 out_dir
        root = spec.get("catalogue_root", "runs/depth")
        subprocess.run(
            [sys.executable, "scripts/depth_catalogue.py", "--dir", root, "--out", cat],
            check=False,
        )
        out(f"目录已重建 {cat}（root={root}）")

    if any(r["status"] in ("train-failed", "verify-failed") for r in rows):
        sys.exit(1)


if __name__ == "__main__":
    main()
