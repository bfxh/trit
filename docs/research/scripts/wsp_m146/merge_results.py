"""Merge M1.46 per-corpus outputs and compute secondary summaries.

The probe driver writes one raw/compare/primary triple per corpus under
``D:\\trit-results\\<corpus>``. This script concatenates them into
``docs/research/data`` and adds two reproducible summaries:

* ``wsp-classify-m146-summary.tsv``: mean metrics per
  corpus/depth/probe/rule/arm across the five seeds.
* ``wsp-classify-m146-rule-delta.tsv``: mean balanced - unweighted deltas
  for the f32 arm, including how many seeds moved in the expected direction.
* ``wsp-classify-m146-arm-delta.tsv``: mean quantized - f32 deltas under
  both rules (point estimates only; no bootstrap).
* ``wsp-classify-m146-verdict.json``: the preregistered 10/12 and
  3-of-4-per-corpus gate applied to the primary table.

No bootstrap is recomputed here; the driver's compare table already carries
the 10k paired CI for the primary rule difference.
"""

from __future__ import annotations

import csv
import json
from collections import defaultdict
from pathlib import Path

CORPORA = ("hatexplain", "tweet-hate", "davidson")
ROOT = Path(r"D:\trit-results")
OUT = Path(__file__).resolve().parents[2] / "data"
METRICS = ("accuracy", "macro_f1", "recall_non_hate", "recall_hate", "pred_pos_rate")
RAW_FIELDS = [
    "corpus", "depth", "probe", "rule", "arm", "seed", "C", "n",
    "accuracy", "macro_f1", "recall_non_hate", "recall_hate",
    "pred_pos_rate", "prior_fit", "prior_test",
]
COMPARE_FIELDS = [
    "corpus", "kind", "depth", "probe", "arm", "rule_a", "rule_b",
    "metric", "seed", "C", "n", "score_a", "score_b", "point_delta",
    "boot_mean", "ci_low", "ci_high", "verdict",
]
PRIMARY_FIELDS = [
    "corpus", "depth", "probe", "metric", "positive", "n", "mean_point",
    "mean_boot", "verdict",
]


def read_tsv(path: Path) -> list[dict]:
    with path.open(encoding="utf-8", newline="") as fh:
        return list(csv.DictReader(fh, delimiter="\t"))


def write_tsv(path: Path, rows: list[dict], fields: list[str]) -> None:
    with path.open("w", encoding="utf-8", newline="") as fh:
        writer = csv.DictWriter(fh, fieldnames=fields, delimiter="\t",
                                lineterminator="\n")
        writer.writeheader()
        writer.writerows(rows)


def f(row: dict, key: str) -> float:
    return float(row[key])


def mean(values: list[float]) -> float:
    return sum(values) / len(values)


def main() -> None:
    OUT.mkdir(parents=True, exist_ok=True)
    raw, compare, primary = [], [], []
    for corpus in CORPORA:
        root = ROOT / corpus
        raw.extend(read_tsv(root / "wsp-classify-m146-raw.tsv"))
        compare.extend(read_tsv(root / "wsp-classify-m146-compare.tsv"))
        primary.extend(read_tsv(root / "wsp-classify-m146-primary.tsv"))

    if len(raw) != 240:
        raise SystemExit(f"raw rows {len(raw)} != 240")
    if len(compare) != 300:
        raise SystemExit(f"compare rows {len(compare)} != 300")
    if len(primary) != 12:
        raise SystemExit(f"primary rows {len(primary)} != 12")

    write_tsv(OUT / "wsp-classify-m146-raw.tsv", raw, RAW_FIELDS)
    write_tsv(OUT / "wsp-classify-m146-compare.tsv", compare, COMPARE_FIELDS)
    write_tsv(OUT / "wsp-classify-m146-primary.tsv", primary, PRIMARY_FIELDS)

    grouped = defaultdict(list)
    for row in raw:
        key = (row["corpus"], row["depth"], row["probe"], row["rule"], row["arm"])
        grouped[key].append(row)

    summary = []
    for key in sorted(grouped):
        rows = grouped[key]
        if len(rows) != 5:
            raise SystemExit(f"{key}: expected 5 seeds, got {len(rows)}")
        item = {
            "corpus": key[0], "depth": key[1], "probe": key[2],
            "rule": key[3], "arm": key[4], "n_seed": len(rows),
            "C": rows[0]["C"], "n": rows[0]["n"],
        }
        for metric in METRICS:
            item[metric] = mean([f(r, metric) for r in rows])
        summary.append(item)
    write_tsv(OUT / "wsp-classify-m146-summary.tsv", summary,
              ["corpus", "depth", "probe", "rule", "arm", "n_seed", "C", "n"]
              + list(METRICS))

    by_cell = {(i["corpus"], i["depth"], i["probe"], i["rule"], i["arm"]): i
               for i in summary}
    deltas = []
    for corpus in CORPORA:
        for depth in ("l12", "l6"):
            for probe in ("logreg", "linear_svc"):
                a = by_cell[(corpus, depth, probe, "balanced", "f32")]
                b = by_cell[(corpus, depth, probe, "unweighted", "f32")]
                for metric in METRICS:
                    per_seed = []
                    for row in grouped[(corpus, depth, probe, "balanced", "f32")]:
                        seed = row["seed"]
                        other = next(
                            r for r in grouped[(corpus, depth, probe,
                                               "unweighted", "f32")]
                            if r["seed"] == seed
                        )
                        per_seed.append(f(row, metric) - f(other, metric))
                    deltas.append({
                        "corpus": corpus, "depth": depth, "probe": probe,
                        "metric": metric,
                        "unweighted": b[metric], "balanced": a[metric],
                        "delta": a[metric] - b[metric],
                        "seeds_positive": sum(v > 0 for v in per_seed),
                        "seeds_negative": sum(v < 0 for v in per_seed),
                    })
    write_tsv(OUT / "wsp-classify-m146-rule-delta.tsv", deltas,
              ["corpus", "depth", "probe", "metric", "unweighted", "balanced",
               "delta", "seeds_positive", "seeds_negative"])

    arm_deltas = []
    for corpus in CORPORA:
        for depth in ("l12", "l6"):
            for probe in ("logreg", "linear_svc"):
                quant = "r12" if depth == "l12" else "rescan"
                for rule in ("unweighted", "balanced"):
                    a = by_cell[(corpus, depth, probe, rule, quant)]
                    b = by_cell[(corpus, depth, probe, rule, "f32")]
                    for metric in ("accuracy", "macro_f1"):
                        arm_deltas.append({
                            "corpus": corpus, "depth": depth, "probe": probe,
                            "rule": rule, "arm": quant, "metric": metric,
                            "arm_score": a[metric], "f32_score": b[metric],
                            "delta": a[metric] - b[metric],
                        })
    write_tsv(OUT / "wsp-classify-m146-arm-delta.tsv", arm_deltas,
              ["corpus", "depth", "probe", "rule", "arm", "metric",
               "arm_score", "f32_score", "delta"])

    supported = [r for r in primary if r["verdict"] == "supported"]
    per_corpus = {
        corpus: sum(r["verdict"] == "supported" for r in primary
                    if r["corpus"] == corpus)
        for corpus in CORPORA
    }
    cells = []
    for row in primary:
        item = dict(row)
        for key in ("positive", "n"):
            item[key] = int(item[key])
        for key in ("mean_point", "mean_boot"):
            item[key] = float(item[key])
        cells.append(item)

    supported = len(supported)
    verdict = {
        "metric": "balanced f32 - unweighted f32 macro_f1",
        "gate": {
            "supported_cells": ">= 10/12",
            "per_corpus": ">= 3/4 supported in every corpus",
        },
        "supported_cells": supported,
        "total_cells": len(primary),
        "per_corpus": per_corpus,
        "supported": supported >= 10 and all(v >= 3 for v in per_corpus.values()),
        "verdict": "supported" if supported >= 10 and all(
            v >= 3 for v in per_corpus.values()
        ) else "not_supported",
        "cells": cells,
    }
    (OUT / "wsp-classify-m146-verdict.json").write_text(
        json.dumps(verdict, indent=2, ensure_ascii=False) + "\n",
        encoding="utf-8",
    )

    print(json.dumps(verdict, indent=2, ensure_ascii=False))
    print("\nrule deltas, f32:")
    for row in deltas:
        print(
            f'{row["corpus"]:11s} {row["depth"]:3s} {row["probe"]:10s} '
            f'{row["metric"]:14s} {row["delta"]:+.6f} '
            f'({row["seeds_positive"]}/{row["seeds_negative"]})'
        )


if __name__ == "__main__":
    main()
