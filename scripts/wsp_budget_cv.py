#!/usr/bin/env python3
"""WSP-BUDGET 选择器交叉拟合评估。

`wsp_budget_rescan.py` 只用一张全量块表选一次候选；这个脚本把选择本身放进
交叉拟合：每折只用训练折的 36 块 Δ 排序，测试折不参与选块；各折候选在测试
折上的预测拼成 OOF 向量，再和纯 int3/int4 做配对 bootstrap。

  python scripts/wsp_budget_cv.py \
    --base budget-l6-pure-int3.json \
    --perturb-3to2 perturb-full-3to2 --perturb-3to4 perturb-full-3to4 \
    --pairs miniLM-bert6-sts-full/pairs.json \
    --int3 int3.json --int4 int4.json \
    --folds 5 --repeats 2 --out-dir cv5 \
    --run --exe target/release/examples/bert6.exe \
    --model all-MiniLM-L6-v2.safetensors --data-dir miniLM-bert6-sts-full \
    --layers 6 --threads 24 --f32-cache l6-f32-cache-full.json

`--resume` 复用已完成的折结果；`--select-only` 只写预算不评测；
`--self-test` 不读模型，只校验折划分覆盖和 OOF 拼接。
"""
from __future__ import annotations

import argparse
import json
import math
import os
import random
import subprocess
import sys
from pathlib import Path

SCRIPT_DIR = Path(__file__).resolve().parent
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

from wsp_budget_rescan import select_blocks  # noqa: E402

KIND_SHORT = {
    "attention.self.query": "q",
    "attention.self.key": "k",
    "attention.self.value": "v",
    "attention.output.dense": "o",
    "intermediate.dense": "i",
    "output.dense": "f",
}


def load_json(path: str | Path) -> dict:
    return json.loads(Path(path).read_text(encoding="utf-8"))


def write_json(path: str | Path, obj: dict) -> None:
    Path(path).parent.mkdir(parents=True, exist_ok=True)
    Path(path).write_text(json.dumps(obj, ensure_ascii=False, indent=2), encoding="utf-8")


def preds_of(path: str | Path) -> list[float]:
    data = load_json(path)
    try:
        preds = data["modes"]["f32"]["sts"]["preds"]
    except KeyError as exc:
        raise SystemExit(f"{path} 没有 modes.f32.sts.preds：{exc}")
    if not isinstance(preds, list) or not preds:
        raise SystemExit(f"{path} 的 preds 不是非空列表")
    return [float(v) for v in preds]


def labels_of(path: str | Path) -> list[float]:
    data = load_json(path)
    pairs = data.get("pairs")
    if not isinstance(pairs, list) or not pairs:
        raise SystemExit(f"{path} 没有非空 pairs 列表")
    return [float(p["score"]) for p in pairs]


def pearson(x: list[float], y: list[float]) -> float:
    n = len(x)
    if n == 0:
        return float("nan")
    mx = math.fsum(x) / n
    my = math.fsum(y) / n
    dx = [v - mx for v in x]
    dy = [v - my for v in y]
    sx = math.sqrt(math.fsum(v * v for v in dx))
    sy = math.sqrt(math.fsum(v * v for v in dy))
    if sx == 0.0 or sy == 0.0:
        return float("nan")
    return math.fsum(dx[i] * dy[i] for i in range(n)) / (sx * sy)


def rank(values: list[float]) -> list[float]:
    order = sorted(range(len(values)), key=lambda i: values[i])
    out = [0.0] * len(values)
    i = 0
    while i < len(order):
        j = i + 1
        while j < len(order) and values[order[j]] == values[order[i]]:
            j += 1
        avg = (i + j - 1) / 2.0 + 1.0
        for k in range(i, j):
            out[order[k]] = avg
        i = j
    return out


def spearman(x: list[float], y: list[float]) -> float:
    return pearson(rank(x), rank(y))


def metrics(preds: list[float], gold: list[float], idx: list[int]) -> tuple[float, float]:
    return (
        pearson([preds[i] for i in idx], [gold[i] for i in idx]),
        spearman([preds[i] for i in idx], [gold[i] for i in idx]),
    )


def split_folds(n: int, k: int, seed: int) -> list[list[int]]:
    if k < 2 or k > n:
        raise SystemExit(f"--folds 必须在 2..{n}")
    order = list(range(n))
    random.Random(seed).shuffle(order)
    return [sorted(order[i::k]) for i in range(k)]


def variant_path(directory: Path, layer: int, kind: str, bits: int) -> Path:
    return directory / f"p-{layer}{KIND_SHORT.get(kind, kind)}-to{bits}-result.json"


def delta_rows(
    base_preds: list[float],
    gold: list[float],
    train: list[int],
    directory: Path,
    blocks: list[dict],
    bits: int,
) -> dict[tuple[int, str], dict]:
    base_p, base_s = metrics(base_preds, gold, train)
    rows: dict[tuple[int, str], dict] = {}
    for block in blocks:
        layer = int(block["layer"])
        kind = block["kind"]
        path = variant_path(directory, layer, kind, bits)
        if not path.exists():
            raise SystemExit(f"缺少块表结果：{path}")
        p, s = metrics(preds_of(path), gold, train)
        rows[(layer, kind)] = {
            "layer": layer,
            "kind": kind,
            "status": "ok",
            "delta_pearson": p - base_p,
            "delta_spearman": s - base_s,
        }
    return rows


def run_candidate(
    args: argparse.Namespace, budget_path: Path, result_path: Path
) -> None:
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
            "TRIT_DUMP_PREDS": "1",
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
        raise SystemExit(f"评测失败：{proc.returncode}，日志 {log_path}")
    if not result_path.exists():
        raise SystemExit(f"评测没有产出 {result_path}")


def bootstrap(
    baseline: Path,
    candidate: Path,
    pairs: Path,
    out: Path,
    iters: int,
) -> None:
    cmd = [
        sys.executable,
        str(SCRIPT_DIR / "wsp_budget_bootstrap.py"),
        "--baseline",
        str(baseline),
        "--candidate",
        str(candidate),
        "--pairs",
        str(pairs),
        "--out",
        str(out),
        "--iters",
        str(iters),
    ]
    proc = subprocess.run(cmd, text=True)
    if proc.returncode != 0:
        raise SystemExit(f"bootstrap 失败：{proc.returncode}")


def stats(values: list[float]) -> dict:
    if not values:
        return {"n": 0}
    return {
        "n": len(values),
        "mean": math.fsum(values) / len(values),
        "min": min(values),
        "max": max(values),
    }


def self_test() -> None:
    n = 23
    folds = split_folds(n, 5, 20261124)
    assert sum(len(f) for f in folds) == n
    assert sorted(i for f in folds for i in f) == list(range(n))
    assert all(len(f) in (4, 5) for f in folds), [len(f) for f in folds]
    oof = [None] * n
    for fi, test in enumerate(folds):
        for i in test:
            assert oof[i] is None
            oof[i] = fi
    assert all(v is not None for v in oof)
    assert len(oof) == n
    print("self-test: ok")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--base", default="")
    parser.add_argument("--perturb-3to2", default="")
    parser.add_argument("--perturb-3to4", default="")
    parser.add_argument("--pairs", default="")
    parser.add_argument("--int3", default="")
    parser.add_argument("--int4", default="")
    parser.add_argument("--folds", type=int, default=5)
    parser.add_argument("--repeats", type=int, default=2)
    parser.add_argument("--seed", type=int, default=20261124)
    parser.add_argument("--per-shape", type=int, default=2)
    parser.add_argument("--out-dir", default="")
    parser.add_argument("--run", action="store_true")
    parser.add_argument("--resume", action="store_true")
    parser.add_argument("--select-only", action="store_true")
    parser.add_argument("--exe", default="")
    parser.add_argument("--model", default="")
    parser.add_argument("--data-dir", default="")
    parser.add_argument("--f32-cache", default="")
    parser.add_argument("--threads", type=int, default=24)
    parser.add_argument("--layers", type=int, default=12)
    parser.add_argument("--bootstrap-iters", type=int, default=10000)
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()

    if args.self_test:
        self_test()
        return

    for flag, value in (
        ("--base", args.base),
        ("--perturb-3to2", args.perturb_3to2),
        ("--perturb-3to4", args.perturb_3to4),
        ("--pairs", args.pairs),
        ("--int3", args.int3),
        ("--int4", args.int4),
        ("--out-dir", args.out_dir),
    ):
        if not value:
            raise SystemExit(f"缺少 {flag}")
    if args.run and not (args.exe and args.model and args.data_dir):
        raise SystemExit("--run 需要 --exe --model --data-dir")

    base = load_json(args.base)
    base["_base_path"] = args.base
    blocks = list(base["blocks"])
    gold = labels_of(args.pairs)
    int3 = preds_of(args.int3)
    int4 = preds_of(args.int4)
    n = len(gold)
    if len(int3) != n or len(int4) != n:
        raise SystemExit(f"长度不一致：pairs={n} int3={len(int3)} int4={len(int4)}")
    if len(blocks) == 0:
        raise SystemExit("--base 没有 blocks")

    out_dir = Path(args.out_dir)
    out_dir.mkdir(parents=True, exist_ok=True)
    all_repeats = []

    for repeat in range(args.repeats):
        seed = args.seed + repeat
        folds = split_folds(n, args.folds, seed)
        oof: list[float | None] = [None] * n
        fold_records = []

        for fi, test in enumerate(folds):
            train = sorted(set(range(n)) - set(test))
            down_rows = delta_rows(
                int3, gold, train, Path(args.perturb_3to2), blocks, 2
            )
            up_rows = delta_rows(
                int3, gold, train, Path(args.perturb_3to4), blocks, 4
            )
            result = select_blocks(base, down_rows, up_rows, args.per_shape)
            budget = result["budget"]
            tag = f"l6-cv-s{seed}-r{repeat}-f{fi}"
            budget_path = out_dir / f"{tag}.json"
            report_path = out_dir / f"{tag}-report.json"
            result_path = out_dir / f"{tag}-result.json"
            write_json(budget_path, budget)
            write_json(
                report_path,
                {
                    "seed": seed,
                    "repeat": repeat,
                    "fold": fi,
                    "train_n": len(train),
                    "test_n": len(test),
                    "per_shape": args.per_shape,
                    "actual_bytes": budget["actual_bytes"],
                    "counts": budget["counts"],
                    "groups": result["report"],
                },
            )
            record = {
                "fold": fi,
                "test_n": len(test),
                "budget": budget_path.name,
                "actual_bytes": budget["actual_bytes"],
                "counts": budget["counts"],
                "up": budget["variant"]["up"],
                "down": budget["variant"]["down"],
            }
            fold_records.append(record)

            if args.select_only:
                continue
            if not (args.run or args.resume):
                raise SystemExit("需要 --run 或 --resume（或使用 --select-only）")
            if not (args.resume and result_path.exists()):
                if not args.run:
                    raise SystemExit(f"--resume 找不到 {result_path}")
                run_candidate(args, budget_path, result_path)
            preds = preds_of(result_path)
            if len(preds) != n:
                raise SystemExit(f"{result_path} preds 长度 {len(preds)}，期望 {n}")
            for i in test:
                oof[i] = preds[i]
            record["result"] = result_path.name
            test_p, test_s = metrics(preds, gold, test)
            base_p, base_s = metrics(int3, gold, test)
            int4_p, int4_s = metrics(int4, gold, test)
            record["test_pearson"] = test_p
            record["test_spearman"] = test_s
            record["test_int3_pearson"] = base_p
            record["test_int3_spearman"] = base_s
            record["test_int4_pearson"] = int4_p
            record["test_int4_spearman"] = int4_s
            record["delta_vs_int3"] = {"pearson": test_p - base_p, "spearman": test_s - base_s}
            record["delta_vs_int4"] = {"pearson": test_p - int4_p, "spearman": test_s - int4_s}

        if args.select_only:
            summary = {
                "seed": seed,
                "repeat": repeat,
                "folds": args.folds,
                "per_shape": args.per_shape,
                "fold_records": fold_records,
                "oof": None,
            }
            summary_path = out_dir / f"l6-cv-s{seed}-r{repeat}-summary.json"
            write_json(summary_path, summary)
            all_repeats.append({"seed": seed, "select_only": True, "fold_records": fold_records})
            print(json.dumps({"seed": seed, "select_only": True, "folds": len(fold_records)}))
            continue

        if any(v is None for v in oof):
            raise SystemExit(f"seed {seed} 的 OOF 没有填满")
        oof_values = [float(v) for v in oof]  # type: ignore[arg-type]
        oof_metrics = {
            "pearson": pearson(oof_values, gold),
            "spearman": spearman(oof_values, gold),
        }
        int3_metrics = {
            "pearson": pearson(int3, gold),
            "spearman": spearman(int3, gold),
        }
        int4_metrics = {
            "pearson": pearson(int4, gold),
            "spearman": spearman(int4, gold),
        }
        oof_path = out_dir / f"l6-cv-s{seed}-r{repeat}-oof-result.json"
        write_json(
            oof_path,
            {
                "kind": "cross-fitted-oof",
                "seed": seed,
                "repeat": repeat,
                "folds": args.folds,
                "per_shape": args.per_shape,
                "modes": {
                    "f32": {
                        "sts": {
                            "pearson": oof_metrics["pearson"],
                            "spearman": oof_metrics["spearman"],
                            "preds": oof_values,
                        }
                    }
                },
                "fold_records": fold_records,
            },
        )
        summary = {
            "seed": seed,
            "repeat": repeat,
            "folds": args.folds,
            "per_shape": args.per_shape,
            "oof_metrics": oof_metrics,
            "int3_metrics": int3_metrics,
            "int4_metrics": int4_metrics,
            "delta_vs_int3": {
                k: oof_metrics[k] - int3_metrics[k] for k in ("pearson", "spearman")
            },
            "delta_vs_int4": {
                k: oof_metrics[k] - int4_metrics[k] for k in ("pearson", "spearman")
            },
            "fold_records": fold_records,
        }
        summary_path = out_dir / f"l6-cv-s{seed}-r{repeat}-summary.json"
        write_json(summary_path, summary)
        if args.bootstrap_iters > 0:
            bootstrap(
                Path(args.int3),
                oof_path,
                Path(args.pairs),
                out_dir / f"l6-cv-s{seed}-r{repeat}-vs-int3.json",
                args.bootstrap_iters,
            )
            bootstrap(
                Path(args.int4),
                oof_path,
                Path(args.pairs),
                out_dir / f"l6-cv-s{seed}-r{repeat}-vs-int4.json",
                args.bootstrap_iters,
            )
        all_repeats.append(summary)
        print(
            json.dumps(
                {
                    "seed": seed,
                    "oof": oof_metrics,
                    "delta_vs_int3": summary["delta_vs_int3"],
                    "delta_vs_int4": summary["delta_vs_int4"],
                },
                ensure_ascii=False,
            )
        )

    evaluated = [r for r in all_repeats if "oof_metrics" in r]
    aggregate = None
    if evaluated:
        fold_records = [f for r in evaluated for f in r["fold_records"]]
        aggregate = {
            "oof_pearson": stats([r["oof_metrics"]["pearson"] for r in evaluated]),
            "oof_spearman": stats([r["oof_metrics"]["spearman"] for r in evaluated]),
            "delta_vs_int3_pearson": stats(
                [r["delta_vs_int3"]["pearson"] for r in evaluated]
            ),
            "delta_vs_int3_spearman": stats(
                [r["delta_vs_int3"]["spearman"] for r in evaluated]
            ),
            "delta_vs_int4_pearson": stats(
                [r["delta_vs_int4"]["pearson"] for r in evaluated]
            ),
            "delta_vs_int4_spearman": stats(
                [r["delta_vs_int4"]["spearman"] for r in evaluated]
            ),
            "fold_delta_vs_int3_pearson": stats(
                [f["delta_vs_int3"]["pearson"] for f in fold_records]
            ),
            "fold_delta_vs_int3_spearman": stats(
                [f["delta_vs_int3"]["spearman"] for f in fold_records]
            ),
            "fold_delta_vs_int4_pearson": stats(
                [f["delta_vs_int4"]["pearson"] for f in fold_records]
            ),
            "fold_delta_vs_int4_spearman": stats(
                [f["delta_vs_int4"]["spearman"] for f in fold_records]
            ),
            "fold_delta_vs_int3_positive": {
                "pearson": sum(f["delta_vs_int3"]["pearson"] > 0 for f in fold_records),
                "spearman": sum(f["delta_vs_int3"]["spearman"] > 0 for f in fold_records),
                "n": len(fold_records),
            },
            "fold_delta_vs_int4_positive": {
                "pearson": sum(f["delta_vs_int4"]["pearson"] > 0 for f in fold_records),
                "spearman": sum(f["delta_vs_int4"]["spearman"] > 0 for f in fold_records),
                "n": len(fold_records),
            },
        }

    write_json(
        out_dir / "cv-summary.json",
        {
            "base": args.base,
            "perturb_3to2": args.perturb_3to2,
            "perturb_3to4": args.perturb_3to4,
            "folds": args.folds,
            "repeats": args.repeats,
            "seed": args.seed,
            "per_shape": args.per_shape,
            "aggregate": aggregate,
            "repeats_results": all_repeats,
        },
    )


if __name__ == "__main__":
    if len(sys.argv) == 2 and sys.argv[1] == "--self-test":
        self_test()
    else:
        main()
