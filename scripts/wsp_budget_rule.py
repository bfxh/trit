#!/usr/bin/env python3
"""WSP-BUDGET 跨模型相对深度规则。

`wsp_budget_rescan.py` 用目标模型自己的 36 块表选块；这个脚本反过来：只从
**另一个模型**的 72 块表学一个按 `(相对深度, 块类型)` 的先验，完全不读目标
模型的块表，再给目标模型生成严格同字节候选。

  python scripts/wsp_budget_rule.py \
    --base budget-l6-pure-int3.json \
    --table docs/research/data/wsp-budget-l12-perturb.tsv \
    --target-layers 6 --per-shape 2 \
    --out budget-l6-rule.json --report budget-l6-rule-report.json

先验：对每个 kind，把源模型每层的 ΔSpearman 看成相对深度 `layer/(L-1)` 的
折线；目标块用同样的相对深度线性插值。降级取插值 ΔS 最大（伤害最小），
升级取插值 ΔS 最大（收益最大）。选择仍按 `(rows, cols, group)` 配平，保证
总字节严格相等。

`--table` 同时接受两种表头：本仓库 L12 的宽表
（`delta_pearson_3to2/...`），以及 L6 的长表
（`direction/layer/kind/delta_pearson/delta_spearman/delta_bytes`，每块两行）。
长表在读取时按 `(layer, kind)` 折叠成宽表，因此两个方向都能用仓库里已有的
块表复算，不需要另存一份。

`--self-test` 只校验插值和选择结构，不读模型。
"""
from __future__ import annotations

import argparse
import csv
import json
import math
import sys
from pathlib import Path

SCRIPT_DIR = Path(__file__).resolve().parent
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

from wsp_budget_rescan import matrix_cost, run_candidate, select_blocks  # noqa: E402

KIND_SHORT = {
    "attention.self.query": "q",
    "attention.self.key": "k",
    "attention.self.value": "v",
    "attention.output.dense": "o",
    "intermediate.dense": "i",
    "output.dense": "f",
}
SHORT_KIND = {v: k for k, v in KIND_SHORT.items()}


def load_json(path: str | Path) -> dict:
    return json.loads(Path(path).read_text(encoding="utf-8"))


def write_json(path: str | Path, obj: dict) -> None:
    Path(path).parent.mkdir(parents=True, exist_ok=True)
    Path(path).write_text(json.dumps(obj, ensure_ascii=False, indent=2), encoding="utf-8")


def load_table(path: str | Path) -> list[dict]:
    """读宽表或长表；长表按 (layer, kind) 折叠成宽表。"""
    with Path(path).open(encoding="utf-8", newline="") as handle:
        reader = csv.DictReader(handle, delimiter="\t")
        fields = reader.fieldnames or []
        raw = list(reader)

    rows = []
    if "direction" in fields:
        grouped: dict[tuple[int, str], dict] = {}
        for row in raw:
            direction = row["direction"]
            if direction not in ("3->2", "3->4"):
                raise SystemExit(f"{path}: 未知 direction {direction}")
            layer = int(row["layer"])
            kind = SHORT_KIND.get(row["kind"], row["kind"])
            rec = grouped.setdefault((layer, kind), {"layer": layer, "kind": kind})
            suffix = "3to2" if direction == "3->2" else "3to4"
            rec[f"delta_pearson_{suffix}"] = float(row["delta_pearson"])
            rec[f"delta_spearman_{suffix}"] = float(row["delta_spearman"])
        rows = list(grouped.values())
        required = [
            "delta_pearson_3to2", "delta_spearman_3to2",
            "delta_pearson_3to4", "delta_spearman_3to4",
        ]
        for row in rows:
            missing = [key for key in required if key not in row]
            if missing:
                raise SystemExit(f"{path}: {row['layer']}/{row['kind']} 缺少 {missing}")
    else:
        for row in raw:
            rows.append(
                {
                    "layer": int(row["layer"]),
                    "kind": SHORT_KIND.get(row["kind"], row["kind"]),
                    "delta_pearson_3to2": float(row["delta_pearson_3to2"]),
                    "delta_spearman_3to2": float(row["delta_spearman_3to2"]),
                    "delta_pearson_3to4": float(row["delta_pearson_3to4"]),
                    "delta_spearman_3to4": float(row["delta_spearman_3to4"]),
                }
            )
    if not rows:
        raise SystemExit(f"{path} 没有数据行")
    return rows


def interpolate(points: list[tuple[float, float]], x: float) -> float:
    points = sorted(points)
    if not points:
        raise SystemExit("插值点为空")
    if x <= points[0][0]:
        return points[0][1]
    if x >= points[-1][0]:
        return points[-1][1]
    for (x0, y0), (x1, y1) in zip(points, points[1:]):
        if x0 <= x <= x1:
            t = (x - x0) / (x1 - x0)
            return y0 + t * (y1 - y0)
    raise SystemExit("插值区间断裂")


def prior_rows(table: list[dict], target_blocks: list[dict], target_layers: int) -> tuple[dict, dict]:
    if target_layers < 2:
        raise SystemExit("--target-layers 必须 >= 2")
    by_kind: dict[str, list[tuple[float, float, float]]] = {}
    source_layers = max(row["layer"] for row in table) + 1
    if source_layers < 2:
        raise SystemExit("源表至少要有 2 层")
    for row in table:
        d = row["layer"] / (source_layers - 1)
        by_kind.setdefault(row["kind"], []).append(
            (d, row["delta_spearman_3to2"], row["delta_spearman_3to4"])
        )
    down_rows = {}
    up_rows = {}
    for block in target_blocks:
        layer = int(block["layer"])
        kind = block["kind"]
        if kind not in by_kind:
            raise SystemExit(f"源表没有 kind {kind}")
        d = layer / (target_layers - 1)
        ds2 = interpolate([(x, y2) for x, y2, _ in by_kind[kind]], d)
        ds4 = interpolate([(x, y4) for x, _, y4 in by_kind[kind]], d)
        down_rows[(layer, kind)] = {
            "layer": layer,
            "kind": kind,
            "delta_pearson": ds2,
            "delta_spearman": ds2,
        }
        up_rows[(layer, kind)] = {
            "layer": layer,
            "kind": kind,
            "delta_pearson": ds4,
            "delta_spearman": ds4,
        }
    return down_rows, up_rows


def self_test() -> None:
    assert interpolate([(0.0, 0.0), (1.0, 10.0)], 0.5) == 5.0
    assert interpolate([(0.0, 0.0), (1.0, 10.0)], -1.0) == 0.0
    assert interpolate([(0.0, 0.0), (1.0, 10.0)], 2.0) == 10.0

    table = [
        {"layer": 0, "kind": "attention.self.query", "delta_pearson_3to2": 0.0, "delta_spearman_3to2": 0.0, "delta_pearson_3to4": 0.0, "delta_spearman_3to4": 0.0},
        {"layer": 1, "kind": "attention.self.query", "delta_pearson_3to2": -1.0, "delta_spearman_3to2": -1.0, "delta_pearson_3to4": 1.0, "delta_spearman_3to4": 1.0},
        {"layer": 0, "kind": "attention.self.key", "delta_pearson_3to2": 0.5, "delta_spearman_3to2": 0.5, "delta_pearson_3to4": 0.5, "delta_spearman_3to4": 0.5},
        {"layer": 1, "kind": "attention.self.key", "delta_pearson_3to2": 0.5, "delta_spearman_3to2": 0.5, "delta_pearson_3to4": 0.5, "delta_spearman_3to4": 0.5},
    ]
    blocks = [
        {"layer": 0, "kind": "attention.self.query", "rows": 4, "cols": 4, "bits": 3, "proxy_err": 1.0},
        {"layer": 1, "kind": "attention.self.query", "rows": 4, "cols": 4, "bits": 3, "proxy_err": 1.0},
        {"layer": 0, "kind": "attention.self.key", "rows": 4, "cols": 4, "bits": 3, "proxy_err": 1.0},
        {"layer": 1, "kind": "attention.self.key", "rows": 4, "cols": 4, "bits": 3, "proxy_err": 1.0},
    ]
    for block in blocks:
        block["actual_bytes"] = matrix_cost(4, 4, 3, 32)
    base = {
        "group": 32,
        "actual_bytes": sum(int(b["actual_bytes"]) for b in blocks),
        "proxy_err_int3": len(blocks),
        "blocks": blocks,
    }
    down, up = prior_rows(table, blocks, target_layers=2)
    assert down[(1, "attention.self.query")]["delta_spearman"] == -1.0
    assert up[(1, "attention.self.query")]["delta_spearman"] == 1.0
    result = select_blocks(base, down, up, per_shape=1)
    assert result["budget"]["actual_bytes"] == base["actual_bytes"]
    print("self-test: ok")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--base", default="")
    parser.add_argument("--table", default="")
    parser.add_argument("--target-layers", type=int, default=12)
    parser.add_argument("--per-shape", type=int, default=2)
    parser.add_argument("--out", default="")
    parser.add_argument("--report", default="")
    parser.add_argument("--run", action="store_true")
    parser.add_argument("--exe", default="")
    parser.add_argument("--model", default="")
    parser.add_argument("--data-dir", default="")
    parser.add_argument("--result", default="")
    parser.add_argument("--f32-cache", default="")
    parser.add_argument("--threads", type=int, default=24)
    parser.add_argument("--layers", type=int, default=12)
    parser.add_argument("--dump-preds", action="store_true")
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()

    if args.self_test:
        self_test()
        return

    for flag, value in (
        ("--base", args.base),
        ("--table", args.table),
        ("--out", args.out),
    ):
        if not value:
            raise SystemExit(f"缺少 {flag}")

    base = load_json(args.base)
    base["_base_path"] = args.base
    table = load_table(args.table)
    blocks = list(base["blocks"])
    down_rows, up_rows = prior_rows(table, blocks, args.target_layers)
    result = select_blocks(base, down_rows, up_rows, args.per_shape)
    budget = result["budget"]
    budget["variant"]["selector"] = "wsp_budget_rule.py"
    budget["variant"]["prior_table"] = Path(args.table).name
    budget["variant"]["target_layers"] = args.target_layers

    out_path = Path(args.out)
    write_json(out_path, budget)
    report_path = Path(args.report) if args.report else out_path.with_suffix(".report.json")
    write_json(
        report_path,
        {
            "base": args.base,
            "table": args.table,
            "target_layers": args.target_layers,
            "per_shape": args.per_shape,
            "actual_bytes": budget["actual_bytes"],
            "counts": budget["counts"],
            "up": budget["variant"]["up"],
            "down": budget["variant"]["down"],
            "groups": result["report"],
        },
    )
    print(
        json.dumps(
            {
                "out": str(out_path),
                "report": str(report_path),
                "actual_bytes": budget["actual_bytes"],
                "counts": budget["counts"],
                "up": budget["variant"]["up"],
                "down": budget["variant"]["down"],
            },
            ensure_ascii=False,
        )
    )

    if args.run:
        if not (args.exe and args.model and args.data_dir and args.result):
            raise SystemExit("--run 需要 --exe --model --data-dir --result")
        metrics = run_candidate(args, out_path, Path(args.result))
        budget["evaluation"] = metrics
        write_json(out_path, budget)
        print(json.dumps(metrics, ensure_ascii=False))


if __name__ == "__main__":
    if len(sys.argv) == 2 and sys.argv[1] == "--self-test":
        self_test()
    else:
        main()
