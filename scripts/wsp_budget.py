#!/usr/bin/env python3
"""WSP-BUDGET：给定总字节，给每张线性矩阵选 int2/int3/int4/int5。

这是**离线预算编译器**，不改变 bert6 运行时。它只做三件事：
  1. 从 safetensors 读每张矩阵的真实形状；
  2. 用组内 max/scale 的量化误差代理（不是 STS）估敏感度；
  3. 用拉格朗日 + 贪心在总字节内选模式，写出可复算的 budget.json。

字节账与 bert6 的 weight_format 不同：这里**含** per-group f32 scale 和 per-row f32 bias，
因为预算分配必须把元数据算进去。权重位宽字节按 ceil(rows*cols*bits/8)，元数据按
rows*ceil(cols/group)*4 + rows*4。

用法：
  python scripts/wsp_budget.py --budget 10782720 --out budget.json
  python scripts/wsp_budget.py --model ... --layers 12 --group 32
"""
from __future__ import annotations

import argparse
import array
import json
import math
import struct
from pathlib import Path

KINDS = (
    "attention.self.query",
    "attention.self.key",
    "attention.self.value",
    "attention.output.dense",
    "intermediate.dense",
    "output.dense",
)
BITS = (2, 3, 4, 5)


def read_header(path: Path):
    with path.open("rb") as f:
        n = struct.unpack("<Q", f.read(8))[0]
        header = json.loads(f.read(n))
    return header


def read_tensor(path: Path, info) -> array.array:
    start, end = info["data_offsets"]
    with path.open("rb") as f:
        n = struct.unpack("<Q", f.read(8))[0]
        f.seek(8 + n + start)
        raw = f.read(end - start)
    values = array.array("f")
    values.frombytes(raw)
    return values


def matrix_cost(rows: int, cols: int, bits: int, group: int) -> int:
    weight_bytes = (rows * cols * bits + 7) // 8
    groups = (cols + group - 1) // group
    metadata = rows * (groups * 4 + 4)
    return weight_bytes + metadata


def proxy_error(values: array.array, rows: int, cols: int, bits: int, group: int) -> float:
    """组内 max-scale 的 MSE 代理：sum_groups (max/qmax)^2 * n/12。

    只用于排序，不宣称等于真实量化误差，更不等于 STS。
    """
    qmax = (1 << (bits - 1)) - 1
    total = 0.0
    for row in range(rows):
        base = row * cols
        for start in range(0, cols, group):
            end = min(start + group, cols)
            block = values[base + start : base + end]
            amax = max(abs(v) for v in block)
            if amax:
                scale = amax / qmax
                total += scale * scale * (end - start) / 12.0
    return total


def allocate(errors: dict[int, list[float]], costs: dict[int, list[int]], budget: int):
    """拉格朗日选模式，再用剩余字节做贪心升级。"""
    n = len(next(iter(errors.values())))
    def pick(lam: float):
        return [min(BITS, key=lambda b: errors[b][i] + lam * costs[b][i]) for i in range(n)]
    def total_cost(modes):
        return sum(costs[b][i] for i, b in enumerate(modes))
    lo, hi = 0.0, 1.0
    while total_cost(pick(hi)) > budget:
        hi *= 2.0
    for _ in range(80):
        mid = (lo + hi) / 2.0
        if total_cost(pick(mid)) > budget:
            lo = mid
        else:
            hi = mid
    modes = pick(hi)
    cost = total_cost(modes)
    while True:
        best = None
        for i, b in enumerate(modes):
            if b >= BITS[-1]:
                continue
            nxt = b + 1
            extra = costs[nxt][i] - costs[b][i]
            if cost + extra > budget:
                continue
            gain = errors[b][i] - errors[nxt][i]
            score = gain / extra
            if best is None or score > best[0]:
                best = (score, i, nxt, extra)
        if best is None:
            break
        _, i, nxt, extra = best
        modes[i] = nxt
        cost += extra
    return modes, cost


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--model", default=r"C:\Users\lbx13\Downloads\all-MiniLM-L12-v2.safetensors")
    parser.add_argument("--layers", type=int, default=12)
    parser.add_argument("--group", type=int, default=32)
    parser.add_argument("--budget", type=int, default=None)
    parser.add_argument("--out", default=r"C:\Users\lbx13\Downloads\trit-results\budget-l12-int3.json")
    args = parser.parse_args()

    model = Path(args.model)
    header = read_header(model)
    blocks = []
    for layer in range(args.layers):
        for kind in KINDS:
            name = f"encoder.layer.{layer}.{kind}.weight"
            info = header[name]
            rows, cols = info["shape"]
            values = read_tensor(model, info)
            errors = {b: proxy_error(values, rows, cols, b, args.group) for b in BITS}
            costs = {b: matrix_cost(rows, cols, b, args.group) for b in BITS}
            blocks.append({
                "name": name,
                "layer": layer,
                "kind": kind,
                "rows": rows,
                "cols": cols,
                "cost": costs,
                "proxy_err": errors,
            })
            print(f"read {name} {rows}x{cols}", flush=True)

    n = len(blocks)
    errors = {b: [block["proxy_err"][b] for block in blocks] for b in BITS}
    costs = {b: [block["cost"][b] for block in blocks] for b in BITS}
    int3_total = sum(costs[3])
    int4_total = sum(costs[4])
    budget = args.budget if args.budget is not None else int3_total
    modes, cost = allocate(errors, costs, budget)
    counts = {b: sum(1 for x in modes if x == b) for b in BITS}
    baseline_err = math.fsum(errors[3])
    picked_err = math.fsum(errors[b][i] for i, b in enumerate(modes))
    out = {
        "schema": 1,
        "model": model.name,
        "layers": args.layers,
        "group": args.group,
        "budget_bytes": budget,
        "int3_bytes": int3_total,
        "int4_bytes": int4_total,
        "actual_bytes": cost,
        "counts": counts,
        "proxy_err_int3": baseline_err,
        "proxy_err_picked": picked_err,
        "proxy_err_ratio": picked_err / baseline_err if baseline_err else None,
        "blocks": [
            {
                "name": block["name"],
                "layer": block["layer"],
                "kind": block["kind"],
                "rows": block["rows"],
                "cols": block["cols"],
                "bits": modes[i],
                "actual_bytes": costs[modes[i]][i],
                "proxy_err": errors[modes[i]][i],
            }
            for i, block in enumerate(blocks)
        ],
    }
    Path(args.out).parent.mkdir(parents=True, exist_ok=True)
    Path(args.out).write_text(json.dumps(out, ensure_ascii=False, indent=2), encoding="utf-8")
    print(json.dumps({k: out[k] for k in ("budget_bytes", "int3_bytes", "int4_bytes", "actual_bytes", "counts", "proxy_err_ratio")}, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
