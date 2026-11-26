#!/usr/bin/env python3
"""WSP-BUDGET 同字节块交换：把一个块的位宽挪到另一个同形状块。

这是对 `wsp_budget_variant.py` 的补充：变体脚本只能把块设成固定的 2/3/4，
本脚本保留源块的位宽，把它搬到目标块，源块回 3-bit。只要两块 rows/cols 相同，
总字节严格不变；用于回答“同一个 int2 放在哪一层更好”“同一个 int4 升级放哪层更好”。

扫描一个源块到多个目标：

  python scripts/wsp_budget_swap.py \
    --base budget-l12-int3-v6e.json --source 9:o \
    --targets 0:o,1:o,2:o,3:o,4:o,5:o,6:o,7:o,8:o,10:o,11:o \
    --out-dir swap-o --run --exe target/release/examples/bert6.exe \
    --model all-MiniLM-L12-v2.safetensors --data-dir miniLM-l12-sts-128 \
    --f32-cache l12-f32-cache-128.json --threads 6 --jobs 4

组合多个交换（每个 `--move` 一次）：

  python scripts/wsp_budget_swap.py --base ... \
    --move 9:o>0:o --move 11:i>0:i --move 11:f>0:f ...

只生成预算 JSON 时不要加 `--run`。结果按 pair 指标汇总，不做 bootstrap。
"""
from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
from concurrent.futures import ThreadPoolExecutor, as_completed
from pathlib import Path

KIND_ALIASES = {
    "q": "attention.self.query",
    "k": "attention.self.key",
    "v": "attention.self.value",
    "o": "attention.output.dense",
    "i": "intermediate.dense",
    "f": "output.dense",
    "query": "attention.self.query",
    "key": "attention.self.key",
    "value": "attention.self.value",
    "attention.output.dense": "attention.output.dense",
    "intermediate.dense": "intermediate.dense",
    "output.dense": "output.dense",
}


def matrix_cost(rows: int, cols: int, bits: int, group: int) -> int:
    weight_bytes = (rows * cols * bits + 7) // 8
    groups = (cols + group - 1) // group
    metadata = rows * (groups * 4 + 4)
    return weight_bytes + metadata


def parse_block(item: str, flag: str) -> tuple[int, str]:
    item = item.strip()
    if ":" not in item:
        raise SystemExit(f"{flag}: 需要 layer:kind，收到 {item!r}")
    layer_s, kind_s = item.split(":", 1)
    kind = KIND_ALIASES.get(kind_s.strip().lower())
    if kind is None:
        raise SystemExit(f"{flag}: 未知 kind {kind_s!r}")
    return int(layer_s), kind


def parse_blocks(spec: str, flag: str) -> list[tuple[int, str]]:
    return [parse_block(item, flag) for item in spec.split(",") if item.strip()]


def parse_move(spec: str) -> tuple[tuple[int, str], tuple[int, str]]:
    if ">" not in spec:
        raise SystemExit(f"--move 需要 src>dst，收到 {spec!r}")
    src_s, dst_s = spec.split(">", 1)
    return parse_block(src_s, "--move"), parse_block(dst_s, "--move")


SHORT_KIND = {
    "attention.self.query": "q",
    "attention.self.key": "k",
    "attention.self.value": "v",
    "attention.output.dense": "o",
    "intermediate.dense": "i",
    "output.dense": "f",
}


def variant_name(moves: list[tuple[tuple[int, str], tuple[int, str]]]) -> str:
    parts = [f"{src[0]}{SHORT_KIND[src[1]]}to{dst[0]}{SHORT_KIND[dst[1]]}" for src, dst in moves]
    return "swap-" + "-".join(parts)


def build_variant(base: dict, moves: list[tuple[tuple[int, str], tuple[int, str]]], require_exact: bool) -> dict:
    group = int(base["group"])
    base_name = Path(str(base.get("_base_path", ""))).name
    by_key = {(int(b["layer"]), b["kind"]): b for b in base["blocks"]}
    blocks = []
    seen_src: set[tuple[int, str]] = set()
    for src, dst in moves:
        if src in seen_src:
            raise SystemExit(f"源块重复: {src}")
        seen_src.add(src)
        if src not in by_key or dst not in by_key:
            raise SystemExit(f"base 里没有块: {src} / {dst}")
        src_b = by_key[src]
        dst_b = by_key[dst]
        if src_b["bits"] == 3:
            raise SystemExit(f"源块 {src} 已是 int3，没有可搬的位宽")
        if int(dst_b["bits"]) != 3:
            raise SystemExit(f"目标块 {dst} 不是 int3，交换会重叠: bits={dst_b['bits']}")
        if (int(src_b["rows"]), int(src_b["cols"])) != (int(dst_b["rows"]), int(dst_b["cols"])):
            raise SystemExit(f"形状不同，无法同字节交换: {src} {(src_b['rows'], src_b['cols'])} vs {dst} {(dst_b['rows'], dst_b['cols'])}")

    moved_bits = {dst: int(by_key[src]["bits"]) for src, dst in moves}
    reset_bits = {src for src, _ in moves}
    for b in base["blocks"]:
        key = (int(b["layer"]), b["kind"])
        bits = moved_bits.get(key, 3 if key in reset_bits else int(b["bits"]))
        item = dict(b)
        item["bits"] = bits
        item["actual_bytes"] = matrix_cost(int(item["rows"]), int(item["cols"]), bits, group)
        blocks.append(item)

    total = sum(int(b["actual_bytes"]) for b in blocks)
    out = dict(base)
    out["actual_bytes"] = total
    out["counts"] = {str(bits): sum(1 for b in blocks if b["bits"] == bits) for bits in (2, 3, 4, 5)}
    out["proxy_err_picked"] = sum(float(b["proxy_err"]) for b in blocks)
    out["proxy_err_ratio"] = out["proxy_err_picked"] / out["proxy_err_int3"] if out["proxy_err_int3"] else None
    out["variant"] = {
        "base": base_name,
        "moves": [f"{src[0]}:{src[1]}>{dst[0]}:{dst[1]}" for src, dst in moves],
    }
    out.pop("_base_path", None)
    out["blocks"] = blocks
    if require_exact and total != int(base["budget_bytes"]):
        raise SystemExit(f"不是严格同字节：actual={total} budget={base['budget_bytes']}")
    return out


def write_variant(out: dict, path: Path) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(out, ensure_ascii=False, indent=2), encoding="utf-8")


def run_one(spec: dict, args: argparse.Namespace) -> dict:
    budget_path = Path(spec["budget"])
    result_path = Path(spec["result"])
    if result_path.exists() and args.resume:
        return {**spec, "status": "cached", "result": str(result_path)}
    log_path = result_path.with_suffix(".log")
    env = os.environ.copy()
    env.update(
        {
            "TRIT_LAYERS": str(args.layers),
            "TRIT_WEIGHT_BITS": "3",
            "TRIT_BUDGET": str(budget_path),
            "TRIT_THREADS": str(args.threads),
            "TRIT_SKIP_DIAG": "1",
            "TRIT_NO_LAYERS": "1",
        }
    )
    if args.f32_cache:
        env["TRIT_F32_CACHE"] = args.f32_cache
    cmd = [
        args.exe,
        "--file",
        args.model,
        "--data-dir",
        args.data_dir,
        "--activation-mode",
        "f32",
        "--out",
        str(result_path),
    ]
    with log_path.open("w", encoding="utf-8") as log:
        proc = subprocess.run(cmd, env=env, stdout=log, stderr=subprocess.STDOUT, text=True)
    if proc.returncode != 0:
        return {**spec, "status": f"failed:{proc.returncode}", "log": str(log_path)}
    return {**spec, "status": "ok", "result": str(result_path), "log": str(log_path)}


def read_metrics(path: Path) -> dict:
    data = json.loads(path.read_text(encoding="utf-8"))
    sts = data["modes"]["f32"]["sts"]
    budget = data.get("budget") or {}
    return {
        "pearson": float(sts["pearson"]),
        "spearman": float(sts["spearman"]),
        "actual_bytes": budget.get("actual_bytes", data.get("actual_bytes")),
    }


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--base", required=True)
    parser.add_argument("--source", default="", help="扫描源块 layer:kind")
    parser.add_argument("--targets", default="", help="comma list layer:kind")
    parser.add_argument("--move", action="append", default=[], help="src>dst；可重复，组合成一个变体")
    parser.add_argument("--out-dir", required=True)
    parser.add_argument("--require-exact", action="store_true", default=True)
    parser.add_argument("--allow-bytes-change", action="store_false", dest="require_exact")
    parser.add_argument("--run", action="store_true")
    parser.add_argument("--resume", action="store_true", default=True)
    parser.add_argument("--exe", default="")
    parser.add_argument("--model", default="")
    parser.add_argument("--data-dir", default="")
    parser.add_argument("--f32-cache", default="")
    parser.add_argument("--threads", type=int, default=6)
    parser.add_argument("--jobs", type=int, default=4)
    parser.add_argument("--layers", type=int, default=12)
    parser.add_argument("--base-result", default="")
    parser.add_argument("--summary", default="")
    args = parser.parse_args()

    base = json.loads(Path(args.base).read_text(encoding="utf-8"))
    base["_base_path"] = args.base
    out_dir = Path(args.out_dir)
    out_dir.mkdir(parents=True, exist_ok=True)

    if args.move:
        if args.source or args.targets:
            raise SystemExit("--move 与 --source/--targets 不能同时用")
        moves_list = [[parse_move(spec) for spec in args.move]]
        labels = [variant_name(moves_list[0])]
    else:
        if not args.source or not args.targets:
            raise SystemExit("需要 --source/--targets 或 --move")
        source = parse_block(args.source, "--source")
        targets = parse_blocks(args.targets, "--targets")
        moves_list = [[(source, dst)] for dst in targets]
        labels = [variant_name(m) for m in moves_list]

    specs = []
    for moves, label in zip(moves_list, labels):
        budget_path = out_dir / f"{label}.json"
        result_path = out_dir / f"{label}-result.json"
        variant = build_variant(base, moves, args.require_exact)
        write_variant(variant, budget_path)
        specs.append({"label": label, "moves": [f"{s[0]}:{s[1]}>{d[0]}:{d[1]}" for s, d in moves], "budget": str(budget_path), "result": str(result_path)})

    print(f"生成 {len(specs)} 个变体到 {out_dir}")
    for spec in specs:
        print(f"  {spec['label']}: {spec['budget']}")

    if not args.run:
        return
    if not (args.exe and args.model and args.data_dir):
        raise SystemExit("--run 需要 --exe --model --data-dir")

    results = []
    with ThreadPoolExecutor(max_workers=max(1, args.jobs)) as pool:
        futures = [pool.submit(run_one, spec, args) for spec in specs]
        for future in as_completed(futures):
            results.append(future.result())
    results.sort(key=lambda item: item["label"])

    base_metrics = None
    if args.base_result:
        base_metrics = read_metrics(Path(args.base_result))

    summary = []
    for item in results:
        row = dict(item)
        if item["status"] in {"ok", "cached"} and Path(item["result"]).exists():
            row.update(read_metrics(Path(item["result"])))
            if base_metrics:
                row["delta_pearson"] = row["pearson"] - base_metrics["pearson"]
                row["delta_spearman"] = row["spearman"] - base_metrics["spearman"]
            summary.append(row)
        else:
            summary.append(row)

    summary_path = Path(args.summary) if args.summary else out_dir / "swap-summary.json"
    summary_path.write_text(
        json.dumps({"base": args.base, "base_result": args.base_result, "base_metrics": base_metrics, "runs": summary}, ensure_ascii=False, indent=2),
        encoding="utf-8",
    )
    print(f"写入 {summary_path}")

    if base_metrics:
        print(f"{'variant':<28} {'Pearson':>10} {'ΔP':>10} {'Spearman':>10} {'ΔS':>10}")
        for row in summary:
            if "pearson" in row:
                print(
                    f"{row['label']:<28} {row['pearson']:>10.9f} {row.get('delta_pearson', 0):>+10.9f} "
                    f"{row['spearman']:>10.9f} {row.get('delta_spearman', 0):>+10.9f}"
                )


if __name__ == "__main__":
    main()
