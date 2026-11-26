#!/usr/bin/env python3
"""WSP-BUDGET 变体：在规划器 JSON 上改少量块的位宽，并重算真实字节。

规划器只给一版分配；实验常要问“把某块的 int2 挪到另一块，同字节会不会更好”。
本脚本不重新读权重，也不重新算代理误差，只做可审计的手工消融：

  python scripts/wsp_budget_variant.py \
    --base budget-l12-int3.json \
    --up 0:q,0:k,1:q,1:k,2:q,2:k,3:q,3:k,4:k \
    --down 9:o,11:i,11:f \
    --require-exact --out budget-l12-int3-v6e.json

`up` 固定 int4，`down` 固定 int2，**其余块默认重置为 int3**（`--base-bits 3`）；
这样同一份 base 上做消融不会把规划器原来的 int2/int4 带进来。若确实要保留 base 的
非 3-bit 块，加 `--keep-base`。每个块的 actual_bytes 由 rows/cols/bits/group 重算，
绝不沿用旧文件的缓存值。
"""
from __future__ import annotations

import argparse
import json
import math
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


def parse_blocks(spec: str, flag: str) -> set[tuple[int, str]]:
    out: set[tuple[int, str]] = set()
    for item in spec.split(","):
        item = item.strip()
        if not item:
            continue
        if ":" not in item:
            raise SystemExit(f"{flag}: 需要 layer:kind，收到 {item!r}")
        layer_s, kind_s = item.split(":", 1)
        kind = KIND_ALIASES.get(kind_s.strip().lower())
        if kind is None:
            raise SystemExit(f"{flag}: 未知 kind {kind_s!r}")
        out.add((int(layer_s), kind))
    return out


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--base", required=True)
    parser.add_argument("--up", default="", help="comma list layer:kind, set to int4")
    parser.add_argument("--down", default="", help="comma list layer:kind, set to int2")
    parser.add_argument("--base-bits", type=int, default=3, choices=(2, 3, 4, 5), help="非 up/down 块的位宽")
    parser.add_argument("--keep-base", action="store_true", help="保留 base 里非 up/down 块的 bits")
    parser.add_argument("--out", required=True)
    parser.add_argument("--require-exact", action="store_true")
    args = parser.parse_args()

    src = json.loads(Path(args.base).read_text(encoding="utf-8"))
    group = int(src["group"])
    up = parse_blocks(args.up, "--up")
    down = parse_blocks(args.down, "--down")
    overlap = up & down
    if overlap:
        raise SystemExit(f"同一块不能既 up 又 down: {sorted(overlap)}")

    known = {(int(b["layer"]), b["kind"]) for b in src["blocks"]}
    missing = (up | down) - known
    if missing:
        raise SystemExit(f"base 里没有这些块: {sorted(missing)}")

    blocks = []
    for b in src["blocks"]:
        key = (int(b["layer"]), b["kind"])
        bits = 4 if key in up else 2 if key in down else (int(b["bits"]) if args.keep_base else args.base_bits)
        actual = matrix_cost(int(b["rows"]), int(b["cols"]), bits, group)
        item = dict(b)
        item["bits"] = bits
        item["actual_bytes"] = actual
        blocks.append(item)

    total = sum(b["actual_bytes"] for b in blocks)
    budget = int(src["budget_bytes"])
    out = dict(src)
    out["actual_bytes"] = total
    out["counts"] = {str(bits): sum(1 for b in blocks if b["bits"] == bits) for bits in (2, 3, 4, 5)}
    out["proxy_err_picked"] = math.fsum(b["proxy_err"] for b in blocks)
    out["proxy_err_ratio"] = out["proxy_err_picked"] / out["proxy_err_int3"] if out["proxy_err_int3"] else None
    out["variant"] = {
        "base": Path(args.base).name,
        "up": [f"{l}:{k}" for l, k in sorted(up)],
        "down": [f"{l}:{k}" for l, k in sorted(down)],
    }
    out["blocks"] = blocks

    Path(args.out).parent.mkdir(parents=True, exist_ok=True)
    Path(args.out).write_text(json.dumps(out, ensure_ascii=False, indent=2), encoding="utf-8")
    print(
        json.dumps(
            {
                "out": args.out,
                "budget_bytes": budget,
                "actual_bytes": total,
                "delta": total - budget,
                "counts": out["counts"],
            },
            ensure_ascii=False,
        )
    )
    if args.require_exact and total != budget:
        raise SystemExit(f"不是严格同字节：actual={total} budget={budget}")


if __name__ == "__main__":
    main()
