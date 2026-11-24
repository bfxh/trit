#!/usr/bin/env python3
"""WSP-BUDGET 重扫选择器：从双向块表构造严格同字节候选。

`wsp_budget_perturb.py` 只回答“单块降/升一档会怎样”；`wsp_budget_variant.py`
要求人工给 `--up/--down` 列表。本脚本把两张表接起来：

1. 按 `(rows, cols, group)` 把块分形状组；同一形状组内，3→2 省下的字节与
   3→4 花掉的字节相等（都是 `rows*cols/8`），所以每个形状组各选同样数量的
   降/升块，整包严格同字节。
2. 降级候选按 `delta_spearman` 从高到低取（降级伤害最小）；
   升级候选同样从高到低取（升级收益最大），并避开已选降级块。
3. 默认每个形状组选 2 降 + 2 升；可用 `--per-shape` 改这个结构参数。

只选预算：

  python scripts/wsp_budget_rescan.py \
    --base budget-l6-pure-int3.json \
    --summary-3to2 perturb-l6-3to2/perturb-summary.json \
    --summary-3to4 perturb-l6-3to4/perturb-summary.json \
    --per-shape 2 --out budget-l6-rescan.json

选完直接评测：

  python scripts/wsp_budget_rescan.py ... --out budget-l6-rescan.json \
    --run --exe target/release/examples/bert6.exe \
    --model all-MiniLM-L6-v2.safetensors --data-dir miniLM-bert6-sts-full \
    --layers 6 --threads 24 --f32-cache l6-f32-cache-full.json \
    --result l6-rescan-result.json --dump-preds

`--self-test` 不读模型，只用合成块表检查配平、排序和去重。
"""
from __future__ import annotations

import argparse
import json
import math
import os
import subprocess
import sys
from pathlib import Path

KIND_SHORT = {
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


def block_key(item: dict) -> tuple[int, str]:
    return int(item["layer"]), item["kind"]


def shape_key(item: dict, default_group: int) -> tuple[int, int, int]:
    return int(item["rows"]), int(item["cols"]), int(item.get("group", default_group))


def short_key(item: dict) -> str:
    return f"{int(item['layer'])}{KIND_SHORT.get(item['kind'], item['kind'])}"


def load_summary(path: str | Path) -> dict[tuple[int, str], dict]:
    data = json.loads(Path(path).read_text(encoding="utf-8"))
    rows = {}
    for row in data.get("runs", []):
        if row.get("status") not in {"ok", "cached"}:
            continue
        if "delta_spearman" not in row:
            continue
        rows[block_key(row)] = row
    return rows


def select_blocks(
    base: dict,
    down_rows: dict[tuple[int, str], dict],
    up_rows: dict[tuple[int, str], dict],
    per_shape: int,
) -> dict:
    if per_shape < 1:
        raise SystemExit("--per-shape 必须 >= 1")
    blocks = list(base["blocks"])
    if any(int(b["bits"]) != 3 for b in blocks):
        raise SystemExit("--base 必须是纯 int3 预算（所有块 bits == 3）")

    groups: dict[tuple[int, int, int], list[dict]] = {}
    for block in blocks:
        groups.setdefault(shape_key(block, int(base["group"])), []).append(block)

    selected_down: list[tuple[int, str]] = []
    selected_up: list[tuple[int, str]] = []
    report: list[dict] = []
    for shape in sorted(groups):
        members = groups[shape]
        keys = {block_key(b) for b in members}
        down_pool = [down_rows[k] for k in keys if k in down_rows]
        up_pool = [up_rows[k] for k in keys if k in up_rows]
        if len(down_pool) < per_shape or len(up_pool) < per_shape:
            raise SystemExit(
                f"形状组 {shape} 候选不足：down={len(down_pool)} up={len(up_pool)} "
                f"per_shape={per_shape}"
            )

        # 降级取“伤害最小”，升级取“收益最大”；两者都按 delta_spearman 降序。
        down_sorted = sorted(
            down_pool, key=lambda r: (-float(r["delta_spearman"]), block_key(r))
        )
        chosen_down = [block_key(r) for r in down_sorted[:per_shape]]
        down_set = set(chosen_down)
        up_sorted = sorted(
            up_pool, key=lambda r: (-float(r["delta_spearman"]), block_key(r))
        )
        chosen_up = []
        for row in up_sorted:
            key = block_key(row)
            if key not in down_set:
                chosen_up.append(key)
            if len(chosen_up) == per_shape:
                break
        if len(chosen_up) < per_shape:
            raise SystemExit(f"形状组 {shape} 升级候选与降级候选重叠过多")

        selected_down.extend(chosen_down)
        selected_up.extend(chosen_up)
        report.append(
            {
                "shape": {"rows": shape[0], "cols": shape[1], "group": shape[2]},
                "down": [
                    {
                        "block": short_key(block_of(blocks, key)),
                        "delta_pearson": float(down_rows[key]["delta_pearson"]),
                        "delta_spearman": float(down_rows[key]["delta_spearman"]),
                    }
                    for key in chosen_down
                ],
                "up": [
                    {
                        "block": short_key(block_of(blocks, key)),
                        "delta_pearson": float(up_rows[key]["delta_pearson"]),
                        "delta_spearman": float(up_rows[key]["delta_spearman"]),
                    }
                    for key in chosen_up
                ],
            }
        )

    overlap = set(selected_down) & set(selected_up)
    if overlap:
        raise SystemExit(f"同一块不能既 down 又 up: {sorted(overlap)}")

    group = int(base["group"])
    new_blocks = []
    for block in blocks:
        key = block_key(block)
        bits = 4 if key in set(selected_up) else 2 if key in set(selected_down) else 3
        item = dict(block)
        item["bits"] = bits
        item["actual_bytes"] = matrix_cost(
            int(item["rows"]), int(item["cols"]), bits, group
        )
        new_blocks.append(item)

    total = sum(int(b["actual_bytes"]) for b in new_blocks)
    budget = int(base["actual_bytes"])
    if total != budget:
        raise SystemExit(f"配平失败：actual={total} base={budget} delta={total-budget}")

    out = dict(base)
    out["actual_bytes"] = total
    out["counts"] = {
        str(bits): sum(1 for b in new_blocks if int(b["bits"]) == bits)
        for bits in (2, 3, 4, 5)
    }
    out["proxy_err_picked"] = math.fsum(float(b["proxy_err"]) for b in new_blocks)
    out["proxy_err_ratio"] = (
        out["proxy_err_picked"] / float(out["proxy_err_int3"])
        if out.get("proxy_err_int3")
        else None
    )
    out["variant"] = {
        "base": Path(base.get("_base_path", "base.json")).name,
        "selector": "wsp_budget_rescan.py",
        "per_shape": per_shape,
        "up": [f"{l}:{k}" for l, k in sorted(selected_up)],
        "down": [f"{l}:{k}" for l, k in sorted(selected_down)],
    }
    out.pop("_base_path", None)
    out["blocks"] = new_blocks
    return {"budget": out, "report": report}


def block_of(blocks: list[dict], key: tuple[int, str]) -> dict:
    for block in blocks:
        if block_key(block) == key:
            return block
    raise KeyError(key)


def run_candidate(args: argparse.Namespace, budget_path: Path, result_path: Path) -> dict:
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
        raise SystemExit(f"评测失败：{proc.returncode}，日志 {log_path}")
    data = json.loads(result_path.read_text(encoding="utf-8"))
    sts = data["modes"]["f32"]["sts"]
    return {
        "result": str(result_path),
        "log": str(log_path),
        "pearson": float(sts["pearson"]),
        "spearman": float(sts["spearman"]),
    }


def self_test() -> None:
    groups = [(384, 384), (1536, 384), (384, 1536)]
    base_blocks = []
    for gi, (rows, cols) in enumerate(groups):
        for layer in range(2):
            for kind in ("q", "k"):
                block = {
                    "layer": layer,
                    "kind": "attention.self.query" if kind == "q" else "attention.self.key",
                    "rows": rows,
                    "cols": cols,
                    "bits": 3,
                    "group": 32,
                    "proxy_err": 1.0,
                }
                block["actual_bytes"] = matrix_cost(rows, cols, 3, 32)
                base_blocks.append(block)
    base = {
        "group": 32,
        "actual_bytes": sum(int(b["actual_bytes"]) for b in base_blocks),
        "proxy_err_int3": len(base_blocks),
        "blocks": base_blocks,
    }

    down_rows = {}
    up_rows = {}
    for idx, block in enumerate(base_blocks):
        key = block_key(block)
        local = (idx % 4)  # 每个形状组内 2 层 × 2 kind
        # 每组的第 0 块同时是降级和升级最优，检查选择器会避开重叠。
        down_rows[key] = {
            "status": "ok",
            "layer": key[0],
            "kind": key[1],
            "delta_pearson": -0.001 * (local + 1),
            "delta_spearman": -0.001 * (local + 1),
        }
        up_rows[key] = {
            "status": "ok",
            "layer": key[0],
            "kind": key[1],
            "delta_pearson": 0.001 * (4 - local),
            "delta_spearman": 0.001 * (4 - local),
        }

    result = select_blocks(base, down_rows, up_rows, per_shape=1)
    budget = result["budget"]
    assert budget["actual_bytes"] == base["actual_bytes"], budget
    assert budget["counts"] == {"2": 3, "3": 6, "4": 3, "5": 0}, budget["counts"]
    down = {(int(item.split(":", 1)[0]), item.split(":", 1)[1]) for item in budget["variant"]["down"]}
    up = {(int(item.split(":", 1)[0]), item.split(":", 1)[1]) for item in budget["variant"]["up"]}
    assert not (down & up), (down, up)
    # 每组第 0 块（layer 0 query）降级最优，但升级也最优；升级必须让开。
    assert (0, "attention.self.query") in down
    assert (0, "attention.self.query") not in up
    print("self-test: ok")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--base", default="")
    parser.add_argument("--summary-3to2", default="")
    parser.add_argument("--summary-3to4", default="")
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
        ("--summary-3to2", args.summary_3to2),
        ("--summary-3to4", args.summary_3to4),
        ("--out", args.out),
    ):
        if not value:
            raise SystemExit(f"缺少 {flag}")

    base = json.loads(Path(args.base).read_text(encoding="utf-8"))
    base["_base_path"] = args.base
    result = select_blocks(
        base,
        load_summary(args.summary_3to2),
        load_summary(args.summary_3to4),
        args.per_shape,
    )
    budget = result["budget"]
    out_path = Path(args.out)
    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_text(json.dumps(budget, ensure_ascii=False, indent=2), encoding="utf-8")

    report_path = Path(args.report) if args.report else out_path.with_suffix(".report.json")
    report_path.write_text(
        json.dumps(
            {
                "base": args.base,
                "summary_3to2": args.summary_3to2,
                "summary_3to4": args.summary_3to4,
                "per_shape": args.per_shape,
                "actual_bytes": budget["actual_bytes"],
                "counts": budget["counts"],
                "groups": result["report"],
            },
            ensure_ascii=False,
            indent=2,
        ),
        encoding="utf-8",
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
        out_path.write_text(json.dumps(budget, ensure_ascii=False, indent=2), encoding="utf-8")
        print(json.dumps(metrics, ensure_ascii=False))


if __name__ == "__main__":
    if len(sys.argv) == 2 and sys.argv[1] == "--self-test":
        self_test()
    else:
        main()
