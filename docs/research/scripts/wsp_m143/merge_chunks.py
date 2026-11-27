"""Merge M1.43 chunked outputs and recompute the descriptive primary table."""

from __future__ import annotations

import argparse
import csv
from pathlib import Path

SUFFIXES = ("-l12-logreg", "-l12-svc", "-l6-logreg", "-l6-svc")


def read_tsv(path: Path) -> list[dict]:
    with path.open(encoding="utf-8", newline="") as fh:
        return list(csv.DictReader(fh, delimiter="\t"))


def write_tsv(path: Path, rows: list[dict], fields: list[str]) -> None:
    with path.open("w", encoding="utf-8", newline="") as fh:
        writer = csv.DictWriter(fh, delimiter="\t", fieldnames=fields, lineterminator="\n")
        writer.writeheader()
        for row in rows:
            writer.writerow({k: row.get(k, "") for k in fields})


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--root", default=r"D:\trit-results\hatexplain")
    args = ap.parse_args()
    root = Path(args.root)

    raw, comp = [], []
    for suffix in SUFFIXES:
        raw.extend(read_tsv(root / f"wsp-classify-m143-raw{suffix}.tsv"))
        comp.extend(read_tsv(root / f"wsp-classify-m143-compare{suffix}.tsv"))

    raw_fields = ["depth", "probe", "arm", "metric", "seed", "C", "n", "score",
                  "macro_f1", "recall_normal", "recall_hatespeech", "recall_offensive"]
    comp_fields = ["depth", "probe", "arm", "metric", "seed", "C", "n", "score_arm",
                   "score_f32", "point_delta", "boot_mean", "ci_low", "ci_high", "verdict"]
    write_tsv(root / "wsp-classify-m143-raw.tsv", raw, raw_fields)
    write_tsv(root / "wsp-classify-m143-compare.tsv", comp, comp_fields)

    primary = []
    for depth in ("l12", "l6"):
        for probe in ("logreg", "linear_svc"):
            vals = [
                r for r in comp
                if r["depth"] == depth and r["probe"] == probe
                and r["metric"] == "accuracy" and r["verdict"] in ("positive", "negative", "cross")
            ]
            primary.append(
                {
                    "depth": depth,
                    "probe": probe,
                    "positive": sum(r["verdict"] == "positive" for r in vals),
                    "n": len(vals),
                    "mean_point": sum(float(r["point_delta"]) for r in vals) / len(vals),
                    "mean_boot": sum(float(r["boot_mean"]) for r in vals) / len(vals),
                    "note": "descriptive only",
                }
            )
    write_tsv(
        root / "wsp-classify-m143-primary.tsv",
        primary,
        ["depth", "probe", "positive", "n", "mean_point", "mean_boot", "note"],
    )

    print("raw rows", len(raw), "compare rows", len(comp))
    for row in primary:
        print(row)


if __name__ == "__main__":
    main()
