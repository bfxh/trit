"""M1.44: static-bytes vs task-quality Pareto table.

This is a synthesis of already-published measurements, not a new model run.
Cost is the actual artifact byte count reported in
`docs/research/WSP-BUDGET-RESULTS.md` (not weight-only, not runtime RSS).
Quality metrics are the corrected true-f32 arms from M1.29/M1.31 and the
retrieval/STS tables in §11.11.  Only logreg is used for classification
metrics so the rows are comparable; this is not a probe sweep.
"""

from __future__ import annotations

import csv
from dataclasses import dataclass
from pathlib import Path


@dataclass(frozen=True)
class Row:
    depth: str
    task: str
    metric: str
    config: str
    bytes_: int
    value: float
    source: str


# L12 f32 / int3 / int4 / R12; L6 f32 / int3 / int4 / rescan.
# Bytes are the published artifact bytes from §2 and §11.11.
ROWS = [
    # L12
    Row("L12", "STS", "Pearson", "f32", 84_934_656, 0.837595, "§11.11.2"),
    Row("L12", "STS", "Pearson", "int3", 10_782_720, 0.818440, "§11.11.2"),
    Row("L12", "STS", "Pearson", "int4", 13_436_928, 0.837505, "§11.11.2"),
    Row("L12", "STS", "Pearson", "R12", 10_782_720, 0.833671, "§11.11.2"),
    Row("L12", "STS", "Spearman", "f32", 84_934_656, 0.830894, "§11.11.2"),
    Row("L12", "STS", "Spearman", "int3", 10_782_720, 0.813362, "§11.11.2"),
    Row("L12", "STS", "Spearman", "int4", 13_436_928, 0.832047, "§11.11.2"),
    Row("L12", "STS", "Spearman", "R12", 10_782_720, 0.835411, "§11.11.2"),
    Row("L12", "SciFact", "nDCG@10", "f32", 84_934_656, 0.589919, "§11.11.2"),
    Row("L12", "SciFact", "nDCG@10", "int3", 10_782_720, 0.531968, "§11.11.2"),
    Row("L12", "SciFact", "nDCG@10", "int4", 13_436_928, 0.568436, "§11.11.2"),
    Row("L12", "SciFact", "nDCG@10", "R12", 10_782_720, 0.525849, "§11.11.2"),
    Row("L12", "SciFact", "MRR@10", "f32", 84_934_656, 0.553757, "§11.11.2"),
    Row("L12", "SciFact", "MRR@10", "int3", 10_782_720, 0.496705, "§11.11.2"),
    Row("L12", "SciFact", "MRR@10", "int4", 13_436_928, 0.529372, "§11.11.2"),
    Row("L12", "SciFact", "MRR@10", "R12", 10_782_720, 0.482403, "§11.11.2"),
    Row("L12", "SST-2", "accuracy", "f32", 84_934_656, 0.842943, "§11.29.3"),
    Row("L12", "SST-2", "accuracy", "int3", 10_782_720, 0.823174, "§11.29.3"),
    Row("L12", "SST-2", "accuracy", "int4", 13_436_928, 0.840198, "§11.29.3"),
    Row("L12", "SST-2", "accuracy", "R12", 10_782_720, 0.819879, "§11.29.3"),
    Row("L12", "CoLA", "macro-F1", "f32", 84_934_656, 0.548644, "§11.31.2"),
    Row("L12", "CoLA", "macro-F1", "int3", 10_782_720, 0.534089, "§11.31.2"),
    Row("L12", "CoLA", "macro-F1", "int4", 13_436_928, 0.561151, "§11.31.2"),
    Row("L12", "CoLA", "macro-F1", "R12", 10_782_720, 0.543040, "§11.31.2"),
    Row("L12", "TweetEval-hate", "accuracy", "f32", 84_934_656, 0.574250, "§11.31.4"),
    Row("L12", "TweetEval-hate", "accuracy", "int3", 10_782_720, 0.564740, "§11.31.4"),
    Row("L12", "TweetEval-hate", "accuracy", "int4", 13_436_928, 0.565472, "§11.31.4"),
    Row("L12", "TweetEval-hate", "accuracy", "R12", 10_782_720, 0.560717, "§11.31.4"),
    # L6
    Row("L6", "STS", "Pearson", "f32", 42_467_328, 0.827406, "§11.11.3"),
    Row("L6", "STS", "Pearson", "int3", 5_391_360, 0.814594, "§11.11.3"),
    Row("L6", "STS", "Pearson", "int4", 6_718_464, 0.826061, "§11.11.3"),
    Row("L6", "STS", "Pearson", "rescan", 5_391_360, 0.831304, "§11.11.3"),
    Row("L6", "STS", "Spearman", "f32", 42_467_328, 0.820325, "§11.11.3"),
    Row("L6", "STS", "Spearman", "int3", 5_391_360, 0.810425, "§11.11.3"),
    Row("L6", "STS", "Spearman", "int4", 6_718_464, 0.819355, "§11.11.3"),
    Row("L6", "STS", "Spearman", "rescan", 5_391_360, 0.824457, "§11.11.3"),
    Row("L6", "SciFact", "nDCG@10", "f32", 42_467_328, 0.571812, "§11.11.3"),
    Row("L6", "SciFact", "nDCG@10", "int3", 5_391_360, 0.541137, "§11.11.3"),
    Row("L6", "SciFact", "nDCG@10", "int4", 6_718_464, 0.573827, "§11.11.3"),
    Row("L6", "SciFact", "nDCG@10", "rescan", 5_391_360, 0.514939, "§11.11.3"),
    Row("L6", "SciFact", "MRR@10", "f32", 42_467_328, 0.534049, "§11.11.3"),
    Row("L6", "SciFact", "MRR@10", "int3", 5_391_360, 0.510308, "§11.11.3"),
    Row("L6", "SciFact", "MRR@10", "int4", 6_718_464, 0.534526, "§11.11.3"),
    Row("L6", "SciFact", "MRR@10", "rescan", 5_391_360, 0.474242, "§11.11.3"),
    Row("L6", "SST-2", "accuracy", "f32", 42_467_328, 0.811642, "§11.29.4"),
    Row("L6", "SST-2", "accuracy", "int3", 5_391_360, 0.808347, "§11.29.4"),
    Row("L6", "SST-2", "accuracy", "int4", 6_718_464, 0.806700, "§11.29.4"),
    Row("L6", "SST-2", "accuracy", "rescan", 5_391_360, 0.803405, "§11.29.4"),
    Row("L6", "CoLA", "macro-F1", "f32", 42_467_328, 0.553124, "§11.31.3"),
    Row("L6", "CoLA", "macro-F1", "int3", 5_391_360, 0.547084, "§11.31.3"),
    Row("L6", "CoLA", "macro-F1", "int4", 6_718_464, 0.543789, "§11.31.3"),
    Row("L6", "CoLA", "macro-F1", "rescan", 5_391_360, 0.540530, "§11.31.3"),
    Row("L6", "TweetEval-hate", "accuracy", "f32", 42_467_328, 0.546086, "§11.31.5"),
    Row("L6", "TweetEval-hate", "accuracy", "int3", 5_391_360, 0.550475, "§11.31.5"),
    Row("L6", "TweetEval-hate", "accuracy", "int4", 6_718_464, 0.531090, "§11.31.5"),
    Row("L6", "TweetEval-hate", "accuracy", "rescan", 5_391_360, 0.546452, "§11.31.5"),
]


def dominated_by(row: Row, group: list[Row]) -> list[str]:
    """Return configs that weakly dominate row (lower/equal bytes, higher/equal value)."""
    out = []
    for other in group:
        if other.config == row.config:
            continue
        if other.bytes_ <= row.bytes_ and other.value >= row.value:
            if other.bytes_ < row.bytes_ or other.value > row.value:
                out.append(other.config)
    return sorted(out)


def main() -> None:
    out = Path(__file__).resolve().parents[2] / "data" / "wsp-classify-m144-pareto.tsv"
    groups = {}
    for row in ROWS:
        groups.setdefault((row.depth, row.task, row.metric), []).append(row)

    with out.open("w", encoding="utf-8", newline="") as f:
        writer = csv.writer(f, delimiter="\t", lineterminator="\n")
        writer.writerow(
            [
                "depth",
                "task",
                "metric",
                "config",
                "bytes",
                "value",
                "dominated_by",
                "pareto",
                "source",
            ]
        )
        for key, group in groups.items():
            for row in group:
                dom = dominated_by(row, group)
                writer.writerow(
                    [
                        row.depth,
                        row.task,
                        row.metric,
                        row.config,
                        row.bytes_,
                        f"{row.value:.6f}",
                        ",".join(dom) if dom else "-",
                        "0" if dom else "1",
                        row.source,
                    ]
                )

    print(f"wrote {out} ({len(ROWS)} rows)")
    for key, group in groups.items():
        front = [row.config for row in group if not dominated_by(row, group)]
        print(f"{key[0]} {key[1]} {key[2]}: {', '.join(front)}")


if __name__ == "__main__":
    main()
