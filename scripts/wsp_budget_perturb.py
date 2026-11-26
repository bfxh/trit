#!/usr/bin/env python3
"""WSP-BUDGET 逐块位宽扰动：把单个矩阵从 base 位宽改成目标位宽，跑全量。

这是对 `wsp_budget_swap.py` 的补充：交换只允许同形状、同字节；本脚本允许
单块改变位宽，用来测“把某一层某一类矩阵降/升一档，端到端到底掉多少”。
默认场景是从纯 int3 生成 72 个 3→2 变体，全量跑 STS，得到逐块降级敏感度表。

只生成预算 JSON：

  python scripts/wsp_budget_perturb.py \
    --base budget-l12-pure-int3.json --bits 2 --out-dir perturb-3to2

生成并跑全量：

  python scripts/wsp_budget_perturb.py \
    --base budget-l12-pure-int3.json --bits 2 --out-dir perturb-3to2 \
    --run --exe target/release/examples/bert6.exe \
    --model all-MiniLM-L12-v2.safetensors --data-dir miniLM-l12-sts-full \
    --f32-cache l12-f32-cache-full-preds.json --threads 6 --jobs 4 \
    --base-result l12-int3-preds-full.json

结果按 `delta_spearman` 排序；每行保留字节变化，避免把“少花字节”误读成
“同字节更优”。
"""
from __future__ import annotations

import argparse
import json
import os
import subprocess
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
    "attention.self.query": "attention.self.query",
    "attention.self.key": "attention.self.key",
    "attention.self.value": "attention.self.value",
    "attention.output.dense": "attention.output.dense",
    "intermediate.dense": "intermediate.dense",
    "output.dense": "output.dense",
}

SHORT_KIND = {
    "attention.self.query": "q",
    "attention.self.key": "k",
    "attention.self.value": "v",
    "attention.output.dense": "o",
    "intermediate.dense": "i",
    "output.dense": "f",
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


def build_variant(
    base: dict,
    target: tuple[int, str],
    bits_to: int,
    base_path: str,
) -> dict:
    group = int(base["group"])
    by_key = {(int(b["layer"]), b["kind"]): b for b in base["blocks"]}
    if target not in by_key:
        raise SystemExit(f"base 里没有块: {target}")
    target_block = by_key[target]
    bits_from = int(target_block["bits"])
    if bits_from == bits_to:
        raise SystemExit(f"{target} 已经是 {bits_to}-bit，不需要扰动")
    blocks = []
    for block in base["blocks"]:
        item = dict(block)
        if (int(item["layer"]), item["kind"]) == target:
            item["bits"] = bits_to
        item["actual_bytes"] = matrix_cost(
            int(item["rows"]), int(item["cols"]), int(item["bits"]), group
        )
        blocks.append(item)
    total = sum(int(b["actual_bytes"]) for b in blocks)
    out = dict(base)
    out["actual_bytes"] = total
    out["counts"] = {
        str(bits): sum(1 for b in blocks if int(b["bits"]) == bits)
        for bits in (2, 3, 4, 5)
    }
    out["proxy_err_picked"] = sum(float(b["proxy_err"]) for b in blocks)
    out["proxy_err_ratio"] = (
        out["proxy_err_picked"] / float(out["proxy_err_int3"])
        if out.get("proxy_err_int3")
        else None
    )
    out["variant"] = {
        "base": Path(base_path).name,
        "perturbation": f"{target[0]}:{target[1]} {bits_from}->{bits_to}",
    }
    out.pop("_base_path", None)
    out["blocks"] = blocks
    return out


def write_variant(out: dict, path: Path) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(out, ensure_ascii=False, indent=2), encoding="utf-8")


def label_for(target: tuple[int, str], bits_to: int) -> str:
    return f"p-{target[0]}{SHORT_KIND[target[1]]}-to{bits_to}"


def run_one(spec: dict, args: argparse.Namespace) -> dict:
    result_path = Path(spec["result"])
    if result_path.exists() and args.resume:
        return {**spec, "status": "cached", "result": str(result_path)}
    log_path = result_path.with_suffix(".log")
    env = os.environ.copy()
    env.update(
        {
            "TRIT_LAYERS": str(args.layers),
            "TRIT_WEIGHT_BITS": "3",
            "TRIT_BUDGET": spec["budget"],
            "TRIT_THREADS": str(args.threads),
            "TRIT_SKIP_DIAG": "1",
            "TRIT_NO_LAYERS": "1",
        }
    )
    if args.f32_cache:
        env["TRIT_F32_CACHE"] = args.f32_cache
    if args.dump_preds:
        env["TRIT_DUMP_PREDS"] = "1"
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
    weight = data.get("weight_format") or {}
    return {
        "pearson": float(sts["pearson"]),
        "spearman": float(sts["spearman"]),
        "actual_bytes": int(
            weight.get("budget_actual_bytes")
            or weight.get("weight_bytes")
            or data.get("actual_bytes")
        ),
    }


def select_targets(base: dict, targets_spec: str, kinds_spec: str, bits_to: int) -> list[tuple[int, str]]:
    all_targets = [
        (int(block["layer"]), block["kind"])
        for block in base["blocks"]
        if int(block["bits"]) != bits_to
    ]
    if targets_spec:
        wanted = parse_blocks(targets_spec, "--targets")
        all_targets = [item for item in all_targets if item in wanted]
    if kinds_spec:
        kinds = {KIND_ALIASES[item.strip().lower()] for item in kinds_spec.split(",") if item.strip()}
        all_targets = [item for item in all_targets if item[1] in kinds]
    return sorted(all_targets, key=lambda item: (item[0], item[1]))


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--base", required=True)
    parser.add_argument("--bits", type=int, required=True, choices=(2, 3, 4, 5))
    parser.add_argument("--targets", default="", help="可选 layer:kind 列表；默认全部非目标位宽的块")
    parser.add_argument("--kinds", default="", help="可选 q/k/v/o/i/f 过滤")
    parser.add_argument("--out-dir", required=True)
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
    parser.add_argument("--dump-preds", action="store_true")
    args = parser.parse_args()

    base = json.loads(Path(args.base).read_text(encoding="utf-8"))
    out_dir = Path(args.out_dir)
    out_dir.mkdir(parents=True, exist_ok=True)

    targets = select_targets(base, args.targets, args.kinds, args.bits)
    if not targets:
        raise SystemExit("没有可扰动的块")

    specs = []
    for target in targets:
        bits_from = next(
            int(block["bits"])
            for block in base["blocks"]
            if (int(block["layer"]), block["kind"]) == target
        )
        label = label_for(target, args.bits)
        budget_path = out_dir / f"{label}.json"
        result_path = out_dir / f"{label}-result.json"
        variant = build_variant(base, target, args.bits, args.base)
        write_variant(variant, budget_path)
        specs.append(
            {
                "label": label,
                "layer": target[0],
                "kind": target[1],
                "bits_from": bits_from,
                "bits_to": args.bits,
                "budget": str(budget_path),
                "result": str(result_path),
            }
        )

    print(f"生成 {len(specs)} 个单块扰动到 {out_dir}")
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
    results.sort(key=lambda item: (item["layer"], item["kind"]))

    base_metrics = None
    if args.base_result:
        base_metrics = read_metrics(Path(args.base_result))
        # 旧结果可能没有 mixed 字节账；base 的实际字节以预算图为准。
        base_metrics["actual_bytes"] = int(base["actual_bytes"])

    summary = []
    for item in results:
        row = dict(item)
        if item["status"] in {"ok", "cached"} and Path(item["result"]).exists():
            row.update(read_metrics(Path(item["result"])))
            if base_metrics:
                row["delta_pearson"] = row["pearson"] - base_metrics["pearson"]
                row["delta_spearman"] = row["spearman"] - base_metrics["spearman"]
                row["delta_bytes"] = row["actual_bytes"] - base_metrics["actual_bytes"]
            summary.append(row)
        else:
            summary.append(row)

    summary_path = Path(args.summary) if args.summary else out_dir / "perturb-summary.json"
    summary_path.write_text(
        json.dumps(
            {
                "base": args.base,
                "base_result": args.base_result,
                "base_metrics": base_metrics,
                "bits_to": args.bits,
                "runs": summary,
            },
            ensure_ascii=False,
            indent=2,
        ),
        encoding="utf-8",
    )
    print(f"写入 {summary_path}")

    if base_metrics:
        ok = [row for row in summary if "delta_spearman" in row]
        print(f"{'variant':<16} {'Pearson':>10} {'ΔP':>10} {'Spearman':>10} {'ΔS':>10} {'ΔB':>10}")
        for row in sorted(ok, key=lambda item: item["delta_spearman"], reverse=True):
            print(
                f"{row['label']:<16} {row['pearson']:>10.9f} {row['delta_pearson']:>+10.9f} "
                f"{row['spearman']:>10.9f} {row['delta_spearman']:>+10.9f} {row['delta_bytes']:>+10d}"
            )


if __name__ == "__main__":
    main()
